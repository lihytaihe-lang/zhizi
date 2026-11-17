//! Explicit offline 知句 capability. Nothing here schedules search or changes a page.

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub score: f64,
    pub consumed: usize,
}

fn decode(encoded: &[u8]) -> Result<Vec<Candidate>, String> {
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
    use ort::session::Session;
    use ort::value::Tensor;
    use std::ffi::c_void;
    use std::path::Path;
    use std::time::Instant;

    #[derive(Clone, Debug, Default)]
    pub struct Stats {
        pub callback_calls: usize,
        pub successful_calls: usize,
        pub run_rows: usize,
        /// Time inside ORT run(), summed over this search, excluding core search.
        pub onnx_ms: f64,
    }

    pub struct OnnxDistribution {
        session: Session,
        stats: Stats,
    }

    impl OnnxDistribution {
        /// Uses the runtime initialized by ranking::onnx::init_runtime.
        pub fn load(model: &Path, threads: usize) -> Result<Self, String> {
            let session = Session::builder().map_err(|e| e.to_string())?
                .with_intra_threads(threads).map_err(|e| e.to_string())?
                .with_inter_threads(1).map_err(|e| e.to_string())?
                .commit_from_file(model).map_err(|e| e.to_string())?;
            Ok(Self { session, stats: Stats::default() })
        }

        pub fn stats(&self) -> &Stats { &self.stats }

        pub fn reset_stats(&mut self) { self.stats = Stats::default(); }

        fn distribution(&mut self, tokens: &[i32], rows: usize, cols: usize, out: &mut [f32], vocab: usize)
            -> Result<(), String> {
            self.stats.callback_calls += 1;
            let (length, tokens) = trim_tokens(tokens, rows, cols, vocab)?;
            let input = Tensor::from_array(([rows, length], tokens)).map_err(|e| e.to_string())?;
            let started = Instant::now();
            let result = self.session.run(ort::inputs!["tokens" => input]);
            self.stats.onnx_ms += started.elapsed().as_secs_f64() * 1000.0;
            let outputs = result.map_err(|e| e.to_string())?;
            let (shape, logprobs) = outputs.get("logprobs").ok_or("missing next-character distribution")?
                .try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            if shape.as_ref() != [rows as i64, vocab as i64] || logprobs.len() != out.len()
                || logprobs.iter().any(|value| !value.is_finite()) {
                return Err("invalid next-character distribution shape/values".into());
            }
            out.copy_from_slice(logprobs);
            self.stats.successful_calls += 1;
            self.stats.run_rows += rows;
            Ok(())
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
