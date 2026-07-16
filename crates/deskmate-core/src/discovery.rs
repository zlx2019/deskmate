//! 发现层: 让局域网内的 deskmate 节点互相看见
//!
//! 双通道设计:
//! - 主通道: mDNS/DNS-SD 注册与浏览 `_deskmate._tcp.local.` 服务
//! - 兜底通道: UDP 组播周期 announce(部分企业路由器禁 mDNS)
//!
//! 任一通道可用即可工作, 两者都初始化失败才报错。
//! 节点生命周期: 心跳 5s → 超时 15s 判定下线 → 退出时发 goodbye 报文。
//! 发现报文只带小字段, 头像等大数据在 TCP 连接建立后按需拉取。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::protocol::PeerInfo;

use crate::config::{
    EVENT_CHANNEL_CAP, HEARTBEAT_INTERVAL, PEER_PROBE_INTERVAL, PEER_PROBE_TIMEOUT, PEER_TIMEOUT,
};

/// mDNS 服务类型
pub const MDNS_SERVICE_TYPE: &str = "_deskmate._tcp.local.";
/// UDP 组播组(选用 224.0.0.0/24 段, 路由器兼容性最好)
const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 168);

/// 发现层错误
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// mDNS 与 UDP 组播两个通道全部初始化失败
    #[error("发现服务不可用: mDNS 与 UDP 组播均初始化失败")]
    AllChannelsFailed,
    /// UDP socket 操作失败
    #[error("UDP 组播通道错误: {0}")]
    Io(#[from] std::io::Error),
    /// mDNS daemon 错误
    #[error("mDNS 通道错误: {0}")]
    Mdns(#[from] mdns_sd::Error),
}

/// 局域网内的一个在线节点
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// 设备信息(ID/名称/指纹/平台)
    pub info: PeerInfo,
    /// 候选地址列表(多网卡场景不止一个; 非回环 IPv4 排前, 连接时逐个尝试)
    pub addrs: Vec<IpAddr>,
    /// 控制/数据通道 TCP 端口
    pub port: u16,
}

/// 节点上下线事件
#[derive(Debug, Clone)]
pub enum PeerEvent {
    /// 节点上线或信息更新
    Up(Peer),
    /// 节点下线(参数为证书指纹)
    Down(String),
}

/// UDP 组播报文类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AnnounceKind {
    /// 周期广播: 我在线
    Announce,
    /// 对 announce 的单播应答: 让新节点立刻看到老节点
    Response,
    /// 优雅下线
    Goodbye,
}

/// UDP 组播报文(IP 取自 UDP 源地址, 不放报文内)
#[derive(Debug, Serialize, Deserialize)]
struct AnnouncePacket {
    /// 报文类型
    kind: AnnounceKind,
    /// 设备信息
    info: PeerInfo,
    /// TCP 监听端口
    tcp_port: u16,
}

/// 序列化 UDP 组播报文; 失败时返回空字节(字段均为简单类型, 实际不会发生)
fn encode_packet(kind: AnnounceKind, info: &PeerInfo, tcp_port: u16) -> Vec<u8> {
    serde_json::to_vec(&AnnouncePacket {
        kind,
        info: info.clone(),
        tcp_port,
    })
    .unwrap_or_default()
}

/// UDP 通道的预序列化报文组(身份热更新时整组替换, 各任务发送前读取)
struct UdpPackets {
    /// 周期广播
    announce: Vec<u8>,
    /// 对 announce 的单播应答
    response: Vec<u8>,
    /// 优雅下线
    goodbye: Vec<u8>,
}

impl UdpPackets {
    /// 按当前身份编码整组报文; passive(隐身)模式只收不发, 全部置空
    fn encode(info: &PeerInfo, tcp_port: u16, passive: bool) -> Self {
        if passive {
            return Self {
                announce: Vec::new(),
                response: Vec::new(),
                goodbye: Vec::new(),
            };
        }
        Self {
            announce: encode_packet(AnnounceKind::Announce, info, tcp_port),
            response: encode_packet(AnnounceKind::Response, info, tcp_port),
            goodbye: encode_packet(AnnounceKind::Goodbye, info, tcp_port),
        }
    }
}

/// 报文组共享句柄
type SharedPackets = Arc<std::sync::RwLock<UdpPackets>>;

/// 读报文组(毒锁直接恢复内部数据)
fn read_packets(packets: &SharedPackets) -> std::sync::RwLockReadGuard<'_, UdpPackets> {
    packets.read().unwrap_or_else(PoisonError::into_inner)
}

/// 节点注册表: 聚合 mDNS 与 UDP 两个来源, 维护在线状态并分发事件
struct Registry {
    /// 本机指纹(过滤自见)
    self_fingerprint: String,
    /// 在线节点表, key = 证书指纹
    peers: Mutex<HashMap<String, PeerState>>,
    /// 事件发送端(满时丢弃, 消费方可用快照兜底)
    events: mpsc::Sender<PeerEvent>,
}

/// 节点信息的来源通道
#[derive(Clone, Copy, PartialEq, Eq)]
enum PeerSource {
    /// mDNS 浏览: 事件驱动, 服务稳定期间不会重复产生 Resolved 事件
    Mdns,
    /// UDP 组播: 心跳驱动, 每个心跳周期刷新
    Udp,
}

