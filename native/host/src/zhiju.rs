//! Explicit offline 知句 capability. Nothing here schedules search or changes a page.

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub score: f64,
    pub consumed: usize,
}

pub(crate) fn decode(encoded: &[u8]) -> Result<Vec<Candidate>, String> {
    let encoded = std::str::from_utf8(encoded).map_err(|e| e.to_string())?;
    encoded.lines().map(|line| {
        let mut fields = line.split('\t');
        let text = fields.next().filter(|text| !text.is_empty()).ok_or("empty 知句 text")?;
        let score = fields.next().and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite()).ok_or("invalid 知句 score")?;
        let consumed = fields.next().and_then(|value| value.parse().ok()).ok_or("invalid 知句 consumption")?;
        if fields.next().is_some() { return Err("extra 知句 field".into()); }
        Ok(Candidate { text: text.into(), score, consumed })
    }).collect()
}

impl super::Host {
    pub fn set_zhiju_vocab(&mut self, path: &std::path::Path) -> Result<(), String> {
        let path = super::path_bytes(path)?;
        let status = unsafe { agent_ime_core::aim_session_set_zhiju_vocab(self.session, path.as_ptr(), path.len()) };
        if status == 0 { Ok(()) } else { Err(format!("load 知句 vocabulary: {status}")) }
    }

    /// Explicit synchronous search, for offline use or a caller-owned worker.
    /// Status codes are the core ABI's codes (not applicable = -3).
    #[cfg(feature = "model")]
    pub fn zhiju_search(&mut self, model: &mut onnx::OnnxDistribution, beam: usize, budget_ms: u64)
        -> Result<Vec<Candidate>, i32> {
        model.reset_stats();
        let capacity = (if beam == 0 { 8 } else { beam }).checked_mul(256).ok_or(-2)?;
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(capacity).map_err(|_| -2)?;
        buffer.resize(capacity, 0);
        let mut length = buffer.len();
        let status = unsafe { agent_ime_core::aim_session_zhiju_search(self.session, Some(onnx::scorer),
            (model as *mut onnx::OnnxDistribution).cast(), beam, budget_ms, buffer.as_mut_ptr(), &mut length) };
        if status != 0 { return Err(status); }
        decode(&buffer[..length]).map_err(|_| -5)
    }
}

#[cfg(feature = "model")]
pub mod onnx {
    use ort::session::{Session, SessionOutputs};
    use ort::value::{DynValue, Tensor};
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Instant;
    use std::collections::VecDeque;

    #[derive(Clone, Debug, Default)]
    pub struct Stats {
        pub callback_calls: usize,
        pub successful_calls: usize,
        pub run_rows: usize,
        /// Time inside ORT run(), summed over this search, excluding core search.
        pub onnx_ms: f64,
        pub cached_calls: usize,
        pub memo_rows: usize,
    }

    pub struct OnnxDistribution {
        session: Session,
        step: Option<Session>,
        cache: Option<Cache>,
        empty_context: Option<EmptyContext>,
        memo: PrefixMemo,
        pending_memo: Vec<(Vec<i32>, Vec<f32>)>,
        stats: Stats,
    }

    #[derive(Clone)]
    struct Cache {
        tokens: Vec<i32>, rows: usize, length: usize,
        keys: Arc<DynValue>, values: Arc<DynValue>,
    }

    struct EmptyContext {
        scores: Vec<f32>,
        cache: Cache,
    }

