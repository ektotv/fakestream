//! The media layer. Everything that touches libav* lives here, and the FFI
//! itself is confined further, to `media::ffi`.

pub mod ffi;
pub mod mux;
pub mod source;
pub mod verify;

pub use ffi::FfiError;

/// Anything that can go wrong while generating media.
#[derive(Debug)]
pub enum MediaError {
    /// A codec ffmpeg was expected to provide is missing from this build.
    MissingCodec(&'static str),
    /// libav* rejected a call, carrying its own message.
    Ffmpeg { doing: &'static str, detail: String },
    /// The FFI boundary refused a malformed request.
    Ffi(FfiError),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCodec(name) => {
                write!(formatter, "this ffmpeg build has no {name} encoder")
            }
            Self::Ffmpeg { doing, detail } => write!(formatter, "{doing}: {detail}"),
            Self::Ffi(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<FfiError> for MediaError {
    fn from(error: FfiError) -> Self {
        Self::Ffi(error)
    }
}

/// Wrap an rsmpeg result with what we were attempting, since its errors carry
/// an errno and little else.
pub(crate) fn context<T, E: std::fmt::Display>(
    doing: &'static str,
    result: Result<T, E>,
) -> Result<T, MediaError> {
    result.map_err(|error| MediaError::Ffmpeg {
        doing,
        detail: error.to_string(),
    })
}
