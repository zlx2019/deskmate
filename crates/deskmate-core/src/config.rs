//! 引擎调优常量: 心跳/超时/缓冲区等散落参数的集中定义
//!
//! 此前这些数值散落在各模块, 排查"该调哪个旋钮"需要跨文件检索;
//! 集中后一处可查、一处可调。若未来需要运行时配置(设置页/CLI 参数),
//! 以本模块为字段清单升级为注入式配置结构, 常量转为其默认值。
//!
//! 端口与协议侧上限不在此列: 端口默认值(`DEFAULT_TCP_PORT` 等)在 crate 根,
//! 帧/头像大小上限是双端一致的协议合同, 定义在 [`crate::protocol`]。

use std::time::Duration;

// ---- 发现层(discovery)----

/// 心跳间隔: UDP 组播 announce 的周期
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// 节点超时: 超过该时长未见心跳即判定下线(容忍连续丢 2 次心跳)
pub const PEER_TIMEOUT: Duration = Duration::from_secs(15);

/// 节点事件通道容量(满时丢弃, 消费方可用快照兜底)
pub const EVENT_CHANNEL_CAP: usize = 64;

// ---- 传输层(transfer)----

/// 数据通道单次读写块大小(1 MiB, 足以跑满 2.5GbE, 见 docs/PLAN.md 4.4)
pub const CHUNK_SIZE: usize = 1024 * 1024;

/// 等待接收方决策的超时时长(人在环上, 用长超时)
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(300);

/// 握手/应答类消息的等待时长
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// 单个候选地址的 TCP 连接超时(多网卡逐个尝试, 不宜过长)
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// 数据连接的内核收发缓冲上限(各平台默认多为 128-256KB, 高带宽或
/// WiFi 抖动下限制在途窗口; 上限按需增长, 并非立即占用内存)
pub const SOCKET_BUFFER_SIZE: usize = 4 * 1024 * 1024;

/// 旁路页缓存的文件大小阈值: 阈值以上的单次顺序传输不驻留系统页缓存
/// (macOS F_NOCACHE), 避免大文件把其他应用的热页挤掉;
/// 小文件保持走缓存 —— 接收完大概率马上被打开
pub const NOCACHE_THRESHOLD: u64 = 64 * 1024 * 1024;

/// 数据通道空闲上限: 单次 chunk 读/写超过该时长无进展即中断(保留断点可续传)
///
/// 暂停会经控制连接显式转告对端(协议 1.4 起双向 Pause/Resume 帧),
/// 双方 pump 都挂起等待、不吃本超时, 故无需再为"不可见暂停"放宽。
/// 不取更短的原因: 断点续传两端各自重放已传段进哈希器(纯本地 IO),
/// 磁盘速度差会让快端空等慢端, 需给这段差值留余量。
/// 该值同时是恶意"半开连接"占用资源的时间上限。
pub const DATA_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

// ---- 接收端连接治理(receiver)----

/// 并发连接数上限: 超出直接拒绝新连接(防 slow-loris 耗尽 fd/内存)
///
/// 正常场景每对端占 1-2 条(控制 + 数据), 128 足够宽松。
pub const MAX_CONCURRENT_CONNECTIONS: usize = 128;

/// 未认证阶段(TLS 握手 + 首帧)的超时, 挡住"连上后不说话"的占坑连接
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// 已接受但发送方一直未建数据连接的任务的存活上限, 到期清理(防泄漏)
pub const PENDING_TTL: Duration = Duration::from_secs(300);

/// 过期任务的清扫周期
pub const PENDING_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

// ---- 接收端 PIN 门(receiver)----

/// PIN 暴力破解限速窗口
pub const PIN_WINDOW: Duration = Duration::from_secs(60);

/// 窗口内允许的最大失败次数, 达到后该来源整窗一律拒绝
pub const PIN_MAX_FAILURES: u32 = 5;

/// 同时追踪失败计数的来源(TLS 指纹)上限, 超出保守拒绝新来源(防表膨胀)
pub const PIN_TRACK_CAP: usize = 1024;
