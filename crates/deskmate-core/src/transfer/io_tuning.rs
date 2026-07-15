//! 平台 IO 调优: 文件预分配、页缓存旁路、socket 缓冲(PLAN 4.4)
//!
//! v1 清单的收尾(socket 缓冲、预分配)+ v2 中对 Win/Mac 有实效的
//! F_NOCACHE; v2 整体(io_uring/kTLS/多流)已决定不做, 见 PLAN 4.4 注记。
//! 三者都是尽力而为的优化: 除预分配的磁盘不足外, 失败不影响传输正确性。

use tokio::fs::File;
use tokio::net::TcpStream;

use crate::config::{NOCACHE_THRESHOLD, SOCKET_BUFFER_SIZE};

/// 调大数据连接的内核收发缓冲(高带宽或 WiFi 抖动下默认值限制在途窗口)
///
/// 失败仅记录: 缓冲上限是优化项, 各平台默认值也能正常工作。
pub(crate) fn tune_socket(stream: &TcpStream) {
    let sock = socket2::SockRef::from(stream);
    if let Err(e) = sock.set_recv_buffer_size(SOCKET_BUFFER_SIZE) {
        tracing::debug!("设置接收缓冲失败(沿用系统默认): {e}");
    }
    if let Err(e) = sock.set_send_buffer_size(SOCKET_BUFFER_SIZE) {
        tracing::debug!("设置发送缓冲失败(沿用系统默认): {e}");
    }
}

/// 预分配文件空间: 磁盘不足在传输开始即失败(而非写到一半),
/// 并给文件系统一次性分配整段空间的机会, 减少碎片
///
/// 关键约束: **不能改变文件的可见长度** —— 断点续传以 `.part` 的
/// `metadata.len()` 为断点, 预分配若扩了 EOF, 空洞会被当成已收数据。
/// 三平台用的都是"只保留空间"语义: macOS `F_PREALLOCATE` /
/// Linux `fallocate(KEEP_SIZE)` / Windows `FileAllocationInfo`。
/// 文件系统不支持(网络卷等)静默跳过; 返回 Err 仅代表磁盘空间不足。
pub(crate) async fn preallocate(file: &File, size: u64) -> std::io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    // 克隆句柄移入阻塞线程池: 真实磁盘分配可能耗时(几十 GB 级),
    // 且句柄所有权随闭包, 不受调用方 future 提前取消的影响
    let dup = file.try_clone().await?.into_std().await;
    tokio::task::spawn_blocking(move || do_preallocate(&dup, size))
        .await
        .map_err(std::io::Error::other)?
}

/// macOS: F_PREALLOCATE 保留空间(不改 EOF); 先尝试连续区段, 碎片时退化为分散分配
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "F_PREALLOCATE 无安全封装, fcntl 仅作用于自有句柄"
)]
fn do_preallocate(file: &std::fs::File, size: u64) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut store = libc::fstore_t {
        fst_flags: libc::F_ALLOCATECONTIG,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: i64::try_from(size).unwrap_or(i64::MAX),
        fst_bytesalloc: 0,
    };
    let mut ret = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    if ret == -1 {
        store.fst_flags = libc::F_ALLOCATEALL;
        ret = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    }
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        // 文件系统不支持预分配(SMB 网络卷等)不算失败, 只失去优化
        if err.raw_os_error() == Some(libc::ENOTSUP) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

/// Linux: fallocate + KEEP_SIZE(CI 测试路径; 不改 EOF)
#[cfg(target_os = "linux")]
#[expect(unsafe_code, reason = "fallocate 无安全封装, 仅作用于自有句柄")]
fn do_preallocate(file: &std::fs::File, size: u64) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let len = i64::try_from(size).unwrap_or(i64::MAX);
    let ret = unsafe { libc::fallocate(file.as_raw_fd(), libc::FALLOC_FL_KEEP_SIZE, 0, len) };
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        // 文件系统不支持(部分网络/FUSE 卷)不算失败, 只失去优化
        if matches!(
            err.raw_os_error(),
            Some(libc::EOPNOTSUPP) | Some(libc::ENOSYS)
        ) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

/// Windows: FileAllocationInfo 设置分配大小(不改 EndOfFile)
///
/// 注意: 仅 CI 编译验证过, 双机验收时需实机回归大文件接收路径。
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "SetFileInformationByHandle 无安全封装, 仅作用于自有句柄"
)]
fn do_preallocate(file: &std::fs::File, size: u64) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALLOCATION_INFO, FileAllocationInfo, SetFileInformationByHandle,
    };
    let info = FILE_ALLOCATION_INFO {
        AllocationSize: i64::try_from(size).unwrap_or(i64::MAX),
    };
    let ret = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileAllocationInfo,
            std::ptr::from_ref(&info).cast(),
            u32::try_from(std::mem::size_of::<FILE_ALLOCATION_INFO>()).unwrap_or(u32::MAX),
        )
    };
    if ret == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// 其余平台: 无对应"只保留空间"的原语, 跳过(仅失去优化)
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn do_preallocate(_file: &std::fs::File, _size: u64) -> std::io::Result<()> {
    Ok(())
}

/// 大文件传输旁路页缓存(macOS F_NOCACHE)
///
/// 单次顺序收发的几十 GB 数据没有复用价值, 全走缓存会把系统里
/// 其他应用的热页挤掉; 阈值以下的小文件保持走缓存。
/// 仅设置 IO 驻留策略, 失败无碍, 其余平台为空操作。
#[cfg_attr(
    target_os = "macos",
    expect(unsafe_code, reason = "F_NOCACHE 无安全封装, fcntl 仅设置标志")
)]
pub(crate) fn advise_no_cache(file: &File, size: u64) {
    if size < NOCACHE_THRESHOLD {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
            tracing::debug!(
                "F_NOCACHE 设置失败(仅失去缓存优化): {}",
                std::io::Error::last_os_error()
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = file;
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    /// 预分配不得改变文件可见长度: 断点续传以 .part 的 len 为断点,
    /// 预分配扩了 EOF 会让空洞被当成已收数据(哈希必错)
    #[tokio::test]
    async fn preallocate_keeps_file_length() {
        let path = std::env::temp_dir().join(format!("dm-prealloc-{}", uuid::Uuid::new_v4()));
        let mut file = tokio::fs::File::create(&path).await.unwrap();
        super::preallocate(&file, 8 * 1024 * 1024).await.unwrap();
        assert_eq!(
            tokio::fs::metadata(&path).await.unwrap().len(),
            0,
            "预分配改变了文件长度, 会破坏断点续传"
        );

        // 写入后长度只反映实际写入量
        file.write_all(b"hello").await.unwrap();
        file.flush().await.unwrap();
        drop(file);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 5);
        let _ = std::fs::remove_file(&path);
    }

    /// 空文件预分配是空操作, 不报错
    #[tokio::test]
    async fn preallocate_zero_is_noop() {
        let path = std::env::temp_dir().join(format!("dm-prealloc0-{}", uuid::Uuid::new_v4()));
        let file = tokio::fs::File::create(&path).await.unwrap();
        super::preallocate(&file, 0).await.unwrap();
        drop(file);
        let _ = std::fs::remove_file(&path);
    }
}
