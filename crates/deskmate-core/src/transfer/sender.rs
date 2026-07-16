//! 发送端引擎: 发起控制会话、等待对方决策、经数据连接推送文件
//!
//! 会话流程(方案 4.2 节):
//! 1. 控制连接: TLS 握手(pin 指纹)→ Hello/HelloAck → TransferRequest
//! 2. 等待接收方决策(人在环上, 超时 5 分钟)
//! 3. 数据连接: DataHello → 逐文件 FileHeader + 原始字节流 + FileFooter
//! 4. 控制连接读半部持续监听对端的暂停/恢复/取消指令

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use rustls_pki_types::ServerName;
use tokio::io::{AsyncReadExt, ReadHalf, WriteHalf, split};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::PROTOCOL_VERSION;
use crate::identity::DeviceIdentity;
use crate::protocol::{
    ControlMessage, FileMeta, MAX_AVATAR_SIZE, PeerInfo, ProtocolError, check_version, read_frame,
    write_frame,
};
use crate::tls::{client_config, peer_fingerprint};

use super::{
    ControlState, EventSink, OFFER_TIMEOUT, TransferError, TransferEvent, collect_files,
    graceful_close, send_file_body,
};

use crate::config::{CONNECT_TIMEOUT, REPLY_TIMEOUT};

/// 发送结果摘要
#[derive(Debug)]
pub struct SendSummary {
    /// 传输任务 ID
    pub transfer_id: String,
    /// 对端设备信息
    pub peer: PeerInfo,
    /// 实际发送的文件数(对方可能部分接受)
    pub files_sent: usize,
    /// 实际发送的字节数
    pub bytes_sent: u64,
}

/// 发送文件/目录到目标节点, 直至传输结束
///
/// - `expected_fp`: Some 时严格校验对端证书指纹(来自发现层);
///   None 为 CLI 直连联调模式, 调用方必须向用户展示实际指纹
/// - `transfer_id`: Some 时使用调用方生成的任务 ID(便于启动前预注册控制通道),
///   None 时内部生成, 均通过 [`SendSummary`] 返回
/// - `pin`: 对端启用配对 PIN 时必须携带; 缺失或错误返回 [`TransferError::PinRequired`]
/// - `control`: 本端控制通道(暂停/继续/取消)
/// - `events`: 进度与结果事件流
#[expect(
    clippy::too_many_arguments,
    reason = "公开入口, 参数即一次传输的完整上下文"
)]
pub async fn send_files(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: Option<String>,
    pin: Option<String>,
    paths: &[PathBuf],
    control: watch::Receiver<ControlState>,
    events: mpsc::Sender<TransferEvent>,
) -> Result<SendSummary, TransferError> {
    let sink = EventSink::new(events);

    // 收集清单并编号
    let entries = collect_files_blocking(paths).await?;
    let files = build_manifest(&entries);
    let total_size: u64 = files.iter().map(|f| f.size).sum();
    let transfer_id = transfer_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // 控制连接 + 握手
    let (mut ctrl, peer) = connect_and_hello(identity, addrs, port, expected_fp.clone()).await?;

    let accepted = negotiate_offer(&mut ctrl, &transfer_id, files.clone(), total_size, pin).await?;

    // 发送计划: 被接受的文件全部从 0 起发
    let plan: Vec<SendItem> = files
        .iter()
        .filter(|f| accepted.contains(&f.file_id))
        .map(|f| SendItem {
            file_id: f.file_id,
            abs_path: entries[f.file_id as usize].0.clone(),
            rel_path: f.rel_path.clone(),
            size: f.size,
            offset: 0,
        })
        .collect();

    run_data_phase(
        identity,
        addrs,
        port,
        expected_fp,
        transfer_id,
        ctrl,
        peer,
        &plan,
        control,
        &sink,
    )
    .await
}

/// 在阻塞线程池中收集文件清单(目录递归遍历是同步 IO, 不能占用异步执行器)
async fn collect_files_blocking(
    paths: &[PathBuf],
) -> Result<Vec<(PathBuf, String, u64)>, TransferError> {
    let owned = paths.to_vec();
    tokio::task::spawn_blocking(move || collect_files(&owned))
        .await
        .map_err(|e| TransferError::Io(std::io::Error::other(e)))?
}

/// 把收集到的文件清单按顺序编号成协议元数据
fn build_manifest(entries: &[(PathBuf, String, u64)]) -> Vec<FileMeta> {
    entries
        .iter()
        .enumerate()
        .map(|(i, (_, rel, size))| FileMeta {
            file_id: u32::try_from(i).unwrap_or(u32::MAX),
            rel_path: rel.clone(),
            size: *size,
        })
        .collect()
}

