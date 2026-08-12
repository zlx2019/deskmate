//! Session layer: custom framed control-channel protocol from design decision #2.
//!
//! Frames use a four-byte big-endian length prefix followed by a JSON body.
//! Control and data channels share one TCP listening port and are distinguished
//! by the first frame: `Hello` starts a control session and `DataHello` starts a data stream.
//!
//! Transfer state machine:
//! ```text
//! Idle → Requested → Accepted | Rejected
//! Accepted → Transferring ⇄ Paused
//! Transferring → Completed
//!              | Cancelled   (explicit cancellation deletes temporary .part files)
//!              | Interrupted (unexpected disconnect retains .part files for resume)
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::PROTOCOL_VERSION;

/// Maximum frame length of one MiB, preventing malicious frames from exhausting memory.
pub const MAX_FRAME_LEN: u32 = 1024 * 1024;

/// Maximum avatar size of 256 KiB; a 128 by 128 JPEG is normally under 20 KiB.
pub const MAX_AVATAR_SIZE: u64 = 256 * 1024;

/// Protocol layer errors.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Underlying I/O failure, including peer disconnects.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Frame length exceeds [`MAX_FRAME_LEN`].
    #[error("frame length {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    FrameTooLarge(u32),
    /// JSON message encoding or decoding failed.
    #[error("message codec error: {0}")]
    Codec(#[from] serde_json::Error),
    /// Protocol major versions differ, so communication is rejected.
    #[error("incompatible protocol versions: peer {peer}, local {local}")]
    VersionMismatch {
        /// Peer version.
        peer: String,
        /// Local version.
        local: String,
    },
    /// A message is invalid for the current session state.
    #[error("unexpected message: expected {expected}, received {got}")]
    Unexpected {
        /// Expected message type.
        expected: &'static str,
        /// Description of the received message.
        got: String,
    },
}

/// Device information exchanged during the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Unique device ID (UUID).
    pub device_id: String,
    /// Display name.
    pub name: String,
    /// Hexadecimal BLAKE3 certificate fingerprint.
    pub fingerprint: String,
    /// Platform identifier: macos, windows, or linux.
    pub platform: String,
    /// Built-in emoji avatar; the UI falls back to initials when absent.
    ///
    /// The serde default preserves compatibility with versions that omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// Operating-system version, for example `"macOS 15.3.1"`.
    ///
    /// Added in protocol 1.3 and optional for older versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

/// File metadata in a `TransferRequest` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// File index within the transfer, used by the data channel.
    pub file_id: u32,
    /// Relative path using `/` separators; the receiver must sanitize before joining.
    pub rel_path: String,
    /// File size in bytes.
    pub size: u64,
}

/// Per-file resume state in a `ResumeInfo` manifest.
///
/// The sender compares `rel_path` and `size` with its local manifest. A mismatch
/// means the source changed and the full-file hash cannot continue, so the task
/// cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeFileState {
    /// File index retained from the original `TransferRequest`.
    pub file_id: u32,
    /// Relative path.
    pub rel_path: String,
    /// Total file size in bytes.
    pub size: u64,
    /// Bytes persisted by the receiver; resume starts at this offset.
    pub received: u64,
}

