//! fakestream generates synthetic test video for people building AV players.
//!
//! Every output is made from nothing. There are no seed files and no
//! third-party sample clips, and caption text is generated lorem ipsum.

pub mod captions;
pub mod fixtures;
pub mod media;
pub mod progress;
pub mod serve;
