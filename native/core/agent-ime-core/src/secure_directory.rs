#![cfg_attr(not(any(test, feature = "librime")), allow(dead_code))]

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::{self, OpenOptions};
use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::raw::{c_char, c_int, c_uint};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};

#[cfg(unix)]
const O_RDONLY: c_int = 0;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: c_int = 0x0000_0100;
#[cfg(target_os = "macos")]
const O_DIRECTORY: c_int = 0x0010_0000;
#[cfg(target_os = "macos")]
const O_CLOEXEC: c_int = 0x0100_0000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: c_int = 0x0002_0000;
#[cfg(target_os = "linux")]
const O_DIRECTORY: c_int = 0x0001_0000;
#[cfg(target_os = "linux")]
const O_CLOEXEC: c_int = 0x0008_0000;

#[cfg(unix)]
unsafe extern "C" {
    fn geteuid() -> c_uint;
    fn mkdirat(directory_descriptor: c_int, path: *const c_char, mode: c_uint) -> c_int;
    fn openat(directory_descriptor: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
}

#[cfg(unix)]
pub(crate) fn effective_user_id() -> u32 {
    unsafe { geteuid() }
}

/// macOS exposes two root-owned compatibility links that are part of the
/// platform ABI. Convert only those exact aliases to their physical spelling;
/// arbitrary user-controlled symlinks remain forbidden by the component walk.
pub(crate) fn physical_runtime_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        for (alias, physical) in [("/tmp", "/private/tmp"), ("/var", "/private/var")] {
            if path == Path::new(alias) {
                return PathBuf::from(physical);
            }
            if let Ok(remainder) = path.strip_prefix(alias) {
                return PathBuf::from(physical).join(remainder);
            }
        }
    }
    path.to_path_buf()
}

#[cfg(unix)]
pub(crate) fn validate_owned_directory(file: &File, description: &str) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != effective_user_id() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{description} is not an owned real directory"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
/// Opens an absolute directory without following a symlink in any component.
/// Only the final directory must be owned by this effective user: system
/// ancestors such as `/`, `/Users`, and `/private/tmp` are normally root-owned.
fn open_owned_directory_chain(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "secure directory path must be absolute",
        ));
    }
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC)
        .open("/")?;
    let mut saw_normal = false;
    for component in path.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "secure directory path contains a non-normal component",
            ));
        };
        saw_normal = true;
        let name = CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "secure directory component contains NUL",
            )
        })?;
        let descriptor = unsafe {
            openat(
                current.as_raw_fd(),
                name.as_ptr(),
                O_RDONLY | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
                0,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        current = unsafe { File::from_raw_fd(descriptor) };
        if !current.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "secure directory component changed type",
            ));
        }
    }
    if !saw_normal {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "filesystem root is not an owned runtime directory",
        ));
    }
    validate_owned_directory(&current, "secure directory")?;
    Ok(current)
}

/// Creates only the final directory name under a stable, no-follow parent fd.
/// Missing ancestors are rejected rather than manufactured through a pathname.
#[cfg(unix)]
pub(crate) fn open_or_create_owned_leaf_directory(path: &Path, mode: u32) -> io::Result<File> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "secure directory has no parent",
        )
    })?;
    let leaf = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "secure directory has no leaf")
    })?;
    let leaf = CString::new(leaf.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "secure directory leaf contains NUL",
        )
    })?;
    let parent = open_owned_directory_chain(parent)?;

    loop {
        let descriptor = unsafe {
            openat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                O_RDONLY | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC,
                0,
            )
        };
        if descriptor >= 0 {
            let directory = unsafe { File::from_raw_fd(descriptor) };
            validate_owned_directory(&directory, "runtime leaf directory")?;
            directory.set_permissions(fs::Permissions::from_mode(mode))?;
            return Ok(directory);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
        if unsafe { mkdirat(parent.as_raw_fd(), leaf.as_ptr(), mode) } == 0 {
            parent.sync_all()?;
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
}


// Windows: the per-user profile directory is already protected by its ACL, and there
// is no fd-relative API. Same contract: only the leaf is created, and neither the leaf
// nor its parent may be a reparse point (symlink or junction).
#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn effective_user_id() -> u32 {
    0
}

#[cfg(windows)]
fn reject_reparse_point(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "secure directory is not a real directory",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[cfg(windows)]
pub(crate) fn validate_owned_directory(file: &File, description: &str) -> io::Result<()> {
    if !file.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{description} is not a real directory"),
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn open_or_create_owned_leaf_directory(path: &Path, _mode: u32) -> io::Result<File> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "secure directory has no parent")
    })?;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "secure directory path must be absolute",
        ));
    }
    reject_reparse_point(parent)?;
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    reject_reparse_point(path)?;
    open_directory(path)
}

/// A private regular file owned by this user with a single link (Windows: a regular
/// file; the profile ACL keeps it private).
pub(crate) fn private_regular_file(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        metadata.file_type().is_file()
            && metadata.uid() == effective_user_id()
            && metadata.nlink() == 1
            && metadata.mode() & 0o077 == 0
    }
    #[cfg(windows)]
    {
        metadata.file_type().is_file()
    }
}

/// Owner-only permissions for a file this process creates.
pub(crate) fn private_create(options: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    {
        options.mode(0o600)
    }
    #[cfg(windows)]
    {
        options
    }
}

/// Makes a rename inside `directory` durable (Windows: renames are durable once
/// MoveFileEx returns; directories cannot be opened for flushing).
pub(crate) fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(directory)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = directory;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_platform_root_aliases_are_normalized() {
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                physical_runtime_path(Path::new("/tmp/a")),
                Path::new("/private/tmp/a")
            );
            assert_eq!(
                physical_runtime_path(Path::new("/var/folders/a")),
                Path::new("/private/var/folders/a")
            );
        }
        assert_eq!(
            physical_runtime_path(Path::new("/various/a")),
            Path::new("/various/a")
        );
        assert_eq!(
            physical_runtime_path(Path::new("/Users/a")),
            Path::new("/Users/a")
        );
    }
}
