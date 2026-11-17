//! Training-data legality only. A forced scorer proves a dictionary/pinyin path;
//! its output is never a model quality score or a product candidate.
use agent_ime_core::*;
use serde::Deserialize;
use std::{collections::HashMap, ffi::c_void, io::{BufRead, Write}, path::Path};

#[derive(Deserialize)]
struct Row { id: String, raw: String, context: String, target: String }
struct Forced { expected: Vec<i32> }
unsafe extern "C" fn force(user: *mut c_void, tokens: *const i32, rows: usize,
    cols: usize, out: *mut f32, vocab: usize) -> i32 {
    if user.is_null() || tokens.is_null() || out.is_null() || cols != 40 { return -1; }
    let state = unsafe { &*user.cast::<Forced>() };
    let tokens = unsafe { std::slice::from_raw_parts(tokens, rows * cols) };
    let output = unsafe { std::slice::from_raw_parts_mut(out, rows * vocab) };
    output.fill(-1000.0);
    for (prefix, scores) in tokens.chunks(cols).zip(output.chunks_mut(vocab)) {
        let n = prefix.iter().take_while(|&&v| v != 0).count();
        if n < state.expected.len() && state.expected.starts_with(&prefix[..n]) {
            let id = state.expected[n] as usize;
            if id >= vocab { return -1; }
            scores[id] = 100.0;
        }
    }
    0
}
unsafe fn key(session: *mut AimSession, kind: u32, text: &[u8]) -> Result<(), String> {
    let event = AimKeyEvent { kind, text: text.as_ptr(), text_len: text.len(), modifiers: 0, security_state: 0, delivery_id: 0 };
    let mut result = std::mem::MaybeUninit::<AimResult>::uninit();
    let status = unsafe { aim_session_process(session, &event, result.as_mut_ptr()) };
    if status != 0 { return Err(format!("key status {status}")); }
    unsafe { aim_result_free(result.as_mut_ptr()) };
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 { return Err("usage: zhiju_pathcheck shared blank-user itos input.jsonl output.jsonl".into()); }
    let user = Path::new(&args[1]);
    if user.exists() && std::fs::read_dir(user)?.next().is_some() { return Err("requires empty user dir".into()); }
    std::fs::create_dir_all(user)?;
    let symbols: Vec<String> = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let ids: HashMap<char,i32> = symbols.iter().enumerate().filter_map(|(i,s)| {
        let mut c = s.chars(); let first = c.next()?; c.next().is_none().then_some((first,i as i32))
    }).collect();
    let app = b"zhizi.pathcheck";
    let session = unsafe { aim_session_create(args[0].as_ptr(), args[0].len(), args[1].as_ptr(), args[1].len(), app.as_ptr(), app.len()) };
    if session.is_null() { return Err("session failed".into()); }
    struct Session(*mut AimSession);
    impl Drop for Session { fn drop(&mut self) { unsafe { aim_session_destroy(self.0) } } }
    let _guard = Session(session);
    let status = unsafe { aim_session_set_zhiju_vocab(session,args[2].as_ptr(),args[2].len()) };
    if status != 0 { return Err(format!("vocab status {status}").into()); }
    let input = std::io::BufReader::new(std::fs::File::open(&args[3])?);
    let mut output = std::io::BufWriter::new(std::fs::OpenOptions::new().write(true).create_new(true).open(&args[4])?);
    let mut count = 0; let mut reachable = 0;
    for line in input.lines() {
        let row: Row = serde_json::from_str(&line?)?;
        if row.id.is_empty() || row.target.is_empty() || !row.raw.bytes().all(|b| b.is_ascii_lowercase())
            || row.context.chars().count() + row.target.chars().count() > 38 { return Err("invalid data row".into()); }
        unsafe { key(session,6,b"")? };
        let status = unsafe { aim_session_set_preceding_text(session,row.context.as_ptr(),row.context.len()) };
        if status != 0 { return Err(format!("context status {status}").into()); }
        for ch in row.raw.bytes() { unsafe { key(session,0,&[ch])? } }
        let mut expected = vec![1];
        for ch in row.context.chars().chain(row.target.chars()) { expected.push(*ids.get(&ch).ok_or("unknown symbol")?); }
        let mut forced = Forced { expected };
        let mut bytes = vec![0u8;4096]; let mut length = bytes.len();
        let status = unsafe { aim_session_zhiju_search(session,Some(force),(&mut forced as *mut Forced).cast(),8,1000,bytes.as_mut_ptr(),&mut length) };
        if ![0,-3].contains(&status) { return Err(format!("search status {status}").into()); }
        let text = std::str::from_utf8(&bytes[..length])?;
        let found = text.lines().any(|l| l.split('\t').next() == Some(&row.target));
        count += 1; reachable += usize::from(found);
        writeln!(output,"{}",serde_json::json!({"id":row.id,"reachable":found,"status":status,"scorer":"forced-path-only-not-a-model"}))?;
        if count % 2000 == 0 { output.flush()?; eprintln!("checked={count} reachable={reachable}"); }
    }
    eprintln!("pathcheck complete checked={count} reachable={reachable}");
    Ok(())
}
