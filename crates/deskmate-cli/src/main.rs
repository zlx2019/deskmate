//! deskmate-cli: 协议联调与验证工具(M1 里程碑交付物)
//!
//! 在 Tauri UI 就绪前, 用于在两台机器间验证节点发现、握手与文件收发。

mod commands;
mod output;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// 命令行定义: 子命令 + 全局选项(clap 自动生成帮助与错误提示)
#[derive(Parser)]
#[command(
    name = "deskmate-cli",
    version,
    about = "deskmate — 局域网传输联调工具",
    after_help = "<目标> 形式:\n  节点名称 | 指纹前缀 | 节点IP | ip:port(直连, 跳过发现与指纹校验)"
)]
struct Cli {
    /// 身份数据目录(默认 ~/.deskmate)
    #[arg(long, global = true, value_name = "目录")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// 子命令
#[derive(Subcommand)]
enum Command {
    /// 显示本机身份
    Id,
    /// 驻留接收(--yes 自动接受)
    Listen {
        /// 下载目录(默认 ~/Downloads/deskmate)
        #[arg(long = "dir", value_name = "下载目录")]
        download_dir: Option<PathBuf>,
        /// 监听端口
        #[arg(long, value_name = "端口", default_value_t = deskmate_core::DEFAULT_TCP_PORT)]
        port: u16,
        /// 临时昵称
        #[arg(long, value_name = "昵称")]
        name: Option<String>,
        /// 自动接受全部请求
        #[arg(long = "yes")]
        auto_accept: bool,
    },
    /// 扫描在线节点(默认 6s)
    Scan {
        /// 等待秒数
        #[arg(long = "wait", value_name = "秒数", default_value_t = 6)]
        wait_secs: u64,
    },
    /// 发送文件/目录
    Send {
        /// 待发送路径(至少一个)
        #[arg(value_name = "路径", required = true)]
        paths: Vec<PathBuf>,
        /// 目标(名称/指纹前缀/IP/ip:port)
        #[arg(long = "to", value_name = "目标")]
        target: String,
    },
    /// 发送文本(逐字节原样送达)
    Text {
        /// 文本内容(可用引号包裹)
        #[arg(value_name = "文本")]
        text: String,
        /// 目标(名称/指纹前缀/IP/ip:port)
        #[arg(long = "to", value_name = "目标")]
        target: String,
    },
}

/// 通用参数(解析后传给各子命令实现)
struct CommonArgs {
    /// 身份数据目录
    data_dir: PathBuf,
}

/// 入口: 初始化日志, 解析参数并分发子命令
#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();
    let common = CommonArgs {
        data_dir: cli.data_dir.unwrap_or_else(default_data_dir),
    };
    match cli.command {
        Command::Id => commands::cmd_id(&common).await,
        Command::Listen {
            download_dir,
            port,
            name,
            auto_accept,
        } => {
            let dir = download_dir.unwrap_or_else(default_download_dir);
            commands::cmd_listen(&common, dir, port, name, auto_accept).await
        }
        Command::Scan { wait_secs } => commands::cmd_scan(&common, wait_secs).await,
        Command::Send { paths, target } => commands::cmd_send(&common, paths, &target).await,
        Command::Text { text, target } => commands::cmd_text(&common, &text, &target).await,
    }
}

/// 初始化 tracing 日志: 输出到 stderr, 级别由 RUST_LOG 控制(默认 warn)
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// 用户主目录(HOME / USERPROFILE), 兜底当前目录
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 默认身份数据目录: ~/.deskmate
fn default_data_dir() -> PathBuf {
    home_dir().join(".deskmate")
}

/// 默认下载目录: ~/Downloads/deskmate
fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads").join("deskmate")
}