/// 发送传输请求并等待接收方决策, 返回被接受的 file_id 集合
///
/// 人在环上, 用长超时; PIN 校验失败与整单拒绝分别映射为专用错误。
async fn negotiate_offer(
    ctrl: &mut TlsStream<TcpStream>,
    transfer_id: &str,
    files: Vec<FileMeta>,
    total_size: u64,
    pin: Option<String>,
) -> Result<HashSet<u32>, TransferError> {
    write_frame(
        ctrl,
        &ControlMessage::TransferRequest {
            transfer_id: transfer_id.to_string(),
            files,
            total_size,
            pin,
        },
    )
    .await?;
    let resp = tokio::time::timeout(OFFER_TIMEOUT, read_frame(ctrl))
        .await
        .map_err(|_| TransferError::Timeout("接收方决策"))??;
    let ControlMessage::TransferResponse {
        accepted_files,
        reason,
        pin_required,
        ..
    } = resp
    else {
        return Err(unexpected("transfer_response", &resp));
    };
    if pin_required {
        return Err(TransferError::PinRequired);
    }
    if accepted_files.is_empty() {
        return Err(TransferError::Rejected { reason });
    }
    Ok(accepted_files.into_iter().collect())
}

/// 续传意外中断的任务: 向接收端协商断点后仅补发缺失段
///
/// `transfer_id`/`paths` 必须与原次发送一致; 接收端按 rel_path 与 size
/// 校验对齐, 源文件有任何变化(改名/改大小)则拒绝续传(整文件哈希无法衔接)。
#[expect(
    clippy::too_many_arguments,
    reason = "公开入口, 参数即一次续传的完整上下文"
)]
pub async fn resume_send(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: &str,
    paths: &[PathBuf],
    control: watch::Receiver<ControlState>,
    events: mpsc::Sender<TransferEvent>,
) -> Result<SendSummary, TransferError> {
    let sink = EventSink::new(events);

    // 本地清单按相对路径索引(目录遍历顺序跨次不稳定, 不能依赖 file_id 对位)
    let entries = collect_files_blocking(paths).await?;
    let by_rel: HashMap<&str, (&PathBuf, u64)> = entries
        .iter()
        .map(|(abs, rel, size)| (rel.as_str(), (abs, *size)))
        .collect();

    // 断点协商
    let (mut ctrl, peer) = connect_and_hello(identity, addrs, port, expected_fp.clone()).await?;
    write_frame(
        &mut ctrl,
        &ControlMessage::ResumeQuery {
            transfer_id: transfer_id.to_string(),
        },
    )
    .await?;
    let resp = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut ctrl))
        .await
        .map_err(|_| TransferError::Timeout("断点应答"))??;
    let ControlMessage::ResumeInfo { files, .. } = resp else {
        return Err(unexpected("resume_info", &resp));
    };
    if files.is_empty() {
        return Err(TransferError::ResumeUnavailable(
            "对端没有该任务的断点数据(可能已完成或元数据丢失)".to_string(),
        ));
    }

    // 与本地文件对齐校验后生成补发计划
    let mut plan = Vec::with_capacity(files.len());
    for state in &files {
        let Some((abs, size)) = by_rel.get(state.rel_path.as_str()) else {
            return Err(TransferError::ResumeUnavailable(format!(
                "本地缺少文件: {}",
                state.rel_path
            )));
        };
        if *size != state.size {
            return Err(TransferError::ResumeUnavailable(format!(
                "源文件已变化: {}",
                state.rel_path
            )));
        }
        plan.push(SendItem {
            file_id: state.file_id,
            abs_path: (*abs).clone(),
            rel_path: state.rel_path.clone(),
            size: state.size,
            offset: state.received.min(state.size),
        });
    }

    run_data_phase(
        identity,
        addrs,
        port,
        expected_fp,
        transfer_id.to_string(),
        ctrl,
        peer,
        &plan,
        control,
        &sink,
    )
    .await
}

