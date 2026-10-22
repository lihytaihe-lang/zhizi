use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::{CStr, CString, OsStr},
    fmt,
    fs::{File, Metadata, Permissions},
    io::{self, Read, Write},
    os::{
        raw::{c_char, c_int, c_uint},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
            io::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
        },
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const FORMAT_V1_HEADER: &str = "AIMSNAP1";
const FORMAT_V2_HEADER: &str = "AIMSNAP2";
const POINTER_MAXIMUM_BYTES: usize = 4_096;
const SNAPSHOT_MAXIMUM_BYTES: usize = 16 * 1024 * 1024;
const SNAPSHOT_MAXIMUM_ENTRIES: usize = 100_000;
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const PUBLISH_LOCK_NAME: &str = ".snapshot-publish.lock";

// Darwin values are fixed by <sys/fcntl.h>. This target is the macOS manager.
const O_RDONLY: c_int = 0x0000;
const O_WRONLY: c_int = 0x0001;
const O_RDWR: c_int = 0x0002;
const O_NONBLOCK: c_int = 0x0004;
const O_NOFOLLOW: c_int = 0x0100;
const O_CREAT: c_int = 0x0200;
const O_EXCL: c_int = 0x0800;
const O_DIRECTORY: c_int = 0x0010_0000;
const O_CLOEXEC: c_int = 0x0100_0000;
const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const LOCK_EX: c_int = 0x02;
const LOCK_UN: c_int = 0x08;

static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    pub version: u64,
    pub schema_version: u64,
    pub checksum: String,
    pub file_name: String,
    pub entry_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct LexiconSnapshotSource {
    pub source_kind: String,
    pub source: String,
    pub license: String,
    pub confidence_millis: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LexiconSnapshotEntry {
    pub id: String,
    pub text: String,
    pub pinyin: String,
    pub aliases: Vec<String>,
    pub professional_packs: Vec<String>,
    pub project_domains: Vec<String>,
    pub sources: Vec<LexiconSnapshotSource>,
    pub base_weight: i64,
    pub fixed: bool,
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub demoted: bool,
    #[serde(
        default = "default_candidate_layer",
        skip_serializing_if = "is_first_layer"
    )]
    pub candidate_layer: String,
    pub updated_at_ms: i64,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_candidate_layer() -> String {
    "first".into()
}

fn is_first_layer(value: &String) -> bool {
    value == "first"
}

pub struct SnapshotStore {
    root: PathBuf,
}

