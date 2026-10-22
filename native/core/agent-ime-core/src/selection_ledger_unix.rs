use crate::secure_directory::{effective_user_id, open_or_create_owned_leaf_directory};
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const DIRECTORY_NAME: &str = ".agent-ime-selection-ledger-v1";
const LOCK_FILE_NAME: &str = "ledger.lock";
const LEDGER_FILE_NAME: &str = "delivery-ids.bin";
const MAGIC: &[u8; 8] = b"AIMSDL1\n";
const MAXIMUM_IDENTIFIERS: usize = 4_096;
const HEADER_BYTES: usize = MAGIC.len() + std::mem::size_of::<u32>();
const MAXIMUM_FILE_BYTES: usize = HEADER_BYTES + MAXIMUM_IDENTIFIERS * std::mem::size_of::<u64>();

const LOCK_EX: c_int = 0x02;
const LOCK_NB: c_int = 0x04;
const LOCK_UN: c_int = 0x08;
const O_RDWR: c_int = 0x0000_0002;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: c_int = 0x0000_0100;
#[cfg(target_os = "macos")]
const O_NONBLOCK: c_int = 0x0000_0004;
#[cfg(target_os = "macos")]
const O_CREAT: c_int = 0x0000_0200;
#[cfg(target_os = "macos")]
const O_EXCL: c_int = 0x0000_0800;
#[cfg(target_os = "macos")]
const O_CLOEXEC: c_int = 0x0100_0000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: c_int = 0x0002_0000;
#[cfg(target_os = "linux")]
const O_NONBLOCK: c_int = 0x0000_0800;
#[cfg(target_os = "linux")]
const O_CREAT: c_int = 0x0000_0040;
#[cfg(target_os = "linux")]
const O_EXCL: c_int = 0x0000_0080;
#[cfg(target_os = "linux")]
const O_CLOEXEC: c_int = 0x0008_0000;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_NOFOLLOW: c_int = 0;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_NONBLOCK: c_int = 0;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_CREAT: c_int = 0;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_EXCL: c_int = 0;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_CLOEXEC: c_int = 0;

unsafe extern "C" {
    fn flock(file_descriptor: c_int, operation: c_int) -> c_int;
    fn openat(directory_descriptor: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionClaim {
    Fresh,
    AlreadyClaimed,
    Unavailable,
}

/// A small, privacy-preserving write-ahead claim ledger for librime learning.
///
/// Only opaque, boot-scoped physical-event IDs are retained. A claim is made
/// durable before librime is allowed to learn. Consequently a crash may lose
/// one learning update, but replay can never apply the same update twice while
/// its ID remains in the bounded ledger.
pub(crate) struct SelectionDeliveryLedger {
    directory: PathBuf,
}

impl SelectionDeliveryLedger {
    pub(crate) fn new(user_directory: &Path) -> Self {
        Self {
            directory: user_directory.join(DIRECTORY_NAME),
        }
    }

    pub(crate) fn claim(&self, identifier: u64) -> SelectionClaim {
        if identifier == 0 {
            return SelectionClaim::Unavailable;
        }
        self.claim_locked(identifier)
            .unwrap_or(SelectionClaim::Unavailable)
    }

    fn claim_locked(&self, identifier: u64) -> io::Result<SelectionClaim> {
        let directory = StableDirectory::open_or_create(&self.directory)?;
        let lock = LockedFile::acquire(&directory)?;
        let (mut ledger, created) = directory.open_single_link_regular(LEDGER_FILE_NAME, true)?;
        let mut identifiers = Self::read_identifiers(&mut ledger, created)?;
        if identifiers.contains(&identifier) {
            ledger.sync_all()?;
            directory.verify_child_identity(LEDGER_FILE_NAME, &ledger)?;
            lock.verify_identity(&directory)?;
            directory.verify_path_identity()?;
            return Ok(SelectionClaim::AlreadyClaimed);
        }
        if identifiers.len() == MAXIMUM_IDENTIFIERS {
            identifiers.remove(0);
        }
        identifiers.push(identifier);
        Self::persist_identifiers(&directory, &mut ledger, &identifiers)?;
        lock.verify_identity(&directory)?;
        directory.verify_path_identity()?;
        Ok(SelectionClaim::Fresh)
    }

    fn read_identifiers(file: &mut File, created: bool) -> io::Result<Vec<u64>> {
        let metadata = file.metadata()?;
        if metadata.len() > MAXIMUM_FILE_BYTES as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ledger exceeds its bound",
            ));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        if created {
            if metadata.len() != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "new ledger was not empty",
                ));
            }
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(0))?;
        let mut data = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(file)
            .take((MAXIMUM_FILE_BYTES + 1) as u64)
            .read_to_end(&mut data)?;
        Self::decode(&data)
    }

    fn persist_identifiers(
        directory: &StableDirectory,
        file: &mut File,
        identifiers: &[u64],
    ) -> io::Result<()> {
        let data = Self::encode(identifiers)?;
        // Update the already-open, validated inode in place. A crash during
        // this write can corrupt the bounded ledger, but corruption fails
        // closed on every later selection. Avoiding path-based replacement
        // also means a swapped symlink or hard link can never be chmod'ed,
        // unlinked, or overwritten by this process.
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(&data)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
        directory.file.sync_all()?;
        directory.verify_child_identity(LEDGER_FILE_NAME, file)
    }

    fn encode(identifiers: &[u64]) -> io::Result<Vec<u8>> {
        if identifiers.len() > MAXIMUM_IDENTIFIERS
            || identifiers.contains(&0)
            || identifiers.iter().copied().collect::<HashSet<_>>().len() != identifiers.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid delivery ID set",
            ));
        }
        let mut data = Vec::with_capacity(HEADER_BYTES + identifiers.len() * 8);
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&(identifiers.len() as u32).to_be_bytes());
        for identifier in identifiers {
            data.extend_from_slice(&identifier.to_be_bytes());
        }
        Ok(data)
    }

    fn decode(data: &[u8]) -> io::Result<Vec<u64>> {
        if data.len() < HEADER_BYTES
            || data.len() > MAXIMUM_FILE_BYTES
            || &data[..MAGIC.len()] != MAGIC
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid ledger header",
            ));
        }
        let count =
            u32::from_be_bytes(data[MAGIC.len()..HEADER_BYTES].try_into().unwrap()) as usize;
        if count > MAXIMUM_IDENTIFIERS || data.len() != HEADER_BYTES + count * 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid ledger length",
            ));
        }
        let mut identifiers = Vec::with_capacity(count);
        let mut unique = HashSet::with_capacity(count);
        for chunk in data[HEADER_BYTES..].chunks_exact(8) {
            let identifier = u64::from_be_bytes(chunk.try_into().unwrap());
            if identifier == 0 || !unique.insert(identifier) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid ledger identifier",
                ));
            }
            identifiers.push(identifier);
        }
        Ok(identifiers)
    }
}