/// 注册表内的节点状态
///
/// 存活按来源通道分别维护: mDNS 只能由 ServiceRemoved(goodbye/TTL
/// 过期)判死, 不能按时间超时 —— 它没有周期心跳可刷新; UDP 超过
/// 超时窗口无心跳即判死。两个通道都死了节点才下线, 否则在组播被
/// 网络拦截(仅 mDNS 可达)的环境里节点会在 15s 后被误踢。
struct PeerState {
    /// 节点信息
    peer: Peer,
    /// 最近一次 UDP 心跳时间(从未经 UDP 见到为 None)
    last_udp: Option<Instant>,
    /// mDNS 通道在线(ServiceResolved 置位, ServiceRemoved 清除)
    mdns_alive: bool,
    /// 最近一次 TCP 探活的发起时刻(节流用; 上线时视作刚探过)
    last_probe: Option<Instant>,
}

impl Registry {
    /// 取锁; 毒锁直接恢复内部数据(表内无跨线程不变量)
    fn lock_peers(&self) -> MutexGuard<'_, HashMap<String, PeerState>> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 新增或刷新节点; 信息有变化才发 Up 事件, 心跳只刷新时间
    ///
    /// 地址合并: mDNS(多地址)与 UDP(单一源地址)两个通道互补,
    /// 合并去重后统一规整排序(IPv4 优先, 稳定排序保证不因通道交替抖动);
    /// 规整后无可用地址的更新直接丢弃, 等待后续携带可用地址的事件。
    /// 过期地址不逐个剔除, 依赖节点整体超时下线后重建列表。
    fn upsert(&self, mut peer: Peer, source: PeerSource) {
        if peer.info.fingerprint == self.self_fingerprint {
            return;
        }
        let mut peers = self.lock_peers();
        let fingerprint = peer.info.fingerprint.clone();
        // 继承另一通道的存活标志: 新来源只增强存活, 不清除对方
        let (changed, mut last_udp, mut mdns_alive, last_probe) = match peers.get(&fingerprint) {
            Some(state) => {
                let mut merged = state.peer.addrs.clone();
                for addr in &peer.addrs {
                    if !merged.contains(addr) {
                        merged.push(*addr);
                    }
                }
                peer.addrs = normalize_addrs(merged);
                (
                    state.peer != peer,
                    state.last_udp,
                    state.mdns_alive,
                    state.last_probe,
                )
            }
            None => {
                peer.addrs = normalize_addrs(peer.addrs);
                // 上线本身即活性证明, 视作刚探过: 首次探活等满一个完整间隔
                (true, None, false, Some(Instant::now()))
            }
        };
        if peer.addrs.is_empty() {
            return;
        }
        match source {
            PeerSource::Udp => last_udp = Some(Instant::now()),
            PeerSource::Mdns => mdns_alive = true,
        }
        peers.insert(
            fingerprint,
            PeerState {
                peer: peer.clone(),
                last_udp,
                mdns_alive,
                last_probe,
            },
        );
        drop(peers);
        if changed {
            self.emit(PeerEvent::Up(peer));
        }
    }

    /// 按指纹移除节点并发 Down 事件
    fn remove(&self, fingerprint: &str) {
        let existed = self.lock_peers().remove(fingerprint).is_some();
        if existed {
            self.emit(PeerEvent::Down(fingerprint.to_string()));
        }
    }

    /// mDNS 服务消失(goodbye 或 TTL 过期): 清除 mDNS 存活标志
    ///
    /// UDP 心跳仍然新鲜时保留节点(单通道退化不该闪下线), 否则立即
    /// 移除。ServiceRemoved 只给出 instance 名即设备 ID。
    fn mdns_removed(&self, device_id: &str, udp_timeout: Duration) {
        let mut peers = self.lock_peers();
        let Some((fp, state)) = peers
            .iter_mut()
            .find(|(_, s)| s.peer.info.device_id == device_id)
        else {
            return;
        };
        state.mdns_alive = false;
        let udp_dead = state.last_udp.is_none_or(|t| t.elapsed() > udp_timeout);
        let fp = fp.clone();
        drop(peers);
        if udp_dead {
            self.remove(&fp);
        }
    }

    /// 清理已死节点, 并返回需要 TCP 探活的可疑节点清单
    ///
    /// - 清理: mDNS 不在线且 UDP 心跳超时(或从未有过)→ 直接移除;
    ///   mDNS 在线的节点不按时间踢 —— 它的下线由 ServiceRemoved 驱动
    /// - 探活: "仅 mDNS 在线且 UDP 静默"的节点无法靠时间判死(mDNS 无
    ///   周期心跳, 崩溃节点要等 SRV TTL ≈2min 过期), 挑出来交调用方
    ///   连接探测。此处顺带打探测时间戳(按 [`PEER_PROBE_INTERVAL`]
    ///   节流), 返回即"该探了"
    fn sweep(&self, timeout: Duration) -> Vec<Peer> {
        let now = Instant::now();
        let mut probes = Vec::new();
        let expired: Vec<String> = {
            let mut peers = self.lock_peers();
            for state in peers.values_mut() {
                let udp_silent = state.last_udp.is_none_or(|t| t.elapsed() > timeout);
                let probe_due = state
                    .last_probe
                    .is_none_or(|t| t.elapsed() > PEER_PROBE_INTERVAL);
                if state.mdns_alive && udp_silent && probe_due {
                    state.last_probe = Some(now);
                    probes.push(state.peer.clone());
                }
            }
            peers
                .iter()
                .filter(|(_, s)| !s.mdns_alive && s.last_udp.is_none_or(|t| t.elapsed() > timeout))
                .map(|(fp, _)| fp.clone())
                .collect()
        };
        for fp in expired {
            tracing::debug!(fingerprint = %fp, "节点心跳超时, 判定下线");
            self.remove(&fp);
        }
        probes
    }