impl SnapshotStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn publish_fixture(&self, version: u64) -> Result<SnapshotMeta, String> {
        let entries = [
            ("agentfixture", "联合快照夹具", "fixture", 100_i64),
            ("offline", "离线", "fixture", 90_i64),
        ];
        let body = format!(
            "{FORMAT_V1_HEADER}\nversion={version}\nentry_count={}\n{}",
            entries.len(),
            entries
                .iter()
                .map(|(code, text, source, weight)| {
                    format!("{code}\t{text}\t{source}\t{weight}\n")
                })
                .collect::<String>()
        );
        self.publish_bytes(version, body.as_bytes())
    }

    pub fn publish_lexicon(
        &self,
        version: u64,
        mut entries: Vec<LexiconSnapshotEntry>,
    ) -> Result<SnapshotMeta, String> {
        for entry in &mut entries {
            normalize_lexicon_entry(entry)?;
        }
        let mut entry_ids = HashSet::new();
        if entries
            .iter()
            .any(|entry| !entry_ids.insert(entry.id.clone()))
        {
            return Err("个人词快照包含重复 ID。".into());
        }
        entries.sort_by(|left, right| {
            (&left.text, &left.pinyin, &left.id).cmp(&(&right.text, &right.pinyin, &right.id))
        });
        if entries
            .windows(2)
            .any(|pair| pair[0].text == pair[1].text && pair[0].pinyin == pair[1].pinyin)
        {
            return Err("个人词快照包含重复 text+pinyin。".into());
        }
        let mut body = format!(
            "{FORMAT_V2_HEADER}\nschema_version=2\nversion={version}\nentry_count={}\n",
            entries.len()
        );
        for entry in entries {
            let line = serde_json::to_string(&entry)
                .map_err(|error| format!("无法序列化个人词快照：{error}"))?;
            body.push_str(&line);
            body.push('\n');
        }
        self.publish_bytes(version, body.as_bytes())
    }

    fn publish_bytes(&self, version: u64, body: &[u8]) -> Result<SnapshotMeta, String> {
        let parsed = validate_snapshot_bytes(body, None, Some(version), None)
            .map_err(|error| format!("快照内容无效：{error}"))?;
        let checksum = sha256(body);
        let file_name = format!("snapshot-{version}-{checksum}.aimsnap");

        let directory = SecureDirectory::open_or_create(&self.root, DIRECTORY_MODE)
            .map_err(|error| format!("无法安全打开快照目录：{error}"))?;
        // Publishing is one transaction across the immutable snapshot, the
        // `current` pointer, and retention. The persistent inode makes the
        // lock effective across manager processes as well as threads.
        let _publish_lock = directory
            .acquire_exclusive_lock(PUBLISH_LOCK_NAME)
            .map_err(|error| format!("无法锁定快照发布事务：{error}"))?;
        self.reject_conflicting_version(&directory, version, &checksum)?;
        directory
            .atomic_write(body, &file_name, SNAPSHOT_MAXIMUM_BYTES, None)
            .map_err(|error| format!("无法原子发布快照：{error}"))?;

        write_pointer(
            &directory,
            version,
            &checksum,
            &file_name,
            parsed.entry_count,
        )?;
        self.prune(&directory, 2)
            .map_err(|error| format!("无法安全清理旧快照：{error}"))?;

        Ok(SnapshotMeta {
            version,
            schema_version: parsed.schema_version,
            checksum,
            file_name,
            entry_count: parsed.entry_count,
        })
    }

    pub fn rollback_to(&self, version: u64) -> Result<SnapshotMeta, String> {
        let directory = SecureDirectory::open_existing(&self.root, DIRECTORY_MODE)
            .map_err(|error| format!("无法安全打开快照目录：{error}"))?;
        let _publish_lock = directory
            .acquire_exclusive_lock(PUBLISH_LOCK_NAME)
            .map_err(|error| format!("无法锁定快照回滚事务：{error}"))?;
        let mut matches = Vec::new();
        for name in directory
            .entry_names()
            .map_err(|error| format!("无法枚举回滚版本：{error}"))?
        {
            let Some(file) = SnapshotFileName::parse(&name) else {
                continue;
            };
            if file.version != version {
                continue;
            }
            let Ok((bytes, _)) = directory.read_regular(&name, SNAPSHOT_MAXIMUM_BYTES) else {
                continue;
            };
            let Ok(parsed) =
                validate_snapshot_bytes(&bytes, Some(&file.checksum), Some(version), None)
            else {
                continue;
            };
            matches.push((file, parsed));
        }
        if matches.len() != 1 {
            return Err(format!(
                "回滚版本 {version} 必须恰好对应一个有效快照，实际为 {} 个。",
                matches.len()
            ));
        }
        let (file, parsed) = matches.remove(0);
        write_pointer(
            &directory,
            version,
            &file.checksum,
            &file.file_name,
            parsed.entry_count,
        )?;
        self.prune(&directory, 2)
            .map_err(|error| format!("无法安全清理回滚后的快照：{error}"))?;
        Ok(SnapshotMeta {
            version,
            schema_version: parsed.schema_version,
            checksum: file.checksum,
            file_name: file.file_name,
            entry_count: parsed.entry_count,
        })
    }

    pub fn load_current(&self) -> Result<SnapshotMeta, String> {
        let directory = SecureDirectory::open_existing(&self.root, DIRECTORY_MODE)
            .map_err(|error| format!("无法安全打开快照目录：{error}"))?;
        let pointer_bytes = directory
            .read_regular("current", POINTER_MAXIMUM_BYTES)
            .map_err(|error| format!("无法读取当前快照指针：{error}"))?
            .0;
        let pointer =
            parse_pointer(&pointer_bytes).map_err(|error| format!("快照指针无效：{error}"))?;
        let snapshot_bytes = directory
            .read_regular(&pointer.file_name, SNAPSHOT_MAXIMUM_BYTES)
            .map_err(|error| format!("无法读取快照：{error}"))?
            .0;
        let parsed = validate_snapshot_bytes(
            &snapshot_bytes,
            Some(&pointer.checksum),
            Some(pointer.version),
            Some(pointer.entry_count),
        )
        .map_err(|error| format!("快照无效：{error}"))?;
        Ok(SnapshotMeta {
            version: parsed.version,
            schema_version: parsed.schema_version,
            checksum: pointer.checksum,
            file_name: pointer.file_name,
            entry_count: parsed.entry_count,
        })
    }

    fn reject_conflicting_version(
        &self,
        directory: &SecureDirectory,
        version: u64,
        checksum: &str,
    ) -> Result<(), String> {
        for name in directory
            .entry_names()
            .map_err(|error| format!("无法检查快照版本冲突：{error}"))?
        {
            let Some(file) = SnapshotFileName::parse(&name) else {
                continue;
            };
            if file.version == version && file.checksum != checksum {
                return Err(format!(
                    "快照版本 {version} 已存在不同 checksum，版本号不可复用。"
                ));
            }
        }
        Ok(())
    }

    fn prune(&self, directory: &SecureDirectory, keep: usize) -> SecureResult<()> {
        directory.verify_identity()?;
        // Never trust the publisher's in-memory value here. Retention is
        // derived again from the durable pointer while the publish lock is
        // held, so it cannot delete another publisher's current target.
        let pointer_bytes = directory.read_regular("current", POINTER_MAXIMUM_BYTES)?.0;
        let pointer = parse_pointer(&pointer_bytes).map_err(SecureError::InvalidData)?;
        let snapshot_bytes = directory
            .read_regular(&pointer.file_name, SNAPSHOT_MAXIMUM_BYTES)?
            .0;
        validate_snapshot_bytes(
            &snapshot_bytes,
            Some(&pointer.checksum),
            Some(pointer.version),
            Some(pointer.entry_count),
        )
        .map_err(SecureError::InvalidData)?;
        let mut candidates = Vec::new();
        for name in directory.entry_names()? {
            let Some(parsed) = SnapshotFileName::parse(&name) else {
                continue;
            };
            // Unsafe entries (symlink, hard link, wrong owner/mode, non-regular)
            // and invalid snapshot-shaped files are foreign. They are ignored,
            // never pruned merely because their leaf name resembles ours.
            match directory.read_regular(&name, SNAPSHOT_MAXIMUM_BYTES) {
                Ok((bytes, identity))
                    if validate_snapshot_bytes(
                        &bytes,
                        Some(&parsed.checksum),
                        Some(parsed.version),
                        None,
                    )
                    .is_ok() =>
                {
                    candidates.push((parsed, identity));
                }
                Ok(_) => {}
                Err(
                    SecureError::Missing
                    | SecureError::UnsafeLeaf
                    | SecureError::Changed
                    | SecureError::TooLarge,
                ) => {
                    directory.verify_identity()?;
                }
                Err(error) => return Err(error),
            }
        }
        candidates.sort_by(|(left, _), (right, _)| {
            right
                .version
                .cmp(&left.version)
                .then_with(|| right.file_name.cmp(&left.file_name))
        });

        let mut retained = HashSet::new();
        retained.insert(pointer.file_name);
        for (candidate, _) in &candidates {
            if retained.len() >= keep {
                break;
            }
            retained.insert(candidate.file_name.clone());
        }
        for (candidate, observed_identity) in candidates {
            if retained.contains(&candidate.file_name) {
                continue;
            }
            let current = match directory.read_regular(&candidate.file_name, SNAPSHOT_MAXIMUM_BYTES)
            {
                Ok((bytes, identity))
                    if validate_snapshot_bytes(
                        &bytes,
                        Some(&candidate.checksum),
                        Some(candidate.version),
                        None,
                    )
                    .is_ok() =>
                {
                    identity
                }
                Ok(_) => continue,
                Err(
                    SecureError::Missing
                    | SecureError::UnsafeLeaf
                    | SecureError::Changed
                    | SecureError::TooLarge,
                ) => {
                    directory.verify_identity()?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if current != observed_identity {
                continue;
            }
            directory.verify_leaf(&candidate.file_name, &observed_identity)?;
            directory.verify_identity()?;
            unlink_name(directory.file.as_raw_fd(), &candidate.file_name)?;
            directory
                .file
                .sync_all()
                .map_err(SecureError::SyncDirectory)?;
            directory.verify_identity()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
enum SecureError {
    UnsafePath,
    UnsafeLeaf,
    Missing,
    TooLarge,
    Changed,
    InvalidData(&'static str),
    NoTemporaryName,
    Open(io::Error),
    Create(io::Error),
    Read(io::Error),
    Write(io::Error),
    SyncFile(io::Error),
    SyncDirectory(io::Error),
    Rename(io::Error),
    Unlink(io::Error),
    Enumerate(io::Error),
}

impl fmt::Display for SecureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafePath => formatter.write_str("路径不安全"),
            Self::UnsafeLeaf => formatter.write_str("目录项不是当前用户拥有的 0600 单链接普通文件"),
            Self::Missing => formatter.write_str("目录项不存在"),
            Self::TooLarge => formatter.write_str("文件超过大小上限"),
            Self::Changed => formatter.write_str("文件或目录在操作期间发生变化"),
            Self::InvalidData(reason) => write!(formatter, "内容无效：{reason}"),
            Self::NoTemporaryName => formatter.write_str("无法分配唯一临时文件名"),
            Self::Open(error) => write!(formatter, "打开失败：{error}"),
            Self::Create(error) => write!(formatter, "创建失败：{error}"),
            Self::Read(error) => write!(formatter, "读取失败：{error}"),
            Self::Write(error) => write!(formatter, "写入失败：{error}"),
            Self::SyncFile(error) => write!(formatter, "同步文件失败：{error}"),
            Self::SyncDirectory(error) => write!(formatter, "同步目录失败：{error}"),
            Self::Rename(error) => write!(formatter, "原子替换失败：{error}"),
            Self::Unlink(error) => write!(formatter, "删除失败：{error}"),
            Self::Enumerate(error) => write!(formatter, "枚举失败：{error}"),
        }
    }
}

type SecureResult<T> = Result<T, SecureError>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

impl DirectoryIdentity {
    fn from_metadata(metadata: &Metadata) -> SecureResult<Self> {
        if metadata.mode() & S_IFMT != S_IFDIR {
            return Err(SecureError::UnsafePath);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn matches(&self, metadata: &Metadata) -> bool {
        metadata.mode() & S_IFMT == S_IFDIR
            && self.device == metadata.dev()
            && self.inode == metadata.ino()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    links: u64,
    mode: u32,
    owner: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn private_regular(metadata: &Metadata) -> SecureResult<Self> {
        if metadata.mode() & S_IFMT != S_IFREG
            || metadata.nlink() != 1
            || metadata.uid() != effective_user_id()
            || metadata.mode() & 0o777 != FILE_MODE
        {
            return Err(SecureError::UnsafeLeaf);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            links: metadata.nlink(),
            mode: metadata.mode(),
            owner: metadata.uid(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }

    fn same_object_and_contents(&self, other: &Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.size == other.size
            && self.links == other.links
            && self.mode == other.mode
            && self.owner == other.owner
    }
}

struct DirectoryAnchor {
    parent: File,
    leaf: CString,
    identity: DirectoryIdentity,
}

struct SecureDirectory {
    file: File,
    identity: DirectoryIdentity,
    anchors: Vec<DirectoryAnchor>,
    required_mode: u32,
    require_current_user: bool,
}

struct ExclusiveFileLock {
    file: File,
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        let _ = unsafe { flock(self.file.as_raw_fd(), LOCK_UN) };
    }
}

impl SecureDirectory {
    fn open_existing(path: &Path, required_mode: u32) -> SecureResult<Self> {
        Self::open_absolute(path, false, required_mode, false)
    }

    fn open_or_create(path: &Path, required_mode: u32) -> SecureResult<Self> {
        Self::open_absolute(path, true, required_mode, true)
    }

    fn open_absolute(
        path: &Path,
        create_missing: bool,
        required_mode: u32,
        set_final_mode: bool,
    ) -> SecureResult<Self> {
        let physical_path = physical_runtime_path(path);
        let components = absolute_components(&physical_path)?;
        let mut current = open_path(
            Path::new("/"),
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
        )
        .map_err(SecureError::Open)?;
        let mut anchors = Vec::with_capacity(components.len());

        for component in components {
            let leaf = cstring(component).ok_or(SecureError::UnsafePath)?;
            let child = match open_at(
                current.as_raw_fd(),
                &leaf,
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                None,
            ) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound && create_missing => {
                    match mkdir_at(current.as_raw_fd(), &leaf, DIRECTORY_MODE) {
                        Ok(()) => current.sync_all().map_err(SecureError::SyncDirectory)?,
                        Err(create_error)
                            if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(create_error) => return Err(SecureError::Create(create_error)),
                    }
                    open_at(
                        current.as_raw_fd(),
                        &leaf,
                        O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                        None,
                    )
                    .map_err(SecureError::Open)?
                }
                Err(error) => return Err(SecureError::Open(error)),
            };
            let metadata = child.metadata().map_err(SecureError::Open)?;
            let identity = DirectoryIdentity::from_metadata(&metadata)?;
            anchors.push(DirectoryAnchor {
                parent: current.try_clone().map_err(SecureError::Open)?,
                leaf,
                identity,
            });
            current = child;
        }

        let metadata = current.metadata().map_err(SecureError::Open)?;
        let identity = DirectoryIdentity::from_metadata(&metadata)?;
        if metadata.uid() != effective_user_id() {
            return Err(SecureError::UnsafePath);
        }
        if set_final_mode {
            current
                .set_permissions(Permissions::from_mode(required_mode))
                .map_err(SecureError::Create)?;
            current.sync_all().map_err(SecureError::SyncDirectory)?;
        }
        let metadata = current.metadata().map_err(SecureError::Open)?;
        if !identity.matches(&metadata)
            || metadata.uid() != effective_user_id()
            || metadata.mode() & 0o777 != required_mode
        {
            return Err(SecureError::UnsafePath);
        }
        let directory = Self {
            file: current,
            identity,
            anchors,
            required_mode,
            require_current_user: true,
        };
        directory.verify_identity()?;
        Ok(directory)
    }

    fn verify_identity(&self) -> SecureResult<()> {
        for anchor in &self.anchors {
            let opened = open_at(
                anchor.parent.as_raw_fd(),
                &anchor.leaf,
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                None,
            )
            .map_err(|_| SecureError::Changed)?;
            let metadata = opened.metadata().map_err(|_| SecureError::Changed)?;
            if !anchor.identity.matches(&metadata) {
                return Err(SecureError::Changed);
            }
        }
        let metadata = self.file.metadata().map_err(|_| SecureError::Changed)?;
        if !self.identity.matches(&metadata)
            || (self.require_current_user && metadata.uid() != effective_user_id())
            || metadata.mode() & 0o777 != self.required_mode
        {
            return Err(SecureError::Changed);
        }
        Ok(())
    }

    fn acquire_exclusive_lock(&self, name: &str) -> SecureResult<ExclusiveFileLock> {
        let leaf = safe_leaf(name).ok_or(SecureError::UnsafeLeaf)?;
        self.verify_identity()?;

        let (file, created) = match open_at(
            self.file.as_raw_fd(),
            &leaf,
            O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            Some(FILE_MODE),
        ) {
            Ok(file) => (file, true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (
                open_at(
                    self.file.as_raw_fd(),
                    &leaf,
                    O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
                    None,
                )
                .map_err(SecureError::Open)?,
                false,
            ),
            Err(error) => return Err(SecureError::Create(error)),
        };

        let initial = file.metadata().map_err(SecureError::Open)?;
        if initial.mode() & S_IFMT != S_IFREG
            || initial.nlink() != 1
            || initial.uid() != effective_user_id()
        {
            return Err(SecureError::UnsafeLeaf);
        }
        if created {
            file.set_permissions(Permissions::from_mode(FILE_MODE))
                .map_err(SecureError::Create)?;
            file.sync_all().map_err(SecureError::SyncFile)?;
            self.file.sync_all().map_err(SecureError::SyncDirectory)?;
        }
        let identity =
            FileIdentity::private_regular(&file.metadata().map_err(|_| SecureError::Changed)?)?;
        self.verify_leaf(name, &identity)?;
        self.verify_identity()?;

        loop {
            if unsafe { flock(file.as_raw_fd(), LOCK_EX) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(SecureError::Open(error));
            }
        }

        // A replaced lock pathname would split publishers across different
        // inodes. Fail closed after acquisition instead of proceeding under
        // an obsolete lock.
        self.verify_leaf(name, &identity)?;
        self.verify_identity()?;
        Ok(ExclusiveFileLock { file })
    }

    fn open_regular_unchecked(&self, name: &str) -> SecureResult<(File, FileIdentity)> {
        let leaf = safe_leaf(name).ok_or(SecureError::UnsafeLeaf)?;
        let file = match open_at(
            self.file.as_raw_fd(),
            &leaf,
            O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
            None,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(SecureError::Missing)
            }
            Err(_) => return Err(SecureError::UnsafeLeaf),
        };
        let identity =
            FileIdentity::private_regular(&file.metadata().map_err(|_| SecureError::Changed)?)?;
        Ok((file, identity))
    }

    fn read_regular(
        &self,
        name: &str,
        maximum_bytes: usize,
    ) -> SecureResult<(Vec<u8>, FileIdentity)> {
        self.verify_identity()?;
        let (mut file, before) = self.open_regular_unchecked(name)?;
        if before.size > maximum_bytes as u64 {
            return Err(SecureError::TooLarge);
        }
        let limit = maximum_bytes.checked_add(1).ok_or(SecureError::TooLarge)? as u64;
        let mut data = Vec::with_capacity(before.size as usize);
        (&mut file)
            .take(limit)
            .read_to_end(&mut data)
            .map_err(SecureError::Read)?;
        if data.len() > maximum_bytes || data.len() as u64 != before.size {
            return Err(SecureError::Changed);
        }
        let after =
            FileIdentity::private_regular(&file.metadata().map_err(|_| SecureError::Changed)?)?;
        if before != after {
            return Err(SecureError::Changed);
        }
        self.verify_leaf(name, &after)?;
        self.verify_identity()?;
        Ok((data, after))
    }

    fn verify_leaf(&self, name: &str, expected: &FileIdentity) -> SecureResult<()> {
        let (_, current) = self.open_regular_unchecked(name)?;
        if &current != expected {
            return Err(SecureError::Changed);
        }
        Ok(())
    }

    fn destination_status(&self, name: &str) -> SecureResult<Option<FileIdentity>> {
        self.verify_identity()?;
        match self.open_regular_unchecked(name) {
            Ok((_, identity)) => Ok(Some(identity)),
            Err(SecureError::Missing) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn atomic_write(
        &self,
        data: &[u8],
        destination_name: &str,
        maximum_bytes: usize,
        temporary_names: Option<&[String]>,
    ) -> SecureResult<FileIdentity> {
        if data.len() > maximum_bytes {
            return Err(SecureError::TooLarge);
        }
        let destination = safe_leaf(destination_name).ok_or(SecureError::UnsafeLeaf)?;
        self.verify_identity()?;
        let _ = self.destination_status(destination_name)?;

        let generated;
        let candidates = if let Some(names) = temporary_names {
            names
        } else {
            generated = (0..64)
                .map(|_| temporary_name(destination_name))
                .collect::<Vec<_>>();
            &generated
        };

        for temporary_name in candidates {
            let Some(temporary) = safe_leaf(temporary_name) else {
                continue;
            };
            let mut temporary_file = match open_at(
                self.file.as_raw_fd(),
                &temporary,
                O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC,
                Some(FILE_MODE),
            ) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(SecureError::Create(error)),
            };
            let mut renamed = false;
            let result = (|| {
                temporary_file
                    .set_permissions(Permissions::from_mode(FILE_MODE))
                    .map_err(SecureError::Create)?;
                temporary_file.write_all(data).map_err(SecureError::Write)?;
                temporary_file.sync_all().map_err(SecureError::SyncFile)?;
                let temporary_identity = FileIdentity::private_regular(
                    &temporary_file
                        .metadata()
                        .map_err(|_| SecureError::Changed)?,
                )?;
                if temporary_identity.size != data.len() as u64 {
                    return Err(SecureError::Changed);
                }
                self.verify_leaf(temporary_name, &temporary_identity)?;
                let _ = self.destination_status(destination_name)?;
                self.verify_identity()?;
                let status = unsafe {
                    renameat(
                        self.file.as_raw_fd(),
                        temporary.as_ptr(),
                        self.file.as_raw_fd(),
                        destination.as_ptr(),
                    )
                };
                if status != 0 {
                    return Err(SecureError::Rename(io::Error::last_os_error()));
                }
                renamed = true;
                self.file.sync_all().map_err(SecureError::SyncDirectory)?;
                let (_, final_identity) = self.open_regular_unchecked(destination_name)?;
                if !temporary_identity.same_object_and_contents(&final_identity) {
                    return Err(SecureError::Changed);
                }
                self.verify_identity()?;
                Ok(final_identity)
            })();
            if !renamed {
                let _ = self.unlink_exact(temporary_name, &temporary_file);
            }
            return result;
        }
        Err(SecureError::NoTemporaryName)
    }

    fn unlink_exact(&self, name: &str, opened: &File) -> SecureResult<()> {
        let expected =
            FileIdentity::private_regular(&opened.metadata().map_err(|_| SecureError::Changed)?)?;
        self.verify_leaf(name, &expected)?;
        self.verify_identity()?;
        unlink_name(self.file.as_raw_fd(), name)?;
        self.file.sync_all().map_err(SecureError::SyncDirectory)
    }

    fn entry_names(&self) -> SecureResult<Vec<String>> {
        self.verify_identity()?;
        // A cloned descriptor shares its directory stream offset. Re-open `.`
        // so repeated conflict checks and retention scans each start at the
        // beginning of this already verified directory inode.
        let dot = CString::new(".").expect("static directory leaf");
        let duplicate = open_at(
            self.file.as_raw_fd(),
            &dot,
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            None,
        )
        .map_err(SecureError::Enumerate)?;
        if !self
            .identity
            .matches(&duplicate.metadata().map_err(SecureError::Enumerate)?)
        {
            return Err(SecureError::Changed);
        }
        let duplicate_descriptor = duplicate.into_raw_fd();
        let stream = unsafe { fdopendir(duplicate_descriptor) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            unsafe { close(duplicate_descriptor) };
            return Err(SecureError::Enumerate(error));
        }
        let mut names = Vec::new();
        loop {
            set_errno(0);
            let entry = unsafe { readdir(stream) };
            if entry.is_null() {
                let error = errno_value();
                let close_status = unsafe { closedir(stream) };
                if error != 0 {
                    return Err(SecureError::Enumerate(io::Error::from_raw_os_error(error)));
                }
                if close_status != 0 {
                    return Err(SecureError::Enumerate(io::Error::last_os_error()));
                }
                break;
            }
            let name_length = unsafe { (*entry).d_namlen as usize };
            if name_length >= 1024 {
                unsafe { closedir(stream) };
                return Err(SecureError::Enumerate(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid directory entry length",
                )));
            }
            let bytes = unsafe {
                std::slice::from_raw_parts((*entry).d_name.as_ptr().cast::<u8>(), name_length)
            };
            if let Ok(name) = std::str::from_utf8(bytes) {
                if safe_leaf(name).is_some() {
                    names.push(name.to_string());
                    if names.len() > SNAPSHOT_MAXIMUM_ENTRIES {
                        unsafe { closedir(stream) };
                        return Err(SecureError::TooLarge);
                    }
                }
            }
        }
        self.verify_identity()?;
        Ok(names)
    }
}

#[derive(Debug)]
struct SnapshotPointer {
    version: u64,
    checksum: String,
    file_name: String,
    entry_count: usize,
}

#[derive(Debug)]
struct ParsedSnapshot {
    version: u64,
    schema_version: u64,
    entry_count: usize,
}

#[derive(Debug)]
struct SnapshotFileName {
    version: u64,
    checksum: String,
    file_name: String,
}

impl SnapshotFileName {
    fn parse(file_name: &str) -> Option<Self> {
        safe_leaf(file_name)?;
        let body = file_name
            .strip_prefix("snapshot-")?
            .strip_suffix(".aimsnap")?;
        let (version_text, checksum) = body.split_once('-')?;
        let version = canonical_u64(version_text)?;
        if !is_lower_hex_checksum(checksum)
            || file_name != format!("snapshot-{version}-{checksum}.aimsnap")
        {
            return None;
        }
        Some(Self {
            version,
            checksum: checksum.to_string(),
            file_name: file_name.to_string(),
        })
    }
}

fn parse_pointer(bytes: &[u8]) -> Result<SnapshotPointer, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "指针不是 UTF-8")?;
    let lines = exact_lines(text, 4)?;
    if lines.len() != 4 {
        return Err("指针必须恰好包含四个字段");
    }
    let mut version = None;
    let mut checksum = None;
    let mut file_name = None;
    let mut entry_count = None;
    for line in lines {
        let (key, value) = line.split_once('=').ok_or("指针字段格式无效")?;
        match key {
            "version" if version.is_none() => version = canonical_u64(value),
            "checksum" if checksum.is_none() && is_lower_hex_checksum(value) => {
                checksum = Some(value.to_string())
            }
            "file" if file_name.is_none() => file_name = Some(value.to_string()),
            "entry_count" if entry_count.is_none() => {
                entry_count = canonical_usize(value, SNAPSHOT_MAXIMUM_ENTRIES)
            }
            _ => return Err("指针包含重复、未知或无效字段"),
        }
    }
    let version = version.ok_or("版本无效")?;
    let checksum = checksum.ok_or("checksum 无效")?;
    let file_name = file_name.ok_or("文件名无效")?;
    let entry_count = entry_count.ok_or("条目数无效")?;
    let parsed_name = SnapshotFileName::parse(&file_name).ok_or("文件名不安全")?;
    if parsed_name.version != version || parsed_name.checksum != checksum {
        return Err("文件名与指针元数据不一致");
    }
    Ok(SnapshotPointer {
        version,
        checksum,
        file_name,
        entry_count,
    })
}

fn validate_snapshot_bytes(
    bytes: &[u8],
    expected_checksum: Option<&str>,
    expected_version: Option<u64>,
    expected_count: Option<usize>,
) -> Result<ParsedSnapshot, &'static str> {
    if bytes.len() > SNAPSHOT_MAXIMUM_BYTES {
        return Err("快照超过大小上限");
    }
    let checksum = sha256(bytes);
    if expected_checksum.is_some_and(|expected| expected != checksum) {
        return Err("checksum 不匹配");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "快照不是 UTF-8")?;
    let maximum_lines = if text.starts_with(FORMAT_V2_HEADER) {
        SNAPSHOT_MAXIMUM_ENTRIES + 4
    } else {
        SNAPSHOT_MAXIMUM_ENTRIES + 3
    };
    let lines = exact_lines(text, maximum_lines)?;
    let (schema_version, version_line, count_line, entries_start) = match lines.first().copied() {
        Some(FORMAT_V1_HEADER) if lines.len() >= 3 => (1, lines[1], lines[2], 3),
        Some(FORMAT_V2_HEADER) if lines.len() >= 4 && lines[1] == "schema_version=2" => {
            (2, lines[2], lines[3], 4)
        }
        _ => return Err("格式头或 schema 版本无效"),
    };
    let version = canonical_u64(
        version_line
            .strip_prefix("version=")
            .ok_or("嵌入版本缺失")?,
    )
    .ok_or("嵌入版本无效")?;
    let entry_count = canonical_usize(
        count_line
            .strip_prefix("entry_count=")
            .ok_or("嵌入条目数缺失")?,
        SNAPSHOT_MAXIMUM_ENTRIES,
    )
    .ok_or("嵌入条目数无效")?;
    let entries_valid = if schema_version == 1 {
        lines[entries_start..]
            .iter()
            .all(|line| valid_snapshot_entry(line))
    } else {
        valid_lexicon_snapshot_entries(&lines[entries_start..])
    };
    if expected_version.is_some_and(|expected| expected != version)
        || expected_count.is_some_and(|expected| expected != entry_count)
        || lines.len() != entry_count + entries_start
        || !entries_valid
    {
        return Err("快照元数据或条目无效");
    }
    Ok(ParsedSnapshot {
        version,
        schema_version,
        entry_count,
    })
}

fn write_pointer(
    directory: &SecureDirectory,
    version: u64,
    checksum: &str,
    file_name: &str,
    entry_count: usize,
) -> Result<(), String> {
    let pointer = format!(
        "version={version}\nchecksum={checksum}\nfile={file_name}\nentry_count={entry_count}\n"
    );
    parse_pointer(pointer.as_bytes()).map_err(|error| format!("快照指针无效：{error}"))?;
    directory
        .atomic_write(pointer.as_bytes(), "current", POINTER_MAXIMUM_BYTES, None)
        .map_err(|error| format!("无法原子更新快照指针：{error}"))?;
    Ok(())
}

fn normalize_lexicon_entry(entry: &mut LexiconSnapshotEntry) -> Result<(), String> {
    for (label, value) in [
        ("id", entry.id.as_str()),
        ("text", entry.text.as_str()),
        ("pinyin", entry.pinyin.as_str()),
    ] {
        if value.trim().is_empty() || value != value.trim() {
            return Err(format!("个人词快照字段 {label} 为空或未规范化。"));
        }
    }
    if entry.updated_at_ms < 0 {
        return Err("个人词快照更新时间不能为负数。".into());
    }
    sort_unique_nonempty("aliases", &mut entry.aliases)?;
    sort_unique_nonempty("professional_packs", &mut entry.professional_packs)?;
    sort_unique_nonempty("project_domains", &mut entry.project_domains)?;
    if entry.sources.is_empty() {
        return Err("个人词快照必须保留至少一个来源。".into());
    }
    for source in &entry.sources {
        for (label, value) in [
            ("source_kind", source.source_kind.as_str()),
            ("source", source.source.as_str()),
            ("license", source.license.as_str()),
        ] {
            if value.trim().is_empty() || value != value.trim() {
                return Err(format!("个人词快照字段 {label} 为空或未规范化。"));
            }
        }
        if !(0..=1000).contains(&source.confidence_millis) {
            return Err("个人词快照来源置信度超出范围。".into());
        }
    }
    entry.sources.sort();
    if entry.sources.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("个人词快照包含重复来源。".into());
    }
    Ok(())
}

fn sort_unique_nonempty(label: &str, values: &mut [String]) -> Result<(), String> {
    if values
        .iter()
        .any(|value| value.trim().is_empty() || value != value.trim())
    {
        return Err(format!("个人词快照字段 {label} 含空值或未规范化值。"));
    }
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!("个人词快照字段 {label} 含重复值。"));
    }
    Ok(())
}

