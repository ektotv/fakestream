//! Declarations for the parts of libcaption we use.
//!
//! Written by hand rather than generated. The surface is a handful of
//! functions, so a binding generator would be a dependency earning very little,
//! and hand declarations leave room to say why each piece is here.
//!
//! Every struct is opaque. Allocation happens in `shim.c`, where the real
//! headers are in scope, so no size or layout is ever guessed on this side.

use std::os::raw::{c_char, c_double, c_int, c_uchar};

/// Opaque `caption_frame_t`, a 608 screen buffer plus decoder state.
pub enum CaptionFrame {}

/// Opaque `sei_t`, the head of a list of SEI messages.
pub enum Sei {}

/// Opaque `sei_message_t`.
pub enum SeiMessage {}

unsafe extern "C" {
    /// Allocate and initialise a caption frame. Null on allocation failure.
    pub fn fakestream_caption_frame_new() -> *mut CaptionFrame;
    pub fn fakestream_caption_frame_free(frame: *mut CaptionFrame);

    /// Allocate and initialise an SEI list. Null on allocation failure.
    pub fn fakestream_sei_new(timestamp: c_double) -> *mut Sei;
    /// Frees the message list and the struct together.
    pub fn fakestream_sei_free(sei: *mut Sei);

    /// Lay UTF-8 text onto the frame's screen buffer, wrapping to 608 rows.
    pub fn caption_frame_from_text(frame: *mut CaptionFrame, data: *const c_char) -> c_int;

    /// Encode a caption frame into SEI messages, one per frame of transmission.
    pub fn sei_from_caption_frame(sei: *mut Sei, frame: *mut CaptionFrame) -> c_int;

    /// First message in the list, or null when empty.
    pub fn fakestream_sei_message_head(sei: *mut Sei) -> *mut SeiMessage;
    /// Next message, or null at the end.
    pub fn sei_message_next(message: *mut SeiMessage) -> *mut SeiMessage;
    /// Payload length in bytes.
    pub fn sei_message_size(message: *mut SeiMessage) -> usize;
    /// Payload bytes, an ITU T.35 blob for the messages we care about.
    pub fn sei_message_data(message: *mut SeiMessage) -> *mut c_uchar;
}

/// Control codes from `eia608_control_t`, only the ones fakestream issues.
pub const ERASE_DISPLAY_MEMORY: c_int = 0x142C;

unsafe extern "C" {
    /// Encode a control command into a 608 word, applying channel selection.
    pub fn eia608_control_command(command: c_int, channel: c_int) -> u16;
}