    /// 探活失败的落地: 清除 mDNS 存活标志并移除节点
    ///
    /// 探测期间(2s 窗口)节点可能刚经 UDP 复活(如网络闪断恢复),
    /// 心跳仍新鲜时以新证据为准保留 —— 探测结论只否定"mDNS 独证的存活"。
    fn probe_failed(&self, fingerprint: &str, udp_timeout: Duration) {
        let mut peers = self.lock_peers();
        let Some(state) = peers.get_mut(fingerprint) else {
            return;
        };
        if state.last_udp.is_some_and(|t| t.elapsed() <= udp_timeout) {
            return;
        }
        state.mdns_alive = false;
        drop(peers);
        tracing::info!(fingerprint = %fingerprint, "TCP 探活失败, 判定节点已崩溃下线");
        self.remove(fingerprint);
    }

    /// 当前在线节点快照
    fn snapshot(&self) -> Vec<Peer> {
        self.lock_peers().values().map(|s| s.peer.clone()).collect()
    }

    /// 发送事件; 通道满则丢弃(消费方可随时用快照校正)
    fn emit(&self, event: PeerEvent) {
        if let Err(e) = self.events.try_send(event) {
            tracing::debug!("节点事件通道已满, 丢弃事件: {e}");
        }
    }
}

/// 广播侧的可变状态(隐身热切换与身份热更新共同维护, 单锁保一致)
struct BroadcastState {
    /// 当前广播身份(退出隐身重编码报文组用)
    info: PeerInfo,
    /// 隐身模式: 只收不发
    passive: bool,
    /// 本机 mDNS 服务全名(已注册时 Some, unregister 用)
    mdns_fullname: Option<String>,
}

/// 发现服务: 注册本机 + 监听全网, 通过事件通道上报节点变化
pub struct DiscoveryService {
    /// 节点注册表
    registry: Arc<Registry>,
    /// mDNS daemon(初始化失败则为 None, 降级为纯 UDP)
    mdns: Option<mdns_sd::ServiceDaemon>,
    /// UDP 组播 socket(初始化失败则为 None, 降级为纯 mDNS)
    udp: Option<Arc<UdpSocket>>,
    /// UDP 目标地址(组播组 + 端口)
    udp_target: (Ipv4Addr, u16),
    /// UDP 报文组(身份热更新时整组替换)
    packets: SharedPackets,
    /// 广播的 TCP 端口(mDNS 重注册用, 启动后不变)
    tcp_port: u16,
    /// 广播身份 / 隐身开关 / mDNS 注册名(运行时可变)
    state: Mutex<BroadcastState>,
    /// 后台任务句柄(shutdown 时中止)
    tasks: Vec<JoinHandle<()>>,
}

