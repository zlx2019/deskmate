//! CLI output helpers: byte formatting, a single-line progress bar, and a peer table.

use std::net::IpAddr;
use std::time::Instant;

use deskmate_core::discovery::Peer;

/// Formats candidate addresses as the first address plus the remaining count,
/// for example `192.168.1.2:42424 (+1)`.
pub fn addrs_label(addrs: &[IpAddr], port: u16) -> String {
    match addrs.first() {
        Some(a) if addrs.len() > 1 => format!("{a}:{port} (+{})", addrs.len() - 1),
        Some(a) => format!("{a}:{port}"),
        None => "-".to_string(),
    }
}

/// Formats a byte count for humans, for example `1536` as `"1.5 KB"`.
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

/// A single-line progress bar on stderr with rate-limited rendering and smoothed speed.
pub struct ProgressBar {
    /// Last render time, used for rate limiting.
    last_render: Option<Instant>,
    /// Last sampled byte count and time, used to calculate instantaneous speed.
    last_sample: Option<(u64, Instant)>,
    /// Smoothed speed in bytes per second.
    speed: f64,
    /// Whether the current line contains progress output.
    active: bool,
}

impl ProgressBar {
    /// Creates an empty progress bar.
    pub fn new() -> Self {
        Self {
            last_render: None,
            last_sample: None,
            speed: 0.0,
            active: false,
        }
    }

    /// Updates progress and redraws at most every 100 ms, always drawing on completion.
    pub fn update(&mut self, label: &str, done: u64, size: u64) {
        let now = Instant::now();
        // Sample speed and apply exponential smoothing.
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

    /// Clears the progress line before printing a normal message.
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

/// Prints a table of online peers.
pub fn print_peer_table(peers: &[Peer]) {
    if peers.is_empty() {
        println!("(no online peers found)");
        return;
    }
    println!(
        "{:<20} {:<22} {:<8} Fingerprint (first 12)",
        "Name", "Address", "Platform"
    );
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
