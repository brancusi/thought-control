//! Attachment nodes (FORMAT.md "Attachments"): every file a note shows is its own
//! node, keyed from its path, embedded through an `embed` edge derived from the text. No note's
//! text ever changes. Existing vaults are brought along by one undoable transaction by thc.

mod common;

use serde_json::Value;
use std::path::PathBuf;

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-imgnodes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.ok(&["init", "vault"]);
        v
    }

    fn cmd(&self, args: &[&str]) -> std::process::Output {
        let mut c = common::thc();
        c.args(args).current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_ACTOR", "human");
        c.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.cmd(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.ok(&a)).unwrap()
    }

    /// A PNG of w×h (signature and IHDR are all the dimensions need).
    fn png(&self, rel: &str, w: u32, h: u32) {
        let mut b: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        let p = self.root.join("vault").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    fn marker(&self) -> PathBuf {
        self.root.join("cache/attachments-v1")
    }

    fn doctor(&self) -> Value {
        self.json(&["doctor"])
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn drifting(d: &Value) -> bool {
    d["issues"].as_array().unwrap().iter().any(|i| i.as_str().unwrap_or("").contains("attachments and embeds disagree"))
}

#[test]
fn an_older_vaults_attachment_lines_get_nodes_once_undoably_and_text_never_changes() {
    let v = V::new("upgrade");
    v.png("files/2026/10/ab12-boiler.png", 640, 480);
    // Lines written as before (--plain reads nothing, as older thc wrote them).
    let line = "![boiler label](files/2026/10/ab12-boiler.png)";
    let note = v.json(&["add", "--plain", line])["nodes"][0]["id"].as_str().unwrap().to_string();
    let _ = std::fs::remove_file(v.marker());
    assert!(drifting(&v.doctor()), "doctor sees the note without its node");
    // The next write: one transaction by thc first, then the write.
    v.ok(&["add", "something else"]);
    let img = v.json(&["q", "is:image"]);
    assert_eq!(img["items"].as_array().unwrap().len(), 1, "{img}");
    let node = &img["items"][0];
    assert_eq!(node["title"], "boiler label");
    let id = node["id"].as_str().unwrap().to_string();
    let shown = v.json(&["show", &id]);
    assert_eq!(shown["file"]["w"], 640, "{shown}");
    assert_eq!(shown["file"]["kind"], "image");
    assert_eq!(shown["embeds"][0]["id"], note.as_str(), "used in");
    assert_eq!(v.json(&["show", &note])["text"], line, "the note's text is as it was");
    assert_eq!(v.json(&["show", &note])["attachments"][0]["id"], id.as_str());
    assert!(!drifting(&v.doctor()));
    assert_eq!(v.json(&["q", &format!("embeds:{id}")])["items"][0]["id"], note.as_str());
    // By thc, in its own transaction: undo takes it all back, the text untouched.
    let log = v.json(&["log", "--by", "thc"]);
    let tx = log["events"][0]["tx"].as_str().expect("a thc transaction").to_string();
    v.ok(&["undo", "--tx", &tx[tx.len() - 6..]]);
    assert!(v.json(&["q", "is:image"])["items"].as_array().unwrap().is_empty());
    assert_eq!(v.json(&["show", &note])["text"], line);
    // doctor --fix --yes brings it back; running it again changes nothing (idempotent).
    v.ok(&["--yes", "doctor", "--fix"]);
    assert_eq!(v.json(&["q", "is:image"])["items"].as_array().unwrap().len(), 1);
    let before = v.json(&["log"])["events"].as_array().unwrap().len();
    v.ok(&["--yes", "doctor", "--fix"]);
    assert_eq!(v.json(&["log"])["events"].as_array().unwrap().len(), before, "nothing to do writes nothing");
}

#[test]
fn new_lines_and_thc_attach_embed_at_once_and_one_file_is_one_node() {
    let v = V::new("new");
    let page = v.json(&["page", "new", "Lisbon flat"])["nodes"][0]["id"].as_str().unwrap().to_string();
    let file = v.root.join("plan.png");
    {
        let mut b: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        b.extend_from_slice(&100u32.to_be_bytes());
        b.extend_from_slice(&50u32.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&file, b).unwrap();
    }
    let a = v.json(&["attach", &page, file.to_str().unwrap(), "--caption", "floor plan"]);
    let aid = a["attachment"].as_str().expect("the attachment node").to_string();
    let shown = v.json(&["show", &aid]);
    assert_eq!(shown["title"], "floor plan");
    assert_eq!(shown["props"]["w"], 100, "the file's size known at once: {shown}");
    assert_eq!(shown["embeds"].as_array().unwrap().len(), 1);
    // The same file shown in a second note: the same node, used in two.
    let path = a["path"].as_str().unwrap();
    v.ok(&["add", &format!("![plan again]({path})"), "--under", &page]);
    assert_eq!(v.json(&["q", "is:attachment"])["items"].as_array().unwrap().len(), 1);
    assert_eq!(v.json(&["show", &aid])["embeds"].as_array().unwrap().len(), 2);
    // Attachment nodes aren't pages.
    assert!(!v.ok(&["q", "is:page"]).contains("floor plan"));
    // A note's text changed to drop the image: its embed goes.
    let second = v.json(&["q", "text:\"plan again\""])["items"][0]["id"].as_str().unwrap().to_string();
    v.ok(&["text", &second, "no picture now"]);
    assert_eq!(v.json(&["show", &aid])["embeds"].as_array().unwrap().len(), 1);
    assert!(!drifting(&v.doctor()));
}
