use std::collections::HashSet;
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::raw::{c_short, c_void};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: &str = "AIMIPC/1";
pub const MAX_PAYLOAD: usize = 4096;
pub const MAX_PENDING_REQUESTS: usize = 256;
const BACKEND_UNAVAILABLE: &str = "backend-unavailable";

const O_WRONLY: i32 = 0x0001;
const O_RDWR: i32 = 0x0002;
const O_NONBLOCK: i32 = 0x0004;
const O_CREAT: i32 = 0x0200;
const O_EXCL: i32 = 0x0800;
const O_NOFOLLOW: i32 = 0x0100;
const O_DIRECTORY: i32 = 0x0010_0000;
const O_CLOEXEC: i32 = 0x0100_0000;
const O_NOFOLLOW_ANY: i32 = 0x2000_0000;
const AT_SYMLINK_NOFOLLOW: i32 = 0x0020;
const S_IFMT: u16 = 0o170000;
const S_IFSOCK: u16 = 0o140000;
const S_IFREG: u16 = 0o100000;
const LOCK_EX: i32 = 0x02;
const LOCK_NB: i32 = 0x04;
const AF_UNIX: i32 = 1;
const SOCK_STREAM: i32 = 1;
const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const FD_CLOEXEC: i32 = 1;
const SOL_SOCKET: i32 = 0xffff;
const SO_ERROR: i32 = 0x1007;
const POLLIN: c_short = 0x0001;
const POLLOUT: c_short = 0x0004;
const DARWIN_EINPROGRESS: i32 = 36;
const INSTANCE_LOCK_NAME: &str = ".agent-ime-ipc-instance.lock";

#[repr(C)]
#[derive(Clone, Copy)]
struct DarwinTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DarwinStat {
    st_dev: i32,
    st_mode: u16,
    st_nlink: u16,
    st_ino: u64,
    st_uid: u32,
    st_gid: u32,
    st_rdev: i32,
    st_atimespec: DarwinTimespec,
    st_mtimespec: DarwinTimespec,
    st_ctimespec: DarwinTimespec,
    st_birthtimespec: DarwinTimespec,
    st_size: i64,
    st_blocks: i64,
    st_blksize: i32,
    st_flags: u32,
    st_gen: u32,
    st_lspare: i32,
    st_qspare: [i64; 2],
}

#[repr(C)]
struct DarwinSockAddrUn {
    sun_len: u8,
    sun_family: u8,
    sun_path: [i8; 104],
}

#[repr(C)]
struct DarwinPollFd {
    fd: i32,
    events: c_short,
    revents: c_short,
}

unsafe extern "C" {
    fn getuid() -> u32;
    fn getpeereid(socket: i32, euid: *mut u32, egid: *mut u32) -> i32;
    fn openat(directory: i32, path: *const i8, flags: i32, ...) -> i32;
    fn mkdirat(directory: i32, path: *const i8, mode: u16) -> i32;
    fn unlinkat(directory: i32, path: *const i8, flags: i32) -> i32;
    fn fchmodat(directory: i32, path: *const i8, mode: u16, flags: i32) -> i32;
    fn fstatat(directory: i32, path: *const i8, status: *mut DarwinStat, flags: i32) -> i32;
    fn fsync(file: i32) -> i32;
    fn flock(file: i32, operation: i32) -> i32;
    fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
    fn connect(socket: i32, address: *const c_void, address_length: u32) -> i32;
    #[cfg(test)]
    fn listen(socket: i32, backlog: i32) -> i32;
    fn poll(descriptors: *mut DarwinPollFd, count: u32, timeout_milliseconds: i32) -> i32;
    fn getsockopt(
        socket: i32,
        level: i32,
        option: i32,
        value: *mut c_void,
        value_length: *mut u32,
    ) -> i32;
    fn fcntl(descriptor: i32, command: i32, ...) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: i32,
    inode: u64,
    mode: u16,
    links: u16,
    owner: u32,
}

impl From<DarwinStat> for FileIdentity {
    fn from(status: DarwinStat) -> Self {
        Self {
            device: status.st_dev,
            inode: status.st_ino,
            mode: status.st_mode,
            links: status.st_nlink,
            owner: status.st_uid,
        }
    }
}

impl FileIdentity {
    fn is_socket(self) -> bool {
        self.mode & S_IFMT == S_IFSOCK
    }

    fn is_regular(self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }

    fn permissions(self) -> u16 {
        self.mode & 0o777
    }

    fn same_object(self, other: Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }
}

struct PrivateParent {
    directory: File,
    canonical_path: PathBuf,
    leaf: CString,
}

pub struct Server {
    listener: UnixListener,
    parent: File,
    socket_name: CString,
    socket_identity: FileIdentity,
    pending: HashSet<String>,
    _instance_lock: File,
    _active_path: ActivePathGuard,
}

static ACTIVE_SERVER_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

struct ActivePathGuard {
    path: PathBuf,
}

impl ActivePathGuard {
    fn reserve(path: PathBuf) -> Result<Self, String> {
        let mut active = ACTIVE_SERVER_PATHS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !active.insert(path.clone()) {
            return Err("socket-already-active-in-process".into());
        }
        Ok(Self { path })
    }
}

impl Drop for ActivePathGuard {
    fn drop(&mut self) {
        ACTIVE_SERVER_PATHS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.path);
    }
}

