//! Synthetic offline attribution only. Same production beam/search, relaxed
//! diagnostic budget, FP32 ONNX scorer; never changes the IME or its page.
use agent_ime_core::*;
use serde::Deserialize;
use std::{ffi::c_void,path::Path};
use zhizi_host::zhiju::onnx::{self,OnnxDistribution};

#[derive(Deserialize)]
struct Case { id:String,input:String,context:String,expected:String }
struct Scoring {
    model:OnnxDistribution,symbols:Vec<String>,context_len:usize,target:String,
    adapted:Option<(OnnxDistribution,usize)>,
    callbacks:Vec<serde_json::Value>,
}
unsafe extern "C" fn score(user:*mut c_void,tokens:*const i32,rows:usize,cols:usize,out:*mut f32,vocab:usize)->i32 {
    if user.is_null()||tokens.is_null()||out.is_null()||rows==0||cols!=40||vocab<3{return -1;}
    let state=unsafe { &mut *user.cast::<Scoring>() };
    let input=unsafe {std::slice::from_raw_parts(tokens,rows*cols)};
    let length=input[..cols].iter().position(|&v|v==0).unwrap_or(cols);
    let use_adapted=state.adapted.as_ref().is_some_and(|(_,minimum)|length>=*minimum);
    let model=if use_adapted {&mut state.adapted.as_mut().unwrap().0} else {&mut state.model};
    let status=unsafe { onnx::scorer((model as *mut OnnxDistribution).cast(),tokens,rows,cols,out,vocab) };
    if status!=0{return status;}
    let tokens=unsafe {std::slice::from_raw_parts(tokens,rows*cols)};
    let mut generated=Vec::new();
    for row in tokens.chunks_exact(cols) {
        let n=row.iter().position(|&v|v==0).unwrap_or(cols);
        let Some(ids)=row.get(1+state.context_len..n) else{return -1;};
        let mut text=String::new();
        for &id in ids {
            let Some(symbol)=usize::try_from(id).ok().and_then(|id|state.symbols.get(id)) else{return -1;};
            text.push_str(symbol);
        }
        generated.push(text);
    }
    let alive=generated.iter().any(|s|state.target.starts_with(s));
    state.callbacks.push(serde_json::json!({"step":state.callbacks.len(),"prefixes":generated,"targetPrefixAlive":alive,"adaptedModel":use_adapted}));
    0
}
unsafe fn key(session:*mut AimSession,kind:u32,text:&[u8])->Result<(),String>{
    let event=AimKeyEvent{kind,text:text.as_ptr(),text_len:text.len(),modifiers:0,security_state:0,delivery_id:0};
    let mut result=std::mem::MaybeUninit::<AimResult>::uninit();
    let status=unsafe{aim_session_process(session,&event,result.as_mut_ptr())};
    if status!=0{return Err(format!("key status {status}"));}
    unsafe{aim_result_free(result.as_mut_ptr())};Ok(())
}
fn main()->Result<(),Box<dyn std::error::Error>>{
    let args:Vec<_>=std::env::args().skip(1).collect();
    if ![7,9].contains(&args.len()){return Err("usage: zhiju_trace shared blank-user itos model.onnx cases.json new-report.json diagnostic-budget-ms [adapted.onnx minimum-prefix-tokens]".into());}
    let user=Path::new(&args[1]);
    if user.exists()&&std::fs::read_dir(user)?.next().is_some(){return Err("requires empty user dir".into());}
    if Path::new(&args[5]).exists(){return Err("refusing report overwrite".into());}
    let budget:u64=args[6].parse()?;
    if budget<1000{return Err("diagnostic budget must be explicit and >=1000ms; not production acceptance".into());}
    std::fs::create_dir_all(user)?;
    let symbols:Vec<String>=serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let ort_path=std::env::var("ZHIZI_ORT")?;
    zhizi_host::ranking::onnx::init_runtime(Path::new(&ort_path))?;
    let model=OnnxDistribution::load(Path::new(&args[3]),2)?;
    let adapted=if args.len()==9 {
        let minimum:usize=args[8].parse()?;
        if !(2..=39).contains(&minimum){return Err("invalid minimum prefix".into());}
        Some((OnnxDistribution::load(Path::new(&args[7]),2)?,minimum))
    } else {None};
    let app=b"zhizi.synthetic-trace";
    let session=unsafe{aim_session_create(args[0].as_ptr(),args[0].len(),args[1].as_ptr(),args[1].len(),app.as_ptr(),app.len())};
    if session.is_null(){return Err("session failed".into());}
    struct Session(*mut AimSession);
    impl Drop for Session{fn drop(&mut self){unsafe{aim_session_destroy(self.0)}}}
    let _guard=Session(session);
    if unsafe{aim_session_set_zhiju_vocab(session,args[2].as_ptr(),args[2].len())}!=0{return Err("vocab failed".into());}
    let cases:Vec<Case>=serde_json::from_slice(&std::fs::read(&args[4])?)?;
    let mut state=Scoring{model,adapted,symbols,context_len:0,target:String::new(),callbacks:vec![]};
    let mut reports=Vec::new();
    for c in cases {
        if c.id.is_empty()||c.expected.is_empty()||c.context.chars().count()+c.expected.chars().count()>38
            ||!c.input.bytes().all(|b|b.is_ascii_lowercase()) {return Err("invalid synthetic diagnostic case".into());}
        unsafe{key(session,6,b"")?};
        if unsafe{aim_session_set_preceding_text(session,c.context.as_ptr(),c.context.len())}!=0{return Err("context failed".into());}
        for ch in c.input.bytes(){unsafe{key(session,0,&[ch])?};}
        state.context_len=c.context.chars().count();state.target=c.expected.clone();state.callbacks.clear();state.model.reset_stats();
        if let Some((model,_))=&mut state.adapted {model.reset_stats();}
        let mut bytes=vec![0u8;4096];let mut len=bytes.len();let started=std::time::Instant::now();
        let status=unsafe{aim_session_zhiju_search(session,Some(score),(&mut state as *mut Scoring).cast(),8,budget,bytes.as_mut_ptr(),&mut len)};
        if status!=0{return Err(format!("{} search status{status}",c.id).into());}
        let records=std::str::from_utf8(&bytes[..len])?;
        let candidates:Vec<_>=records.lines().map(|l|l.split('\t').next().unwrap()).collect();
        let lost=state.callbacks.iter().find(|r|r["targetPrefixAlive"]==false).map(|r|r["step"].clone());
        reports.push(serde_json::json!({"id":c.id,"input":c.input,"context":c.context,"expected":c.expected,
          "candidates":candidates,"encodedScores":records,"targetRank":candidates.iter().position(|s|*s==c.expected),
          "firstCallbackWithoutTargetPrefix":lost,"callbacks":state.callbacks,"searchMs":started.elapsed().as_secs_f64()*1000.0,
          "modelMs":state.model.stats().onnx_ms+state.adapted.as_ref().map_or(0.,|(model,_)|model.stats().onnx_ms),"beam":8,"diagnosticBudgetMS":budget}));
    }
    let report=serde_json::json!({"scope":"synthetic offline beam attribution only; not controller or40ms acceptance","rows":reports});
    std::fs::write(&args[5],serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
