//! caretline is a text-editing engine: its own code names no host concept. What a line means
//! (a task, a status, a due date) is the host's, added through `Host` (see docs/caretline).

const WORDS: &[&str] = &[
    "task", "tasks", "todo", "done", "doing", "waiting", "cancelled", "completed", "checkbox", "checkboxes", "journal",
    "vault", "thc", "priority", "due", "note", "notes",
];

fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            // The vendored Helix files are Helix's own words.
            if p.file_name().is_some_and(|n| n != "helix") {
                files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn the_engine_names_no_host_concept() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    files(&root.join("src"), &mut paths);
    files(&root.join("examples"), &mut paths);
    let mut found = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(&p).unwrap();
        for (n, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            for w in lower.split(|c: char| !c.is_alphanumeric()) {
                if WORDS.contains(&w) {
                    found.push(format!("{}:{}: {w}: {}", p.strip_prefix(root).unwrap().display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(found.is_empty(), "host words in caretline:\n{}", found.join("\n"));
}
