//! Turning text into each caption format a player might have to handle.
//!
//! The formats split by who does the rendering. DVB is a bitmap we draw
//! ourselves, so glyph work happens here. CEA-608 is a byte stream the player
//! renders. Text formats carry UTF-8 and the player does everything.

pub mod ass;
pub mod cea608;
pub mod dvb;
pub mod libcaption;
pub mod rolling;
pub mod script;
pub mod text;
