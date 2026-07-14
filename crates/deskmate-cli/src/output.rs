//! CLI 输出辅助: 字节格式化、单行进度条、节点表格

use std::net::IpAddr;
use std::time::Instant;

use deskmate_core::discovery::Peer;

/// 候选地址标签: 首地址 + 其余数量, 如 `192.168.1.2:42424 (+1)`
pub fn addrs_label(addrs: &[IpAddr], port: u16) -> String {
    match addrs.first() {
        Some(a) if addrs.len() > 1 => format!("{a}:{port} (+{})", addrs.len() - 1),
        Some(a) => format!("{a}:{port}"),
        None => "-".to_string(),
    }
}

/// 人类可读的字节数(1536 → "1.5 KB")
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 单行覆写进度条(stderr, 限频渲染 + 指数平滑速度)
pub struct ProgressBar {
    /// 上次渲染时间(限频用)
    last_render: Option<Instant>,
    /// 上次采样的完成字节数与时间(算瞬时速度)
    last_sample: Option<(u64, Instant)>,
    /// 平滑后的速度(字节/秒)
    speed: f64,
    /// 当前行是否有内容(clear 用)
    active: bool,
}

impl ProgressBar {
    /// 创建空进度条
    pub fn new() -> Self {
        Self {
            last_render: None,
            last_sample: None,
            speed: 0.0,
            active: false,
        }
    }

    /// 更新进度并按需重绘(≥100ms 一次; 文件完成时必绘)
    pub fn update(&mut self, label: &str, done: u64, size: u64) {
        let now = Instant::now();
        // 速度采样 + 指数平滑
        if let Some((prev_done, prev_t)) = self.last_sample {
            let dt = now.duration_since(prev_t).as_secs_f64();
            if dt > 0.0 && done >= prev_done {
                let inst = (done - prev_done) as f64 / dt;
                self.speed = if self.speed == 0.0 {
                    inst
                } else {
                    self.speed * 0.7 + inst * 0.3
                };
            }
        }
        self.last_sample = Some((done, now));

        let finished = done >= size;
        let due = self
            .last_render
            .map(|t| now.duration_since(t).as_millis() >= 100)
            .unwrap_or(true);
        if !due && !finished {
            return;
        }
        self.last_render = Some(now);

        let pct = done.saturating_mul(100) / size.max(1);
        let filled = (done.saturating_mul(20) / size.max(1)) as usize;
        let bar: String = "=".repeat(filled.min(20)) + &" ".repeat(20usize.saturating_sub(filled));
        eprint!(
            "\r  {label} [{bar}] {pct:>3}% {:>10}/s   ",
            human_bytes(self.speed as u64)
        );
        self.active = true;
    }

    /// 清除当前进度行(在打印普通消息前调用, 避免串行)
    pub fn clear(&mut self) {
        if self.active {
            eprint!("\r{:80}\r", "");
            self.active = false;
        }
    }
}

impl Default for ProgressBar {
    fn default() -> Self {
        Self::new()
    }
}

/// 打印在线节点表格
pub fn print_peer_table(peers: &[Peer]) {
    if peers.is_empty() {
        println!("(未发现任何在线节点)");
        return;
    }
    println!("{:<20} {:<22} {:<8} 指纹(前12位)", "名称", "地址", "平台");
    for p in peers {
        println!(
            "{:<20} {:<22} {:<8} {}",
            p.info.name,
            addrs_label(&p.addrs, p.port),
            p.info.platform,
            p.info.fingerprint.get(..12).unwrap_or(&p.info.fingerprint),
        );
    }
}