fn valid_lexicon_snapshot_entries(lines: &[&str]) -> bool {
    let mut previous_key: Option<(String, String, String)> = None;
    let mut ids = HashSet::new();
    for line in lines {
        let Ok(entry) = serde_json::from_str::<LexiconSnapshotEntry>(line) else {
            return false;
        };
        if serde_json::to_string(&entry).ok().as_deref() != Some(*line)
            || !is_canonical_lexicon_entry(&entry)
            || !ids.insert(entry.id.clone())
        {
            return false;
        }
        let key = (entry.text.clone(), entry.pinyin.clone(), entry.id.clone());
        if previous_key.as_ref().is_some_and(|previous| {
            previous >= &key || (previous.0 == key.0 && previous.1 == key.1)
        }) {
            return false;
        }
        previous_key = Some(key);
    }
    true
}

fn is_canonical_lexicon_entry(entry: &LexiconSnapshotEntry) -> bool {
    let required = [&entry.id, &entry.text, &entry.pinyin];
    required
        .iter()
        .all(|value| !value.trim().is_empty() && value.as_str() == value.trim())
        && entry.updated_at_ms >= 0
        && matches!(entry.candidate_layer.as_str(), "first" | "secondary")
        && canonical_string_list(&entry.aliases)
        && canonical_string_list(&entry.professional_packs)
        && canonical_string_list(&entry.project_domains)
        && !entry.sources.is_empty()
        && entry.sources.windows(2).all(|pair| pair[0] < pair[1])
        && entry.sources.iter().all(|source| {
            [&source.source_kind, &source.source, &source.license]
                .iter()
                .all(|value| !value.trim().is_empty() && value.as_str() == value.trim())
                && (0..=1000).contains(&source.confidence_millis)
        })
}

