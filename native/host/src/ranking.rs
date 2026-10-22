//! 顺手·知字 on top of the core. The decisions (C3 compositions, C2/C1 group order, the
//! owner's habits) live in the core (`agent_ime_core::ranking`), shared with macOS; this
//! module runs the model.
//!
//! Scoring runs on a worker thread. A key waits up to a short budget for it; a later
//! result is applied when the shell polls, if no key arrived in between.

pub use agent_ime_core::ranking::{
    COMPOSITION_COMMENT, COMPOSITION_LEAD_MARGIN, COMPOSITION_OFFER_COMMENT, EDIT_PENALTY, HEAVY_GARBLE_COMMENT,
    OwnerStyle, blended_order, composition_lead, composition_margin, composition_offer, edit_penalty, group_order,
    model_first_order,
};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

/// Log probability of each candidate after the context, summed over the candidate's
/// characters (the same protocol as eval_owner.ZhiziScorer and the macOS scorer).
pub trait Scorer: Send {
    fn score(&mut self, context: &str, candidates: &[String]) -> Result<Vec<f64>, String>;
}

/// What one key asks the model.
pub(crate) struct Job {
    pub generation: u64,
    pub context: String,
    /// Decoder's first candidate followed by the core's whole-reading compositions.
    pub composition: Vec<String>,
    /// Candidates covering the whole input (texts, in page order).
    pub group: Vec<String>,
}

pub(crate) struct Done {
    pub generation: u64,
    pub composition_scores: Option<Vec<f64>>,
    pub group_scores: Option<Vec<f64>>,
    pub elapsed: Duration,
}

/// The worker thread that owns the scorer.
pub(crate) struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl Worker {
    pub fn start(mut scorer: Box<dyn Scorer>, notify: Option<Box<dyn Fn() + Send>>) -> Self {
        let (jobs, job_receiver) = channel::<Job>();
        let (done_sender, done) = channel::<Done>();
        std::thread::Builder::new()
            .name("zhizi-model".into())
            .spawn(move || {
                while let Ok(mut job) = job_receiver.recv() {
                    // Only the newest key matters.
                    while let Ok(newer) = job_receiver.try_recv() {
                        job = newer;
                    }
                    let started = Instant::now();
                    let composition_scores = (job.composition.len() >= 2)
                        .then(|| scorer.score(&job.context, &job.composition).ok())
                        .flatten()
                        .filter(|scores| scores.len() == job.composition.len() && scores.iter().all(|s| s.is_finite()));
                    let group_scores = (job.group.len() >= 2)
                        .then(|| scorer.score(&job.context, &job.group).ok())
                        .flatten()
                        .filter(|scores| scores.len() == job.group.len() && scores.iter().all(|s| s.is_finite()));
                    let finished = Done {
                        generation: job.generation,
                        composition_scores,
                        group_scores,
                        elapsed: started.elapsed(),
                    };
                    if done_sender.send(finished).is_err() {
                        break;
                    }
                    if let Some(notify) = &notify {
                        notify();
                    }
                }
            })
            .expect("model thread");
        Self { jobs, done }
    }

    pub fn submit(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Waits up to `budget` for the result of `generation`, dropping older ones.
    pub fn wait(&self, generation: u64, budget: Duration) -> Option<Done> {
        let deadline = Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.done.recv_timeout(remaining) {
                Ok(done) if done.generation == generation => return Some(done),
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    pub fn try_take(&self, generation: u64) -> Option<Done> {
        let mut found = None;
        while let Ok(done) = self.done.try_recv() {
            if done.generation == generation {
                found = Some(done);
            }
        }
        found
    }
}

#[cfg(feature = "model")]
pub mod onnx {
    //! ONNX Runtime scorer for the exported 顺手·知字 (dynamic batch and length;
    //! scripts/context_model/zhizi/export_onnx.py).
    use super::Scorer;
    use ort::session::Session;
    use ort::value::Tensor;
    use std::collections::HashMap;
    use std::path::Path;

    pub struct OnnxScorer {
        session: Session,
        vocabulary: HashMap<String, i32>,
    }

    /// Loads the onnxruntime library once per process.
    pub fn init_runtime(library: &Path) -> Result<(), String> {
        static INIT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        INIT.get_or_init(|| {
            ort::init_from(library)
                .map_err(|error| error.to_string())?
                .with_name("zhizi")
                .commit();
            Ok(())
        })
        .clone()
    }

    impl OnnxScorer {
        pub fn load(model: &Path, itos: &Path, threads: usize) -> Result<Self, String> {
            let itos: Vec<String> = serde_json::from_slice(&std::fs::read(itos).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if itos.get(..3) != Some(&["<pad>".to_string(), "<bos>".to_string(), "<unk>".to_string()][..]) {
                return Err("itos.json is not the 顺手·知字 vocabulary".into());
            }
            let vocabulary = itos.into_iter().enumerate().map(|(i, s)| (s, i as i32)).collect();
            let session = Session::builder()
                .map_err(|e| e.to_string())?
                .with_intra_threads(threads)
                .map_err(|e| e.to_string())?
                .commit_from_file(model)
                .map_err(|e| e.to_string())?;
            Ok(Self { session, vocabulary })
        }

        fn token(&self, character: char) -> i32 {
            self.vocabulary.get(character.encode_utf8(&mut [0; 4]) as &str).copied().unwrap_or(2)
        }
    }

    impl Scorer for OnnxScorer {
        fn score(&mut self, context: &str, candidates: &[String]) -> Result<Vec<f64>, String> {
            let longest = candidates.iter().map(|c| c.chars().count()).max().unwrap_or(0);
            if candidates.is_empty() || longest == 0 || longest > 39 {
                return Err("candidates out of range".into());
            }
            // One context window for every candidate (sized by the longest), scored
            // eight rows at a time like the macOS model, so the scores stay comparable.
            let context: Vec<char> = context.chars().collect();
            let keep = context.len().min(39 - longest);
            let prefix: Vec<i32> = std::iter::once(1)
                .chain(context[context.len() - keep..].iter().map(|&c| self.token(c)))
                .collect();
            let mut scores = Vec::with_capacity(candidates.len());
            for chunk in candidates.chunks(8) {
                let chunk_longest = chunk.iter().map(|c| c.chars().count()).max().unwrap_or(0);
                let length = prefix.len() + chunk_longest;
                let rows = chunk.len();
                let mut tokens = vec![0i32; rows * length];
                for (row, text) in chunk.iter().enumerate() {
                    for (column, token) in prefix.iter().copied().chain(text.chars().map(|c| self.token(c))).enumerate() {
                        tokens[row * length + column] = token;
                    }
                }
                let input = Tensor::from_array(([rows, length], tokens)).map_err(|e| e.to_string())?;
                let outputs = self.session.run(ort::inputs!["tokens" => input]).map_err(|e| e.to_string())?;
                let (_, logprobs) = outputs["logprobs"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
                let width = length - 1;
                let start = prefix.len() - 1;
                for (row, text) in chunk.iter().enumerate() {
                    let count = text.chars().count();
                    let score: f64 = (start..start + count).map(|i| logprobs[row * width + i] as f64).sum();
                    if !score.is_finite() {
                        return Err("non-finite score".into());
                    }
                    scores.push(score);
                }
            }
            Ok(scores)
        }
    }
}
