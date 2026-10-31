use crate::secure_directory::{private_create, private_regular_file};
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

const LOCK_FILE_NAME: &str = ".agent_zhixu_learning_v1.lock";
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x01;
const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x02;
const ERROR_LOCK_VIOLATION: i32 = 33;

#[repr(C)]
#[derive(Default)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct FileInformation {
    attributes: u32,
    creation_time: [u32; 2],
    access_time: [u32; 2],
    write_time: [u32; 2],
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LockFileEx(
        file: *mut c_void,
        flags: u32,
        reserved: u32,
        low: u32,
        high: u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn UnlockFileEx(
        file: *mut c_void,
        reserved: u32,
        low: u32,
        high: u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetFileInformationByHandle(file: *mut c_void, information: *mut FileInformation) -> i32;
}

fn information(file: &File) -> io::Result<FileInformation> {
    let mut result = FileInformation::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut result) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result)
}

pub(super) struct LearningLock {
    file: File,
}

impl LearningLock {
    pub(super) fn acquire(user_dir: &Path) -> io::Result<Self> {
        let path = user_dir.join(LOCK_FILE_NAME);
        let mut options = OpenOptions::new();
        // Sharing reads and writes permits contenders to reach LockFileEx, but
        // withholding DELETE keeps this lock's inode stable until all close it.
        private_create(options.read(true).write(true).create(true).truncate(false));
        options
            .share_mode(0x01 | 0x02)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        let actual = information(&file)?;
        if !private_regular_file(&metadata)
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || actual.links != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid Zhixu learning lock",
            ));
        }
        let mut overlapped = Overlapped::default();
        if unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK,
                0,
                1,
                0,
                &mut overlapped,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return Err(if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
                io::Error::new(io::ErrorKind::TimedOut, "Zhixu learning lock is busy")
            } else {
                error
            });
        }
        let locked = Self { file };
        let named_metadata = fs::symlink_metadata(&path)?;
        let named = OpenOptions::new()
            .read(true)
            .share_mode(0x01 | 0x02)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let named_information = information(&named)?;
        if !private_regular_file(&named_metadata)
            || named_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || actual.volume_serial != named_information.volume_serial
            || actual.index_high != named_information.index_high
            || actual.index_low != named_information.index_low
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
        let mut overlapped = Overlapped::default();
        let _ = unsafe { UnlockFileEx(self.file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let directory = std::env::temp_dir().join(format!(
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
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_hardlinked_lock_without_changing_target() {
        let directory = scratch();
        let target = directory.join("target");
        fs::write(&target, b"unchanged").unwrap();
        fs::hard_link(&target, directory.join(LOCK_FILE_NAME)).unwrap();
        assert!(LearningLock::acquire(&directory).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"unchanged");
        fs::remove_dir_all(directory).unwrap();
    }
}