struct StableDirectory {
    file: File,
    path: PathBuf,
}

impl StableDirectory {
    fn open_or_create(path: &Path) -> io::Result<Self> {
        let directory = Self {
            file: open_or_create_owned_leaf_directory(path, 0o700)?,
            path: path.to_path_buf(),
        };
        directory.verify_path_identity()?;
        Ok(directory)
    }

    fn open_single_link_regular(&self, name: &str, create: bool) -> io::Result<(File, bool)> {
        let name = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "ledger leaf contains NUL"))?;
        let existing_flags = O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC;
        let (file, created) = match self.open_at(&name, existing_flags, 0) {
            Ok(file) => (file, false),
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => (
                self.open_at(&name, existing_flags | O_CREAT | O_EXCL, 0o600)?,
                true,
            ),
            Err(error) => return Err(error),
        };
        Self::validate_single_link_regular(&file)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        self.verify_path_identity()?;
        Ok((file, created))
    }

    fn open_at(&self, name: &CStr, flags: c_int, mode: c_uint) -> io::Result<File> {
        let descriptor = unsafe { openat(self.file.as_raw_fd(), name.as_ptr(), flags, mode) };
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    fn validate_single_link_regular(file: &File) -> io::Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != effective_user_id() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ledger entry is not a single-link regular file",
            ));
        }
        Ok(())
    }

    fn verify_child_identity(&self, name: &str, expected: &File) -> io::Result<()> {
        let name = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "ledger leaf contains NUL"))?;
        let current = self.open_at(&name, O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, 0)?;
        Self::validate_single_link_regular(&current)?;
        if !same_identity(&current.metadata()?, &expected.metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ledger directory entry changed identity",
            ));
        }
        Ok(())
    }

    fn verify_path_identity(&self) -> io::Result<()> {
        let current = fs::symlink_metadata(&self.path)?;
        if current.file_type().is_symlink()
            || !current.is_dir()
            || current.uid() != effective_user_id()
            || !same_identity(&current, &self.file.metadata()?)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ledger directory path changed identity",
            ));
        }
        Ok(())
    }
}

fn same_identity(lhs: &fs::Metadata, rhs: &fs::Metadata) -> bool {
    lhs.dev() == rhs.dev() && lhs.ino() == rhs.ino()
}

struct LockedFile {
    file: File,
}

