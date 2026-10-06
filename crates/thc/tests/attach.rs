//! Attachments (docs/design/attachments.md): `thc attach`, `show --json`, `doctor`.

mod common;

use serde_json::Value;
use std::path::Path;

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    common::thc().args(args).current_dir(root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_ACTOR", "claude").output().unwrap()
}

fn ok(root: &Path, args: &[&str]) -> String {
    let o = run(root, args);
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into()
}

/// A 4×3 PNG (header only is enough for its size).
fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    b.extend(4u32.to_be_bytes());
    b.extend(3u32.to_be_bytes());
    b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b
}

#[test]
fn t3_t6_t7_attach_show_doctor_and_the_limit() {
    let root = std::env::temp_dir().join(format!("thc-attach-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    ok(&root, &["init", "vault"]);
    let issue = serde_json::from_str::<Value>(&ok(&root, &["--json", "todo", "Shift drops capitals"])).unwrap()["nodes"][0]["id"].as_str().unwrap().to_string();
    let shot = root.join("wezterm shift.png");
    std::fs::write(&shot, png()).unwrap();
    // T3: a child image line with the caption; --json has the path and the size.
    let a: Value = serde_json::from_str(&ok(&root, &["--json", "attach", &issue, shot.to_str().unwrap(), "--caption", "after the fix"])).unwrap();
    let path = a["path"].as_str().unwrap().to_string();
    assert!(path.starts_with("files/") && path.ends_with("-wezterm-shift.png"), "{a}");
    assert_eq!((a["w"].as_u64(), a["h"].as_u64()), (Some(4), Some(3)));
    assert!(root.join("vault").join(&path).exists());
    // T6: show --json lists it, with abs, for an agent to read.
    let s: Value = serde_json::from_str(&ok(&root, &["--json", "show", &issue])).unwrap();
    let att = &s["attachments"][0];
    assert_eq!(att["caption"], "after the fix");
    assert_eq!(att["path"], path.as_str());
    assert!(Path::new(att["abs"].as_str().unwrap()).exists(), "{s}");
    assert!(s["children"].to_string().contains(&format!("![after the fix]({path})")));
    // Doctor: nothing wrong; then a missing file and an orphan.
    let d: Value = serde_json::from_str(&ok(&root, &["--json", "doctor"])).unwrap();
    assert!(d["issues"].as_array().unwrap().is_empty(), "{d}");
    std::fs::rename(root.join("vault").join(&path), root.join("vault/files/stray.png")).unwrap();
    let d: Value = serde_json::from_str(&ok(&root, &["--json", "doctor"])).unwrap();
    assert!(d["issues"].to_string().contains(&format!("▣ missing: {path}")), "{d}");
    assert!(d["notices"].to_string().contains("files/stray.png isn't referenced"), "{d}");
    ok(&root, &["--yes", "doctor", "--fix"]);
    assert!(root.join("vault/files/.orphans/stray.png").exists(), "moved aside, never deleted");
    // T7: over the vault's limit, refused with the size and the setting.
    std::fs::write(root.join("vault/settings.toml"), "[attachments]\nmax_mb = 1\n").unwrap();
    let big = root.join("screenshot.png");
    std::fs::write(&big, vec![0u8; 2 * 1024 * 1024 + 10]).unwrap();
    let o = run(&root, &["attach", &issue, big.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&o.stderr).contains("screenshot is 3 MB · the limit is 1 MB ([attachments] max_mb)"), "{}", String::from_utf8_lossy(&o.stderr));
    let _ = std::fs::remove_dir_all(&root);
}

/// T1, T2 and §3 in the TUI: ⌥V attaches the clipboard's image (THC_CLIPBOARD_IMAGE stands in
/// for the clipboard in tests), a dropped file's path offers `attach …? y`, the line draws as a
/// chip, and Enter on it opens the file (not launched in a snapshot).
#[test]
fn t1_t2_paste_and_drop_an_image_in_the_tui() {
    let root = std::env::temp_dir().join(format!("thc-attach-tui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    ok(&root, &["init", "vault"]);
    let shot = root.join("shot.png");
    std::fs::write(&shot, png()).unwrap();
    let tui = |keys: &str, write: bool, clip: bool| {
        let mut c = common::thc();
        c.args(["j", "--no-focus"]).current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_ACTOR", "human").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", keys);
        if write {
            c.env("THC_TUI_SNAPSHOT_WRITE", "1");
        }
        if clip {
            c.env("THC_CLIPBOARD_IMAGE", &shot);
        }
        String::from_utf8_lossy(&c.output().unwrap().stdout).to_string()
    };
    let notes = || -> Vec<String> {
        let v: Value = serde_json::from_str(&ok(&root, &["--json", "q", "journal=today"])).unwrap();
        v["items"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap().to_string()).collect()
    };
    // T1: ⌥V with an image on the clipboard: its own note, a file, the bar's confirmation.
    let f = tui("before<m-v>after<esc>", true, true);
    let ns = notes();
    let img = ns.iter().find(|t| t.starts_with("![screenshot")).unwrap_or_else(|| panic!("{ns:?}\n{f}")).clone();
    assert!(ns.contains(&"before".to_string()) && ns.contains(&"after".to_string()), "{ns:?}");
    let path = img.split("](").nth(1).unwrap().trim_end_matches(')').to_string();
    assert!(root.join("vault").join(&path).exists());
    // The chip, with the caret elsewhere; Enter on it opens it.
    let f = tui("<c-home>", false, false);
    assert!(f.contains("▣ screenshot") && f.contains("4×3") && f.contains("⌃O open"), "{f}");
    // With no image, ⌥V is the plain-paste toggle, as before.
    assert!(tui("<m-v>", false, false).contains("next paste is plain"));
    // T2: a dropped file (its path, pasted) attaches at once, no prompt.
    let before = notes().iter().filter(|t| t.starts_with("![shot]")).count();
    let f = tui(&format!("<c-end><paste:{}><nop><esc>", shot.display()), true, false);
    assert!(!f.contains("y yes"), "no prompt: {f}");
    assert_eq!(notes().iter().filter(|t| t.starts_with("![shot]")).count(), before + 1, "{:?}", notes());
    // ⌃Z right after a drop keeps the path as text instead.
    tui(&format!("<c-end><paste:{}><nop><c-z><esc>", shot.display()), true, false);
    assert!(notes().iter().any(|t| t == shot.to_str().unwrap()), "{:?}", notes());
    // ⌥V first: the drop pastes as plain text.
    tui(&format!("<c-end>x <m-v><paste:{}><nop><esc>", shot.display()), true, false);
    assert!(notes().iter().any(|t| t.starts_with("x ") && t.contains(shot.to_str().unwrap())), "{:?}", notes());
    let _ = std::fs::remove_dir_all(&root);
}

/// editing.md §6-7: moving around an attachment never opens it. A click puts the
/// caret there and Enter starts a new line after it; ⌃O and a double-click open it.
#[test]
fn moving_onto_an_attachment_never_opens_it() {
    let root = std::env::temp_dir().join(format!("thc-attach-noopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    ok(&root, &["init", "vault"]);
    let page = serde_json::from_str::<Value>(&ok(&root, &["--json", "page", "new", "Shots"])).unwrap();
    let page = page["nodes"][0]["id"].as_str().or(page["id"].as_str()).unwrap().to_string();
    let shot = root.join("shot.png");
    std::fs::write(&shot, png()).unwrap();
    ok(&root, &["add", "--plain", "--under", &page, "above"]);
    ok(&root, &["attach", &page, shot.to_str().unwrap(), "--caption", "the shot"]);
    let tui = |keys: &str| {
        let mut c = common::thc();
        c.args(["p", "Shots", "--no-focus"]).current_dir(&root).env("THC_VAULT", root.join("vault")).env("THC_CACHE_DIR", root.join("cache")).env("THC_ACTOR", "human").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", keys);
        String::from_utf8_lossy(&c.output().unwrap().stdout).to_string()
    };
    let f = tui("");
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("▣ the shot")).unwrap_or_else(|| panic!("{f}"));
    let x = line.find("the shot").unwrap();
    let opened = |f: &str| f.contains("(skipped in snapshot)");
    // Arrow onto it, Enter on it, a single click on it: never opened.
    assert!(!opened(&tui("<c-home><down>")), "arriving by ↓");
    assert!(!opened(&tui("<c-home><down><cr>")), "Enter on it");
    assert!(!opened(&tui(&format!("<click:{x},{y}>"))), "a click on it");
    // ⌃O and a double-click do.
    assert!(opened(&tui("<c-home><down><c-o>")), "⌃O opens it");
    assert!(opened(&tui(&format!("<dclick:{x},{y}>"))), "a double-click opens it");
    let _ = std::fs::remove_dir_all(&root);
}
