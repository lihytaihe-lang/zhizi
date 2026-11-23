//! Hourly uploads for the explicitly selected distribution channel. Public and friend
//! builds use separate consent receipts, credentials, cursors and launch agents.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, Ordering}};

#[cfg(feature = "test-upload")]
#[path = "test_upload.rs"]
mod channel;
#[cfg(not(feature = "test-upload"))]
mod channel {
    pub const FLAG: &str = "--upload-experience-logs";
    pub const BASE_URL: &str = "https://zhizi.ihongui.com/zhizi/v1/experience";
    pub const LAUNCH_AGENT_LABEL: &str = "com.agentinputlayer.zhizi.experience";
    pub const DIRECTORY: &str = "ExperienceUpload";
    pub const PARTICIPATION_FILE: &str = "experience-participation.json";
    pub const PARTICIPATION_NOTICE: &str = "experience-v1";
}
pub use channel::{FLAG, LAUNCH_AGENT_LABEL};
use channel::{BASE_URL, DIRECTORY, PARTICIPATION_FILE, PARTICIPATION_NOTICE};
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const INTERVAL_SECONDS: u32 = 3600;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    device_id: String,
    token: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct State {
    offset: u64,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    accepted: u64,
    duplicates: u64,
    rejected: u64,
    #[serde(default)]
    cancelled: bool,
}

fn support_directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("无法确定用户目录。")?;
    Ok(PathBuf::from(home).join("Library/Application Support/AgentInputMethod"))
}

fn upload_directory() -> Result<PathBuf, String> {
    let directory = support_directory()?.join(DIRECTORY);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    set_mode(&directory, 0o700);
    Ok(directory)
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    set_mode(&temporary, 0o600);
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| error.to_string())
}

async fn credentials(directory: &Path, client: &reqwest::Client) -> Result<Credentials, String> {
    let path = directory.join("credentials.json");
    if let Some(credentials) = read_json::<Credentials>(&path) {
        return Ok(credentials);
    }
    #[cfg(not(feature = "test-upload"))]
    let registration = serde_json::json!({"platform":"macos", "notice":"experience-v1"});
    #[cfg(feature = "test-upload")]
    let registration = channel::registration(directory)?;
    if !upload_permitted() {
        return Err("已撤回同意或停止记录。".into());
    }
    let response = client
        .post(format!("{BASE_URL}/register"))
        .json(&registration)
        .send()
        .await
        .map_err(|error| format!("注册失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("注册失败：HTTP {}", response.status().as_u16()));
    }
    let credentials: Credentials = response.json().await.map_err(|error| error.to_string())?;
    write_private(&path, &serde_json::to_vec(&credentials).map_err(|error| error.to_string())?)?;
    #[cfg(feature = "test-upload")]
    channel::registered(directory);
    Ok(credentials)
}

/// The complete lines from `offset`, at most CHUNK_BYTES (one longer line goes alone).
fn next_chunk(events: &Path, offset: u64) -> Result<Vec<u8>, String> {
    let mut file = fs::File::open(events).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
    let mut buffer = Vec::new();
    file.take(CHUNK_BYTES as u64)
        .read_to_end(&mut buffer)
        .map_err(|error| error.to_string())?;
    match buffer.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => buffer.truncate(end + 1),
        None if buffer.len() == CHUNK_BYTES => {
            // A single line longer than a chunk: send it whole.
            let mut rest = Vec::new();
            let mut file = fs::File::open(events).map_err(|error| error.to_string())?;
            file.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
            std::io::BufRead::read_until(&mut std::io::BufReader::new(file), b'\n', &mut rest)
                .map_err(|error| error.to_string())?;
            buffer = if rest.ends_with(b"\n") { rest } else { Vec::new() };
        }
        None => buffer.clear(), // the writer is mid-line; wait for the next run
    }
    Ok(buffer)
}

/// Records leave this machine only while the person has agreed (consent on file for the current
/// notice) and recording is on; otherwise nothing is registered or sent.
fn upload_permitted() -> bool {
    crate::quality_directory().is_ok_and(|quality| upload_permitted_at(&quality))
}



pub(crate) fn should_ask_on_launch(quality: &Path) -> bool {
    !quality.join(PARTICIPATION_FILE).exists()
        && ((!quality.join("consent.json").exists() && !quality.join("config.json").exists())
            || crate::quality_recording_status_at(quality).is_ok_and(|mode| mode.enabled))
}

