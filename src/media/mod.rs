//! The media layer. Everything that touches libav* lives here, and the FFI
//! itself is confined further, to `media::ffi`.

pub mod clock;
pub mod ffi;
pub mod hls;
pub mod live;
pub mod mux;
pub mod overlay;
pub mod pmt;
pub mod source;
pub mod subtitles;
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

/// How much ffmpeg says for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loudness {
    /// Errors only. libx264 and the muxers otherwise print a screen of
    /// statistics per clip, which shreds any progress display and buries
    /// anything that actually matters.
    Errors,
    /// Everything, for working out why a file came out wrong.
    Everything,
}

/// Set how much libav* logs. Affects the whole process, so call it once at
/// startup.
pub fn set_loudness(loudness: Loudness) {
    let level = match loudness {
        Loudness::Errors => rsmpeg::ffi::AV_LOG_ERROR,
        Loudness::Everything => rsmpeg::ffi::AV_LOG_DEBUG,
    };

    // SAFETY: ffmpeg's own global setter, taking a plain integer.
    unsafe { rsmpeg::ffi::av_log_set_level(level as i32) };
}
