//! The music library: finding and reading audio files, keeping the `tracks` table in step
//! with the folders, and the browse and search queries over it. Ports src/main/library.ts.

pub mod export;
pub mod folders;
mod js;
pub mod metadata;
pub mod query;
pub mod scan;
pub mod walk;

pub use scan::{ScanTicket, Scanner};
pub use walk::{is_audio_file, Walker, AUDIO_EXTENSIONS};

#[doc(hidden)]
pub mod js_for_tests {
    //! The JavaScript and Node behaviours, exposed for integration tests.
    pub use super::js::{normalize_posix, normalize_win32, path_join};
}
