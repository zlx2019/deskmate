//! Platform I/O tuning: preallocation, page-cache bypass, and socket buffers.
//!
//! This completes the v1 socket-buffer and preallocation work and includes the
//! useful Windows/macOS `F_NOCACHE` portion of v2. The rest of v2, including
//! io_uring, kTLS, and multiple streams, is intentionally excluded; see PLAN 4.4.
//! All three are best-effort optimizations. Failures do not affect correctness
//! except when preallocation reports insufficient disk space.

use tokio::fs::File;
use tokio::net::TcpStream;

use crate::config::{NOCACHE_THRESHOLD, SOCKET_BUFFER_SIZE};

/// Enlarges kernel send and receive buffers for data connections.
///
/// Platform defaults can restrict the in-flight window on high-bandwidth or
/// unstable Wi-Fi links. Failure is logged only because defaults remain functional.
pub(crate) fn tune_socket(stream: &TcpStream) {
    let sock = socket2::SockRef::from(stream);
    if let Err(e) = sock.set_recv_buffer_size(SOCKET_BUFFER_SIZE) {
        tracing::debug!("failed to set receive buffer; using the system default: {e}");
    }
    if let Err(e) = sock.set_send_buffer_size(SOCKET_BUFFER_SIZE) {
        tracing::debug!("failed to set send buffer; using the system default: {e}");
    }
}

/// Preallocates file space so insufficient storage fails before transfer starts
/// and the filesystem can allocate one extent to reduce fragmentation.
///
/// The critical constraint is that the visible file length must not change.
/// Resume uses `.part` metadata length as its offset, so extending EOF would
/// treat holes as received data. All three platforms reserve space without
/// changing length: macOS `F_PREALLOCATE`, Linux `fallocate(KEEP_SIZE)`, and
/// Windows `FileAllocationInfo`. Unsupported filesystems such as network volumes
/// are skipped silently; `Err` only indicates insufficient disk space.
pub(crate) async fn preallocate(file: &File, size: u64) -> std::io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    // Move a cloned handle into the blocking pool because allocating tens of
    // gigabytes can take time. Closure ownership also survives caller cancellation.
    let dup = file.try_clone().await?.into_std().await;
    tokio::task::spawn_blocking(move || do_preallocate(&dup, size))
        .await
        .map_err(std::io::Error::other)?
}

/// macOS: `F_PREALLOCATE` reserves space without changing EOF, preferring a
/// contiguous extent and falling back to non-contiguous allocation.
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "F_PREALLOCATE has no safe wrapper and fcntl only accesses an owned handle"
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
        // Unsupported filesystems such as SMB volumes only lose the optimization.
        if err.raw_os_error() == Some(libc::ENOTSUP) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

/// Linux: `fallocate` with `KEEP_SIZE`, covered by CI without changing EOF.
#[cfg(target_os = "linux")]
#[expect(
    unsafe_code,
    reason = "fallocate has no safe wrapper and only accesses an owned handle"
)]
fn do_preallocate(file: &std::fs::File, size: u64) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let len = i64::try_from(size).unwrap_or(i64::MAX);
    let ret = unsafe { libc::fallocate(file.as_raw_fd(), libc::FALLOC_FL_KEEP_SIZE, 0, len) };
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        // Unsupported network or FUSE filesystems only lose the optimization.
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

/// Windows: `FileAllocationInfo` sets allocation size without changing `EndOfFile`.
///
/// This is compile-checked only in CI and needs large-file receive testing on
/// real hardware during two-device acceptance.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "SetFileInformationByHandle has no safe wrapper and only accesses an owned handle"
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

/// Other platforms lack a reserve-without-resize primitive and skip the optimization.
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn do_preallocate(_file: &std::fs::File, _size: u64) -> std::io::Result<()> {
    Ok(())
}

/// Bypasses the page cache for large transfers with macOS `F_NOCACHE`.
///
/// Tens of gigabytes of one-pass sequential I/O have no reuse value and can evict
/// hot pages from other applications. Smaller files remain cached. This only
/// adjusts I/O residency policy, so failure is harmless and other platforms are no-ops.
#[cfg_attr(
    target_os = "macos",
    expect(
        unsafe_code,
        reason = "F_NOCACHE has no safe wrapper and fcntl only sets a flag"
    )
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
                "failed to set F_NOCACHE; only the cache optimization is lost: {}",
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

    /// Preallocation must not change visible length because resume uses `.part`
    /// length as its offset; extending EOF would treat holes as received data.
    #[tokio::test]
    async fn preallocate_keeps_file_length() {
        let path = std::env::temp_dir().join(format!("dm-prealloc-{}", uuid::Uuid::new_v4()));
        let mut file = tokio::fs::File::create(&path).await.unwrap();
        super::preallocate(&file, 8 * 1024 * 1024).await.unwrap();
        assert_eq!(
            tokio::fs::metadata(&path).await.unwrap().len(),
            0,
            "preallocation changed file length and would break resume"
        );

        // Length reflects only bytes actually written.
        file.write_all(b"hello").await.unwrap();
        file.flush().await.unwrap();
        drop(file);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 5);
        let _ = std::fs::remove_file(&path);
    }

    /// Preallocating an empty file is a successful no-op.
    #[tokio::test]
    async fn preallocate_zero_is_noop() {
        let path = std::env::temp_dir().join(format!("dm-prealloc0-{}", uuid::Uuid::new_v4()));
        let file = tokio::fs::File::create(&path).await.unwrap();
        super::preallocate(&file, 0).await.unwrap();
        drop(file);
        let _ = std::fs::remove_file(&path);
    }
}