    /// At most 64 exact model prefixes, 2 MiB, confined to the current composition.
    /// No reading/key heuristics or score transformations are stored here.
    #[derive(Default)]
    struct PrefixMemo { rows: VecDeque<(Vec<i32>, Vec<f32>)>, bytes: usize }
    impl PrefixMemo {
        fn get(&mut self, tokens: &[i32]) -> Option<&[f32]> {
            let at = self.rows.iter().position(|(key, _)| key == tokens)?;
            let entry = self.rows.remove(at)?;
            self.rows.push_back(entry);
            self.rows.back().map(|(_, scores)| scores.as_slice())
        }
        fn insert(&mut self, tokens: Vec<i32>, scores: Vec<f32>) {
            let size = (tokens.len()+scores.len())*4;
            if size > 2*1024*1024 { return; }
            if let Some(at) = self.rows.iter().position(|(key, _)| key == &tokens) {
                if let Some((key, value)) = self.rows.remove(at) { self.bytes -= (key.len()+value.len())*4; }
            }
            self.rows.push_back((tokens, scores)); self.bytes += size;
            while self.rows.len()>64 || self.bytes>2*1024*1024 {
                if let Some((key, value)) = self.rows.pop_front() { self.bytes -= (key.len()+value.len())*4; }
            }
        }
    }

    /// A beam can duplicate and reorder its parents. Reuse only an exact prefix,
    /// copying caches into that same new row order; a divergent prefix prefills.
    fn parents(old: &[i32], rows: usize, length: usize, new: &[i32], new_rows: usize, new_length: usize) -> Option<Vec<usize>> {
        if new_length != length + 1 || old.len() != rows * length || new.len() != new_rows * new_length { return None; }
        new.chunks_exact(new_length).map(|row| old.chunks_exact(length).position(|p| p == &row[..length])).collect()
    }

    fn reorder_cache(data: &[f32], old_rows: usize, length: usize, parents: &[usize]) -> Result<Vec<f32>, String> {
        const LAYERS: usize = 8;
        const HEADS: usize = 8;
        const DIM: usize = 64;
        let stride = HEADS * length * DIM;
        if data.len() != LAYERS * old_rows * stride || parents.iter().any(|&p| p >= old_rows) {
            return Err("invalid 知句 cache dimensions".into());
        }
        let mut output = Vec::with_capacity(LAYERS * parents.len() * stride);
        for layer in 0..LAYERS {
            for &parent in parents {
                let offset = (layer * old_rows + parent) * stride;
                output.extend_from_slice(&data[offset..offset+stride]);
            }
        }
        Ok(output)
    }

    fn cache_input(value: Arc<DynValue>, old_rows: usize, length: usize, parents: &[usize]) -> Result<Arc<DynValue>, String> {
        if parents.len() == old_rows && parents.iter().copied().eq(0..old_rows) {
            return Ok(value);
        }
        let (_, data) = value.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let ordered = reorder_cache(data, old_rows, length, parents)?;
        Tensor::from_array(([8, parents.len(), 8, length, 64], ordered))
            .map(|tensor| Arc::new(tensor.into_dyn())).map_err(|err| err.to_string())
    }

    fn take_cache_output(
        outputs: &mut SessionOutputs<'_>, name: &str, rows: usize, length: usize,
    ) -> Result<DynValue, String> {
        let value = outputs.remove(name).ok_or_else(|| format!("missing 知句 cache output {name}"))?;
        let (shape, data) = value.try_extract_tensor::<f32>().map_err(|err| err.to_string())?;
        if shape.as_ref() != [8, rows as i64, 8, length as i64, 64] || data.iter().any(|v| !v.is_finite()) {
            return Err(format!("invalid 知句 cache output {name}"));
        }
        Ok(value)
    }