impl DiscoveryService {
    /// 启动发现服务: 注册本机信息并开始监听, 返回服务句柄与节点事件流
    ///
    /// `passive` 为 true 时只监听不广播(scan/send 等短暂场景,
    /// 不注册 mDNS、不发 announce/response/goodbye), 即"隐身"模式。
    pub async fn start(
        info: PeerInfo,
        tcp_port: u16,
        discovery_port: u16,
        passive: bool,
    ) -> Result<(Self, mpsc::Receiver<PeerEvent>), DiscoveryError> {
        let (events_tx, events_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let registry = Arc::new(Registry {
            self_fingerprint: info.fingerprint.clone(),
            peers: Mutex::new(HashMap::new()),
            events: events_tx,
        });
        let mut tasks = Vec::new();

        // 通道一: mDNS 注册 + 浏览
        let (mdns, mdns_fullname) =
            match start_mdns(&info, tcp_port, passive, &registry, &mut tasks) {
                Ok(pair) => (Some(pair.0), pair.1),
                Err(e) => {
                    tracing::warn!("mDNS 初始化失败, 降级为纯 UDP 组播: {e}");
                    (None, None)
                }
            };

        // 通道二: UDP 组播 announce/response(报文组共享, 身份热更新时整组替换)
        let packets: SharedPackets = Arc::new(std::sync::RwLock::new(UdpPackets::encode(
            &info, tcp_port, passive,
        )));
        let udp_target = (MULTICAST_GROUP, discovery_port);
        let udp = match start_udp(
            &info,
            discovery_port,
            Arc::clone(&packets),
            &registry,
            &mut tasks,
        )
        .await
        {
            Ok(socket) => Some(socket),
            Err(e) => {
                // 常见于同机多实例(端口被占), 此时 mDNS 仍可互见
                tracing::warn!("UDP 组播初始化失败, 降级为纯 mDNS: {e}");
                None
            }
        };

        if mdns.is_none() && udp.is_none() {
            return Err(DiscoveryError::AllChannelsFailed);
        }

        // 超时清理 + 崩溃探活任务(探测并发进行, 不阻塞清扫节奏)
        let sweeper = Arc::clone(&registry);
        tasks.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
            loop {
                tick.tick().await;
                for peer in sweeper.sweep(PEER_TIMEOUT) {
                    tokio::spawn(probe_peer(Arc::clone(&sweeper), peer));
                }
            }
        }));

        // 睡眠唤醒自愈任务(UDP 通道可用时)
        if let Some(socket) = &udp {
            tasks.push(tokio::spawn(sleep_watchdog(
                Arc::clone(socket),
                udp_target,
                Arc::clone(&packets),
            )));
        }

        Ok((
            Self {
                registry,
                mdns,
                udp,
                udp_target,
                packets,
                tcp_port,
                state: Mutex::new(BroadcastState {
                    info,
                    passive,
                    mdns_fullname,
                }),
                tasks,
            },
            events_rx,
        ))
    }

    /// 热切换隐身模式(即时生效, 不中断发现服务; 与当前状态相同时为空操作)
    ///
    /// 开启: 先发 goodbye 让对端立即下线, 再清空 UDP 报文组(心跳静默)、
    /// 注销 mDNS 服务(对端经 ServiceRemoved 同步下线);
    /// 关闭: 重编码报文组、重新注册 mDNS, 并立即 announce 加速对端可见。
    /// 与 update_info 同理可能在无 tokio runtime 的线程被调用
    /// (Tauri 同步命令的 IPC 线程), 全程只用同步/非阻塞接口。
    pub fn set_passive(&self, passive: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.passive == passive {
            return;
        }
        state.passive = passive;
        if passive {
            // 先取告别报文再置空报文组, 顺序反了就发不出去了;
            // UDP 不可靠, 连发两次提高送达率, 失败无碍(对端心跳超时兜底)
            let goodbye = read_packets(&self.packets).goodbye.clone();
            if let Some(udp) = &self.udp
                && !goodbye.is_empty()
            {
                for _ in 0..2 {
                    let _ = udp.try_send_to(&goodbye, self.udp_target.into());
                }
            }
            *self.packets.write().unwrap_or_else(PoisonError::into_inner) =
                UdpPackets::encode(&state.info, self.tcp_port, true);
            if let Some(daemon) = &self.mdns
                && let Some(fullname) = state.mdns_fullname.take()
                && let Err(e) = daemon.unregister(&fullname)
            {
                tracing::warn!("mDNS 注销失败(对端将等 TTL 过期才下线): {e}");
            }
            tracing::info!("已进入隐身模式(只收不发)");
        } else {
            *self.packets.write().unwrap_or_else(PoisonError::into_inner) =
                UdpPackets::encode(&state.info, self.tcp_port, false);
            if let Some(daemon) = &self.mdns {
                match build_mdns_service(&state.info, self.tcp_port) {
                    Ok(service) => {
                        let fullname = service.get_fullname().to_string();
                        match daemon.register(service) {
                            Ok(()) => state.mdns_fullname = Some(fullname),
                            Err(e) => tracing::warn!("mDNS 重新注册失败: {e}"),
                        }
                    }
                    Err(e) => tracing::warn!("mDNS 服务信息构造失败: {e}"),
                }
            }
            // 立即宣告一次, 对端第一时间看到本机(否则要等下个心跳周期)
            if let Some(udp) = &self.udp {
                let announce = read_packets(&self.packets).announce.clone();
                if !announce.is_empty()
                    && let Err(e) = udp.try_send_to(&announce, self.udp_target.into())
                {
                    tracing::debug!("退出隐身的即时 announce 发送失败(心跳会补发): {e}");
                }
            }
            tracing::info!("已退出隐身模式, 恢复广播");
        }
    }

    /// 热更新广播身份(昵称/头像变更即时生效, 不中断发现服务)
    ///
    /// 指纹与端口是身份根基不可改, 仅展示性字段(name/avatar)允许更新:
    /// - UDP: 整组重编码报文, 心跳/应答/告别随即使用新内容, 并立即补发
    ///   一次 announce 加速对端可见(否则要等下个心跳)
    /// - mDNS: 同名重复 register —— daemon 内部为覆盖语义, 会立刻广播
    ///   新 TXT 记录, 对端不会经历"下线再上线"
    pub fn update_info(&self, info: &PeerInfo) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.info = info.clone();
        *self.packets.write().unwrap_or_else(PoisonError::into_inner) =
            UdpPackets::encode(info, self.tcp_port, state.passive);
        if state.passive {
            return;
        }
        drop(state);
        if let Some(udp) = &self.udp {
            // 必须用同步接口: 本方法可能在无 tokio runtime 的线程被调用
            // (如 Tauri 同步命令的 IPC 线程), tokio::spawn 会直接 panic。
            // UDP 单报文 try_send_to 瞬时完成, 偶发失败由 5s 心跳自然兜底。
            let announce = read_packets(&self.packets).announce.clone();
            if !announce.is_empty()
                && let Err(e) = udp.try_send_to(&announce, self.udp_target.into())
            {
                tracing::debug!("身份更新的即时 announce 发送失败(心跳会补发): {e}");
            }
        }
        if let Some(daemon) = &self.mdns {
            match build_mdns_service(info, self.tcp_port) {
                Ok(service) => {
                    if let Err(e) = daemon.register(service) {
                        tracing::warn!("mDNS 身份更新失败: {e}");
                    }
                }
                Err(e) => tracing::warn!("mDNS 服务信息构造失败: {e}"),
            }
        }
    }

    /// 当前在线节点快照(事件流之外的兜底查询)
    pub fn peers(&self) -> Vec<Peer> {
        self.registry.snapshot()
    }

    /// 按证书指纹查询单个在线节点(只克隆命中项, 避免全表快照)
    pub fn peer_by_fingerprint(&self, fingerprint: &str) -> Option<Peer> {
        self.registry
            .lock_peers()
            .get(fingerprint)
            .map(|s| s.peer.clone())
    }

    /// 优雅关闭: 发 goodbye、注销 mDNS、停掉后台任务(幂等, 可多次调用)
    pub async fn shutdown(&self) {
        if let Some(udp) = &self.udp {
            // UDP 不可靠, goodbye 连发两次提高送达率(passive 模式报文为空不发)
            let goodbye = read_packets(&self.packets).goodbye.clone();
            if !goodbye.is_empty() {
                for _ in 0..2 {
                    let _ = udp.send_to(&goodbye, self.udp_target).await;
                }
            }
        }
        if let Some(mdns) = &self.mdns {
            // 隐身期间 fullname 为 None(已注销), 但 daemon 的浏览仍需关停
            let fullname = self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .mdns_fullname
                .take();
            if let Some(fullname) = fullname {
                let _ = mdns.unregister(&fullname);
            }
            let _ = mdns.shutdown();
        }
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// TCP 探活一个"仅 mDNS 在线"的可疑节点: 逐个候选地址尝试连接其传输端口
///
/// 任一地址连接成功即判活(端口有监听 = 进程活着), 随即断开 —— 不做
/// TLS 不发数据, 一次三次握手足矣; 全部被拒/超时则确认死亡。崩溃进程的
/// 监听端口被 OS 立即回收(connect 秒收 RST), 拔线/断电等无应答场景由
/// [`PEER_PROBE_TIMEOUT`] 兜底。
async fn probe_peer(registry: Arc<Registry>, peer: Peer) {
    for addr in &peer.addrs {
        // 连接成功即证明进程在(drop 即断开); 被拒/超时则换下一个候选地址
        if let Ok(Ok(_alive)) = tokio::time::timeout(
            PEER_PROBE_TIMEOUT,
            tokio::net::TcpStream::connect((*addr, peer.port)),
        )
        .await
        {
            return;
        }
    }
    registry.probe_failed(&peer.info.fingerprint, PEER_TIMEOUT);
}

/// 睡眠唤醒自愈: 检测系统睡眠恢复后重新加入组播组并立即宣告
///
/// 系统睡眠会静默丢失 IGMP 组播成员关系 —— 唤醒后本机收不到别人的
/// announce、别人也发现不了本机, 发现层就此失灵且无任何报错。
/// 检测依据: 睡眠期间单调钟(Instant)停摆而墙钟(SystemTime)照走,
/// 两者前进量之差即"停摆时长", 超过阈值判定睡过。
/// (mDNS daemon 自带网络接口监控, 唤醒自愈交由其内部处理;
/// Windows 的单调钟计入睡眠时间, 检测不到时靠心跳超时自然收敛。)
async fn sleep_watchdog(udp: Arc<UdpSocket>, target: (Ipv4Addr, u16), packets: SharedPackets) {
    /// 检测周期
    const TICK: Duration = Duration::from_secs(30);
    /// 停摆超过该时长判定为睡眠恢复(NTP 校时的偏移远小于此, 不会误报)
    const STALL_JUMP: Duration = Duration::from_secs(60);
    let mut wall = std::time::SystemTime::now();
    let mut mono = Instant::now();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        let wall_gap = std::time::SystemTime::now()
            .duration_since(wall)
            .unwrap_or_default();
        let mono_gap = mono.elapsed();
        wall = std::time::SystemTime::now();
        mono = Instant::now();
        let stalled = wall_gap.saturating_sub(mono_gap);
        if stalled < STALL_JUMP {
            continue;
        }
        tracing::info!(
            stalled_secs = stalled.as_secs(),
            "检测到系统睡眠恢复, 重建组播成员关系"
        );
        // 先退再进: socket 仍持有旧成员状态时重复 join 会报错
        let _ = udp.leave_multicast_v4(target.0, Ipv4Addr::UNSPECIFIED);
        if let Err(e) = udp.join_multicast_v4(target.0, Ipv4Addr::UNSPECIFIED) {
            tracing::warn!("重新加入组播组失败(等待下轮重试): {e}");
            continue;
        }
        // 立即宣告一次, 让对端第一时间看到本机(passive 模式报文为空不发)
        let announce = read_packets(&packets).announce.clone();
        if !announce.is_empty() {
            let _ = udp.send_to(&announce, target).await;
        }
    }
}