pub(crate) fn participation_since(quality: &Path) -> Option<i64> {
    let data = crate::read_regular_local_file(&quality.join(PARTICIPATION_FILE), "测试上传选择").ok()??;
    if data.len() > 4096 { return None; }
    let choice: crate::QualityRecordingConsent = serde_json::from_slice(&data).ok()?;
    (choice.granted && choice.notice_version == PARTICIPATION_NOTICE && choice.granted_at_ms > 0)
        .then_some(choice.granted_at_ms)
}

pub(crate) fn write_participation(quality: &Path, agreed: bool, now: i64) -> Result<(), String> {
    let choice = crate::QualityRecordingConsent {
        granted: agreed, granted_at_ms: now, notice_version: PARTICIPATION_NOTICE.into(),
    };
    let data = serde_json::to_vec(&choice).map_err(|error| error.to_string())?;
    crate::atomic_write_quality_file(quality, PARTICIPATION_FILE, &data, "测试上传选择")
}

pub(crate) fn set_participation(quality: &Path, agreed: bool, now: i64) -> Result<(), String> {
    if agreed && !crate::quality_recording_consent_status_at(quality)?.granted {
        crate::grant_quality_recording_consent_at(quality, now)?;
    }
    let _lock = crate::acquire_quality_lock(quality)?;
    write_participation(quality, agreed, now)
}

fn upload_permitted_at(quality: &Path) -> bool {
    participation_since(quality).is_some()
        && crate::quality_recording_consent_status_at(quality).is_ok_and(|choice| choice.granted)
        && crate::quality_recording_status_at(quality).is_ok_and(|mode| mode.enabled
            && (mode.continuous || crate::current_time_ms().is_ok_and(|now| now < mode.expires_at_ms)))
}

fn permitted_records(chunk: &[u8], since: i64) -> Vec<u8> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Receipt { #[serde(rename = "recordedAtMs", alias = "timestampMs")] timestamp_ms: i64 }
    let mut body = Vec::with_capacity(chunk.len());
    for line in chunk.split_inclusive(|c| *c == b'\n') {
        if serde_json::from_slice::<Receipt>(line).is_ok_and(|event| event.timestamp_ms >= since) {
            #[cfg(feature = "test-upload")]
            body.extend_from_slice(line);
            #[cfg(not(feature = "test-upload"))]
            if let Ok(mut record) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(line) {
                record.retain(|key, _| matches!(key.as_str(), "id" | "recordedAtMs" | "build" | "outcome"
                    | "pinyin" | "candidates" | "selectedIndex" | "committedText" | "backspaces"
                    | "maxPage" | "redoAfterCommit" | "durationMs"));
                if record.contains_key("id") {
                    serde_json::to_writer(&mut body, &record).expect("writing to Vec cannot fail");
                    body.push(b'\n');
                }
            }
        }
    }
    body
}

async fn upload() -> Result<Totals, String> {
    if !upload_permitted() { return Ok(Totals::default()); }
    let quality = crate::quality_directory()?;
    let progress = Arc::new(Mutex::new(Totals::default()));
    monitored_upload(quality, progress.clone(), upload_inner(progress)).await
}

