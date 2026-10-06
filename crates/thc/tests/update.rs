//! `thc update` end to end on a scratch copy: a signed manifest installs, a tampered one doesn't,
//! and `--rollback` puts the previous binary back.

mod common;

use std::path::Path;
use std::process::Command;

fn target() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        _ => "unknown",
    }
}

fn run(bin: &Path, args: &[&str], env: &[(&str, String)]) -> (i32, String, String) {
    let mut c = Command::new(bin);
    common::sandbox(&mut c);
    c.args(args).env_remove("CLI_UPDATE_KEY").env("THC_NO_UPDATE_CHECK", "1");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

#[test]
fn signed_updates_install_and_roll_back() {
    let real = Path::new(env!("CARGO_BIN_EXE_thc"));
    let tmp = std::env::temp_dir().join(format!("thc-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("stage")).unwrap();
    let installed = tmp.join("thc");
    std::fs::copy(real, &installed).unwrap();

    // The release key (private half in a 600 file, never printed) and a signed "new" build.
    let (code, public, _) = run(real, &["release-tool", "keygen", tmp.join("key").to_str().unwrap()], &[]);
    assert_eq!(code, 0);
    let public = public.trim().to_string();
    let seed = std::fs::read_to_string(tmp.join("key")).unwrap().trim().to_string();
    assert!(!public.contains(&seed), "keygen prints only the public half");
    let script = tmp.join("stage/thc");
    std::fs::write(&script, "#!/bin/sh\necho 'thc 9.9.9'\n").unwrap();
    Command::new("chmod").arg("+x").arg(&script).status().unwrap();
    let tarball = tmp.join("thc-9.9.9.tar.gz");
    assert!(Command::new("tar").arg("-czf").arg(&tarball).arg("-C").arg(tmp.join("stage")).arg("thc").status().unwrap().success());
    let sum = Command::new("shasum").args(["-a", "256"]).arg(&tarball).output().unwrap();
    let sha = String::from_utf8_lossy(&sum.stdout).split_whitespace().next().unwrap().to_string();
    let (_, sig, _) = run(real, &["release-tool", "sign", "9.9.9", target(), &sha], &[("CLI_UPDATE_KEY", seed.clone())]);
    let manifest = |sig: &str| {
        serde_json::json!({ "version": "9.9.9", "notes": "- a test release", "targets": { target(): { "url": format!("file://{}", tarball.display()), "sha256": sha, "sig": sig.trim() } } })
            .to_string()
    };
    let env = |m: &str| {
        vec![
            ("THC_UPDATE_URL", format!("file://{}", tmp.join(m).display())),
            ("THC_UPDATE_PUBKEY", public.clone()),
            ("HOME", tmp.display().to_string()),
            ("XDG_CACHE_HOME", tmp.join("cache").display().to_string()),
        ]
    };

    // --check only reports.
    std::fs::write(tmp.join("good.json"), manifest(&sig)).unwrap();
    let (code, outp, _) = run(&installed, &["--json", "update", "--check"], &env("good.json"));
    assert_eq!(code, 0);
    assert!(outp.contains("\"available\":true"), "{outp}");

    // A signature over something else is refused and nothing changes.
    let other_sig = run(real, &["release-tool", "sign", "9.9.8", target(), &sha], &[("CLI_UPDATE_KEY", seed.clone())]).1;
    std::fs::write(tmp.join("bad.json"), manifest(&other_sig)).unwrap();
    let (code, _, err) = run(&installed, &["update"], &env("bad.json"));
    assert_ne!(code, 0);
    assert!(err.contains("signature"), "{err}");
    assert!(run(&installed, &["--version"], &[]).1.starts_with("thc 0."), "untouched after a bad signature");

    // The good one installs, keeping thc.prev.
    let (code, outp, err) = run(&installed, &["update"], &env("good.json"));
    assert_eq!(code, 0, "{outp}{err}");
    assert!(outp.contains("→ 9.9.9"), "{outp}");
    assert_eq!(run(&installed, &["--version"], &[]).1.trim(), "thc 9.9.9");
    assert!(tmp.join("thc.prev").is_file());

    // Rollback (run as thc.prev, since the "new" thc is a script).
    let (code, outp, err) = run(&tmp.join("thc.prev"), &["update", "--rollback"], &env("good.json"));
    assert_eq!(code, 0, "{outp}{err}");
    assert!(run(&installed, &["--version"], &[]).1.starts_with("thc 0."), "the previous version is back");
    let _ = std::fs::remove_dir_all(&tmp);
}