/// 构造本机的 mDNS 服务信息(首次注册与身份热更新共用)
///
/// instance 用 device_id 保证唯一且不随昵称变(fullname 稳定);
/// host 同名避免与真实主机名记录冲突。
fn build_mdns_service(
    info: &PeerInfo,
    tcp_port: u16,
) -> Result<mdns_sd::ServiceInfo, mdns_sd::Error> {
    let mut props: HashMap<String, String> = [
        ("id".to_string(), info.device_id.clone()),
        ("name".to_string(), info.name.clone()),
        ("fp".to_string(), info.fingerprint.clone()),
        ("platform".to_string(), info.platform.clone()),
    ]
    .into();
    // 头像为可选字段, 仅在设置了才广播(TXT 记录只增加几字节)
    if let Some(avatar) = &info.avatar {
        props.insert("avatar".to_string(), avatar.clone());
    }
    // OS 版本描述(协议 1.3 起, 可选)
    if let Some(osv) = &info.os_version {
        props.insert("osv".to_string(), osv.clone());
    }
    Ok(mdns_sd::ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &info.device_id,
        &format!("{}.local.", info.device_id),
        "",
        tcp_port,
        props,
    )?
    .enable_addr_auto())
}

/// 初始化 mDNS: 注册本机服务(passive 时跳过)并启动浏览任务
fn start_mdns(
    info: &PeerInfo,
    tcp_port: u16,
    passive: bool,
    registry: &Arc<Registry>,
    tasks: &mut Vec<JoinHandle<()>>,
) -> Result<(mdns_sd::ServiceDaemon, Option<String>), DiscoveryError> {
    let daemon = mdns_sd::ServiceDaemon::new()?;

    let fullname = if passive {
        None
    } else {
        let service = build_mdns_service(info, tcp_port)?;
        let name = service.get_fullname().to_string();
        daemon.register(service)?;
        Some(name)
    };

    let receiver = daemon.browse(MDNS_SERVICE_TYPE)?;
    let reg = Arc::clone(registry);
    tasks.push(tokio::spawn(async move {
        while let Ok(event) = receiver.recv_async().await {
            match event {
                mdns_sd::ServiceEvent::ServiceResolved(svc) => {
                    if let Some(peer) = peer_from_mdns(&svc) {
                        reg.upsert(peer, PeerSource::Mdns);
                    }
                }
                mdns_sd::ServiceEvent::ServiceRemoved(_ty, fullname) => {
                    if let Some(device_id) = instance_of(&fullname) {
                        reg.mdns_removed(device_id, PEER_TIMEOUT);
                    }
                }
                _ => {}
            }
        }
    }));

    Ok((daemon, fullname))
}