    impl OnnxDistribution {
        /// Uses the runtime initialized by ranking::onnx::init_runtime.
        pub fn load(model: &Path, threads: usize) -> Result<Self, String> {
            let session = Session::builder().map_err(|e| e.to_string())?
                .with_intra_threads(threads).map_err(|e| e.to_string())?
                .with_inter_threads(1).map_err(|e| e.to_string())?
                .commit_from_file(model).map_err(|e| e.to_string())?;
            let descriptor = model.parent().ok_or("missing model directory")?.join("model.json");
            let step = if descriptor.exists() {
                let descriptor: serde_json::Value = serde_json::from_slice(&std::fs::read(descriptor).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                if descriptor["format"] != "zhiju-onnx-cache/1" || descriptor["capacity"] != 40
                    || descriptor["layers"] != 8 || descriptor["heads"] != 8 || descriptor["headDimension"] != 64
                    || descriptor["stepModel"] != "zhiju30m_v3_step.onnx" {
                    return Err("unsupported 知句 cache model descriptor".into());
                }
                Some(Session::builder().map_err(|e| e.to_string())?.with_intra_threads(threads).map_err(|e| e.to_string())?
                    .with_inter_threads(1).map_err(|e| e.to_string())?
                    .commit_from_file(model.with_file_name("zhiju30m_v3_step.onnx")).map_err(|e| e.to_string())?)
            } else { None };
            Ok(Self { session, step, cache: None, empty_context: None, memo: PrefixMemo::default(),
                pending_memo: Vec::new(), stats: Stats::default() })
        }

        pub fn stats(&self) -> &Stats { &self.stats }

        pub fn reset_stats(&mut self) { self.stats = Stats::default(); self.cache = None; self.pending_memo.clear(); }

        fn distribution(&mut self, tokens: &[i32], rows: usize, cols: usize, out: &mut [f32], vocab: usize)
            -> Result<(), String> {
            self.stats.callback_calls += 1;
            let (length, trimmed) = trim_tokens(tokens, rows, cols, vocab)?;
            self.pending_memo.clear();
            // A single BOS token contains no document text. Its attention state
            // is reusable across compositions on this model.
            if trimmed == [1] && let Some(initial) = &self.empty_context
                && initial.scores.len() == out.len()
            {
                out.copy_from_slice(&initial.scores);
                self.cache = Some(initial.cache.clone());
                self.stats.successful_calls += 1;
                return Ok(());
            }
            let mut missing = Vec::new();
            let mut input = Vec::new();
            for (row, prefix) in trimmed.chunks_exact(length).enumerate() {
                if let Some(scores) = self.memo.get(prefix).filter(|s| s.len()==vocab) {
                    out[row*vocab..(row+1)*vocab].copy_from_slice(scores);
                    self.stats.memo_rows += 1;
                } else {
                    missing.push(row); input.extend_from_slice(&tokens[row*cols..(row+1)*cols]);
                }
            }
            if !missing.is_empty() {
                let mut scores = vec![0.0; missing.len()*vocab];
                self.compute_distribution(&input, missing.len(), cols, &mut scores, vocab)?;
                for (from, &row) in missing.iter().enumerate() {
                    let value = &scores[from*vocab..(from+1)*vocab];
                    out[row*vocab..(row+1)*vocab].copy_from_slice(value);
                    self.pending_memo.push((trimmed[row*length..(row+1)*length].to_vec(), value.to_vec()));
                }
            }
            self.stats.successful_calls += 1;
            Ok(())
        }

        fn compute_distribution(&mut self, tokens: &[i32], rows: usize, cols: usize, out: &mut [f32], vocab: usize)
            -> Result<(), String> {
            let (length, tokens) = trim_tokens(tokens, rows, cols, vocab)?;
            let prior = self.cache.take();
            let lineage = prior.as_ref().and_then(|cache| parents(&cache.tokens, cache.rows, cache.length, &tokens, rows, length));
            let cached_model = self.step.is_some();
            let started = Instant::now();
            let result = if let (Some(step), Some(cache), Some(parents)) = (&mut self.step, prior, lineage) {
                let keys = cache_input(cache.keys, cache.rows, cache.length, &parents)?;
                let values = cache_input(cache.values, cache.rows, cache.length, &parents)?;
                let last: Vec<i32> = tokens.chunks_exact(length).map(|row| row[length-1]).collect();
                let input = Tensor::from_array(([rows, 1], last)).map_err(|e| e.to_string())?;
                self.stats.cached_calls += 1;
                step.run(ort::inputs!["tokens" => input, "keys" => keys.as_ref(), "values" => values.as_ref()])
            } else {
                let input = Tensor::from_array(([rows, length], tokens.clone())).map_err(|e| e.to_string())?;
                self.session.run(ort::inputs!["tokens" => input])
            };
            self.stats.onnx_ms += started.elapsed().as_secs_f64() * 1000.0;
            let mut outputs = result.map_err(|e| e.to_string())?;
            let (shape, logprobs) = outputs.get("logprobs").ok_or("missing next-character distribution")?
                .try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            if shape.as_ref() != [rows as i64, vocab as i64] || logprobs.len() != out.len()
                || logprobs.iter().any(|value| !value.is_finite()) {
                return Err("invalid next-character distribution shape/values".into());
            }
            out.copy_from_slice(logprobs);
            if cached_model {
                let keys = Arc::new(take_cache_output(&mut outputs, "new_keys", rows, length)?);
                let values = Arc::new(take_cache_output(&mut outputs, "new_values", rows, length)?);
                let empty_context = tokens == [1];
                let cache = Cache { tokens, rows, length, keys, values };
                if empty_context {
                    self.empty_context = Some(EmptyContext { scores: out.to_vec(), cache: cache.clone() });
                }
                self.cache = Some(cache);
            }
            self.stats.run_rows += rows;
            Ok(())
        }
    }

