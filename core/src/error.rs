/// Errors surfaced across the FFI boundary. In Swift these are thrown as `BarBudsError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BarBudsError {
    #[error("malformed frame: {reason}")]
    MalformedFrame { reason: String },

    #[error("checksum mismatch: expected {expected:#04x}, got {actual:#04x}")]
    ChecksumMismatch { expected: u8, actual: u8 },

    #[error("unknown frame type {frame_type:#04x}")]
    UnknownFrameType { frame_type: u8 },

    #[error("not supported by this device: {feature}")]
    Unsupported { feature: String },

    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },
}

impl BarBudsError {
    pub(crate) fn malformed(reason: impl Into<String>) -> Self {
        Self::MalformedFrame { reason: reason.into() }
    }

    pub(crate) fn invalid_argument(reason: impl Into<String>) -> Self {
        Self::InvalidArgument { reason: reason.into() }
    }
}