/// Control and data channel messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Control-session handshake from initiator to receiver.
    Hello {
        /// Protocol version (`major.minor`).
        version: String,
        /// Initiator device information.
        info: PeerInfo,
    },
    /// Handshake response with receiver device information.
    HelloAck {
        /// Protocol version (`major.minor`).
        version: String,
        /// Receiver device information.
        info: PeerInfo,
    },
    /// Transfer request containing only metadata while awaiting a receiver decision.
    TransferRequest {
        /// Transfer task ID (UUID).
        transfer_id: String,
        /// File manifest.
        files: Vec<FileMeta>,
        /// Total size in bytes.
        total_size: u64,
        /// Pairing PIN required when the receiver has PIN protection enabled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<String>,
    },
    /// Transfer response; an empty `accepted_files` list means rejection.
    TransferResponse {
        /// Corresponding transfer task ID.
        transfer_id: String,
        /// Accepted file indexes, allowing partial acceptance.
        accepted_files: Vec<u32>,
        /// Localized rejection text retained as a fallback for older versions.
        reason: Option<String>,
        /// Whether rejection was caused by a missing or incorrect PIN.
        #[serde(default)]
        pin_required: bool,
        /// Structured rejection code added in protocol 1.4 and rendered locally by the sender.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
    },
    /// Text delivered byte for byte without trimming or escaping.
    Text {
        /// UTF-8 text content.
        text: String,
        /// Pairing PIN, with the same behavior as `TransferRequest`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<String>,
    },
    /// Text receipt acknowledgment.
    TextAck,
    /// Text rejection, added in 1.2 and currently used only for PIN failures.
    TextRejected {
        /// Whether the PIN was missing or incorrect.
        pin_required: bool,
    },
    /// Pauses a transfer; either endpoint may initiate it.
    Pause {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Resumes a transfer.
    Resume {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Cancels a transfer and causes the receiver to delete temporary `.part` files.
    Cancel {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// First data-channel frame declaring which transfer the connection carries.
    DataHello {
        /// Transfer task ID.
        transfer_id: String,
    },
    /// Data-channel file header followed by `size - offset` bytes of raw file content.
    FileHeader {
        /// File index from the `TransferRequest` manifest.
        file_id: u32,
        /// Starting offset, zero in M1 or the received byte count when resuming.
        offset: u64,
    },
    /// Data-channel file footer with the full-file BLAKE3 digest.
    FileFooter {
        /// File index.
        file_id: u32,
        /// Hexadecimal full-file BLAKE3 hash.
        hash: String,
    },
    /// Resume negotiation from design decision #3, sent after an unexpected disconnect.
    ResumeQuery {
        /// Original transfer task ID.
        transfer_id: String,
    },
    /// Resume response listing received bytes for incomplete files.
    ///
    /// `files` is empty when metadata is missing, the task is complete, identities
    /// differ, or the task otherwise cannot be resumed.
    ResumeInfo {
        /// Corresponding transfer task ID.
        transfer_id: String,
        /// Resumable files, excluding files already persisted in full.
        files: Vec<ResumeFileState>,
    },
    /// Requests a peer avatar after an `img:<hash>` advertisement misses the local cache.
    AvatarRequest,
    /// Avatar response followed by `size` bytes of raw image data.
    AvatarResponse {
        /// Lowercase hexadecimal BLAKE3 image hash, empty when no avatar is set.
        hash: String,
        /// Image size in bytes; zero means no avatar and no following data.
        size: u64,
    },
    /// Indicates that all data-channel files have been sent.
    DataDone,
    /// Gracefully closes the session.
    Bye,
}

impl ControlMessage {
    /// Returns the short message type used in logs and errors.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::HelloAck { .. } => "hello_ack",
            Self::TransferRequest { .. } => "transfer_request",
            Self::TransferResponse { .. } => "transfer_response",
            Self::Text { .. } => "text",
            Self::TextAck => "text_ack",
            Self::TextRejected { .. } => "text_rejected",
            Self::Pause { .. } => "pause",
            Self::Resume { .. } => "resume",
            Self::Cancel { .. } => "cancel",
            Self::DataHello { .. } => "data_hello",
            Self::FileHeader { .. } => "file_header",
            Self::FileFooter { .. } => "file_footer",
            Self::ResumeQuery { .. } => "resume_query",
            Self::ResumeInfo { .. } => "resume_info",
            Self::AvatarRequest => "avatar_request",
            Self::AvatarResponse { .. } => "avatar_response",
            Self::DataDone => "data_done",
            Self::Bye => "bye",
        }
    }
}