impl Server {
    pub fn bind(path: &Path) -> Result<Self, String> {
        let PrivateParent {
            directory,
            canonical_path,
            leaf,
        } = open_private_parent(path, true)?;
        let directory_fd = directory.as_raw_fd();
        let socket_path = canonical_path.join(os_str_from_cstring(&leaf));
        let active_path = ActivePathGuard::reserve(socket_path.clone())?;
        let instance_lock = acquire_instance_lock(directory_fd)?;

        if let Some(existing) = entry_identity(directory_fd, &leaf)
            .map_err(|error| format!("inspect socket entry: {error}"))?
        {
            let current_uid = unsafe { getuid() };
            if !existing.is_socket()
                || existing.owner != current_uid
                || existing.links != 1
                || existing.permissions() != 0o600
            {
                return Err("unsafe-existing-socket-entry".into());
            }
            unlink_entry(directory_fd, &leaf)
                .map_err(|error| format!("remove stale socket: {error}"))?;
            sync_directory(directory_fd)
                .map_err(|error| format!("sync socket dir after stale removal: {error}"))?;
        }

        let listener =
            UnixListener::bind(&socket_path).map_err(|error| format!("bind socket: {error}"))?;
        let initial_identity = match entry_identity(directory_fd, &leaf)
            .map_err(|error| format!("inspect bound socket: {error}"))?
        {
            Some(identity)
                if identity.is_socket()
                    && identity.owner == unsafe { getuid() }
                    && identity.links == 1 =>
            {
                identity
            }
            Some(_) => return Err("bound-socket-identity-invalid".into()),
            None => return Err("bound-socket-identity-invalid".into()),
        };

        match parent_path_matches(&canonical_path, &directory) {
            Ok(true) => {}
            Ok(false) => {
                guarded_unlink(directory_fd, &leaf, initial_identity);
                return Err("socket-parent-changed-during-bind".into());
            }
            Err(error) => {
                guarded_unlink(directory_fd, &leaf, initial_identity);
                return Err(format!("verify socket parent identity: {error}"));
            }
        }

        let chmod_status =
            unsafe { fchmodat(directory_fd, leaf.as_ptr(), 0o600, AT_SYMLINK_NOFOLLOW) };
        if chmod_status != 0 {
            let error = io::Error::last_os_error();
            guarded_unlink(directory_fd, &leaf, initial_identity);
            return Err(format!("set socket permissions: {error}"));
        }
        let socket_identity = match entry_identity(directory_fd, &leaf) {
            Ok(Some(identity))
                if identity.same_object(initial_identity)
                    && identity.is_socket()
                    && identity.owner == unsafe { getuid() }
                    && identity.links == 1
                    && identity.permissions() == 0o600 =>
            {
                identity
            }
            Ok(_) => {
                guarded_unlink(directory_fd, &leaf, initial_identity);
                return Err("bound-socket-identity-changed".into());
            }
            Err(error) => {
                guarded_unlink(directory_fd, &leaf, initial_identity);
                return Err(format!("reinspect bound socket: {error}"));
            }
        };
        if let Err(error) = sync_directory(directory_fd) {
            guarded_unlink(directory_fd, &leaf, socket_identity);
            return Err(format!("sync socket dir after bind: {error}"));
        }
        match entry_identity(directory_fd, &leaf) {
            Ok(Some(identity)) if identity == socket_identity => {}
            Ok(_) => {
                guarded_unlink(directory_fd, &leaf, socket_identity);
                return Err("bound-socket-identity-changed-after-sync".into());
            }
            Err(error) => {
                guarded_unlink(directory_fd, &leaf, socket_identity);
                return Err(format!("reinspect bound socket after sync: {error}"));
            }
        }
        match parent_path_matches(&canonical_path, &directory) {
            Ok(true) => {}
            Ok(false) => {
                guarded_unlink(directory_fd, &leaf, socket_identity);
                return Err("socket-parent-changed-after-bind".into());
            }
            Err(error) => {
                guarded_unlink(directory_fd, &leaf, socket_identity);
                return Err(format!("reinspect socket parent after bind: {error}"));
            }
        }

        Ok(Self {
            listener,
            parent: directory,
            socket_name: leaf,
            socket_identity,
            pending: HashSet::new(),
            _instance_lock: instance_lock,
            _active_path: active_path,
        })
    }

    pub fn serve_once(&mut self) -> Result<(), String> {
        let (mut stream, _) = self
            .listener
            .accept()
            .map_err(|error| format!("accept: {error}"))?;
        verify_same_user(&stream)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        let request = match read_bounded_line(&mut stream) {
            Ok(line) => line,
            Err(BoundedLineError::Empty) => {
                return write_protocol_error(&mut stream, "empty-request");
            }
            Err(BoundedLineError::MissingNewline) => {
                return write_protocol_error(&mut stream, "missing-newline");
            }
            Err(BoundedLineError::TooLarge) => {
                return write_protocol_error(&mut stream, "payload-too-large");
            }
            Err(BoundedLineError::Io(error)) => return Err(format!("read request: {error}")),
            Err(BoundedLineError::InvalidUtf8) => {
                return write_protocol_error(&mut stream, "invalid-utf8");
            }
        };
        let response = self.handle(&request);
        stream
            .write_all(response.as_bytes())
            .map_err(|error| format!("write response: {error}"))
    }

    fn handle(&mut self, line: &str) -> String {
        if !line.is_ascii() {
            return "error unknown non-ascii-payload\n".into();
        }
        let mut parts = line.split_ascii_whitespace();
        let Some(version) = parts.next() else {
            return "error unknown protocol-version\n".into();
        };
        if version != PROTOCOL_VERSION {
            return "error unknown protocol-version\n".into();
        }
        let Some(command) = parts.next() else {
            return "error unknown missing-command\n".into();
        };
        let Some(request_id) = parts.next() else {
            return "error unknown missing-request-id\n".into();
        };
        if request_id.len() > 128 {
            return "error unknown request-id-too-long\n".into();
        }
        if request_id.is_empty()
            || !request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
        {
            return "error unknown invalid-request-id\n".into();
        }
        let argument = parts.next();
        let has_unknown_fields = parts.next().is_some();
        match command {
            "hello" if argument.is_some() || has_unknown_fields => {
                format!("error {request_id} unexpected-fields\n")
            }
            "hello" => format!(
                "ok {request_id} hello ime=0.3.0 backend=0.3.0 capabilities=health,synthetic_request,cancel\n"
            ),
            "health" if argument.is_some() || has_unknown_fields => {
                format!("error {request_id} unexpected-fields\n")
            }
            "health" => format!("ok {request_id} health status=ready snapshot=fixture-v1\n"),
            "request" if argument == Some("synthetic") && !has_unknown_fields => {
                if self.pending.contains(request_id) {
                    return format!("ok {request_id} accepted synthetic-only\n");
                }
                if self.pending.len() >= MAX_PENDING_REQUESTS {
                    return format!("error {request_id} pending-capacity\n");
                }
                self.pending.insert(request_id.to_string());
                format!("ok {request_id} accepted synthetic-only\n")
            }
            "cancel" if argument.is_some() || has_unknown_fields => {
                format!("error {request_id} unexpected-fields\n")
            }
            "cancel" => {
                let removed = self.pending.remove(request_id);
                format!("ok {request_id} cancel removed={}\n", u8::from(removed))
            }
            "request" => format!("error {request_id} real-payload-disabled-in-v0.3\n"),
            _ => format!("error {request_id} unknown-command\n"),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        guarded_unlink(
            self.parent.as_raw_fd(),
            &self.socket_name,
            self.socket_identity,
        );
    }
}

pub fn request(path: &Path, message: &str, timeout: Duration) -> Result<String, String> {
    if message.as_bytes().contains(&b'\n') || message.as_bytes().contains(&b'\r') {
        return Err("invalid-payload-newline".into());
    }
    if message.len() + 1 > MAX_PAYLOAD {
        return Err("payload-too-large".into());
    }
    let (request_id, command) = client_request_identity(message)?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(backend_unavailable)?;
    let mut stream =
        connect_nonblocking_until(path, deadline).map_err(|_| backend_unavailable())?;
    verify_same_user(&stream).map_err(|_| backend_unavailable())?;
    let mut payload = Vec::with_capacity(message.len() + 1);
    payload.extend_from_slice(message.as_bytes());
    payload.push(b'\n');
    write_all_nonblocking_until(&mut stream, &payload, deadline)
        .map_err(|_| backend_unavailable())?;
    let response = read_bounded_line_nonblocking_until(&mut stream, deadline)
        .map_err(|_| backend_unavailable())?;
    validate_client_response(&response, request_id, command)?;
    Ok(format!("{response}\n"))
}

#[derive(Clone, Copy)]
enum ClientCommand {
    Hello,
    Health,
    SyntheticRequest,
    Cancel,
}

fn backend_unavailable() -> String {
    BACKEND_UNAVAILABLE.into()
}

fn client_request_identity(message: &str) -> Result<(&str, ClientCommand), String> {
    if !message.is_ascii() {
        return Err(backend_unavailable());
    }
    let mut fields = message.split_ascii_whitespace();
    if fields.next() != Some(PROTOCOL_VERSION) {
        return Err(backend_unavailable());
    }
    let command = fields.next().ok_or_else(backend_unavailable)?;
    let request_id = fields.next().ok_or_else(backend_unavailable)?;
    if request_id.is_empty()
        || request_id.len() > 128
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err(backend_unavailable());
    }
    let command = match command {
        "hello" if fields.next().is_none() => ClientCommand::Hello,
        "health" if fields.next().is_none() => ClientCommand::Health,
        "cancel" if fields.next().is_none() => ClientCommand::Cancel,
        "request" if fields.next() == Some("synthetic") && fields.next().is_none() => {
            ClientCommand::SyntheticRequest
        }
        _ => return Err(backend_unavailable()),
    };
    Ok((request_id, command))
}

