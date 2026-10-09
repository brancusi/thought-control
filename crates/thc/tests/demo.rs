//! The demo's process boundary must ignore the launching project's real vault/config.
use std::process::Command;

fn snapshot(root: &std::path::Path, keep: bool) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_thc"));
    cmd.arg("demo");
    if keep {
        cmd.arg("--keep");
    }
    cmd.current_dir(root)
        .env("HOME", root)
        .env("THC_CONFIG_DIR", root.join("config"))
        .env("THC_VAULT", root.join("do-not-open"))
        .env("THC_BOARD", "do-not-open")
        .env("THC_READONLY", "1")
        .env("THC_ACTOR", "example-agent")
        .env("THC_TUI_SNAPSHOT", "80x24")
        .env(
            "THC_TUI_KEYS",
            "<f2>Written here. <f2><f2><f2><f2>x<f2><f3>",
        )
        .output()
        .unwrap()
}

#[test]
fn demo_ignores_inherited_vault_and_config_and_removes_scratch() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/config.toml"),
        "sentinel-invalid-settings",
    )
    .unwrap();
    std::fs::write(root.path().join(".thc.toml"), "vault = 'do-not-open'\n").unwrap();
    let out = snapshot(root.path(), false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Written here."));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Scratch session removed"));
    let scratch = stderr
        .lines()
        .find_map(|s| s.strip_prefix("Scratch session: "))
        .unwrap();
    assert!(
        !std::path::Path::new(scratch).exists(),
        "default exit must remove the scratch directory"
    );
    assert!(!root.path().join("do-not-open").exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("config/config.toml")).unwrap(),
        "sentinel-invalid-settings"
    );
}

#[test]
fn demo_keep_preserves_records_and_internal_child_refuses_plain_invocation() {
    let root = tempfile::tempdir().unwrap();
    let out = snapshot(root.path(), true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let path = stderr
        .lines()
        .find_map(|s| s.strip_prefix("Scratch session preserved at "))
        .unwrap();
    let kept = std::path::PathBuf::from(path);
    assert!(kept.join("vault/log").exists());
    fn logs(path: &std::path::Path) -> String {
        std::fs::read_dir(path)
            .unwrap()
            .map(|e| {
                let path = e.unwrap().path();
                if path.is_dir() {
                    logs(&path)
                } else {
                    std::fs::read_to_string(path).unwrap()
                }
            })
            .collect()
    }
    let log = logs(&kept.join("vault/log"));
    assert!(log.contains("Written here."));
    // Only remove this test's newly-created sandbox, never a supplied vault.
    assert_eq!(
        std::fs::read(kept.join(".teaching-sandbox")).unwrap(),
        b"thc-demo-v1\n"
    );
    std::fs::remove_dir_all(&kept).unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_thc"))
        .args(["demo", "--isolated"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(!root.path().join("vault").exists());
}