/// 数据阶段公共骨架: 拆分控制连接监听对端指令 → 推送数据 → 收尾上报终态
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即会话上下文")]
async fn run_data_phase(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: String,
    ctrl: TlsStream<TcpStream>,
    peer: PeerInfo,
    plan: &[SendItem],
    control: watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<SendSummary, TransferError> {
    // 控制连接拆分: 读半部监听对端指令, 写半部转告本端指令(结束时由其发 Bye)
    let (ctrl_read, ctrl_write) = split(ctrl);
    let (remote_tx, remote_rx) = watch::channel(ControlState::Running);
    let listen_task = tokio::spawn(listen_remote_control(
        ctrl_read,
        transfer_id.clone(),
        remote_tx,
        sink.clone(),
    ));
    let (stop_tx, stop_rx) = oneshot::channel();
    let forward_task = tokio::spawn(forward_local_control(
        ctrl_write,
        control.clone(),
        transfer_id.clone(),
        stop_rx,
    ));

    let result = push_data(
        identity,
        addrs,
        port,
        expected_fp,
        &transfer_id,
        plan,
        control,
        remote_rx,
        sink,
    )
    .await;

    // 收尾: 停监听; 转发任务补齐未同步的终态并发 Bye 后自行结束
    listen_task.abort();
    let _ = stop_tx.send(());
    let _ = forward_task.await;
    match &result {
        Ok(_) => {
            sink.notify(TransferEvent::Completed {
                transfer_id: transfer_id.clone(),
            })
            .await;
        }
        Err(TransferError::Cancelled) => {
            sink.notify(TransferEvent::Cancelled {
                transfer_id: transfer_id.clone(),
            })
            .await;
        }
        Err(e) => {
            sink.notify(TransferEvent::Interrupted {
                transfer_id: transfer_id.clone(),
                reason: e.to_string(),
            })
            .await;
        }
    }

    let (files_sent, bytes_sent) = result?;
    Ok(SendSummary {
        transfer_id,
        peer,
        files_sent,
        bytes_sent,
    })
}

/// 发送文本(逐字节一致), 返回对端设备信息
///
/// `pin`: 对端启用配对 PIN 时必须携带; 缺失或错误返回 [`TransferError::PinRequired`]
pub async fn send_text(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    pin: Option<String>,
    text: &str,
) -> Result<PeerInfo, TransferError> {
    let (mut ctrl, peer) = connect_and_hello(identity, addrs, port, expected_fp).await?;
    write_frame(
        &mut ctrl,
        &ControlMessage::Text {
            text: text.to_string(),
            pin,
        },
    )
    .await?;
    let resp = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut ctrl))
        .await
        .map_err(|_| TransferError::Timeout("文本确认"))??;
    match resp {
        ControlMessage::TextAck => {}
        ControlMessage::TextRejected { pin_required: true } => {
            return Err(TransferError::PinRequired);
        }
        other => return Err(unexpected("text_ack", &other)),
    }
    let _ = write_frame(&mut ctrl, &ControlMessage::Bye).await;
    Ok(peer)
}

/// 拉取对端头像图片, 返回 `(哈希, 图片字节)`; 对端未设置头像返回 None
///
/// 调用时机: 发现层广播的 avatar 为 `img:<hash>` 且本地缓存未命中。
/// 应答数据必须与其声明的哈希一致, 不一致按投毒丢弃(HashMismatch)。
pub async fn fetch_avatar(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
) -> Result<Option<(String, Vec<u8>)>, TransferError> {
    let (mut ctrl, _peer) = connect_and_hello(identity, addrs, port, expected_fp).await?;
    write_frame(&mut ctrl, &ControlMessage::AvatarRequest).await?;
    let resp = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut ctrl))
        .await
        .map_err(|_| TransferError::Timeout("头像应答"))??;
    let ControlMessage::AvatarResponse { hash, size } = resp else {
        return Err(unexpected("avatar_response", &resp));
    };
    if size == 0 {
        let _ = write_frame(&mut ctrl, &ControlMessage::Bye).await;
        return Ok(None);
    }
    if size > MAX_AVATAR_SIZE {
        return Err(TransferError::AvatarTooLarge { size });
    }
    // 帧后紧跟的裸图片字节
    let mut data = vec![0u8; usize::try_from(size).unwrap_or(0)];
    tokio::time::timeout(REPLY_TIMEOUT, ctrl.read_exact(&mut data))
        .await
        .map_err(|_| TransferError::Timeout("头像数据"))??;
    if blake3::hash(&data).to_hex().to_string() != hash {
        return Err(TransferError::HashMismatch {
            rel_path: "avatar".to_string(),
        });
    }
    let _ = write_frame(&mut ctrl, &ControlMessage::Bye).await;
    Ok(Some((hash, data)))
}

/// 逐个尝试候选地址(多网卡场景), 返回第一个建立成功的 TCP 连接
async fn connect_first(addrs: &[IpAddr], port: u16) -> Result<TcpStream, TransferError> {
    let mut last_err: Option<std::io::Error> = None;
    for addr in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((*addr, port))).await {
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(e)) => {
                tracing::debug!(%addr, "候选地址连接失败: {e}");
                last_err = Some(e);
            }
            Err(_) => {
                tracing::debug!(%addr, "候选地址连接超时");
                last_err = Some(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("连接 {addr}:{port} 超时"),
                ));
            }
        }
    }
    Err(TransferError::Io(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "候选地址列表为空")
    })))
}