fn canonical_string_list(values: &[String]) -> bool {
    values
        .iter()
        .all(|value| !value.trim().is_empty() && value == value.trim())
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn exact_lines(text: &str, maximum_lines: usize) -> Result<Vec<&str>, &'static str> {
    let mut lines = Vec::with_capacity(maximum_lines.min(64));
    let mut pieces = text.split('\n').peekable();
    while let Some(line) = pieces.next() {
        if line.is_empty() {
            if pieces.peek().is_none() && !lines.is_empty() {
                break;
            }
            return Err("包含空行或多余换行");
        }
        if lines.len() == maximum_lines {
            return Err("行数超过上限");
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return Err("包含空行或多余换行");
    }
    Ok(lines)
}

fn valid_snapshot_entry(line: &str) -> bool {
    let mut fields = line.split('\t');
    let Some(code) = fields.next() else {
        return false;
    };
    let Some(text) = fields.next() else {
        return false;
    };
    let Some(source) = fields.next() else {
        return false;
    };
    let Some(weight) = fields.next() else {
        return false;
    };
    fields.next().is_none()
        && !code.is_empty()
        && !text.is_empty()
        && !source.is_empty()
        && weight
            .parse::<i64>()
            .ok()
            .is_some_and(|parsed| parsed.to_string() == weight)
}

fn canonical_u64(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
}

fn canonical_usize(value: &str, maximum: usize) -> Option<usize> {
    value
        .parse::<usize>()
        .ok()
        .filter(|parsed| *parsed <= maximum && parsed.to_string() == value)
}

fn is_lower_hex_checksum(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn absolute_components(path: &Path) -> SecureResult<Vec<&OsStr>> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(SecureError::UnsafePath);
    }
    let mut result = Vec::new();
    for component in components {
        match component {
            Component::Normal(value) if safe_os_leaf(value) => result.push(value),
            _ => return Err(SecureError::UnsafePath),
        }
    }
    if result.is_empty() {
        return Err(SecureError::UnsafePath);
    }
    Ok(result)
}

