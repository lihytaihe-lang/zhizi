//! Worker-owned 知句 search. The worker never borrows the live Host or session.
use agent_ime_core::*;
use std::{ffi::c_void, path::Path, sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc::{channel, Sender, Receiver}}, time::{Duration, Instant}};

pub const BUDGET: Duration = Duration::from_millis(40);

#[derive(Clone, Debug, Default)]
pub struct Observation {
    pub outcome: &'static str,
    pub latency_ms: f64,
    pub callbacks: usize,
    pub reused_prefix_rows: usize,
}

/// Full next-character log probabilities for a right-padded [rows,40] request.
pub trait Distribution: Send {
    fn reset(&mut self) {}
    /// Retain a newly computed prefix only after the caller checks generation and deadline.
    fn commit_prefixes(&mut self) {}
    fn clear(&mut self) { self.reset(); }
    fn reused_prefix_rows(&self) -> usize { 0 }
    fn score(&mut self, tokens: &[i32], rows: usize, cols: usize, vocab: usize) -> Result<Vec<f32>, String>;
}

pub(crate) struct Snapshot(pub *mut AimZhijuSnapshot);
// The core snapshot owns immutable dictionary data and independent search state.
// Ownership moves to one worker, then back to the shell after its final step.
// It is never shared concurrently and never accesses the originating session.
unsafe impl Send for Snapshot {}
impl Drop for Snapshot { fn drop(&mut self) { unsafe { aim_zhiju_snapshot_destroy(self.0); } } }

pub(crate) struct Job {
    pub generation: u64,
    pub snapshot: Snapshot,
    pub original: super::View,
    pub context: String,
}
pub(crate) struct Done {
    pub job: Job,
    pub candidates: Vec<super::zhiju::Candidate>,
    pub elapsed: Duration,
    pub observation: Observation,
}

struct Search(*mut AimZhijuSearch);
impl Drop for Search { fn drop(&mut self) { unsafe { aim_zhiju_search_destroy(self.0); } } }
struct Request { tokens: Vec<i32>, rows: usize, cols: usize, vocab: usize }
unsafe extern "C" fn request(data: *mut c_void, tokens: *const i32, rows: usize, cols: usize, vocab: usize) {
    if data.is_null() || tokens.is_null() || !(1..=16).contains(&rows) || cols != 40 || vocab < 3 { return; }
    let slot = unsafe { &mut *data.cast::<Option<Request>>() };
    *slot = Some(Request { tokens: unsafe { std::slice::from_raw_parts(tokens, rows * cols) }.to_vec(), rows, cols, vocab });
}

pub(crate) struct Worker {
    jobs: Sender<Message>,
    done: Receiver<Done>,
    generation: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
}
enum Message { Search(Job), Clear }
impl Worker {
    pub fn start(vocabulary: &Path, mut model: Box<dyn Distribution>, notify: Option<Box<dyn Fn() + Send>>) -> Result<Self, String> {
        let vocabulary = super::path_bytes(vocabulary)?;
        let (jobs, receiver) = channel::<Message>();
        let (sender, done) = channel();
        let (ready_tx, ready_rx) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        let epoch = Arc::new(AtomicU64::new(0));
        let current_epoch = epoch.clone();
        std::thread::Builder::new().name("zhizi-zhiju".into()).spawn(move || {
            let search = Search(unsafe { aim_zhiju_search_create(vocabulary.as_ptr(), vocabulary.len()) });
            if search.0.is_null() { let _ = ready_tx.send(Err("知句词表不可用".to_string())); return; }
            let _ = ready_tx.send(Ok(()));
            let mut model_epoch = 0;
            while let Ok(message) = receiver.recv() {
                let mut job = match message {
                    Message::Search(job) => job,
                    Message::Clear => { model.clear(); model_epoch = current_epoch.load(Ordering::Acquire); continue; }
                };
                while let Ok(newer) = receiver.try_recv() {
                    match newer {
                        Message::Search(newer) => job = newer,
                        Message::Clear => { model.clear(); model_epoch = current_epoch.load(Ordering::Acquire); }
                    }
                }
                let epoch = current_epoch.load(Ordering::Acquire);
                if epoch != model_epoch { model.clear(); model_epoch = epoch; }
                let valid = || current.load(Ordering::Acquire) == job.generation;
                if !valid() { continue; }
                if unsafe { aim_zhiju_search_prepare(search.0, job.snapshot.0) } != 0 || !valid() { continue; }
                model.reset();
                let started = Instant::now();
                let mut scores = Vec::<f32>::new();
                let mut candidates = Vec::new();
                let mut outcome = "unavailable";
                let mut callbacks = 0;
                for _ in 0..=16 {
                    if !valid() || started.elapsed() >= BUDGET { break; }
                    let remaining = BUDGET.saturating_sub(started.elapsed());
                    let budget_ms = remaining.as_micros().div_ceil(1000) as u64;
                    let mut pending = None::<Request>;
                    let mut buffer = vec![0u8; 8 * 256];
                    let mut length = buffer.len();
                    let status = unsafe { aim_zhiju_snapshot_step(job.snapshot.0, search.0, Some(request),
                        (&mut pending as *mut Option<Request>).cast(), 8, budget_ms,
                        scores.as_ptr(), scores.len(), buffer.as_mut_ptr(), &mut length) };
                    if !valid() || started.elapsed() >= BUDGET { break; }
                    if status == 1 {
                        let Some(request) = pending else { break; };
                        callbacks += 1;
                        match model.score(&request.tokens, request.rows, request.cols, request.vocab) {
                            Ok(result) if result.len() == request.rows * request.vocab && result.iter().all(|v| v.is_finite()) => {
                                if !valid() || started.elapsed() >= BUDGET { break; }
                                model.commit_prefixes();
                                scores = result;
                            },
                            _ => { outcome = "model_error"; break; },
                        }
                    } else {
                        if status == 0 && length <= buffer.len() {
                            candidates = super::zhiju::decode(&buffer[..length]).unwrap_or_default();
                            outcome = if candidates.is_empty() { "empty" } else { "searched" };
                        }
                        break;
                    }
                }
                let elapsed = started.elapsed();
                if !valid() {
                    if current_epoch.load(Ordering::Acquire) != model_epoch { model.clear(); }
                    continue;
                }
                if elapsed >= BUDGET { candidates.clear(); outcome = "timeout"; }
                let observation = Observation { outcome, latency_ms: elapsed.as_secs_f64()*1000.0, callbacks,
                    reused_prefix_rows: model.reused_prefix_rows() };
                if sender.send(Done { job, candidates, elapsed, observation }).is_err() { break; }
                if let Some(notify) = &notify { notify(); }
            }
        }).map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Self { jobs, done, generation, epoch })
    }
    pub fn advance(&self, generation: u64) { self.generation.store(generation, Ordering::Release); }
    pub fn clear(&self) { self.epoch.fetch_add(1, Ordering::AcqRel); let _ = self.jobs.send(Message::Clear); }
    pub fn submit(&self, job: Job) { let _ = self.jobs.send(Message::Search(job)); }
    pub fn take(&self, generation: u64) -> Option<Done> {
        let mut result = None;
        while let Ok(done) = self.done.try_recv() { if done.job.generation == generation { result = Some(done); } }
        result
    }
}
impl Drop for Worker { fn drop(&mut self) { self.advance(u64::MAX); } }