// Cancelling the parent must also stop its watchdog and in-flight request.
struct UploadWatch {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
    abort: Box<dyn Fn() + Send>,
}
impl Drop for UploadWatch {
    fn drop(&mut self) {
        (self.abort)();
        *self.stop.0.lock().unwrap_or_else(|error| error.into_inner()) = true;
        self.stop.1.notify_all();
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

async fn monitored_upload(
    quality: PathBuf,
    progress: Arc<Mutex<Totals>>,
    work: impl std::future::Future<Output = Result<Totals, String>> + Send + 'static,
) -> Result<Totals, String> {
    if !upload_permitted_at(&quality) { return Ok(Totals { cancelled: true, ..Default::default() }); }
    let task = tauri::async_runtime::spawn(work);
    let abort = task.inner().abort_handle();
    let abort_on_drop = abort.clone();
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let thread_stop = stop.clone();
    let thread_quality = quality.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = cancelled.clone();
    let thread = std::thread::spawn(move || loop {
        if !upload_permitted_at(&thread_quality) {
            thread_cancelled.store(true, Ordering::Release);
            abort.abort();
            break;
        }
        let stopped = thread_stop.0.lock().unwrap_or_else(|error| error.into_inner());
        if *stopped { break; }
        let (stopped, _) = thread_stop.1.wait_timeout(stopped, std::time::Duration::from_millis(50))
            .unwrap_or_else(|error| error.into_inner());
        if *stopped { break; }
    });
    let watch = UploadWatch { stop, thread: Some(thread), abort: Box::new(move || abort_on_drop.abort()) };
    let result = task.await;
    drop(watch);
    if cancelled.load(Ordering::Acquire) || !upload_permitted_at(&quality) {
        let mut totals = progress.lock().unwrap_or_else(|error| error.into_inner()).clone();
        totals.cancelled = true;
        return Ok(totals);
    }
    result.map_err(|error| error.to_string())?
}

async fn upload_inner(progress: Arc<Mutex<Totals>>) -> Result<Totals, String> {
    if !upload_permitted() {
        return Ok(Totals::default());
    }
    let directory = upload_directory()?;
    let events = support_directory()?.join("Quality/events.jsonl");
    let client = client()?;
    let credentials = credentials(&directory, &client).await?;
    upload_chunks(crate::quality_directory()?, directory, events, client, credentials, BASE_URL.into(), progress).await
}

async fn upload_chunks(
    quality: PathBuf, directory: PathBuf, events: PathBuf, client: reqwest::Client,
    credentials: Credentials, base_url: String, progress: Arc<Mutex<Totals>>,
) -> Result<Totals, String> {
    let state_path = directory.join("state.json");
    let mut state = read_json::<State>(&state_path).unwrap_or_default();
    let length = fs::metadata(&events).map(|metadata| metadata.len()).unwrap_or(0);
    if length < state.offset {
        // The record file was cleared or replaced; start over (the server deduplicates).
        state.offset = 0;
    }
    let mut totals = Totals::default();
    loop {
        if !upload_permitted_at(&quality) { totals.cancelled = true; break; }
        let chunk = next_chunk(&events, state.offset).unwrap_or_default();
        if chunk.is_empty() {
            break;
        }
        let Some(since) = participation_since(&quality) else { totals.cancelled = true; break; };
        let body = permitted_records(&chunk, since);
        if !upload_permitted_at(&quality) { totals.cancelled = true; break; }
        if body.is_empty() {
            state.offset += chunk.len() as u64;
            write_private(&state_path, &serde_json::to_vec(&state).map_err(|error| error.to_string())?)?;
            continue;
        }
        let response = client
            .post(format!("{base_url}/events"))
            .bearer_auth(&credentials.token)
            .header("Content-Type", "application/x-ndjson")
            .body(body)
            .send()
            .await
            .map_err(|error| format!("上传失败：{error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("上传失败：HTTP {}", status.as_u16()));
        }
        let result: Totals = response.json().await.map_err(|error| error.to_string())?;
        totals.accepted += result.accepted;
        totals.duplicates += result.duplicates;
        totals.rejected += result.rejected;
        *progress.lock().unwrap_or_else(|error| error.into_inner()) = totals.clone();
        state.offset += chunk.len() as u64;
        write_private(&state_path, &serde_json::to_vec(&state).map_err(|error| error.to_string())?)?;
    }
    *progress.lock().unwrap_or_else(|error| error.into_inner()) = totals.clone();
    Ok(totals)
}

/// Headless entry point; returns the process exit code.
pub fn run_headless() -> i32 {
    if !upload_permitted() { return 0; }
    let outcome = tauri::async_runtime::block_on(upload());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let report = match &outcome {
        Ok(totals) => serde_json::json!({
            "atMs": now, "ok": true, "accepted": totals.accepted,
            "duplicates": totals.duplicates, "rejected": totals.rejected, "cancelled": totals.cancelled,
        }),
        Err(error) => serde_json::json!({ "atMs": now, "ok": false, "error": error }),
    };
    if let Ok(directory) = upload_directory() {
        let _ = write_private(&directory.join("last.json"), report.to_string().as_bytes());
    }
    i32::from(outcome.is_err())
}

/// Installs (or refreshes) the hourly LaunchAgent pointing at this binary.
pub fn ensure_scheduled() {
    if let Err(error) = schedule_upload() { eprintln!("scheduling quality uploads failed: {error}"); }
}

pub(crate) fn schedule_upload() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().map_err(|err| err.to_string())?;
        let home = std::env::var_os("HOME").ok_or("无法确定用户目录。")?;
        let agents = PathBuf::from(home).join("Library/LaunchAgents");
        let path = agents.join(format!("{LAUNCH_AGENT_LABEL}.plist"));
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCH_AGENT_LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>{FLAG}</string></array>
  <key>StartInterval</key><integer>{INTERVAL_SECONDS}</integer>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
</dict>
</plist>
"#,
            xml_escape(&executable.to_string_lossy())
        );
        let domain = format!("gui/{}", unsafe { libc_getuid() });
        let service = format!("{domain}/{LAUNCH_AGENT_LABEL}");
        if fs::read_to_string(&path).ok().as_deref() == Some(plist.as_str()) &&
            std::process::Command::new("/bin/launchctl").args(["print", &service])
                .output().is_ok_and(|status| status.status.success()) {
            return Ok(());
        }
        fs::create_dir_all(&agents).map_err(|error| error.to_string())?;
        write_private(&path, plist.as_bytes())?;
        let _ = std::process::Command::new("/bin/launchctl").args(["bootout", &service]).output();
        let result = std::process::Command::new("/bin/launchctl")
            .args(["bootstrap", &domain, &path.to_string_lossy()]).output().map_err(|err| err.to_string())?;
        if !result.status.success() { return Err("无法启用定时上传，请重新加入计划。".into()); }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
unsafe fn libc_getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UploadFixture(PathBuf);
    impl UploadFixture {
        fn new() -> Self {
            let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let root=std::env::temp_dir().join(format!("upload-cancel-{}-{stamp}",std::process::id()));
            fs::create_dir_all(root.join("Quality")).unwrap();fs::create_dir_all(root.join("Upload")).unwrap();
            fs::write(root.join("Quality/config.json"),br#"{"enabled":true,"continuous":true,"expiresAtMs":0}"#).unwrap();
            fs::write(root.join("Quality/consent.json"),format!(r#"{{"granted":true,"grantedAtMs":1,"noticeVersion":"{}"}}"#,crate::QUALITY_CONSENT_NOTICE_VERSION)).unwrap();
            write_participation(&root.join("Quality"), true, 1).unwrap();
            let line=b"{\"id\":\"synthetic\",\"recordedAtMs\":2}\n";
            fs::write(root.join("Quality/events.jsonl"),line.repeat(CHUNK_BYTES/line.len()+8)).unwrap();
            Self(root)
        }
        fn quality(&self)->PathBuf {self.0.join("Quality")}
        fn directory(&self)->PathBuf {self.0.join("Upload")}
    }
    impl Drop for UploadFixture {fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
    fn withdraw(quality:&Path){fs::write(quality.join("config.json"),br#"{"enabled":false,"continuous":false,"expiresAtMs":0}"#).unwrap();}
    fn receive_request(stream:&mut std::net::TcpStream)->u64 {
        use std::io::BufRead;
        let mut input=std::io::BufReader::new(stream);let mut length=0;
        loop {
            let mut line=String::new();assert!(input.read_line(&mut line).unwrap()>0);
            if line=="\r\n" {break;}
            if let Some((key,value))=line.split_once(':') {
                if key.eq_ignore_ascii_case("content-length"){length=value.trim().parse::<u64>().unwrap();}
            }
        }
        assert!(length>0);
        assert_eq!(std::io::copy(&mut input.take(length),&mut std::io::sink()).unwrap(),length);length
    }
    fn test_client()->reqwest::Client {
        reqwest::Client::builder().no_proxy().http1_only().timeout(std::time::Duration::from_secs(4)).build().unwrap()
    }
    fn test_credentials()->Credentials {Credentials{device_id:"synthetic".into(),token:"local-test-only".into()}}

    #[test]
    fn withdrawal_stops_before_the_next_chunk_and_preserves_acknowledged_offset() {
        let fixture=UploadFixture::new();let quality=fixture.quality();let directory=fixture.directory();
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let finished=Arc::new(AtomicBool::new(false));let server_finished=finished.clone();let server_quality=quality.clone();
        let server=std::thread::spawn(move||{
            let started=std::time::Instant::now();let mut requests=Vec::new();
            while !server_finished.load(Ordering::Acquire)&&started.elapsed()<std::time::Duration::from_secs(5) {
                match listener.accept(){
                    Ok((mut stream,_))=>{
                        // Darwin inherits the listener's nonblocking flag on accept.
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
                        requests.push(receive_request(&mut stream));withdraw(&server_quality);
                        let body=r#"{"accepted":1,"duplicates":0,"rejected":0}"#;
                        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                    }
                    Err(error)if error.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(std::time::Duration::from_millis(5)),
                    Err(error)=>panic!("{error}"),
                }
            } requests
        });
        let progress=Arc::new(Mutex::new(Totals::default()));
        let result=tauri::async_runtime::block_on(upload_chunks(quality.clone(),directory.clone(),quality.join("events.jsonl"),test_client(),test_credentials(),base,progress));
        finished.store(true,Ordering::Release);let requests=server.join().unwrap();let totals=result.unwrap();
        assert_eq!(requests.len(),1);assert_eq!(totals.accepted,1);assert!(totals.cancelled);
        let state:State=read_json(&directory.join("state.json")).unwrap();assert_eq!(state.offset,requests[0]);
        assert!(state.offset<fs::metadata(quality.join("events.jsonl")).unwrap().len());
    }

    #[test]
    fn withdrawal_cancels_a_pending_http_response_without_advancing_offset() {
        let fixture=UploadFixture::new();let quality=fixture.quality();let directory=fixture.directory();
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let base=format!("http://{}",listener.local_addr().unwrap());
        let finished=Arc::new(AtomicBool::new(false));let server_finished=finished.clone();let server_quality=quality.clone();
        let server=std::thread::spawn(move||{
            let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            receive_request(&mut stream);withdraw(&server_quality);
            let started=std::time::Instant::now();
            while !server_finished.load(Ordering::Acquire)&&started.elapsed()<std::time::Duration::from_secs(5){std::thread::sleep(std::time::Duration::from_millis(5));}
        });
        let progress=Arc::new(Mutex::new(Totals::default()));let started=std::time::Instant::now();
        let work=upload_chunks(quality.clone(),directory.clone(),quality.join("events.jsonl"),test_client(),test_credentials(),base,progress.clone());
        let result=tauri::async_runtime::block_on(monitored_upload(quality,progress,work));
        finished.store(true,Ordering::Release);server.join().unwrap();let totals=result.unwrap();
        assert!(started.elapsed()<std::time::Duration::from_secs(2));assert!(totals.cancelled);assert_eq!(totals.accepted,0);
        assert_eq!(read_json::<State>(&directory.join("state.json")).unwrap_or_default().offset,0);
    }

    #[test]
    fn nothing_uploads_without_consent_and_recording_on() {
        let directory = std::env::temp_dir().join(format!("aim-test-consent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        assert!(!upload_permitted_at(&directory), "no choice made");
        fs::write(directory.join("config.json"), br#"{"enabled":true,"expiresAtMs":0,"continuous":true}"#).unwrap();
        assert!(!upload_permitted_at(&directory), "recording on but no consent");
        fs::write(
            directory.join("consent.json"),
            format!(r#"{{"granted":true,"grantedAtMs":1,"noticeVersion":"{}"}}"#, crate::QUALITY_CONSENT_NOTICE_VERSION),
        )
        .unwrap();
        assert!(!upload_permitted_at(&directory), "local consent alone");
        write_participation(&directory, false, 2).unwrap();
        assert!(!should_ask_on_launch(&directory));
        assert!(!upload_permitted_at(&directory));
        set_participation(&directory, true, 3).unwrap();
        assert!(upload_permitted_at(&directory), "explicit upload and recording on");
        #[cfg(feature = "test-upload")]
        let mixed = b"{\"timestampMs\":2}\n{\"timestampMs\":3}\n{\"id\":\"legacy\"}\n";
        #[cfg(feature = "test-upload")]
        assert_eq!(permitted_records(mixed, 3), b"{\"timestampMs\":3}\n");
        fs::write(directory.join("config.json"), br#"{"enabled":true,"expiresAtMs":100,"continuous":false}"#).unwrap();
        assert!(!upload_permitted_at(&directory), "timed recording has expired");
        fs::write(directory.join("config.json"), br#"{"enabled":false,"expiresAtMs":0,"continuous":false}"#).unwrap();
        assert!(!upload_permitted_at(&directory), "switched off");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(feature = "test-upload"))]
    #[test]
    fn public_plan_filters_old_rows_and_private_fields() {
        let input = br#"{"id":"old","recordedAtMs":1}
{"id":"new","recordedAtMs":3,"pinyin":"nihao","device":"private","precedingText":"private"}
"#;
        let row: serde_json::Value = serde_json::from_slice(&permitted_records(input, 2)).unwrap();
        assert_eq!(row, serde_json::json!({"id":"new","recordedAtMs":3,"pinyin":"nihao"}));
    }

    #[test]
    fn chunks_stop_at_complete_lines() {
        let directory = std::env::temp_dir().join(format!("aim-test-upload-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let events = directory.join("events.jsonl");
        fs::write(&events, b"{\"id\":\"a\"}\n{\"id\":\"b\"}\n{\"id\":\"c\"").unwrap();
        assert_eq!(next_chunk(&events, 0).unwrap(), b"{\"id\":\"a\"}\n{\"id\":\"b\"}\n");
        assert_eq!(next_chunk(&events, 22).unwrap(), b"");
        fs::remove_dir_all(directory).unwrap();
    }
}