/// macOS defines `/tmp` and `/var` as root-owned compatibility links. Resolve
/// only those platform aliases before the component-wise no-follow walk;
/// arbitrary symlink components remain rejected.
fn physical_runtime_path(path: &Path) -> PathBuf {
    for (alias, physical) in [("/tmp", "/private/tmp"), ("/var", "/private/var")] {
        if path == Path::new(alias) {
            return PathBuf::from(physical);
        }
        if let Ok(remainder) = path.strip_prefix(alias) {
            return PathBuf::from(physical).join(remainder);
        }
    }
    path.to_path_buf()
}

fn safe_os_leaf(value: &OsStr) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes != b"."
        && bytes != b".."
        && !bytes.contains(&b'/')
        && !bytes.contains(&0)
}

fn cstring(value: &OsStr) -> Option<CString> {
    safe_os_leaf(value)
        .then(|| CString::new(value.as_bytes()).ok())
        .flatten()
}

fn safe_leaf(value: &str) -> Option<CString> {
    cstring(OsStr::new(value))
}

fn temporary_name(destination: &str) -> String {
    let counter = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        ".{destination}.tmp-{}-{nanos}-{counter}",
        std::process::id()
    )
}

fn open_path(path: &Path, flags: c_int) -> io::Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    let descriptor = unsafe { open(path.as_ptr(), flags) };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn open_at(directory: RawFd, name: &CStr, flags: c_int, mode: Option<u32>) -> io::Result<File> {
    let descriptor = match mode {
        Some(mode) => unsafe { openat(directory, name.as_ptr(), flags, mode as c_uint) },
        None => unsafe { openat(directory, name.as_ptr(), flags) },
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn mkdir_at(directory: RawFd, name: &CStr, mode: u32) -> io::Result<()> {
    if unsafe { mkdirat(directory, name.as_ptr(), mode as u16) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn unlink_name(directory: RawFd, name: &str) -> SecureResult<()> {
    let leaf = safe_leaf(name).ok_or(SecureError::UnsafeLeaf)?;
    if unsafe { unlinkat(directory, leaf.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(SecureError::Unlink(io::Error::last_os_error()))
    }
}

fn effective_user_id() -> u32 {
    unsafe { geteuid() }
}

fn errno_value() -> c_int {
    unsafe { *__error() }
}

fn set_errno(value: c_int) {
    unsafe { *__error() = value }
}

#[repr(C, packed(4))]
struct DarwinDirent {
    d_ino: u64,
    d_seekoff: u64,
    d_reclen: u16,
    d_namlen: u16,
    d_type: u8,
    d_name: [c_char; 1024],
}

#[repr(C)]
struct DarwinDir {
    _private: [u8; 0],
}

extern "C" {
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn openat(directory: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn mkdirat(directory: c_int, path: *const c_char, mode: u16) -> c_int;
    fn renameat(
        old_directory: c_int,
        old_path: *const c_char,
        new_directory: c_int,
        new_path: *const c_char,
    ) -> c_int;
    fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    fn flock(descriptor: c_int, operation: c_int) -> c_int;
    fn geteuid() -> c_uint;
    fn __error() -> *mut c_int;
    fn fdopendir(descriptor: c_int) -> *mut DarwinDir;
    fn readdir(directory: *mut DarwinDir) -> *mut DarwinDirent;
    fn closedir(directory: *mut DarwinDir) -> c_int;
    fn close(descriptor: c_int) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, MetadataExt};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    struct TestStore {
        parent: PathBuf,
        root: PathBuf,
        store: SnapshotStore,
    }

    impl TestStore {
        fn new(name: &str) -> Self {
            let parent = PathBuf::from("/private/tmp").join(format!(
                "agent-ime-snapshot-{name}-{}-{}",
                std::process::id(),
                TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let root = parent.join("Snapshots");
            let store = SnapshotStore::new(root.clone());
            Self {
                parent,
                root,
                store,
            }
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.parent);
        }
    }

    fn fixture_bytes(version: u64) -> (Vec<u8>, String, String) {
        let body = format!(
            "{FORMAT_V1_HEADER}\nversion={version}\nentry_count=2\nagentfixture\t联合快照夹具\tfixture\t100\noffline\t离线\tfixture\t90\n"
        )
        .into_bytes();
        let checksum = sha256(&body);
        let name = format!("snapshot-{version}-{checksum}.aimsnap");
        (body, checksum, name)
    }

    fn private_write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, Permissions::from_mode(FILE_MODE)).unwrap();
    }

    fn lexicon_entry(id: &str, text: &str, pinyin: &str) -> LexiconSnapshotEntry {
        LexiconSnapshotEntry {
            id: id.into(),
            text: text.into(),
            pinyin: pinyin.into(),
            aliases: vec![format!("{text}-alias"), format!("{text}-second")],
            professional_packs: vec!["software-engineering".into()],
            project_domains: vec!["agent-input-method".into()],
            sources: vec![LexiconSnapshotSource {
                source_kind: "manual".into(),
                source: "owner-entry".into(),
                license: "private-user-data".into(),
                confidence_millis: 1000,
            }],
            base_weight: 100,
            fixed: true,
            disabled: false,
            demoted: false,
            candidate_layer: "first".into(),
            updated_at_ms: 1_752_624_000_000,
        }
    }

    #[test]
    fn atomic_publish_load_and_retention() {
        let fixture = TestStore::new("publish");
        for version in 1..=3 {
            fixture
                .store
                .publish_fixture(version)
                .expect("publish fixture");
        }
        let current = fixture.store.load_current().expect("load current");
        assert_eq!(current.version, 3);
        let snapshots = fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| SnapshotFileName::parse(&entry.file_name().to_string_lossy()).is_some())
            .collect::<Vec<_>>();
        assert_eq!(snapshots.len(), 2);
        let root_metadata = fs::metadata(&fixture.root).unwrap();
        assert_eq!(root_metadata.mode() & 0o777, DIRECTORY_MODE);
        assert_eq!(root_metadata.uid(), effective_user_id());
        for entry in snapshots {
            let metadata = entry.metadata().unwrap();
            assert_eq!(metadata.mode() & 0o777, FILE_MODE);
            assert_eq!(metadata.uid(), effective_user_id());
            assert_eq!(metadata.nlink(), 1);
        }
    }

    #[test]
    fn lexicon_snapshot_build_is_deterministic_for_equivalent_input() {
        let first = TestStore::new("lexicon-deterministic-a");
        let second = TestStore::new("lexicon-deterministic-b");
        let mut alpha = lexicon_entry("entry-a", "原子加载", "yuan zi jia zai");
        alpha.aliases.reverse();
        let beta = lexicon_entry("entry-b", "快照编译", "kuai zhao bian yi");

        let first_meta = first
            .store
            .publish_lexicon(7, vec![beta.clone(), alpha.clone()])
            .unwrap();
        let second_meta = second.store.publish_lexicon(7, vec![alpha, beta]).unwrap();

        assert_eq!(first_meta.schema_version, 2);
        assert_eq!(first_meta.checksum, second_meta.checksum);
        assert_eq!(
            fs::read(first.root.join(first_meta.file_name)).unwrap(),
            fs::read(second.root.join(second_meta.file_name)).unwrap()
        );
    }

    #[test]
    fn invalid_lexicon_schema_never_replaces_current() {
        let fixture = TestStore::new("lexicon-invalid-schema");
        let accepted = fixture
            .store
            .publish_lexicon(
                1,
                vec![lexicon_entry("entry-a", "有效快照", "you xiao kuai zhao")],
            )
            .unwrap();
        let invalid = b"AIMSNAP2\nschema_version=3\nversion=2\nentry_count=0\n";
        assert!(fixture.store.publish_bytes(2, invalid).is_err());

        let current = fixture.store.load_current().unwrap();
        assert_eq!(current.version, 1);
        assert_eq!(current.checksum, accepted.checksum);
    }

    #[test]
    fn lexicon_snapshot_retains_two_versions_and_rolls_back_atomically() {
        let fixture = TestStore::new("lexicon-rollback");
        let first = fixture
            .store
            .publish_lexicon(40, vec![lexicon_entry("entry-a", "第一版", "di yi ban")])
            .unwrap();
        let second = fixture
            .store
            .publish_lexicon(41, vec![lexicon_entry("entry-b", "第二版", "di er ban")])
            .unwrap();
        assert_eq!(fixture.store.load_current().unwrap().version, 41);
        let snapshot_count = fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| SnapshotFileName::parse(&entry.file_name().to_string_lossy()).is_some())
            .count();
        assert_eq!(snapshot_count, 2);

        let rolled_back = fixture.store.rollback_to(40).unwrap();
        assert_eq!(rolled_back.checksum, first.checksum);
        assert_eq!(fixture.store.load_current().unwrap().version, 40);
        assert!(fixture.root.join(second.file_name).is_file());
        assert!(fixture.store.rollback_to(39).is_err());
        assert_eq!(fixture.store.load_current().unwrap().version, 40);
    }

    #[test]
    fn lexicon_schema_rejects_missing_license_and_noncanonical_json() {
        let mut entry = lexicon_entry("entry-a", "许可证", "xu ke zheng");
        entry.candidate_layer = "secondary".into();
        let canonical = serde_json::to_string(&entry).unwrap();
        let body = format!("AIMSNAP2\nschema_version=2\nversion=1\nentry_count=1\n{canonical}\n");
        assert!(validate_snapshot_bytes(body.as_bytes(), None, Some(1), Some(1)).is_ok());

        let mut missing_license = entry.clone();
        missing_license.sources[0].license.clear();
        let line = serde_json::to_string(&missing_license).unwrap();
        let body = format!("AIMSNAP2\nschema_version=2\nversion=2\nentry_count=1\n{line}\n");
        assert!(validate_snapshot_bytes(body.as_bytes(), None, Some(2), Some(1)).is_err());

        let noncanonical = canonical.replace("\":", "\": ");
        let body =
            format!("AIMSNAP2\nschema_version=2\nversion=3\nentry_count=1\n{noncanonical}\n");
        assert!(validate_snapshot_bytes(body.as_bytes(), None, Some(3), Some(1)).is_err());

        let mut invalid_layer = entry;
        invalid_layer.candidate_layer = "numbered".into();
        let line = serde_json::to_string(&invalid_layer).unwrap();
        let body = format!("AIMSNAP2\nschema_version=2\nversion=4\nentry_count=1\n{line}\n");
        assert!(validate_snapshot_bytes(body.as_bytes(), None, Some(4), Some(1)).is_err());
    }

    #[test]
    fn platform_tmp_alias_is_supported_without_allowing_arbitrary_aliases() {
        let fixture = TestStore::new("tmp-alias");
        let alias_root = Path::new("/tmp").join(
            fixture
                .root
                .strip_prefix("/private/tmp")
                .expect("test root is physically under /private/tmp"),
        );
        let alias_store = SnapshotStore::new(alias_root);
        alias_store.publish_fixture(11).unwrap();
        assert_eq!(alias_store.load_current().unwrap().version, 11);
        assert_eq!(
            physical_runtime_path(Path::new("/various/snapshots")),
            Path::new("/various/snapshots")
        );
    }

    #[test]
    fn pointer_requires_exact_unique_fields_and_safe_matching_name() {
        let fixture = TestStore::new("pointer-strict");
        let accepted = fixture.store.publish_fixture(7).unwrap();
        let current = fixture.root.join("current");
        let (_, checksum, name) = fixture_bytes(7);
        let invalid = [
            format!("version=7\nversion=7\nchecksum={checksum}\nfile={name}\nentry_count=2\n"),
            format!("version=7\nchecksum={checksum}\nfile=../sentinel\nentry_count=2\n"),
            format!("version=07\nchecksum={checksum}\nfile={name}\nentry_count=2\n"),
            format!("version=7\nchecksum={checksum}\nfile={name}\nentry_count=2\n\n"),
        ];
        for pointer in invalid {
            private_write(&current, pointer.as_bytes());
            assert!(fixture.store.load_current().is_err());
        }
        assert_eq!(accepted.version, 7);
    }

    #[test]
    fn rejects_oversized_pointer_and_invalid_snapshot_entry() {
        let fixture = TestStore::new("oversize-entry");
        fixture.store.publish_fixture(1).unwrap();
        private_write(
            &fixture.root.join("current"),
            &vec![b'x'; POINTER_MAXIMUM_BYTES + 1],
        );
        assert!(fixture.store.load_current().is_err());

        let body = b"AIMSNAP1\nversion=2\nentry_count=1\na\tb\tc\tnot-a-weight\n";
        let checksum = sha256(body);
        assert!(validate_snapshot_bytes(body, Some(&checksum), Some(2), Some(1)).is_err());
    }

    #[test]
    fn rejects_line_dense_snapshot_before_unbounded_allocation() {
        let mut body = b"AIMSNAP1\nversion=1\nentry_count=100000\n".to_vec();
        body.extend(std::iter::repeat_n(b"a\n".as_slice(), SNAPSHOT_MAXIMUM_ENTRIES + 1).flatten());
        assert!(body.len() < SNAPSHOT_MAXIMUM_BYTES);
        assert_eq!(
            validate_snapshot_bytes(&body, None, Some(1), Some(SNAPSHOT_MAXIMUM_ENTRIES))
                .unwrap_err(),
            "行数超过上限"
        );

        let tab_dense = format!(
            "AIMSNAP1\nversion=2\nentry_count=1\n{}\n",
            "x\t".repeat(100_000)
        );
        assert!(validate_snapshot_bytes(tab_dense.as_bytes(), None, Some(2), Some(1)).is_err());
    }

    #[test]
    fn concurrent_publish_child() {
        let Some(root) = std::env::var_os("AIM_SNAPSHOT_CONCURRENT_CHILD_ROOT") else {
            return;
        };
        let version = std::env::var("AIM_SNAPSHOT_CONCURRENT_CHILD_VERSION")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let gate = PathBuf::from(std::env::var_os("AIM_SNAPSHOT_CONCURRENT_CHILD_GATE").unwrap());
        for _ in 0..10_000 {
            if gate.exists() {
                SnapshotStore::new(PathBuf::from(root))
                    .publish_fixture(version)
                    .unwrap();
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("concurrent publish gate was never released");
    }

    #[test]
    fn concurrent_publishers_never_prune_the_durable_current_target() {
        let fixture = TestStore::new("concurrent-publish");
        fixture.store.publish_fixture(1).unwrap();
        let test_binary = std::env::current_exe().unwrap();

        for round in 0..3_u64 {
            let gate = fixture.parent.join(format!("publish-gate-{round}"));
            let mut children = (0..12_u64)
                .map(|index| {
                    Command::new(&test_binary)
                        .arg("--exact")
                        .arg("snapshot::tests::concurrent_publish_child")
                        .arg("--nocapture")
                        .env("AIM_SNAPSHOT_CONCURRENT_CHILD_ROOT", &fixture.root)
                        .env(
                            "AIM_SNAPSHOT_CONCURRENT_CHILD_VERSION",
                            (100 + round * 12 + index).to_string(),
                        )
                        .env("AIM_SNAPSHOT_CONCURRENT_CHILD_GATE", &gate)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            private_write(&gate, b"go");
            for child in children.drain(..) {
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "concurrent publisher failed: stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }

            let current = fixture.store.load_current().unwrap();
            assert!(fixture.root.join(&current.file_name).is_file());
            let snapshots = fs::read_dir(&fixture.root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    SnapshotFileName::parse(&entry.file_name().to_string_lossy()).is_some()
                })
                .count();
            assert!(snapshots <= 2, "retention left {snapshots} snapshots");
        }
    }

    #[test]
    fn current_symlink_and_hardlink_never_touch_sentinel() {
        for hard_link in [false, true] {
            let fixture = TestStore::new(if hard_link {
                "current-hard"
            } else {
                "current-sym"
            });
            fixture.store.publish_fixture(1).unwrap();
            let sentinel = fixture.parent.join("sentinel");
            private_write(&sentinel, b"external-sentinel");
            fs::remove_file(fixture.root.join("current")).unwrap();
            if hard_link {
                fs::hard_link(&sentinel, fixture.root.join("current")).unwrap();
            } else {
                symlink(&sentinel, fixture.root.join("current")).unwrap();
            }
            assert!(fixture.store.publish_fixture(2).is_err());
            assert_eq!(fs::read(&sentinel).unwrap(), b"external-sentinel");
            assert_eq!(fs::metadata(&sentinel).unwrap().mode() & 0o777, FILE_MODE);
        }
    }

    #[test]
    fn destination_and_temporary_links_never_touch_sentinel() {
        for hard_link in [false, true] {
            let fixture = TestStore::new(if hard_link {
                "destination-hard"
            } else {
                "destination-sym"
            });
            fixture.store.publish_fixture(1).unwrap();
            let sentinel = fixture.parent.join("sentinel");
            private_write(&sentinel, b"destination-sentinel");
            let (_, _, destination) = fixture_bytes(2);
            if hard_link {
                fs::hard_link(&sentinel, fixture.root.join(&destination)).unwrap();
            } else {
                symlink(&sentinel, fixture.root.join(&destination)).unwrap();
            }
            assert!(fixture.store.publish_fixture(2).is_err());
            assert_eq!(fs::read(&sentinel).unwrap(), b"destination-sentinel");

            let directory = SecureDirectory::open_existing(&fixture.root, DIRECTORY_MODE).unwrap();
            let temp_name = ".fixed-temp".to_string();
            let temp_path = fixture.root.join(&temp_name);
            if hard_link {
                if temp_path.exists() {
                    fs::remove_file(&temp_path).unwrap();
                }
                fs::hard_link(&sentinel, &temp_path).unwrap();
            } else {
                symlink(&sentinel, &temp_path).unwrap();
            }
            assert!(directory
                .atomic_write(b"safe", "other", 64, Some(&[temp_name]))
                .is_err());
            assert_eq!(fs::read(&sentinel).unwrap(), b"destination-sentinel");
        }
    }

    #[test]
    fn root_and_ancestor_symlinks_are_rejected() {
        let fixture = TestStore::new("ancestor-link");
        fs::create_dir_all(&fixture.parent).unwrap();
        fs::set_permissions(&fixture.parent, Permissions::from_mode(DIRECTORY_MODE)).unwrap();
        let external = fixture.parent.join("external");
        fs::create_dir(&external).unwrap();
        fs::set_permissions(&external, Permissions::from_mode(DIRECTORY_MODE)).unwrap();
        let sentinel = external.join("sentinel");
        private_write(&sentinel, b"ancestor-sentinel");
        let linked = fixture.parent.join("linked");
        symlink(&external, &linked).unwrap();
        let store = SnapshotStore::new(linked.join("nested/Snapshots"));
        assert!(store.publish_fixture(1).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"ancestor-sentinel");
        assert!(!external.join("nested").exists());

        symlink(&external, &fixture.root).unwrap();
        assert!(fixture.store.publish_fixture(1).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"ancestor-sentinel");
    }

    #[test]
    fn path_replacement_invalidates_open_directory() {
        let fixture = TestStore::new("path-swap");
        fixture.store.publish_fixture(1).unwrap();
        let directory = SecureDirectory::open_existing(&fixture.root, DIRECTORY_MODE).unwrap();
        let moved = fixture.parent.join("moved-Snapshots");
        fs::rename(&fixture.root, &moved).unwrap();
        let external = fixture.parent.join("external");
        fs::create_dir(&external).unwrap();
        fs::set_permissions(&external, Permissions::from_mode(DIRECTORY_MODE)).unwrap();
        let sentinel = external.join("sentinel");
        private_write(&sentinel, b"replacement-sentinel");
        symlink(&external, &fixture.root).unwrap();
        assert!(directory
            .atomic_write(b"safe", "current", 64, None)
            .is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"replacement-sentinel");
        assert!(!external.join("current").exists());
    }

    #[test]
    fn prune_skips_strict_named_symlink_and_hardlink() {
        for hard_link in [false, true] {
            let fixture = TestStore::new(if hard_link { "prune-hard" } else { "prune-sym" });
            fixture.store.publish_fixture(1).unwrap();
            let sentinel = fixture.parent.join("sentinel");
            private_write(&sentinel, b"prune-sentinel");
            let foreign_name = format!("snapshot-999-{}.aimsnap", "a".repeat(64));
            let foreign = fixture.root.join(&foreign_name);
            if hard_link {
                fs::hard_link(&sentinel, &foreign).unwrap();
            } else {
                symlink(&sentinel, &foreign).unwrap();
            }
            let corrupt_name = format!("snapshot-998-{}.aimsnap", "b".repeat(64));
            let corrupt = fixture.root.join(&corrupt_name);
            private_write(&corrupt, b"not-a-valid-owned-snapshot");
            fixture.store.publish_fixture(2).unwrap();
            fixture.store.publish_fixture(3).unwrap();
            assert!(fs::symlink_metadata(&foreign).is_ok());
            assert_eq!(fs::read(&corrupt).unwrap(), b"not-a-valid-owned-snapshot");
            assert_eq!(fs::read(&sentinel).unwrap(), b"prune-sentinel");
        }
    }

    #[test]
    fn oversized_snapshot_is_rejected_before_reading_body() {
        let fixture = TestStore::new("snapshot-oversize");
        fixture.store.publish_fixture(1).unwrap();
        let bytes = vec![b'x'; SNAPSHOT_MAXIMUM_BYTES + 1];
        let checksum = sha256(&bytes);
        let name = format!("snapshot-2-{checksum}.aimsnap");
        private_write(&fixture.root.join(&name), &bytes);
        let pointer = format!("version=2\nchecksum={checksum}\nfile={name}\nentry_count=1\n");
        private_write(&fixture.root.join("current"), pointer.as_bytes());
        assert!(fixture.store.load_current().is_err());
    }
}