/// 从 mDNS 解析结果构造 Peer; 字段不全(非 deskmate 服务)返回 None
fn peer_from_mdns(svc: &mdns_sd::ResolvedService) -> Option<Peer> {
    let info = PeerInfo {
        device_id: svc.get_property_val_str("id")?.to_string(),
        name: svc.get_property_val_str("name")?.to_string(),
        fingerprint: svc.get_property_val_str("fp")?.to_string(),
        platform: svc.get_property_val_str("platform")?.to_string(),
        avatar: svc.get_property_val_str("avatar").map(str::to_string),
        os_version: svc.get_property_val_str("osv").map(str::to_string),
    };
    // 多网卡会有多个地址: 规整后保留为候选(ScopedIp 携带 scope id, 这里仍只取裸地址)
    let addrs = normalize_addrs(svc.addresses.iter().map(|ip| ip.to_ip_addr()).collect());
    if addrs.is_empty() {
        return None;
    }
    Some(Peer {
        info,
        addrs,
        port: svc.port,
    })
}

/// 规整候选地址: 滤掉无法直连的 IPv6 链路本地地址, 非回环 IPv4 排前
///
/// 结果可能为空(如 mDNS 先到的解析事件只带 AAAA 记录), 此时调用方应
/// 丢弃本次更新, 等待带可用地址的后续事件 —— 决不能把废地址放回列表。
fn normalize_addrs(all: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut addrs: Vec<IpAddr> = all.into_iter().filter(|ip| !is_link_local_v6(ip)).collect();
    // 稳定排序: 同优先级保持先到顺序
    addrs.sort_by_key(|ip| (ip.is_loopback(), !ip.is_ipv4()));
    addrs
}

/// IPv6 链路本地地址(fe80::/10): 缺少 scope id 无法直连, 视为不可用
fn is_link_local_v6(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        IpAddr::V4(_) => false,
    }
}

/// 从 mDNS fullname 提取 instance 名(即设备 ID): 去掉服务类型后缀与尾部分隔点
fn instance_of(fullname: &str) -> Option<&str> {
    fullname
        .strip_suffix(MDNS_SERVICE_TYPE)
        .map(|s| s.trim_end_matches('.'))
}

/// 建组播 UDP socket: 开启地址复用后绑定发现端口
///
/// SO_REUSEADDR(unix 另加 SO_REUSEPORT)让同机多实例都能收到组播、
/// 进程快速重启不受 TIME_WAIT 影响 —— 组播语义下所有复用绑定的
/// socket 各自收到一份报文, 不存在抢包。
fn bind_multicast_socket(discovery_port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&std::net::SocketAddr::from((Ipv4Addr::UNSPECIFIED, discovery_port)).into())?;
    UdpSocket::from_std(sock.into())
}