fn validate_client_response(
    response: &str,
    request_id: &str,
    command: ClientCommand,
) -> Result<(), String> {
    if !response.is_ascii() || response.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(backend_unavailable());
    }
    let fields = response.split_ascii_whitespace().collect::<Vec<_>>();
    let valid = fields.first() == Some(&"ok")
        && fields.get(1) == Some(&request_id)
        && match command {
            ClientCommand::Hello => {
                fields.as_slice()
                    == [
                        "ok",
                        request_id,
                        "hello",
                        "ime=0.3.0",
                        "backend=0.3.0",
                        "capabilities=health,synthetic_request,cancel",
                    ]
            }
            ClientCommand::Health => {
                fields.as_slice()
                    == [
                        "ok",
                        request_id,
                        "health",
                        "status=ready",
                        "snapshot=fixture-v1",
                    ]
            }
            ClientCommand::SyntheticRequest => {
                fields.as_slice() == ["ok", request_id, "accepted", "synthetic-only"]
            }
            ClientCommand::Cancel => {
                fields.len() == 4
                    && fields[0] == "ok"
                    && fields[1] == request_id
                    && fields[2] == "cancel"
                    && matches!(fields[3], "removed=0" | "removed=1")
            }
        };
    if valid {
        Ok(())
    } else {
        Err(backend_unavailable())
    }
}