    impl crate::generation::Distribution for OnnxDistribution {
        fn reset(&mut self) { self.reset_stats(); }
        fn clear(&mut self) { self.reset_stats(); self.memo = PrefixMemo::default(); }
        fn reused_prefix_rows(&self) -> usize { self.stats.memo_rows }
        fn commit_prefixes(&mut self) {
            for (tokens, scores) in self.pending_memo.drain(..) { self.memo.insert(tokens, scores); }
        }
        fn score(&mut self, tokens: &[i32], rows: usize, cols: usize, vocab: usize) -> Result<Vec<f32>, String> {
            let mut output = vec![0.0; rows.checked_mul(vocab).ok_or("invalid 知句 output capacity")?];
            self.distribution(tokens, rows, cols, &mut output, vocab)?;
            Ok(output)
        }
    }

    fn trim_tokens(tokens: &[i32], rows: usize, cols: usize, vocab: usize) -> Result<(usize, Vec<i32>), String> {
        if rows == 0 || cols != 40 || vocab < 3 || rows.checked_mul(cols) != Some(tokens.len()) {
            return Err("invalid 知句 token shape".into());
        }
        let length = tokens[..cols].iter().position(|&id| id == 0).unwrap_or(cols);
        if !(1..=39).contains(&length) { return Err("invalid 知句 prefix length".into()); }
        let mut trimmed = Vec::with_capacity(rows * length);
        for row in tokens.chunks_exact(cols) {
            if row[0] != 1 || row[..length].iter().any(|&id| id <= 0 || id as usize >= vocab)
                || row[length..].iter().any(|&id| id != 0) {
                return Err("invalid 知句 tokens/padding".into());
            }
            trimmed.extend_from_slice(&row[..length]);
        }
        Ok((length, trimmed))
    }