impl LockedFile {
    fn acquire(directory: &StableDirectory) -> io::Result<Self> {
        let (file, created) = directory.open_single_link_regular(LOCK_FILE_NAME, true)?;
        loop {
            if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
                if created {
                    directory.file.sync_all()?;
                }
                let locked = Self { file };
                locked.verify_identity(directory)?;
                return Ok(locked);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn verify_identity(&self, directory: &StableDirectory) -> io::Result<()> {
        directory.verify_child_identity(LOCK_FILE_NAME, &self.file)
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        let _ = unsafe { flock(self.file.as_raw_fd(), LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::raw::c_char;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn mkfifo(path: *const c_char, mode: u32) -> c_int;
    }

    fn create_fifo(path: &Path) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { mkfifo(path.as_ptr(), 0o600) }, 0);
    }

    fn temporary_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        // Use the project test contract's isolated /private/tmp namespace rather than
        // inheriting a potentially symlinked or unusually long TMPDIR path.
        let root = PathBuf::from("/private/tmp").join(format!(
            "agent-ime-ledger-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn durable_claim_is_shared_and_zero_fails_closed() {
        let root = temporary_root();
        let first = SelectionDeliveryLedger::new(&root);
        assert_eq!(first.claim(0), SelectionClaim::Unavailable);
        assert_eq!(first.claim(42), SelectionClaim::Fresh);
        let ledger_file = root.join(DIRECTORY_NAME).join(LEDGER_FILE_NAME);
        fs::set_permissions(&ledger_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            SelectionDeliveryLedger::new(&root).claim(42),
            SelectionClaim::AlreadyClaimed
        );
        assert_eq!(
            fs::metadata(&ledger_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            SelectionDeliveryLedger::new(&root).claim(43),
            SelectionClaim::Fresh
        );
        let metadata = fs::metadata(ledger_file).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(root.join(DIRECTORY_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join(DIRECTORY_NAME).join(LOCK_FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn contended_lock_fails_closed_without_waiting_and_recovers_after_release() {
        let root = temporary_root();
        let ledger = SelectionDeliveryLedger::new(&root);
        let directory = StableDirectory::open_or_create(&ledger.directory).unwrap();
        let held_lock = LockedFile::acquire(&directory).unwrap();

        let started = Instant::now();
        assert_eq!(ledger.claim(901), SelectionClaim::Unavailable);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "contended learning ledger must never block candidate commit"
        );
        assert!(!ledger.directory.join(LEDGER_FILE_NAME).exists());

        drop(held_lock);
        assert_eq!(ledger.claim(901), SelectionClaim::Fresh);
        assert!(ledger.directory.join(LEDGER_FILE_NAME).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_or_symlinked_ledger_fails_closed_without_replacement() {
        let root = temporary_root();
        let ledger = SelectionDeliveryLedger::new(&root);
        fs::create_dir(&ledger.directory).unwrap();
        let file = ledger.directory.join(LEDGER_FILE_NAME);
        fs::write(&file, b"corrupt").unwrap();
        assert_eq!(ledger.claim(99), SelectionClaim::Unavailable);
        assert_eq!(fs::read(&file).unwrap(), b"corrupt");
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink("missing", &file).unwrap();
        assert_eq!(ledger.claim(99), SelectionClaim::Unavailable);
        assert!(
            fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn torn_in_place_update_fails_closed_without_repair_or_replacement() {
        let root = temporary_root();
        let ledger = SelectionDeliveryLedger::new(&root);
        assert_eq!(ledger.claim(71), SelectionClaim::Fresh);
        let file = ledger.directory.join(LEDGER_FILE_NAME);
        let original_identity = fs::metadata(&file).unwrap();

        let complete = SelectionDeliveryLedger::encode(&[71, 72]).unwrap();
        let torn = &complete[..HEADER_BYTES + 3];
        fs::write(&file, torn).unwrap();
        assert!(same_identity(
            &original_identity,
            &fs::metadata(&file).unwrap()
        ));

        assert_eq!(ledger.claim(72), SelectionClaim::Unavailable);
        assert_eq!(ledger.claim(71), SelectionClaim::Unavailable);
        assert_eq!(fs::read(&file).unwrap(), torn);
        assert!(same_identity(
            &original_identity,
            &fs::metadata(&file).unwrap()
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_or_special_ledger_paths_fail_closed() {
        let root = temporary_root();
        let ledger = SelectionDeliveryLedger::new(&root);
        let external = root.join("external-ledger-directory");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"do-not-touch").unwrap();
        fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&external, &ledger.directory).unwrap();
        assert_eq!(ledger.claim(51), SelectionClaim::Unavailable);
        assert_eq!(fs::read(&sentinel).unwrap(), b"do-not-touch");
        assert_eq!(
            fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
            0o640
        );
        fs::remove_file(&ledger.directory).unwrap();

        create_fifo(&ledger.directory);
        assert_eq!(ledger.claim(52), SelectionClaim::Unavailable);
        assert!(
            fs::symlink_metadata(&ledger.directory)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        fs::remove_file(&ledger.directory).unwrap();

        fs::create_dir(&ledger.directory).unwrap();
        let ledger_path = ledger.directory.join(LEDGER_FILE_NAME);
        create_fifo(&ledger_path);
        assert_eq!(ledger.claim(53), SelectionClaim::Unavailable);
        assert!(
            fs::symlink_metadata(&ledger_path)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        fs::remove_file(&ledger_path).unwrap();

        let lock_path = ledger.directory.join(LOCK_FILE_NAME);
        fs::remove_file(&lock_path).unwrap();
        create_fifo(&lock_path);
        assert_eq!(ledger.claim(54), SelectionClaim::Unavailable);
        assert!(
            fs::symlink_metadata(&lock_path)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hard_linked_ledger_files_fail_closed_without_touching_external_sentinels() {
        for (leaf, identifier) in [(LEDGER_FILE_NAME, 61_u64), (LOCK_FILE_NAME, 62_u64)] {
            let root = temporary_root();
            let ledger = SelectionDeliveryLedger::new(&root);
            fs::create_dir(&ledger.directory).unwrap();
            let external = root.join(format!("external-{leaf}"));
            let payload = format!("do-not-touch-{leaf}").into_bytes();
            fs::write(&external, &payload).unwrap();
            fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).unwrap();
            let linked = ledger.directory.join(leaf);
            fs::hard_link(&external, &linked).unwrap();

            assert_eq!(ledger.claim(identifier), SelectionClaim::Unavailable);
            assert_eq!(fs::read(&external).unwrap(), payload);
            let metadata = fs::metadata(&external).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
            assert_eq!(metadata.nlink(), 2);
            assert!(linked.exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn stable_directory_fd_never_follows_a_replacement_symlink() {
        let root = temporary_root();
        let ledger = SelectionDeliveryLedger::new(&root);
        fs::create_dir(&ledger.directory).unwrap();
        let stable = StableDirectory::open_or_create(&ledger.directory).unwrap();
        let moved = root.join("moved-ledger-directory");
        fs::rename(&ledger.directory, &moved).unwrap();

        let external = root.join("external-directory");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"external-do-not-touch").unwrap();
        fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&external, &ledger.directory).unwrap();

        assert!(stable.verify_path_identity().is_err());
        assert!(
            stable
                .open_single_link_regular(LEDGER_FILE_NAME, true)
                .is_err()
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"external-do-not-touch");
        assert_eq!(
            fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(!external.join(LEDGER_FILE_NAME).exists());
        assert!(moved.join(LEDGER_FILE_NAME).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_user_directory_ancestor_fails_closed_without_external_write() {
        let root = temporary_root();
        let user = root.join("Rime");
        fs::create_dir(&user).unwrap();
        let moved = root.join("moved-Rime");
        fs::rename(&user, &moved).unwrap();
        let external = root.join("external-Rime");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"external-do-not-touch").unwrap();
        std::os::unix::fs::symlink(&external, &user).unwrap();

        let ledger = SelectionDeliveryLedger::new(&user);
        assert_eq!(ledger.claim(88), SelectionClaim::Unavailable);
        assert_eq!(fs::read(&sentinel).unwrap(), b"external-do-not-touch");
        assert!(!external.join(DIRECTORY_NAME).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_ledger_remains_bounded_and_evicts_only_the_oldest_claim() {
        let root = temporary_root();
        let ledger = SelectionDeliveryLedger::new(&root);
        fs::create_dir(&ledger.directory).unwrap();
        let file = ledger.directory.join(LEDGER_FILE_NAME);
        let identifiers = (1..=MAXIMUM_IDENTIFIERS as u64).collect::<Vec<_>>();
        fs::write(
            &file,
            SelectionDeliveryLedger::encode(&identifiers).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ledger.claim(MAXIMUM_IDENTIFIERS as u64 + 1),
            SelectionClaim::Fresh
        );
        let persisted = SelectionDeliveryLedger::decode(&fs::read(file).unwrap()).unwrap();
        assert_eq!(persisted.len(), MAXIMUM_IDENTIFIERS);
        assert_eq!(persisted.first(), Some(&2));
        assert_eq!(persisted.last(), Some(&(MAXIMUM_IDENTIFIERS as u64 + 1)));
        fs::remove_dir_all(root).unwrap();
    }
}