/// Writes a four-byte big-endian length, the JSON body, and then flushes.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: &ControlMessage,
) -> Result<(), ProtocolError> {
    let body = serde_json::to_vec(msg)?;
    let len = u32::try_from(body.len()).map_err(|_| ProtocolError::FrameTooLarge(u32::MAX))?;
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// Reads and decodes one frame.
///
/// Oversized frames fail immediately without reading their body.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<ControlMessage, ProtocolError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

/// Accepts a peer protocol version when its major version matches.
pub fn check_version(peer_version: &str) -> Result<(), ProtocolError> {
    if major_of(peer_version) == major_of(PROTOCOL_VERSION) {
        Ok(())
    } else {
        Err(ProtocolError::VersionMismatch {
            peer: peer_version.to_string(),
            local: PROTOCOL_VERSION.to_string(),
        })
    }
}

/// Returns the major component of a version string.
fn major_of(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Representative messages survive a frame round trip through a duplex stream.
    #[tokio::test]
    async fn frame_roundtrip() {
        let samples = vec![
            ControlMessage::Hello {
                version: PROTOCOL_VERSION.to_string(),
                info: PeerInfo {
                    device_id: "d1".into(),
                    name: "n1".into(),
                    fingerprint: "f".repeat(64),
                    platform: "macos".into(),
                    avatar: Some("🦊".into()),
                    os_version: Some("Mac OS 15.3".into()),
                },
            },
            ControlMessage::TransferRequest {
                transfer_id: "t1".into(),
                files: vec![FileMeta {
                    file_id: 0,
                    rel_path: "a/b.txt".into(),
                    size: 42,
                }],
                total_size: 42,
                pin: Some("1234".into()),
            },
            // Text must remain byte-exact, including surrounding whitespace and control bytes.
            ControlMessage::Text {
                text: "  hello\n\t emoji🚀 \0 tail  ".into(),
                pin: None,
            },
            ControlMessage::DataDone,
        ];
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        for msg in &samples {
            write_frame(&mut a, msg).await.unwrap();
            let got = read_frame(&mut b).await.unwrap();
            assert_eq!(
                serde_json::to_string(&got).unwrap(),
                serde_json::to_string(msg).unwrap()
            );
        }
    }

    /// Legacy transfer responses without `reason_code` remain parseable, and an
    /// unset value is not serialized to protect 1.4 backward compatibility.
    #[test]
    fn transfer_response_reason_code_is_backward_compatible() {
        let legacy = r#"{"type":"transfer_response","transfer_id":"t","accepted_files":[],"reason":"busy","pin_required":false}"#;
        let msg: ControlMessage = serde_json::from_str(legacy).unwrap();
        let ControlMessage::TransferResponse { reason_code, .. } = msg else {
            panic!("expected transfer_response");
        };
        assert_eq!(reason_code, None);

        let modern = ControlMessage::TransferResponse {
            transfer_id: "t".into(),
            accepted_files: Vec::new(),
            reason: None,
            pin_required: false,
            reason_code: None,
        };
        assert!(
            !serde_json::to_string(&modern)
                .unwrap()
                .contains("reason_code")
        );
    }

    /// Legacy peer information without avatar or OS version remains parseable,
    /// and unset optional fields are not serialized.
    #[test]
    fn peer_info_optional_fields_are_backward_compatible() {
        let legacy = r#"{"device_id":"d","name":"n","fingerprint":"f","platform":"macos"}"#;
        let info: PeerInfo = serde_json::from_str(legacy).unwrap();
        assert_eq!(info.avatar, None);
        assert_eq!(info.os_version, None);
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("avatar"));
        assert!(!json.contains("os_version"));
    }

    /// Serialization must preserve text content byte for byte.
    #[test]
    fn text_is_byte_exact() {
        let raw = "  space  \u{7f} English ";
        let msg = ControlMessage::Text {
            text: raw.into(),
            pin: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        match serde_json::from_str(&json).unwrap() {
            ControlMessage::Text { text, .. } => assert_eq!(text, raw),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// The reader rejects an oversized frame before parsing its body.
    #[tokio::test]
    async fn oversized_frame_rejected() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let bogus_len = (MAX_FRAME_LEN + 1).to_be_bytes();
        tokio::io::AsyncWriteExt::write_all(&mut a, &bogus_len)
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(ProtocolError::FrameTooLarge(_))
        ));
    }

    /// Matching major versions are compatible; different majors are rejected.
    #[test]
    fn version_compat() {
        assert!(check_version("1.0").is_ok());
        assert!(check_version("1.9").is_ok());
        assert!(matches!(
            check_version("2.0"),
            Err(ProtocolError::VersionMismatch { .. })
        ));
    }
}
