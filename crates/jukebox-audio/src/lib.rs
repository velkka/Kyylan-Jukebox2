//! Kyylan Jukebox's audio engine: the [`AudioPlayer`] the queue engine drives.
//!
//! Files are decoded with symphonia, and Opus with libopus ([`source`]); converted to the
//! output's channels and sample rate with rubato; and played through cpal from a lock-free
//! ring buffer ([`output`]).

mod mp4;
pub mod output;
mod pipeline;
mod player;
pub mod source;

pub use output::{Backend, CpalBackend, VirtualBackend};
pub use player::{AudioPlayer, TrackResolver, DEFAULT_DEVICE};