fn connect_nonblocking_until(path: &Path, deadline: Instant) -> io::Result<UnixStream> {
    let path_bytes = path.as_os_str().as_bytes();
    if path_bytes.is_empty()
        || path_bytes.len() >= std::mem::size_of::<DarwinSockAddrUn>() - 2
        || path_bytes.contains(&0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket path is invalid or too long",
        ));
    }

    let descriptor = unsafe { socket(AF_UNIX, SOCK_STREAM, 0) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    let status_flags = unsafe { fcntl(descriptor, F_GETFL) };
    if status_flags < 0 || unsafe { fcntl(descriptor, F_SETFL, status_flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor_flags = unsafe { fcntl(descriptor, F_GETFD) };
    if descriptor_flags < 0
        || unsafe { fcntl(descriptor, F_SETFD, descriptor_flags | FD_CLOEXEC) } < 0
    {
        return Err(io::Error::last_os_error());
    }

    let address_length = 2 + path_bytes.len() + 1;
    let mut address = DarwinSockAddrUn {
        sun_len: address_length as u8,
        sun_family: AF_UNIX as u8,
        sun_path: [0; 104],
    };
    unsafe {
        std::ptr::copy_nonoverlapping(
            path_bytes.as_ptr(),
            address.sun_path.as_mut_ptr().cast::<u8>(),
            path_bytes.len(),
        );
    }
    let status = unsafe {
        connect(
            descriptor,
            (&raw const address).cast::<c_void>(),
            address_length as u32,
        )
    };
    if status != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(DARWIN_EINPROGRESS)
            && error.kind() != io::ErrorKind::WouldBlock
        {
            return Err(error);
        }
        wait_for_descriptor(descriptor, POLLOUT, deadline)?;
        let mut socket_error = 0_i32;
        let mut socket_error_length = std::mem::size_of::<i32>() as u32;
        if unsafe {
            getsockopt(
                descriptor,
                SOL_SOCKET,
                SO_ERROR,
                (&raw mut socket_error).cast::<c_void>(),
                &mut socket_error_length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if socket_error != 0 {
            return Err(io::Error::from_raw_os_error(socket_error));
        }
    }
    Ok(unsafe { UnixStream::from_raw_fd(file.into_raw_fd()) })
}

fn write_all_nonblocking_until(
    stream: &mut UnixStream,
    data: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        match stream.write(&data[offset..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
            Ok(written) => offset += written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_for_descriptor(stream.as_raw_fd(), POLLOUT, deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn wait_for_descriptor(descriptor: RawFd, events: c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let remaining_nanos = deadline.duration_since(now).as_nanos();
        let timeout_milliseconds = remaining_nanos
            .saturating_add(999_999)
            .checked_div(1_000_000)
            .unwrap_or(u128::MAX)
            .min(i32::MAX as u128) as i32;
        let mut poll_descriptor = DarwinPollFd {
            fd: descriptor,
            events,
            revents: 0,
        };
        let status = unsafe { poll(&mut poll_descriptor, 1, timeout_milliseconds) };
        if status > 0 {
            return Ok(());
        }
        if status == 0 {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn verify_same_user(stream: &UnixStream) -> Result<(), String> {
    let mut peer_uid = 0_u32;
    let mut peer_gid = 0_u32;
    let status = unsafe { getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut peer_gid) };
    if status != 0 {
        return Err("peer-identity-unavailable".into());
    }
    let current_uid = unsafe { getuid() };
    if peer_uid != current_uid {
        return Err("peer-uid-mismatch".into());
    }
    Ok(())
}

pub fn create_private_marker(path: &Path) -> Result<(), String> {
    let PrivateParent {
        directory,
        canonical_path,
        leaf,
    } = open_private_parent(path, false)?;
    let directory_fd = directory.as_raw_fd();
    let descriptor = unsafe {
        openat(
            directory_fd,
            leaf.as_ptr(),
            O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
            0o600_u32,
        )
    };
    if descriptor < 0 {
        return Err(format!(
            "create private marker: {}",
            io::Error::last_os_error()
        ));
    }
    let mut file = unsafe { File::from_raw_fd(descriptor) };
    let metadata_before = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(file);
            return Err(format!("inspect private marker: {error}"));
        }
    };
    let identity = metadata_identity(&metadata_before);
    if !metadata_before.file_type().is_file()
        || identity.owner != unsafe { getuid() }
        || identity.links != 1
    {
        drop(file);
        guarded_unlink(directory_fd, &leaf, identity);
        return Err("private-marker-identity-invalid".into());
    }
    if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
        drop(file);
        guarded_unlink(directory_fd, &leaf, identity);
        return Err(format!("set marker permissions: {error}"));
    }
    let metadata_after = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            drop(file);
            guarded_unlink(directory_fd, &leaf, identity);
            return Err(format!("reinspect private marker: {error}"));
        }
    };
    let identity_after = metadata_identity(&metadata_after);
    if !identity_after.same_object(identity)
        || identity_after.owner != identity.owner
        || identity_after.links != 1
        || identity_after.permissions() != 0o600
    {
        drop(file);
        guarded_unlink(directory_fd, &leaf, identity);
        return Err("private-marker-identity-changed".into());
    }
    let result = (|| -> Result<(), String> {
        file.write_all(b"AIMIPC/1\n")
            .map_err(|error| format!("write private marker: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync private marker: {error}"))?;
        match entry_identity(directory_fd, &leaf) {
            Ok(Some(identity))
                if identity == identity_after
                    && identity.is_regular()
                    && identity.permissions() == 0o600 => {}
            Ok(_) => return Err("private-marker-entry-identity-changed".into()),
            Err(error) => return Err(format!("reinspect private marker entry: {error}")),
        }
        sync_directory(directory_fd).map_err(|error| format!("sync marker directory: {error}"))?;
        match entry_identity(directory_fd, &leaf) {
            Ok(Some(identity)) if identity == identity_after => {}
            Ok(_) => return Err("private-marker-entry-changed-after-sync".into()),
            Err(error) => {
                return Err(format!(
                    "reinspect private marker entry after sync: {error}"
                ));
            }
        }
        match parent_path_matches(&canonical_path, &directory) {
            Ok(true) => {}
            Ok(false) => return Err("private-marker-parent-changed".into()),
            Err(error) => return Err(format!("reinspect private marker parent: {error}")),
        }
        Ok(())
    })();
    if result.is_err() {
        drop(file);
        guarded_unlink(directory_fd, &leaf, identity_after);
    }
    result
}

#[derive(Debug)]
enum BoundedLineError {
    Empty,
    MissingNewline,
    TooLarge,
    InvalidUtf8,
    Io(io::Error),
}

fn read_bounded_line(reader: &mut impl Read) -> Result<String, BoundedLineError> {
    let mut buffer = [0_u8; MAX_PAYLOAD + 1];
    let mut length = 0_usize;
    loop {
        let read = match reader.read(&mut buffer[length..]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(BoundedLineError::Io(error)),
        };
        if read == 0 {
            return if length == 0 {
                Err(BoundedLineError::Empty)
            } else {
                Err(BoundedLineError::MissingNewline)
            };
        }
        let start = length;
        length += read;
        if let Some(offset) = buffer[start..length].iter().position(|byte| *byte == b'\n') {
            let newline = start + offset;
            if newline + 1 > MAX_PAYLOAD {
                return Err(BoundedLineError::TooLarge);
            }
            let mut end = newline;
            if end > 0 && buffer[end - 1] == b'\r' {
                end -= 1;
            }
            return std::str::from_utf8(&buffer[..end])
                .map(str::to_owned)
                .map_err(|_| BoundedLineError::InvalidUtf8);
        }
        if length > MAX_PAYLOAD {
            return Err(BoundedLineError::TooLarge);
        }
    }
}

fn read_bounded_line_nonblocking_until(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<String, BoundedLineError> {
    let mut buffer = [0_u8; MAX_PAYLOAD + 1];
    let mut length = 0_usize;
    loop {
        let read = match stream.read(&mut buffer[length..]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_for_descriptor(stream.as_raw_fd(), POLLIN, deadline)
                    .map_err(BoundedLineError::Io)?;
                continue;
            }
            Err(error) => return Err(BoundedLineError::Io(error)),
        };
        if read == 0 {
            return if length == 0 {
                Err(BoundedLineError::Empty)
            } else {
                Err(BoundedLineError::MissingNewline)
            };
        }
        let start = length;
        length += read;
        if let Some(offset) = buffer[start..length].iter().position(|byte| *byte == b'\n') {
            let newline = start + offset;
            if newline + 1 > MAX_PAYLOAD {
                return Err(BoundedLineError::TooLarge);
            }
            let mut end = newline;
            if end > 0 && buffer[end - 1] == b'\r' {
                end -= 1;
            }
            return std::str::from_utf8(&buffer[..end])
                .map(str::to_owned)
                .map_err(|_| BoundedLineError::InvalidUtf8);
        }
        if length > MAX_PAYLOAD {
            return Err(BoundedLineError::TooLarge);
        }
    }
}

fn write_protocol_error(stream: &mut UnixStream, reason: &str) -> Result<(), String> {
    stream
        .write_all(format!("error unknown {reason}\n").as_bytes())
        .map_err(|error| format!("write response: {error}"))
}

fn open_private_parent(path: &Path, create_parent: bool) -> Result<PrivateParent, String> {
    let physical_path = physical_runtime_path(path);
    if !physical_path.is_absolute()
        || physical_path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err("socket-or-marker-path-must-be-absolute-and-normal".into());
    }
    let path = physical_path.as_path();
    let leaf_os = path
        .file_name()
        .ok_or_else(|| "socket-or-marker-path-must-have-one-leaf".to_string())?;
    if !matches!(
        Path::new(leaf_os).components().next(),
        Some(Component::Normal(_))
    ) || Path::new(leaf_os).components().count() != 1
    {
        return Err("socket-or-marker-leaf-is-invalid".into());
    }
    let leaf = cstring(leaf_os).map_err(|_| "socket-or-marker-leaf-contains-nul".to_string())?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    match fs::symlink_metadata(parent) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err("socket-parent-must-be-a-real-directory".into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create_parent => {
            create_private_parent_leaf(parent)?;
        }
        Err(error) => return Err(format!("inspect socket parent: {error}")),
    }

    let before =
        fs::symlink_metadata(parent).map_err(|error| format!("inspect socket parent: {error}"))?;
    if before.file_type().is_symlink() || !before.file_type().is_dir() {
        return Err("socket-parent-must-be-a-real-directory".into());
    }
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY | O_NOFOLLOW_ANY | O_CLOEXEC)
        .open(parent)
        .map_err(|error| format!("open socket parent without symlinks: {error}"))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("inspect opened socket parent: {error}"))?;
    if before.dev() != opened.dev() || before.ino() != opened.ino() {
        return Err("socket-parent-changed-during-open".into());
    }
    validate_private_directory(&opened)?;
    if !parent_path_matches(parent, &directory)
        .map_err(|error| format!("verify socket parent path: {error}"))?
    {
        return Err("socket-parent-changed-during-open".into());
    }
    let canonical_path =
        fs::canonicalize(parent).map_err(|error| format!("canonicalize socket parent: {error}"))?;
    if !parent_path_matches(&canonical_path, &directory)
        .map_err(|error| format!("verify canonical socket parent: {error}"))?
    {
        return Err("socket-parent-changed-during-open".into());
    }

    Ok(PrivateParent {
        directory,
        canonical_path,
        leaf,
    })
}