/// 初始化 UDP 组播: 加入组播组, 启动心跳与收包任务
///
/// 报文经共享组读取(身份热更新即时生效); passive 模式下报文组为空,
/// 心跳与应答自然静默(只收不发)。
async fn start_udp(
    info: &PeerInfo,
    discovery_port: u16,
    packets: SharedPackets,
    registry: &Arc<Registry>,
    tasks: &mut Vec<JoinHandle<()>>,
) -> Result<Arc<UdpSocket>, DiscoveryError> {
    let socket = bind_multicast_socket(discovery_port)?;
    socket.join_multicast_v4(MULTICAST_GROUP, Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_loop_v4(true)?;
    let socket = Arc::new(socket);

    let sock = Arc::clone(&socket);
    let reg = Arc::clone(registry);
    let self_fp = info.fingerprint.clone();
    tasks.push(tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let announce = read_packets(&packets).announce.clone();
                    if announce.is_empty() {
                        continue;
                    }
                    if let Err(e) = sock.send_to(&announce, (MULTICAST_GROUP, discovery_port)).await {
                        tracing::debug!("UDP announce 发送失败: {e}");
                    }
                }
                recv = sock.recv_from(&mut buf) => {
                    let Ok((n, src)) = recv else { continue };
                    let Ok(packet) = serde_json::from_slice::<AnnouncePacket>(&buf[..n]) else {
                        continue;
                    };
                    if packet.info.fingerprint == self_fp {
                        continue;
                    }
                    match packet.kind {
                        AnnounceKind::Goodbye => reg.remove(&packet.info.fingerprint),
                        kind => {
                            reg.upsert(
                                Peer {
                                    info: packet.info,
                                    addrs: vec![src.ip()],
                                    port: packet.tcp_port,
                                },
                                PeerSource::Udp,
                            );
                            // 收到 announce 回单播 response, 让新节点立刻看到自己
                            if kind == AnnounceKind::Announce {
                                let response = read_packets(&packets).response.clone();
                                if !response.is_empty() {
                                    let _ = sock.send_to(&response, src).await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }));

    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用注册表与事件接收端
    fn test_registry(self_fp: &str) -> (Arc<Registry>, mpsc::Receiver<PeerEvent>) {
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        (
            Arc::new(Registry {
                self_fingerprint: self_fp.to_string(),
                peers: Mutex::new(HashMap::new()),
                events: tx,
            }),
            rx,
        )
    }

    /// 构造测试节点
    fn test_peer(fp: &str, name: &str) -> Peer {
        Peer {
            info: PeerInfo {
                device_id: format!("dev-{fp}"),
                name: name.to_string(),
                fingerprint: fp.to_string(),
                platform: "macos".to_string(),
                avatar: None,
                os_version: None,
            },
            addrs: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2))],
            port: 42424,
        }
    }

    /// 重复心跳不应重复发 Up; 信息变化(如改名)应再次发 Up
    #[tokio::test]
    async fn upsert_emits_only_on_change() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "old"), PeerSource::Udp);
        reg.upsert(test_peer("aaa", "old"), PeerSource::Udp); // 心跳: 无变化
        reg.upsert(test_peer("aaa", "new"), PeerSource::Udp); // 改名: 有变化
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.info.name == "old"));
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.info.name == "new"));
        assert!(rx.try_recv().is_err());
    }

    /// 本机自见报文必须被过滤
    #[tokio::test]
    async fn self_is_filtered() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("self", "me"), PeerSource::Udp);
        assert!(rx.try_recv().is_err());
        assert!(reg.snapshot().is_empty());
    }

    /// 多通道地址合并: 新地址触发一次 Up, 单地址心跳不再抖动, 顺序保持稳定
    #[tokio::test]
    async fn upsert_merges_addrs_stably() {
        let (reg, mut rx) = test_registry("self");
        let addr_a = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        let addr_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        // UDP 先报单地址 A
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.addrs == vec![addr_a]));

        // mDNS 报 [B, A]: 以既有顺序为基合并成 [A, B], 新地址 B 触发 Up
        let mut peer = test_peer("aaa", "n");
        peer.addrs = vec![addr_b, addr_a];
        reg.upsert(peer, PeerSource::Mdns);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.addrs == vec![addr_a, addr_b]));

        // UDP 心跳再报单地址 A: 合并结果不变, 不得再发事件
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        assert!(rx.try_recv().is_err());
    }

    /// remove 只对存在的节点发 Down
    #[tokio::test]
    async fn remove_emits_down_once() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("bbb", "b"), PeerSource::Udp);
        let _ = rx.try_recv();
        reg.remove("bbb");
        reg.remove("bbb"); // 已不存在, 不应再发
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "bbb"));
        assert!(rx.try_recv().is_err());
    }

    /// 回归(线上): 仅 mDNS 可达(UDP 组播被网络拦截)时,
    /// 节点不得被心跳超时误踢 —— mDNS 没有周期事件可刷新时间戳
    #[tokio::test]
    async fn mdns_peer_survives_sweep() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Mdns);
        let _ = rx.try_recv();
        // timeout 为 0: 凡按时间判死的节点都会被踢, mDNS 在线节点必须幸免
        reg.sweep(Duration::ZERO);
        assert!(rx.try_recv().is_err());
        assert_eq!(reg.snapshot().len(), 1);
    }

    /// 纯 UDP 节点心跳超时后应被清理(原有语义不变)
    #[tokio::test]
    async fn udp_peer_swept_after_timeout() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        let _ = rx.try_recv();
        reg.sweep(Duration::ZERO);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "aaa"));
        assert!(reg.snapshot().is_empty());
    }

    /// mDNS 服务消失且无 UDP 心跳: 节点立即下线
    #[tokio::test]
    async fn mdns_removed_downs_peer_without_udp() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Mdns);
        let _ = rx.try_recv();
        reg.mdns_removed("dev-aaa", PEER_TIMEOUT);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "aaa"));
        assert!(reg.snapshot().is_empty());
    }

    /// mDNS 消失但 UDP 心跳仍新鲜: 节点保留, 待 UDP 超时才随 sweep 下线
    #[tokio::test]
    async fn mdns_removed_keeps_peer_with_live_udp() {
        let (reg, mut rx) = test_registry("self");
        reg.upsert(test_peer("aaa", "n"), PeerSource::Mdns);
        reg.upsert(test_peer("aaa", "n"), PeerSource::Udp);
        let _ = rx.try_recv();
        // UDP 心跳在超时窗口内: 不下线
        reg.mdns_removed("dev-aaa", PEER_TIMEOUT);
        assert!(rx.try_recv().is_err());
        assert_eq!(reg.snapshot().len(), 1);
        // UDP 也超时(timeout=0)后由 sweep 收尾
        reg.sweep(Duration::ZERO);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "aaa"));
    }

    /// UDP 报文序列化往返
    #[test]
    fn announce_packet_roundtrip() {
        let packet = AnnouncePacket {
            kind: AnnounceKind::Announce,
            info: test_peer("ccc", "c").info,
            tcp_port: 42424,
        };
        let bytes = serde_json::to_vec(&packet).unwrap();
        let back: AnnouncePacket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.kind, AnnounceKind::Announce);
        assert_eq!(back.info.fingerprint, "ccc");
        assert_eq!(back.tcp_port, 42424);
    }

    /// 地址规整: 滤掉 fe80:: 链路本地, 非回环 IPv4 排前; 纯链路本地时返回空
    #[test]
    fn normalize_addrs_filters_and_sorts() {
        use std::net::Ipv6Addr;
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        let lo4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let ll6 = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
        let lo6 = IpAddr::V6(Ipv6Addr::LOCALHOST);

        // 链路本地被滤掉, 非回环 IPv4 > 回环 IPv4 > 回环 IPv6
        assert_eq!(normalize_addrs(vec![ll6, lo6, lo4, v4]), vec![v4, lo4, lo6]);
        // 只剩链路本地时返回空(调用方丢弃本次更新)
        assert_eq!(normalize_addrs(vec![ll6]), Vec::<IpAddr>::new());
    }

    /// 复现线上问题: mDNS 首个事件只带 fe80:: 时不得上报节点,
    /// 待 IPv4 事件到达后才上报, 且 fe80:: 不得混入候选列表
    #[tokio::test]
    async fn link_local_only_peer_is_deferred() {
        use std::net::Ipv6Addr;
        let (reg, mut rx) = test_registry("self");
        let ll6 = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));

        // 只有链路本地地址: 不进注册表、不发事件
        let mut peer = test_peer("aaa", "n");
        peer.addrs = vec![ll6];
        reg.upsert(peer, PeerSource::Mdns);
        assert!(rx.try_recv().is_err());
        assert!(reg.snapshot().is_empty());

        // IPv4 到达: 正常上报, 列表只含可用地址
        let mut peer = test_peer("aaa", "n");
        peer.addrs = vec![ll6, v4];
        reg.upsert(peer, PeerSource::Mdns);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Up(p)) if p.addrs == vec![v4]));
    }

    /// 从 mDNS fullname 提取 instance(设备 ID)
    #[test]
    fn instance_extraction() {
        assert_eq!(
            instance_of("uuid-1234._deskmate._tcp.local."),
            Some("uuid-1234")
        );
        assert_eq!(instance_of("._deskmate._tcp.local."), Some(""));
    }

    /// sweep 只挑"仅 mDNS 在线且 UDP 静默"且过了节流间隔的节点探活
    #[tokio::test]
    async fn sweep_flags_stale_mdns_only_peers_for_probe() {
        let (reg, _rx) = test_registry("self");
        // aaa: mDNS-only —— 探活对象(上线视作刚探过, 抹掉时间戳模拟间隔已满)
        reg.upsert(test_peer("aaa", "a"), PeerSource::Mdns);
        // bbb: UDP 心跳新鲜 —— 不探
        reg.upsert(test_peer("bbb", "b"), PeerSource::Udp);
        reg.lock_peers().get_mut("aaa").unwrap().last_probe = None;

        let probes = reg.sweep(PEER_TIMEOUT);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].info.fingerprint, "aaa");
        // 时间戳已打上: 下一轮清扫不得重复探测(节流)
        assert!(reg.sweep(PEER_TIMEOUT).is_empty());
        // 探活对象不被清理(mDNS 仍在线)
        assert_eq!(reg.snapshot().len(), 2);
    }

    /// 探活失败的落地: UDP 心跳仍新鲜则以新证据为准保留, 纯 mDNS 节点移除
    #[tokio::test]
    async fn probe_failed_respects_fresh_udp() {
        let (reg, mut rx) = test_registry("self");
        // 双通道在线: 等价于"探测的 2s 窗口内 UDP 复活", 保留
        reg.upsert(test_peer("aaa", "a"), PeerSource::Mdns);
        reg.upsert(test_peer("aaa", "a"), PeerSource::Udp);
        let _ = rx.try_recv();
        reg.probe_failed("aaa", PEER_TIMEOUT);
        assert_eq!(reg.snapshot().len(), 1);
        assert!(rx.try_recv().is_err());

        // 纯 mDNS: 探活失败即下线
        reg.upsert(test_peer("ccc", "c"), PeerSource::Mdns);
        let _ = rx.try_recv();
        reg.probe_failed("ccc", PEER_TIMEOUT);
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "ccc"));
        assert_eq!(reg.snapshot().len(), 1);
    }

    /// 端到端(真实 TCP): 监听在的节点探活后保留, 监听已消失的节点被移除
    #[tokio::test]
    async fn probe_keeps_live_and_removes_dead() {
        let (reg, mut rx) = test_registry("self");

        // 活节点: 真实监听中的端口
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let mut live = test_peer("aaa", "live");
        live.addrs = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        live.port = listener.local_addr().unwrap().port();
        reg.upsert(live.clone(), PeerSource::Mdns);

        // 死节点: bind 拿到端口随即关闭(模拟进程崩溃后端口被 OS 回收)
        let dead_port = {
            let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            l.local_addr().unwrap().port()
        };
        let mut dead = test_peer("bbb", "dead");
        dead.addrs = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        dead.port = dead_port;
        reg.upsert(dead.clone(), PeerSource::Mdns);
        while rx.try_recv().is_ok() {}

        probe_peer(Arc::clone(&reg), live).await;
        probe_peer(Arc::clone(&reg), dead).await;

        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Down(fp)) if fp == "bbb"));
        let snapshot = reg.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].info.fingerprint, "aaa");
        drop(listener);
    }
}
