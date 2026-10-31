use crate::secure_directory::{open_or_create_owned_leaf_directory, private_regular_file};
use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const LOCK_FILE_NAME: &str = ".agent_zhixu_learning_v1.lock";
const LOCK_EX: c_int = 0x02;
const LOCK_NB: c_int = 0x04;
const LOCK_UN: c_int = 0x08;
const O_RDWR: c_int = 0x02;
#[cfg(target_os = "macos")]
const OPEN_FLAGS: c_int = 0x100 | 0x04 | 0x200 | 0x0100_0000;
#[cfg(target_os = "linux")]
const OPEN_FLAGS: c_int = 0x0002_0000 | 0x0800 | 0x40 | 0x0008_0000;

unsafe extern "C" {
    fn flock(file_descriptor: c_int, operation: c_int) -> c_int;
    fn openat(directory_descriptor: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
}

pub(super) struct LearningLock {
    file: File,
}

impl LearningLock {
    pub(super) fn acquire(user_dir: &Path) -> io::Result<Self> {
        let directory = open_or_create_owned_leaf_directory(user_dir, 0o700)?;
        let name = CString::new(LOCK_FILE_NAME).unwrap();
        // Keep creation anchored to the validated directory, even if its path
        // changes. O_NOFOLLOW and 0600 match the private-file creation contract.
        let descriptor = unsafe {
            openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                O_RDWR | OPEN_FLAGS,
                0o600 as c_uint,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        if !private_regular_file(&file.metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid Zhixu learning lock",
            ));
        }
        if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return Err(if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(io::ErrorKind::TimedOut, "Zhixu learning lock is busy")
            } else {
                error
            });
        }
        let locked = Self { file };
        let actual = locked.file.metadata()?;
        let named = fs::symlink_metadata(user_dir.join(LOCK_FILE_NAME))?;
        let actual_directory = directory.metadata()?;
        let named_directory = fs::symlink_metadata(user_dir)?;
        if !private_regular_file(&named)
            || actual.dev() != named.dev()
            || actual.ino() != named.ino()
            || !named_directory.is_dir()
            || actual_directory.dev() != named_directory.dev()
            || actual_directory.ino() != named_directory.ino()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Zhixu learning lock changed identity",
            ));
        }
        Ok(locked)
    }
}

impl Drop for LearningLock {
    fn drop(&mut self) {
        let _ = unsafe { flock(self.file.as_raw_fd(), LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let directory =
            crate::secure_directory::physical_runtime_path(&std::env::temp_dir()).join(format!(
                "zhixu-lock-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn busy_lock_times_out_and_drop_releases_it() {
        let directory = scratch();
        let first = LearningLock::acquire(&directory).unwrap();
        assert!(
            matches!(LearningLock::acquire(&directory), Err(error) if error.kind() == io::ErrorKind::TimedOut)
        );
        drop(first);
        drop(LearningLock::acquire(&directory).unwrap());
        assert_eq!(
            fs::metadata(directory.join(LOCK_FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_links_and_nonprivate_lock_without_changing_them() {
        let directory = scratch();
        let target = directory.join("target");
        fs::write(&target, b"unchanged").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let lock = directory.join(LOCK_FILE_NAME);
        symlink(&target, &lock).unwrap();
        assert!(LearningLock::acquire(&directory).is_err());
        fs::remove_file(&lock).unwrap();
        fs::hard_link(&target, &lock).unwrap();
        assert!(LearningLock::acquire(&directory).is_err());
        fs::remove_file(&lock).unwrap();
        fs::write(&lock, b"unchanged").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(LearningLock::acquire(&directory).is_err());
        assert_eq!(
            fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(target).unwrap(), b"unchanged");
        fs::remove_dir_all(directory).unwrap();
    }
}