/// `/tmp` and `/var` are root-owned compatibility aliases on macOS. Resolve
/// only those two platform paths so O_NOFOLLOW_ANY can still reject every
/// user-controlled symlink component.
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

fn acquire_instance_lock(directory_fd: RawFd) -> Result<File, String> {
    let name = CString::new(INSTANCE_LOCK_NAME).expect("fixed IPC lock name contains no NUL");
    let descriptor = unsafe {
        openat(
            directory_fd,
            name.as_ptr(),
            O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC,
            0o600_u32,
        )
    };
    if descriptor < 0 {
        return Err(format!(
            "open IPC instance lock: {}",
            io::Error::last_os_error()
        ));
    }
    let lock = unsafe { File::from_raw_fd(descriptor) };
    let before = lock
        .metadata()
        .map_err(|error| format!("inspect IPC instance lock: {error}"))?;
    let before_identity = metadata_identity(&before);
    if !before.is_file()
        || before_identity.owner != unsafe { getuid() }
        || before_identity.links != 1
    {
        return Err("unsafe-IPC-instance-lock".into());
    }
    lock.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("set IPC instance lock permissions: {error}"))?;
    let after = lock
        .metadata()
        .map_err(|error| format!("reinspect IPC instance lock: {error}"))?;
    let after_identity = metadata_identity(&after);
    if !after_identity.same_object(before_identity)
        || after_identity.owner != unsafe { getuid() }
        || after_identity.links != 1
        || after_identity.permissions() != 0o600
    {
        return Err("IPC-instance-lock-identity-changed".into());
    }
    if unsafe { flock(lock.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Err("socket-already-active-in-another-process".into());
        }
        return Err(format!("lock IPC instance: {error}"));
    }
    match entry_identity(directory_fd, &name) {
        Ok(Some(identity)) if identity == after_identity => {}
        Ok(_) => return Err("IPC-instance-lock-entry-changed".into()),
        Err(error) => return Err(format!("verify IPC instance lock entry: {error}")),
    }
    sync_directory(directory_fd).map_err(|error| format!("sync IPC lock directory: {error}"))?;
    Ok(lock)
}

fn create_private_parent_leaf(parent: &Path) -> Result<(), String> {
    let name = parent
        .file_name()
        .ok_or_else(|| "socket-parent-must-have-one-creatable-leaf".to_string())?;
    if !matches!(
        Path::new(name).components().next(),
        Some(Component::Normal(_))
    ) || Path::new(name).components().count() != 1
    {
        return Err("socket-parent-leaf-is-invalid".into());
    }
    let grandparent = parent
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let grandparent_metadata = fs::symlink_metadata(grandparent)
        .map_err(|error| format!("inspect socket parent container: {error}"))?;
    if grandparent_metadata.file_type().is_symlink() || !grandparent_metadata.file_type().is_dir() {
        return Err("socket-parent-container-must-be-a-real-directory".into());
    }
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY | O_NOFOLLOW_ANY | O_CLOEXEC)
        .open(grandparent)
        .map_err(|error| format!("open socket parent container: {error}"))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("inspect socket parent container: {error}"))?;
    if grandparent_metadata.dev() != opened.dev() || grandparent_metadata.ino() != opened.ino() {
        return Err("socket-parent-container-changed-during-open".into());
    }
    validate_private_directory(&opened)?;
    if !parent_path_matches(grandparent, &directory)
        .map_err(|error| format!("verify socket parent container path: {error}"))?
    {
        return Err("socket-parent-container-changed-during-open".into());
    }
    let name = cstring(name).map_err(|_| "socket-parent-leaf-contains-nul".to_string())?;
    let status = unsafe { mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
    let created = status == 0;
    if status != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(format!("create socket parent leaf: {error}"));
        }
    }
    if created {
        let descriptor = unsafe {
            openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(format!(
                "open created socket parent leaf: {}",
                io::Error::last_os_error()
            ));
        }
        let created_directory = unsafe { File::from_raw_fd(descriptor) };
        created_directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("set created socket parent permissions: {error}"))?;
        let metadata = created_directory
            .metadata()
            .map_err(|error| format!("inspect created socket parent: {error}"))?;
        validate_private_directory(&metadata)?;
        created_directory
            .sync_all()
            .map_err(|error| format!("sync created socket parent: {error}"))?;
    }
    sync_directory(directory.as_raw_fd())
        .map_err(|error| format!("sync socket parent container: {error}"))
}

fn validate_private_directory(metadata: &fs::Metadata) -> Result<(), String> {
    if !metadata.file_type().is_dir()
        || metadata.uid() != unsafe { getuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err("socket-parent-must-be-current-user-mode-0700".into());
    }
    Ok(())
}

fn parent_path_matches(path: &Path, directory: &File) -> io::Result<bool> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let descriptor_metadata = directory.metadata()?;
    Ok(path_metadata.dev() == descriptor_metadata.dev()
        && path_metadata.ino() == descriptor_metadata.ino())
}