    /// # Safety
    /// Implements AimZhijuScorer. user_data is an exclusively borrowed live
    /// OnnxDistribution; buffers have the ABI's row/column capacities.
    pub unsafe extern "C" fn scorer(user_data: *mut c_void, tokens: *const i32, rows: usize,
        cols: usize, out: *mut f32, vocab: usize) -> i32 {
        if user_data.is_null() || tokens.is_null() || out.is_null() || rows == 0 || cols != 40 || vocab < 3 {
            return -1;
        }
        let (Some(input_len), Some(output_len)) = (rows.checked_mul(cols), rows.checked_mul(vocab)) else { return -1; };
        if input_len > isize::MAX as usize / std::mem::size_of::<i32>()
            || output_len > isize::MAX as usize / std::mem::size_of::<f32>() { return -1; }
        // Never unwind through a C caller, including when ORT rejects a graph.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let model = unsafe { &mut *user_data.cast::<OnnxDistribution>() };
            let tokens = unsafe { std::slice::from_raw_parts(tokens, input_len) };
            let out = unsafe { std::slice::from_raw_parts_mut(out, output_len) };
            model.distribution(tokens, rows, cols, out, vocab)
        }));
        if matches!(result, Ok(Ok(()))) { 0 } else { -1 }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cache_lineage_tracks_reordered_and_duplicated_beams() {
            assert_eq!(parents(&[1, 3, 1, 4], 2, 2, &[1, 4, 8, 1, 3, 9, 1, 4, 7], 3, 3), Some(vec![1, 0, 1]));
            assert_eq!(parents(&[1, 3], 1, 2, &[1, 4, 8], 1, 3), None);
            assert_eq!(parents(&[1, 3], 1, 2, &[1, 3], 1, 2), None);
            let stride = 8 * 2 * 64;
            let values: Vec<f32> = (0..8*2*stride).map(|i| i as f32).collect();
            let got = reorder_cache(&values, 2, 2, &[1, 0, 1]).unwrap();
            for layer in 0..8 { for (new, old) in [1, 0, 1].into_iter().enumerate() {
                assert_eq!(&got[(layer*3+new)*stride..(layer*3+new+1)*stride], &values[(layer*2+old)*stride..(layer*2+old+1)*stride]);
            } }
            assert!(reorder_cache(&values, 2, 2, &[2]).is_err());
        }

        #[test]
        fn prefix_memo_is_exact_bounded_and_replaces_duplicates() {
            let mut memo = PrefixMemo::default();
            memo.insert(vec![1,3], vec![-2.0,-1.0]);
            assert_eq!(memo.get(&[1,3]), Some(&[-2.0, -1.0][..]));
            assert!(memo.get(&[1,4]).is_none());
            memo.insert(vec![1,3], vec![-3.0,-4.0]);
            assert_eq!(memo.rows.len(),1);
            assert_eq!(memo.get(&[1,3]), Some(&[-3.0, -4.0][..]));
            for i in 0..100 { memo.insert(vec![1,i],vec![-1.0;7745]); }
            assert!(memo.rows.len()<=64 && memo.bytes<=2*1024*1024);
            assert!(memo.get(&[1,0]).is_none());
        }

        #[test]
        fn zhiju_trims_right_padding_but_keeps_bos_for_empty_context() {
            let mut tokens = vec![0; 80];
            tokens[0] = 1; tokens[40] = 1;
            assert_eq!(trim_tokens(&tokens, 2, 40, 10).unwrap(), (1, vec![1, 1]));
            tokens[1] = 3; tokens[41] = 9;
            assert_eq!(trim_tokens(&tokens, 2, 40, 10).unwrap(), (2, vec![1, 3, 1, 9]));
        }

        #[test]
        fn zhiju_rejects_malformed_model_batches() {
            assert!(trim_tokens(&[], 0, 40, 10).is_err());
            assert!(trim_tokens(&[1, 2], 1, 2, 10).is_err());
            for row in [vec![0; 40], vec![1; 40]] {
                assert!(trim_tokens(&row, 1, 40, 10).is_err());
            }
            for (at, value) in [(0, 2), (1, -1), (1, 10), (2, 3)] {
                let mut tokens = vec![0; 40]; tokens[0] = 1; tokens[at] = value;
                assert!(trim_tokens(&tokens, 1, 40, 10).is_err());
            }
            let mut tokens = vec![0; 80]; tokens[0] = 1; tokens[40] = 1; tokens[41] = 3;
            assert!(trim_tokens(&tokens, 2, 40, 10).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zhiju_decodes_core_records_in_order() {
        assert_eq!(decode("我们想吃\t-1.25\t4\n我们想菜\t-2\t4\n".as_bytes()).unwrap(), [
            Candidate { text: "我们想吃".into(), score: -1.25, consumed: 4 },
            Candidate { text: "我们想菜".into(), score: -2.0, consumed: 4 },
        ]);
        assert!(decode(b"").unwrap().is_empty());
    }

    #[test]
    fn zhiju_rejects_bad_core_records() {
        for record in ["text\tNaN\t4\n", "text\t-2\tx\n", "\t-2\t4\n", "text\t-2\t4\textra\n", "text\t-2\n"] {
            assert!(decode(record.as_bytes()).is_err());
        }
        assert!(decode(&[0xff]).is_err());
    }
}