/// 建立 TLS 连接(可选指纹 pin)
async fn connect_tls(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
) -> Result<TlsStream<TcpStream>, TransferError> {
    let config = Arc::new(client_config(identity, expected_fp)?);
    let tcp = connect_first(addrs, port).await?;
    tcp.set_nodelay(true)?;
    super::io_tuning::tune_socket(&tcp);
    // SNI 用固定名, 证书校验只看指纹不看域名
    let name = ServerName::try_from("deskmate")
        .map_err(|e| TransferError::Io(std::io::Error::other(e)))?;
    Ok(TlsConnector::from(config).connect(name, tcp).await?)
}

/// 建立控制连接并完成 Hello 握手, 校验版本与对端身份一致性
async fn connect_and_hello(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
) -> Result<(TlsStream<TcpStream>, PeerInfo), TransferError> {
    let mut tls = connect_tls(identity, addrs, port, expected_fp).await?;
    write_frame(
        &mut tls,
        &ControlMessage::Hello {
            version: PROTOCOL_VERSION.to_string(),
            info: identity.peer_info(),
        },
    )
    .await?;
    let ack = tokio::time::timeout(REPLY_TIMEOUT, read_frame(&mut tls))
        .await
        .map_err(|_| TransferError::Timeout("握手应答"))??;
    let ControlMessage::HelloAck { version, info } = ack else {
        return Err(unexpected("hello_ack", &ack));
    };
    check_version(&version)?;
    // 对端声明的指纹必须与其 TLS 证书一致, 防止身份冒充
    let tls_fp = peer_fingerprint(tls.get_ref().1.peer_certificates());
    if tls_fp.as_deref() != Some(info.fingerprint.as_str()) {
        return Err(TransferError::PeerMismatch);
    }
    Ok((tls, info))
}

/// 数据阶段的单文件发送项(首发 offset 为 0, 续传为对端已收字节)
struct SendItem {
    /// 文件序号(与接收端清单一致)
    file_id: u32,
    /// 本地绝对路径
    abs_path: PathBuf,
    /// 相对路径(进度事件展示)
    rel_path: String,
    /// 文件总字节数
    size: u64,
    /// 起始偏移
    offset: u64,
}

/// 数据阶段: 建立数据连接, 按计划逐文件推送
#[expect(clippy::too_many_arguments, reason = "内部装配函数, 参数即会话上下文")]
async fn push_data(
    identity: &DeviceIdentity,
    addrs: &[IpAddr],
    port: u16,
    expected_fp: Option<String>,
    transfer_id: &str,
    plan: &[SendItem],
    mut local: watch::Receiver<ControlState>,
    mut remote: watch::Receiver<ControlState>,
    sink: &EventSink,
) -> Result<(usize, u64), TransferError> {
    let mut data = connect_tls(identity, addrs, port, expected_fp).await?;
    write_frame(
        &mut data,
        &ControlMessage::DataHello {
            transfer_id: transfer_id.to_string(),
        },
    )
    .await?;

    let mut files_sent = 0usize;
    let mut bytes_sent = 0u64;
    // chunk 缓冲与任务 ID 在整个数据阶段复用, 避免逐文件/逐 chunk 重复分配
    let mut buf = vec![0u8; super::CHUNK_SIZE];
    let tid: Arc<str> = Arc::from(transfer_id);
    for item in plan {
        write_frame(
            &mut data,
            &ControlMessage::FileHeader {
                file_id: item.file_id,
                offset: item.offset,
            },
        )
        .await?;

        let (tid, rel, fid, size) = (
            Arc::clone(&tid),
            Arc::<str>::from(item.rel_path.as_str()),
            item.file_id,
            item.size,
        );
        let progress_sink = sink.clone();
        let hash = send_file_body(
            &mut data,
            &item.abs_path,
            item.size,
            item.offset,
            &mut buf,
            move |done| {
                progress_sink.progress(TransferEvent::Progress {
                    transfer_id: Arc::clone(&tid),
                    file_id: fid,
                    rel_path: Arc::clone(&rel),
                    done,
                    size,
                });
            },
            &mut local,
            &mut remote,
        )
        .await?;

        write_frame(
            &mut data,
            &ControlMessage::FileFooter {
                file_id: item.file_id,
                hash,
            },
        )
        .await?;
        sink.notify(TransferEvent::FileCompleted {
            transfer_id: transfer_id.to_string(),
            file_id: item.file_id,
            path: item.abs_path.clone(),
        })
        .await;
        files_sent += 1;
        bytes_sent += item.size.saturating_sub(item.offset);
    }

    write_frame(&mut data, &ControlMessage::DataDone).await?;
    // 必须等对端排空并关闭, 直接 close 会在进程退出时触发 RST 冲掉在途帧
    graceful_close(&mut data).await;
    Ok((files_sent, bytes_sent))
}

