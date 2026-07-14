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

// ---- 接收端 PIN 门(receiver)----

/// PIN 暴力破解限速窗口
pub const PIN_WINDOW: Duration = Duration::from_secs(60);

/// 窗口内允许的最大失败次数, 达到后整窗一律拒绝
pub const PIN_MAX_FAILURES: u32 = 5;
