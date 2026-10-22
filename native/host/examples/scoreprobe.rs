//! Model scores for candidates after each context (local diagnosis):
//!   scoreprobe "还剩,还是" "我觉得" "今年年底能裁"   with ZHIZI_ORT / ZHIZI_MODEL / ZHIZI_ITOS as for `eval`.
use zhizi_host::ranking::Scorer;
fn main() {
    zhizi_host::ranking::onnx::init_runtime(std::env::var("ZHIZI_ORT").unwrap().as_ref()).unwrap();
    let mut scorer = zhizi_host::ranking::onnx::OnnxScorer::load(std::env::var("ZHIZI_MODEL").unwrap().as_ref(), std::env::var("ZHIZI_ITOS").unwrap().as_ref(), 4).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let candidates: Vec<String> = args[0].split(',').map(str::to_string).collect();
    for context in &args[1..] {
        let scores = scorer.score(context, &candidates).unwrap();
        println!("{context:<14} {}", candidates.iter().zip(&scores).map(|(c, s)| format!("{c} {s:.2}")).collect::<Vec<_>>().join("  "));
    }
}
