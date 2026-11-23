/// Errors surfaced across the FFI boundary. In Swift these are thrown as `BarBudsError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BarBudsError {
    #[error("malformed frame: {reason}")]
    MalformedFrame { reason: String },

    #[error("checksum mismatch: expected {expected:#x}, got {actual:#x}")]
    ChecksumMismatch { expected: u32, actual: u32 },

    #[error("unknown frame type {frame_type:#04x}")]
    UnknownFrameType { frame_type: u8 },

    #[error("not supported by this device: {feature}")]
    Unsupported { feature: String },

    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },

    /// The device rejected a request, or the connection isn't ready for it yet.
    #[error("device error: {reason}")]
    Device { reason: String },
}

impl BarBudsError {
    pub(crate) fn malformed(reason: impl Into<String>) -> Self {
        Self::MalformedFrame { reason: reason.into() }
    }

    pub(crate) fn device(reason: impl Into<String>) -> Self {
        Self::Device { reason: reason.into() }
    }

    pub(crate) fn invalid_argument(reason: impl Into<String>) -> Self {
        Self::InvalidArgument { reason: reason.into() }
    }
}