/// 监听对端经控制连接下发的暂停/恢复/取消指令, 写入 remote 状态
///
/// 暂停/恢复顺带上报事件驱动 UI(取消不上报: 数据泵随即以
/// `Cancelled` 终态收尾, 由终态事件统一呈现)。
async fn listen_remote_control(
    mut ctrl_read: ReadHalf<TlsStream<TcpStream>>,
    transfer_id: String,
    remote: watch::Sender<ControlState>,
    sink: EventSink,
) {
    loop {
        match read_frame(&mut ctrl_read).await {
            Ok(ControlMessage::Pause { transfer_id: id }) if id == transfer_id => {
                let _ = remote.send(ControlState::Paused);
                sink.notify(TransferEvent::Paused {
                    transfer_id: transfer_id.clone(),
                })
                .await;
            }
            Ok(ControlMessage::Resume { transfer_id: id }) if id == transfer_id => {
                let _ = remote.send(ControlState::Running);
                sink.notify(TransferEvent::Resumed {
                    transfer_id: transfer_id.clone(),
                })
                .await;
            }
            Ok(ControlMessage::Cancel { transfer_id: id }) if id == transfer_id => {
                let _ = remote.send(ControlState::Cancelled);
                return;
            }
            // 对端关闭或连接断开: 停止监听, 数据阶段自行感知 IO 结果
            Ok(ControlMessage::Bye) | Err(_) => return,
            Ok(other) => {
                tracing::debug!(kind = other.kind(), "控制通道忽略消息");
            }
        }
    }
}

/// 把本端控制状态的变化经控制连接转告接收端(Pause/Resume/Cancel 帧)
///
/// 对端引擎收到即同步暂停/取消语义并驱动其 UI; 连接已断时静默退出,
/// 对端靠数据通道空闲超时兜底。数据阶段结束(stop)或控制源释放时,
/// 补发未同步的取消终态并发 Bye 告别 —— 写半部所有权在此任务,
/// 告别只能由此发出。
async fn forward_local_control(
    mut ctrl_write: WriteHalf<TlsStream<TcpStream>>,
    mut local: watch::Receiver<ControlState>,
    transfer_id: String,
    mut stop: oneshot::Receiver<()>,
) {
    // 对端的初始视角是 Running; 每次醒来先对齐差异, 再等下一次变化
    let mut synced = ControlState::Running;
    loop {
        let now = *local.borrow();
        if now != synced {
            let msg = match now {
                ControlState::Paused => ControlMessage::Pause {
                    transfer_id: transfer_id.clone(),
                },
                ControlState::Running => ControlMessage::Resume {
                    transfer_id: transfer_id.clone(),
                },
                ControlState::Cancelled => ControlMessage::Cancel {
                    transfer_id: transfer_id.clone(),
                },
            };
            if write_frame(&mut ctrl_write, &msg).await.is_err() {
                return; // 控制连接已断, 告别也无从发出
            }
            synced = now;
        }
        if synced == ControlState::Cancelled {
            break; // 取消是终态, 数据阶段随即收尾
        }
        tokio::select! {
            changed = local.changed() => {
                if changed.is_err() {
                    break; // 控制源已释放(调用方收尾)
                }
            }
            _ = &mut stop => break,
        }
    }
    // stop 与状态变化可能同时就绪而 select 选中了 stop: 取消终态必须补发,
    // 否则对端把主动取消误判为意外断连(保留 .part 等续传)
    let last = *local.borrow();
    if last == ControlState::Cancelled && synced != last {
        let _ = write_frame(
            &mut ctrl_write,
            &ControlMessage::Cancel {
                transfer_id: transfer_id.clone(),
            },
        )
        .await;
    }
    let _ = write_frame(&mut ctrl_write, &ControlMessage::Bye).await;
}

/// 构造"收到意外消息"错误
fn unexpected(expected: &'static str, got: &ControlMessage) -> TransferError {
    TransferError::Protocol(ProtocolError::Unexpected {
        expected,
        got: got.kind().to_string(),
    })
}
