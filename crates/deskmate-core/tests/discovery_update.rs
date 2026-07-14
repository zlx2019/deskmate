//! 发现层集成测试: 身份热更新经真实 UDP 组播传播到对端
//!
//! A(广播方)与 B(隐身观察方)走真实 socket; 若运行环境不支持
//! UDP 组播回环(部分 CI 沙箱), 首步探测失败即跳过, 不判失败。

use std::time::Duration;

use deskmate_core::discovery::{DiscoveryService, PeerEvent};
use deskmate_core::protocol::PeerInfo;

/// 测试专用发现端口(避开真实应用的 42425)
const TEST_DISCOVERY_PORT: u16 = 48425;

/// 构造测试身份
fn test_info(name: &str, fingerprint: &str) -> PeerInfo {
    PeerInfo {
        device_id: format!("test-dev-{fingerprint}"),
        name: name.to_string(),
        fingerprint: fingerprint.to_string(),
        platform: "test".to_string(),
        avatar: None,
    }
}

/// 收事件直到匹配谓词或超时(返回 None 表示超时)
async fn wait_for(
    events: &mut tokio::sync::mpsc::Receiver<PeerEvent>,
    mut pred: impl FnMut(&PeerEvent) -> bool,
) -> Option<PeerEvent> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = events.recv().await {
            if pred(&ev) {
                return Some(ev);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// update_info 后对端应收到携带新昵称/头像的 Up 事件(同一指纹, 不经历下线)
#[tokio::test]
async fn update_info_propagates_over_lan() {
    let (svc_a, _events_a) = DiscoveryService::start(
        test_info("原始昵称", "fp-aaaa"),
        40001,
        TEST_DISCOVERY_PORT,
        false,
    )
    .await
    .expect("A 启动失败");
    let (svc_b, mut events_b) = DiscoveryService::start(
        test_info("观察者", "fp-bbbb"),
        40002,
        TEST_DISCOVERY_PORT,
        true,
    )
    .await
    .expect("B 启动失败");

    // 首步探测: B 应看到 A 的原始昵称; 环境不支持组播回环时跳过整个用例
    let Some(_) = wait_for(&mut events_b, |ev| {
        matches!(ev, PeerEvent::Up(p) if p.info.fingerprint == "fp-aaaa" && p.info.name == "原始昵称")
    })
    .await
    else {
        eprintln!("跳过: 本环境不支持 UDP 组播回环, 无法进行发现层集成测试");
        svc_a.shutdown().await;
        svc_b.shutdown().await;
        return;
    };

    // 热更新昵称与头像。刻意在无 tokio runtime 的 std 线程里调用,
    // 复现真实调用方(Tauri 同步命令的 IPC 线程)的上下文 ——
    // 回归防护: update_info 曾因内部 tokio::spawn 在此场景直接 panic
    let mut updated = test_info("改名的设备", "fp-aaaa");
    updated.avatar = Some("🚀".to_string());
    std::thread::scope(|s| {
        s.spawn(|| svc_a.update_info(&updated));
    });

    // 对端应以 Up 事件收到新信息(update_info 会立即补发 announce)
    let got = wait_for(&mut events_b, |ev| {
        matches!(ev, PeerEvent::Up(p) if p.info.fingerprint == "fp-aaaa" && p.info.name == "改名的设备")
    })
    .await
    .expect("对端未收到热更新后的身份");
    if let PeerEvent::Up(p) = got {
        assert_eq!(p.info.avatar.as_deref(), Some("🚀"));
    }

    svc_a.shutdown().await;
    svc_b.shutdown().await;
}
