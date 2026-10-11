//! A list's style lives on its first item (FORMAT.md `list`), and every write keeps it there:
//! `thc mv`, `thc rm` and `thc restore` from the CLI (or an agent), not only the editor's save.

mod common;

use std::path::PathBuf;
use thc_core::event::Actor;
use thc_core::vault::{Paths, Vault};

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = thc_core::scratch::dir(&format!("thc-lists-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        v.cli(&["init", "vault"]);
        v
    }

    fn cli(&self, args: &[&str]) -> String {
        let mut c = common::thc();
        c.current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env_remove("THC_ACTOR");
        let o = c.args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    /// A new note's id.
    fn add(&self, args: &[&str]) -> String {
        let mut a = vec!["add", "--plain", "--json"];
        a.extend_from_slice(args);
        let out: serde_json::Value = serde_json::from_str(&self.cli(&a)).unwrap();
        out["nodes"][0]["id"].as_str().unwrap().to_string()
    }

    /// The `thc edit` lines of `id`'s children (a note edited alone: its children one level
    /// in), without ids: what a person sees.
    fn shown(&self, id: &str) -> String {
        let paths = Paths { vault: self.root.join("vault"), cache: self.root.join("cache") };
        let v = Vault::open(paths, Actor { kind: "human".into(), name: None }, "test").unwrap();
        let text = thc_core::edit::render(&v.store, id).unwrap().text;
        text.lines().skip(1).take_while(|l| !l.starts_with("<!--")).filter(|l| !l.trim().is_empty()).map(|l| format!("{}\n", thc_core::edit::split_id(&l[2..]).0)).collect()
    }

    /// A list `Steps` (alpha, beta, gamma) numbered `1.`: its id and the items'.
    fn numbered(&self) -> (String, [String; 3]) {
        let p = self.add(&["Steps", "--inbox"]);
        let items = ["alpha", "beta", "gamma"].map(|t| self.add(&[t, "--under", &p]));
        self.cli(&["set", &items[0], "list=1."]);
        assert_eq!(self.shown(&p), "1. alpha\n2. beta\n3. gamma\n");
        (p, items)
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn moving_an_item_to_the_top_keeps_the_list_numbered() {
    let v = V::new("mv");
    let (p, [a, b, _]) = v.numbered();
    v.cli(&["mv", &b, "--before", &a]);
    assert_eq!(v.shown(&p), "1. beta\n2. alpha\n3. gamma\n");
    // The style moved with the list's start: on the new first item only.
    let list = |id: &str| -> serde_json::Value { serde_json::from_str::<serde_json::Value>(&v.cli(&["show", id, "--json"])).unwrap()["props"]["list"].clone() };
    assert_eq!((list(&b), list(&a)), (serde_json::json!("1."), serde_json::Value::Null));
    v.cli(&["mv", &b, "--after", &a]);
    assert_eq!(v.shown(&p), "1. alpha\n2. beta\n3. gamma\n");
}

#[test]
fn removing_and_restoring_the_first_item_keeps_the_list_numbered() {
    let v = V::new("rm");
    let (p, [a, _, _]) = v.numbered();
    v.cli(&["rm", &a]);
    assert_eq!(v.shown(&p), "1. beta\n2. gamma\n");
    v.cli(&["restore", &a]);
    assert_eq!(v.shown(&p), "1. alpha\n2. beta\n3. gamma\n");
}

#[test]
fn moving_the_first_item_to_another_list_leaves_both_right() {
    let v = V::new("under");
    let (p, [a, _, _]) = v.numbered();
    let q = v.add(&["Shopping", "--inbox"]);
    let x = v.add(&["milk", "--under", &q]);
    v.add(&["eggs", "--under", &q]);
    // To the top of a bulleted list: it joins that list's style, its old list keeps its own.
    v.cli(&["mv", &a, "--under", &q, "--before", &x]);
    assert_eq!(v.shown(&p), "1. beta\n2. gamma\n");
    assert_eq!(v.shown(&q), "- alpha\n- milk\n- eggs\n");
    // And back: it ends a list it didn't start.
    v.cli(&["mv", &a, "--under", &p]);
    assert_eq!(v.shown(&p), "1. beta\n2. gamma\n3. alpha\n");
    assert_eq!(v.shown(&q), "- milk\n- eggs\n");
}

#[test]
fn setting_the_style_on_any_item_restyles_its_list() {
    let v = V::new("set");
    let (p, [_, b, _]) = v.numbered();
    v.cli(&["set", &b, "list=a)"]);
    assert_eq!(v.shown(&p), "a) alpha\nb) beta\nc) gamma\n");
}
