//! One jukebox per data directory: a second copy started for the same user would only find
//! the port taken.

use std::fs::{File, OpenOptions};
use std::path::Path;

pub const LOCK_FILE: &str = "kyylan-jukebox.lock";

/// Held for as long as the program runs; the OS releases it when the process ends, however
/// it ends.
pub struct InstanceLock {
    _guard: fd_lock::RwLockWriteGuard<'static, File>,
}

pub enum AcquireError {
    /// Another copy is running on this data directory.
    Held,
    Io(std::io::Error),
}

pub fn acquire(data_dir: &Path) -> Result<InstanceLock, AcquireError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join(LOCK_FILE))
        .map_err(AcquireError::Io)?;
    // Lives for the rest of the process, like the lock itself.
    let lock: &'static mut fd_lock::RwLock<File> = Box::leak(Box::new(fd_lock::RwLock::new(file)));
    match lock.try_write() {
        Ok(guard) => Ok(InstanceLock { _guard: guard }),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Err(AcquireError::Held),
        Err(err) => Err(AcquireError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_copy_finds_the_lock_held() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path());
        assert!(first.is_ok());
        assert!(matches!(acquire(dir.path()), Err(AcquireError::Held)));
        drop(first);
    }
}