fn entry_identity(directory: RawFd, leaf: &CString) -> io::Result<Option<FileIdentity>> {
    let mut status = std::mem::MaybeUninit::<DarwinStat>::zeroed();
    let result = unsafe {
        fstatat(
            directory,
            leaf.as_ptr(),
            status.as_mut_ptr(),
            AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(Some(FileIdentity::from(unsafe { status.assume_init() })));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(error)
    }
}

fn unlink_entry(directory: RawFd, leaf: &CString) -> io::Result<()> {
    let status = unsafe { unlinkat(directory, leaf.as_ptr(), 0) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn guarded_unlink(directory: RawFd, leaf: &CString, expected: FileIdentity) {
    let Ok(Some(current)) = entry_identity(directory, leaf) else {
        return;
    };
    if current == expected && unlink_entry(directory, leaf).is_ok() {
        let _ = sync_directory(directory);
    }
}

fn sync_directory(directory: RawFd) -> io::Result<()> {
    let status = unsafe { fsync(directory) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn metadata_identity(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev() as i32,
        inode: metadata.ino(),
        mode: metadata.mode() as u16,
        links: metadata.nlink() as u16,
        owner: metadata.uid(),
    }
}

fn cstring(value: &OsStr) -> Result<CString, std::ffi::NulError> {
    CString::new(value.as_bytes())
}

fn os_str_from_cstring(value: &CString) -> &OsStr {
    OsStr::from_bytes(value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::{FileTypeExt, symlink};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Instant;

    static FIXTURE_SEQUENCE: AtomicUsize = AtomicUsize::new(1);

    struct Fixture {
        root: PathBuf,
        device: u64,
        inode: u64,
        marker_device: u64,
        marker_inode: u64,
        owner_marker: Vec<u8>,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary = fs::canonicalize("/tmp").unwrap();
            let short_name = name.chars().take(8).collect::<String>();
            let root = temporary.join(format!(
                "aimipc-{}-{sequence}-{}",
                std::process::id(),
                short_name
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let owner_marker = format!(
                "aimipc-test-owner:{}:{sequence}:{name}\n",
                std::process::id()
            )
            .into_bytes();
            let marker_path = root.join(".fixture-owner");
            let mut marker = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&marker_path)
                .unwrap();
            marker
                .set_permissions(fs::Permissions::from_mode(0o600))
                .unwrap();
            marker.write_all(&owner_marker).unwrap();
            marker.sync_all().unwrap();
            let root_metadata = fs::symlink_metadata(&root).unwrap();
            let marker_metadata = fs::symlink_metadata(&marker_path).unwrap();
            Self {
                root,
                device: root_metadata.dev(),
                inode: root_metadata.ino(),
                marker_device: marker_metadata.dev(),
                marker_inode: marker_metadata.ino(),
                owner_marker,
            }
        }

        fn private_dir(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        fn socket(&self, parent: &str) -> PathBuf {
            self.root.join(parent).join("control.sock")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let Ok(root_metadata) = fs::symlink_metadata(&self.root) else {
                return;
            };
            let marker_path = self.root.join(".fixture-owner");
            let Ok(marker_metadata) = fs::symlink_metadata(&marker_path) else {
                return;
            };
            if root_metadata.file_type().is_symlink()
                || !root_metadata.file_type().is_dir()
                || root_metadata.dev() != self.device
                || root_metadata.ino() != self.inode
                || root_metadata.uid() != unsafe { getuid() }
                || root_metadata.mode() & 0o777 != 0o700
                || marker_metadata.file_type().is_symlink()
                || !marker_metadata.file_type().is_file()
                || marker_metadata.dev() != self.marker_device
                || marker_metadata.ino() != self.marker_inode
                || marker_metadata.nlink() != 1
                || marker_metadata.uid() != unsafe { getuid() }
                || marker_metadata.mode() & 0o777 != 0o600
                || fs::read(&marker_path).ok().as_deref() != Some(self.owner_marker.as_slice())
            {
                return;
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn exchange(server: &mut Server, path: &Path, message: &'static str) -> String {
        let path = path.to_path_buf();
        let client =
            thread::spawn(move || request(&path, message, Duration::from_secs(1)).unwrap());
        server.serve_once().unwrap();
        client.join().unwrap()
    }

    fn sentinel(path: &Path, contents: &[u8]) -> (Vec<u8>, u32) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o640)).unwrap();
        (
            fs::read(path).unwrap(),
            fs::symlink_metadata(path).unwrap().mode() & 0o777,
        )
    }

    fn assert_sentinel(path: &Path, expected: &(Vec<u8>, u32)) {
        assert_eq!(fs::read(path).unwrap(), expected.0);
        assert_eq!(
            fs::symlink_metadata(path).unwrap().mode() & 0o777,
            expected.1
        );
    }

    #[test]
    fn hello_health_synthetic_request_and_cancel() {
        let fixture = Fixture::new("protocol");
        let path = fixture.socket("private");
        let mut server = Server::bind(&path).unwrap();
        assert!(exchange(&mut server, &path, "AIMIPC/1 hello req-1").contains("capabilities="));
        assert!(exchange(&mut server, &path, "AIMIPC/1 health req-2").contains("status=ready"));
        assert!(
            exchange(&mut server, &path, "AIMIPC/1 request req-3 synthetic").contains("accepted")
        );
        assert!(exchange(&mut server, &path, "AIMIPC/1 cancel req-3").contains("removed=1"));
        assert!(
            server
                .handle("AIMIPC/1 request req-4 user-text")
                .contains("real-payload-disabled")
        );
    }

    #[test]
    fn protocol_rejects_unknown_fields_and_every_non_synthetic_payload() {
        let fixture = Fixture::new("strict-protocol");
        let path = fixture.socket("private");
        let mut server = Server::bind(&path).unwrap();

        for message in [
            "AIMIPC/1 hello req extra",
            "AIMIPC/1 health req extra",
            "AIMIPC/1 cancel req extra",
        ] {
            assert!(server.handle(message).contains("unexpected-fields"));
        }
        for message in [
            "AIMIPC/1 request req",
            "AIMIPC/1 request req user-text",
            "AIMIPC/1 request req synthetic trailing-field",
        ] {
            assert!(server.handle(message).contains("real-payload-disabled"));
        }
        assert!(
            server
                .handle("AIMIPC/1 request req 用户正文")
                .contains("non-ascii-payload")
        );
        assert!(
            server
                .handle("AIMIPC/1 health invalid/request")
                .contains("invalid-request-id")
        );
        assert!(server.pending.is_empty());
    }

    #[test]
    fn platform_tmp_alias_is_normalized_but_arbitrary_paths_are_not() {
        assert_eq!(
            physical_runtime_path(Path::new("/tmp/agent-ime")),
            Path::new("/private/tmp/agent-ime")
        );
        assert_eq!(
            physical_runtime_path(Path::new("/var/folders/agent-ime")),
            Path::new("/private/var/folders/agent-ime")
        );
        assert_eq!(
            physical_runtime_path(Path::new("/various/agent-ime")),
            Path::new("/various/agent-ime")
        );

        let fixture = Fixture::new("tmp-alias");
        let physical = fixture.socket("private");
        let alias = Path::new("/tmp").join(physical.strip_prefix("/private/tmp").unwrap());
        let mut server = Server::bind(&alias).unwrap();
        assert!(exchange(&mut server, &alias, "AIMIPC/1 health alias").contains("status=ready"));
    }

    #[test]
    fn unavailable_backend_fails_without_hot_path_delay() {
        let fixture = Fixture::new("missing");
        let path = fixture.socket("absent");
        let started = Instant::now();
        let result = request(&path, "AIMIPC/1 health req", Duration::from_millis(50));
        assert_eq!(result.unwrap_err(), BACKEND_UNAVAILABLE);
        assert!(started.elapsed() < Duration::from_millis(10));
    }

    #[test]
    fn client_deadline_bounds_saturated_connect_and_trickle_response() {
        let fixture = Fixture::new("deadline");
        let parent = fixture.private_dir("private");
        let backlog_path = parent.join("backlog.sock");
        let backlog_listener = UnixListener::bind(&backlog_path).unwrap();
        assert_eq!(unsafe { listen(backlog_listener.as_raw_fd(), 1) }, 0);

        let mut queued = Vec::new();
        let mut observed_bounded_saturation = false;
        for _ in 0..64 {
            let started = Instant::now();
            match connect_nonblocking_until(
                &backlog_path,
                Instant::now() + Duration::from_millis(20),
            ) {
                Ok(stream) => queued.push(stream),
                Err(_) => {
                    assert!(started.elapsed() < Duration::from_millis(200));
                    observed_bounded_saturation = true;
                    break;
                }
            }
        }
        assert!(
            observed_bounded_saturation,
            "backlog did not saturate within 64 clients"
        );

        let started = Instant::now();
        let error = request(
            &backlog_path,
            "AIMIPC/1 health saturated",
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert_eq!(error, BACKEND_UNAVAILABLE);
        assert!(started.elapsed() < Duration::from_millis(250));
        drop(queued);
        drop(backlog_listener);

        let trickle_path = parent.join("trickle.sock");
        let trickle_listener = UnixListener::bind(&trickle_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = trickle_listener.accept().unwrap();
            let mut request_bytes = [0_u8; 128];
            let _ = stream.read(&mut request_bytes);
            for byte in b"ok slow trickle-response\n" {
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let error = request(
            &trickle_path,
            "AIMIPC/1 health trickle",
            Duration::from_millis(60),
        )
        .unwrap_err();
        assert_eq!(error, BACKEND_UNAVAILABLE);
        assert!(started.elapsed() < Duration::from_millis(250));
        server.join().unwrap();
    }

    #[test]
    fn socket_and_marker_are_private_and_durable() {
        let fixture = Fixture::new("permissions");
        let path = fixture.socket("private");
        let server = Server::bind(&path).unwrap();
        assert_eq!(
            fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let marker = path.with_extension("marker");
        create_private_marker(&marker).unwrap();
        let marker_metadata = fs::symlink_metadata(&marker).unwrap();
        assert_eq!(marker_metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(marker_metadata.nlink(), 1);
        assert_eq!(fs::read(&marker).unwrap(), b"AIMIPC/1\n");
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn parent_symlink_non_directory_and_wrong_mode_are_rejected_without_mutation() {
        let fixture = Fixture::new("parent-reject");
        let real_parent = fixture.private_dir("real-parent");
        let protected = real_parent.join("sentinel");
        let expected = sentinel(&protected, b"parent target");
        let linked_parent = fixture.root.join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        assert!(Server::bind(&linked_parent.join("control.sock")).is_err());
        assert_sentinel(&protected, &expected);
        assert!(
            fs::symlink_metadata(&linked_parent)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let regular_parent = fixture.root.join("regular-parent");
        let regular_expected = sentinel(&regular_parent, b"regular parent");
        assert!(Server::bind(&regular_parent.join("control.sock")).is_err());
        assert_sentinel(&regular_parent, &regular_expected);

        let wrong_mode = fixture.private_dir("wrong-mode");
        fs::set_permissions(&wrong_mode, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Server::bind(&wrong_mode.join("control.sock")).is_err());
        assert_eq!(
            fs::symlink_metadata(&wrong_mode).unwrap().mode() & 0o777,
            0o755
        );

        let ancestor_target = fixture.private_dir("ancestor-target");
        let nested_parent = ancestor_target.join("nested-parent");
        fs::create_dir(&nested_parent).unwrap();
        fs::set_permissions(&nested_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let ancestor_sentinel = nested_parent.join("sentinel");
        let ancestor_expected = sentinel(&ancestor_sentinel, b"ancestor symlink target");
        let ancestor_link = fixture.root.join("ancestor-link");
        symlink(&ancestor_target, &ancestor_link).unwrap();
        let linked_nested_parent = ancestor_link.join("nested-parent");
        assert!(Server::bind(&linked_nested_parent.join("control.sock")).is_err());
        assert!(create_private_marker(&linked_nested_parent.join("ready.marker")).is_err());
        assert_sentinel(&ancestor_sentinel, &ancestor_expected);
        assert!(!nested_parent.join("control.sock").exists());
        assert!(!nested_parent.join("ready.marker").exists());
    }

    #[test]
    fn unsafe_stale_socket_entries_are_never_deleted_or_chmodded() {
        let fixture = Fixture::new("stale-sentinels");
        let parent = fixture.private_dir("private");
        let socket_path = parent.join("control.sock");

        let regular_expected = sentinel(&socket_path, b"regular sentinel");
        assert!(Server::bind(&socket_path).is_err());
        assert_sentinel(&socket_path, &regular_expected);
        fs::remove_file(&socket_path).unwrap();

        let target = parent.join("target");
        let target_expected = sentinel(&target, b"symlink target");
        symlink(&target, &socket_path).unwrap();
        assert!(Server::bind(&socket_path).is_err());
        assert_sentinel(&target, &target_expected);
        assert!(
            fs::symlink_metadata(&socket_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&socket_path).unwrap();

        let hardlink_source = parent.join("hardlink-source");
        let hardlink_expected = sentinel(&hardlink_source, b"hardlink sentinel");
        fs::hard_link(&hardlink_source, &socket_path).unwrap();
        assert!(Server::bind(&socket_path).is_err());
        assert_sentinel(&hardlink_source, &hardlink_expected);
        assert_sentinel(&socket_path, &hardlink_expected);
        assert_eq!(fs::metadata(&hardlink_source).unwrap().nlink(), 2);
    }

    #[test]
    fn safe_current_user_stale_socket_is_replaced() {
        let fixture = Fixture::new("stale-socket");
        let parent = fixture.private_dir("private");
        let path = parent.join("control.sock");
        let stale = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        drop(stale);
        let server = Server::bind(&path).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn second_server_cannot_remove_an_active_socket() {
        let fixture = Fixture::new("active-socket");
        let path = fixture.socket("private");
        let original = Server::bind(&path).unwrap();
        let identity = fs::symlink_metadata(&path).unwrap();

        assert!(Server::bind(&path).is_err());
        let unchanged = fs::symlink_metadata(&path).unwrap();
        assert_eq!(unchanged.dev(), identity.dev());
        assert_eq!(unchanged.ino(), identity.ino());

        drop(original);
        let replacement = Server::bind(&path).unwrap();
        drop(replacement);
        assert!(!path.exists());
    }

    #[test]
    fn drop_preserves_regular_symlink_hardlink_and_socket_replacements() {
        let fixture = Fixture::new("drop-replacement");
        let parent = fixture.private_dir("private");
        let path = parent.join("control.sock");

        let server = Server::bind(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let expected = sentinel(&path, b"replacement sentinel");
        drop(server);
        assert_sentinel(&path, &expected);
        fs::remove_file(&path).unwrap();

        let symlink_target = parent.join("replacement-target");
        let symlink_expected = sentinel(&symlink_target, b"replacement symlink target");
        let server = Server::bind(&path).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&symlink_target, &path).unwrap();
        drop(server);
        assert_sentinel(&symlink_target, &symlink_expected);
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&path).unwrap();

        let hardlink_source = parent.join("replacement-hardlink-source");
        let hardlink_expected = sentinel(&hardlink_source, b"replacement hardlink");
        let server = Server::bind(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::hard_link(&hardlink_source, &path).unwrap();
        drop(server);
        assert_sentinel(&hardlink_source, &hardlink_expected);
        assert_sentinel(&path, &hardlink_expected);
        assert_eq!(fs::metadata(&hardlink_source).unwrap().nlink(), 2);
        fs::remove_file(&path).unwrap();

        let server = Server::bind(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let replacement_listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let replacement_metadata = fs::symlink_metadata(&path).unwrap();
        drop(server);
        let after_drop = fs::symlink_metadata(&path).unwrap();
        assert!(after_drop.file_type().is_socket());
        assert_eq!(after_drop.dev(), replacement_metadata.dev());
        assert_eq!(after_drop.ino(), replacement_metadata.ino());
        drop(replacement_listener);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn drop_uses_stable_parent_and_does_not_touch_replaced_parent_path() {
        let fixture = Fixture::new("drop-parent-replacement");
        let parent = fixture.private_dir("private");
        let path = parent.join("control.sock");
        let server = Server::bind(&path).unwrap();
        let moved_parent = fixture.root.join("moved-private");
        fs::rename(&parent, &moved_parent).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let replacement = parent.join("control.sock");
        let expected = sentinel(&replacement, b"external replacement");
        drop(server);
        assert_sentinel(&replacement, &expected);
        assert!(!moved_parent.join("control.sock").exists());
    }

    #[test]
    fn marker_rejects_symlink_regular_and_hardlink_sentinels() {
        let fixture = Fixture::new("marker-sentinels");
        let parent = fixture.private_dir("private");
        let marker = parent.join("ready.marker");
        let regular_expected = sentinel(&marker, b"regular marker sentinel");
        assert!(create_private_marker(&marker).is_err());
        assert_sentinel(&marker, &regular_expected);
        fs::remove_file(&marker).unwrap();

        let target = parent.join("target");
        let target_expected = sentinel(&target, b"marker target");
        symlink(&target, &marker).unwrap();
        assert!(create_private_marker(&marker).is_err());
        assert_sentinel(&target, &target_expected);
        assert!(
            fs::symlink_metadata(&marker)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&marker).unwrap();

        let hardlink_source = parent.join("hardlink-source");
        let hardlink_expected = sentinel(&hardlink_source, b"marker hardlink");
        fs::hard_link(&hardlink_source, &marker).unwrap();
        assert!(create_private_marker(&marker).is_err());
        assert_sentinel(&hardlink_source, &hardlink_expected);
        assert_sentinel(&marker, &hardlink_expected);
        assert_eq!(fs::metadata(&hardlink_source).unwrap().nlink(), 2);
    }

    #[test]
    fn marker_rejects_symlink_parent_without_touching_target() {
        let fixture = Fixture::new("marker-parent-symlink");
        let real_parent = fixture.private_dir("real-parent");
        let target = real_parent.join("sentinel");
        let expected = sentinel(&target, b"marker parent target");
        let linked_parent = fixture.root.join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        assert!(create_private_marker(&linked_parent.join("ready.marker")).is_err());
        assert_sentinel(&target, &expected);
    }

    #[test]
    fn server_rejects_oversize_and_missing_newline_without_unbounded_read() {
        let fixture = Fixture::new("bounded-server");
        let path = fixture.socket("private");
        let mut server = Server::bind(&path).unwrap();

        let oversized_path = path.clone();
        let oversized = thread::spawn(move || {
            let mut stream = UnixStream::connect(oversized_path).unwrap();
            stream.write_all(&vec![b'x'; MAX_PAYLOAD + 1]).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        server.serve_once().unwrap();
        assert!(oversized.join().unwrap().contains("payload-too-large"));

        let missing_path = path.clone();
        let missing = thread::spawn(move || {
            let mut stream = UnixStream::connect(missing_path).unwrap();
            stream.write_all(b"AIMIPC/1 health no-newline").unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        server.serve_once().unwrap();
        assert!(missing.join().unwrap().contains("missing-newline"));
    }

    #[test]
    fn client_maps_bad_response_framing_to_backend_unavailable() {
        for (name, response) in [
            ("oversize-response", vec![b'x'; MAX_PAYLOAD + 1]),
            (
                "missing-response-newline",
                b"ok request response-without-newline".to_vec(),
            ),
        ] {
            let fixture = Fixture::new(name);
            let parent = fixture.private_dir("private");
            let path = parent.join("control.sock");
            let listener = UnixListener::bind(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 128];
                // Same race as the peer-error fixture: the client rejects the
                // malformed framing and closes first. Tolerate the early close;
                // the client-side BACKEND_UNAVAILABLE assertion is unchanged.
                let _ = stream.read(&mut request);
                let _ = stream.write_all(&response);
                let _ = stream.shutdown(Shutdown::Write);
            });
            let error = request(
                &path,
                "AIMIPC/1 health bounded-client",
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert_eq!(error, BACKEND_UNAVAILABLE);
            server.join().unwrap();
        }
    }

    #[test]
    fn client_maps_peer_errors_version_mismatch_and_wrong_identity_to_backend_unavailable() {
        for (name, response) in [
            (
                "peer-error",
                b"error protocol-client protocol-version\n".to_vec(),
            ),
            (
                "wrong-request-id",
                b"ok another-id health status=ready snapshot=fixture-v1\n".to_vec(),
            ),
            (
                "wrong-response-shape",
                b"ok protocol-client health status=ready snapshot=unexpected\n".to_vec(),
            ),
            ("non-ascii-response", vec![0xff, b'\n']),
        ] {
            let fixture = Fixture::new(name);
            let parent = fixture.private_dir("private");
            let path = parent.join("control.sock");
            let listener = UnixListener::bind(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 128];
                // The client legitimately rejects each malformed response and
                // closes immediately, so serving it can lose the race. Tolerate
                // the early close here; the assertion under test is the
                // client-side BACKEND_UNAVAILABLE below, which is unchanged.
                let _ = stream.read(&mut request);
                let _ = stream.write_all(&response);
                let _ = stream.shutdown(Shutdown::Write);
            });
            let error = request(
                &path,
                "AIMIPC/1 health protocol-client",
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert_eq!(error, BACKEND_UNAVAILABLE);
            server.join().unwrap();
        }

        let fixture = Fixture::new("local-version");
        let path = fixture.socket("absent");
        assert_eq!(
            request(&path, "AIMIPC/2 health local", Duration::from_millis(10)).unwrap_err(),
            BACKEND_UNAVAILABLE
        );
        assert_eq!(
            request(
                &path,
                "AIMIPC/1 request local user-text",
                Duration::from_millis(10),
            )
            .unwrap_err(),
            BACKEND_UNAVAILABLE
        );
    }

    #[test]
    fn pending_capacity_is_bounded_and_duplicate_ids_are_idempotent() {
        let fixture = Fixture::new("pending-cap");
        let path = fixture.socket("private");
        let mut server = Server::bind(&path).unwrap();
        for index in 0..MAX_PENDING_REQUESTS {
            let response = server.handle(&format!("AIMIPC/1 request req-{index} synthetic"));
            assert!(response.contains("accepted synthetic-only"));
        }
        assert_eq!(server.pending.len(), MAX_PENDING_REQUESTS);
        assert!(
            server
                .handle("AIMIPC/1 request req-0 synthetic")
                .contains("accepted synthetic-only")
        );
        assert_eq!(server.pending.len(), MAX_PENDING_REQUESTS);
        assert!(
            server
                .handle("AIMIPC/1 request over-cap synthetic")
                .contains("pending-capacity")
        );
        assert_eq!(server.pending.len(), MAX_PENDING_REQUESTS);
        assert!(server.handle("AIMIPC/1 cancel req-0").contains("removed=1"));
        assert!(
            server
                .handle("AIMIPC/1 request after-cancel synthetic")
                .contains("accepted synthetic-only")
        );
        assert_eq!(server.pending.len(), MAX_PENDING_REQUESTS);
    }
}
