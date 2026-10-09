//! Temporary diagnostic observation harness; does not alter existing acceptance checks.
use super::*;
use serde_json::{json, Value};

fn record(f: &mut Flow, group: usize, desc: &str, phase: &str) -> Value {
    let doc = f.s.app.doc.as_ref();
    json!({"group":group,"desc":desc,"phase":phase,"size":f.s.size,"cursor":f.shot.cursor,
        "scroll":doc.map(|d| d.scroll()),"anchor":doc.map(|d| d.scroll_anchor()),
        "caret":doc.map(|d| { let c=d.caret(); (c.line,c.byte) }),
        "fresh":doc.map(|d| d.has_fresh_end()),
        "new_caret":doc.and_then(|d| d.new_caret_line()).map(|n| (n.depth,format!("{:?}",n.kind))),
        "repin":doc.map(|d| d.repin),
        "lines":doc.map(|d| d.blocks().iter().map(|l| json!({"text":l.text,"depth":l.depth,"new":l.is_new,"gap":l.gap})).collect::<Vec<_>>()),
        "frame":f.shot.text.iter().skip(1).take(f.shot.text.len()-2).cloned().collect::<Vec<_>>()})
}

#[test]
#[ignore = "diagnostic: run with THC_PROBE_INPUT/THC_PROBE_OUTPUT set"]
fn diagnostic_reduced_sequence() {
    let mut f=Flow::new("reduced structural diagnostic",Size::Small,(120,24),Checks { invariants:false,cold:false,replay:false,restore:false });
    f.keys("<c-o>Q4 Plan<cr><tab>").type_text("alpha beta\none\ntwo\nthree").keys("<cr><cr>").type_text("second").idle();
    f.click(doc_at("alpha beta",0)).keys("<cr><home><left><s-tab>");
    let wheel:u32=std::env::var("THC_PROBE_WHEEL").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    f.wheel(false,100,None);
    if wheel>0 { f.wheel(true,wheel,None); }
    f.keys("<home>").resize(120,40);
    let mut observations=vec![record(&mut f,0,"before word-right","draw")];
    f.s.apply(Msg::Key { key:"<m-right>".into() }).unwrap();
    f.shot=draw(&mut f.term,&mut f.s); f.probe();
    observations.push(record(&mut f,1,"word-right before save","draw"));
    f.s.runtime(Msg::Frame);
    f.shot=draw(&mut f.term,&mut f.s); f.probe();
    observations.push(record(&mut f,2,"word-right after save","after-frame"));
    std::fs::write(std::env::var("THC_PROBE_OUTPUT").unwrap(),serde_json::to_string_pretty(&observations).unwrap()).unwrap();
    f.done();
}

#[test]
#[ignore = "diagnostic: run with THC_PROBE_INPUT/THC_PROBE_OUTPUT set"]
fn diagnostic_structural_sequence() {
    let input=std::env::var("THC_PROBE_INPUT").unwrap();
    let output=std::env::var("THC_PROBE_OUTPUT").unwrap();
    let groups:Vec<Value>=serde_json::from_str(&std::fs::read_to_string(input).unwrap()).unwrap();
    let mut f=Flow::new("structural diagnostic",Size::Small,(120,32),Checks { invariants:false,cold:false,replay:false,restore:false });
    let mut observations=Vec::new();
    for (i,g) in groups.iter().enumerate() {
        let msgs:Vec<Msg>=g["msgs"].as_array().unwrap().iter().map(|v| serde_json::from_value(v.clone()).unwrap()).collect();
        let desc=g["desc"].as_str().unwrap();
        for m in msgs.iter().filter(|m| !matches!(m,Msg::Frame)) {
            if let Msg::Resize {w,h}=m { if (*w,*h)!=f.s.size { *f.term.backend_mut()=Emu::new(*w,*h); } }
            f.s.apply(m.clone()).unwrap();
        }
        f.runtime_effects();
        f.shot=draw(&mut f.term,&mut f.s); f.probe();
        observations.push(record(&mut f,i,desc,"draw"));
        // Match the original flow's same-size cold-render observation, not a state patch.
        let (w,h)=f.s.size; f.s.render(w,h,"text").unwrap();
        for m in msgs.iter().filter(|m| matches!(m,Msg::Frame)) {
            f.s.runtime(m.clone());
            f.shot=draw(&mut f.term,&mut f.s); f.probe();
            observations.push(record(&mut f,i,desc,"after-frame"));
            let (w,h)=f.s.size; f.s.render(w,h,"text").unwrap();
        }
    }
    std::fs::write(output,serde_json::to_string_pretty(&observations).unwrap()).unwrap();
    f.done();
}
