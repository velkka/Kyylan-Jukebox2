//! The music library: finding and reading audio files, keeping the `tracks` table in step
//! with the folders, and the browse and search queries over it. Ports src/main/library.ts.

pub mod export;
pub mod folders;
pub mod metadata;
pub mod query;
pub mod scan;
pub mod walk;

pub use scan::{ScanTicket, Scanner};
pub use walk::{is_audio_file, Walker, AUDIO_EXTENSIONS};
