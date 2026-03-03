#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Config,
    Ipc,
    Overlay,
    Audio,
    Model,
    Tts,
    Internal,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Config => "CONFIG",
            ErrorCode::Ipc => "IPC",
            ErrorCode::Overlay => "OVERLAY",
            ErrorCode::Audio => "AUDIO",
            ErrorCode::Model => "MODEL",
            ErrorCode::Tts => "TTS",
            ErrorCode::Internal => "INTERNAL",
        }
    }
}

pub fn format_error(code: ErrorCode, message: impl AsRef<str>) -> String {
    format!("{}:{}", code.as_str(), message.as_ref())
}
