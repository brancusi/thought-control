//! A built-in teaching session. Resolve no user vault: re-exec in a private HOME/cwd,
//! clearing inherited THC overrides before either settings or the editor are opened.
use anyhow::{Result, bail};
use clap::Args;
use std::io::IsTerminal;
use std::process::Command;
use thc_core::event::Actor;
use thc_core::vault::{Paths, Vault};

#[derive(Args)]
pub struct DemoArgs {
    /// Keep the scratch notes and settings after exit (the path is printed).
    #[arg(long)]
    keep: bool,
    /// Internal sandbox child, never an alternative vault selector.
    #[arg(long, hide = true)]
    isolated: bool,
}

pub const SPEC: crate::registry::Spec = crate::early!(
    "demo",
    "Learn THC interactively with overlays in a disposable scratch vault",
    DemoArgs,
    run
);

fn run(_: &crate::cli::Cli, args: DemoArgs) -> Result<()> {
    if args.isolated {
        return child();
    }
    anyhow::ensure!(
        std::env::var_os("THC_TUI_SNAPSHOT").is_some()
            || (std::io::stdin().is_terminal() && std::io::stdout().is_terminal()),
        "thc demo needs an interactive terminal"
    );
    let scratch = tempfile::Builder::new().prefix("thc-demo-").tempdir()?;
    let root = scratch.path().canonicalize()?;
    std::fs::write(root.join(".teaching-sandbox"), b"thc-demo-v1\n")?;
    for dir in ["home", "config", "cache", "runtime"] {
        std::fs::create_dir(root.join(dir))?;
    }
    // Snapshot options are useful supporting evidence, but are not the interactive entrypoint.
    let snapshot: Vec<_> = std::env::vars_os()
        .filter(|(k, _)| {
            matches!(
                k.to_str(),
                Some(
                    "THC_TUI_SNAPSHOT"
                        | "THC_TUI_KEYS"
                        | "THC_TUI_SNAPSHOT_FORMAT"
                        | "THC_TUI_STATE_OUT"
                        | "THC_THEME"
                        | "THC_GLYPHS"
                        | "THC_NOW"
                        | "THC_TUI_TRACE_FILE"
                )
            )
        })
        .collect();
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["demo", "--isolated"]).current_dir(&root);
    for (k, _) in std::env::vars_os().filter(|(k, _)| k.to_string_lossy().starts_with("THC_")) {
        cmd.env_remove(k);
    }
    cmd.envs(snapshot)
        .env("HOME", root.join("home"))
        .env("THC_CONFIG_DIR", root.join("config"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("THC_NO_UPDATE_CHECK", "1")
        .env("THC_TUI_NO_LISTEN", "1")
        .env("THC_ACTOR", "human")
        .env("THC_DEMO_SANDBOX", &root);
    eprintln!(
        "THC teaching demo · synthetic scratch notes only · F2 next / Shift-F2 back / F3 dismiss / Ctrl-Q quit"
    );
    eprintln!("Scratch session: {}", root.display());
    let status = cmd.status();
    if args.keep {
        let kept = scratch.keep();
        eprintln!("Scratch session preserved at {}", kept.display());
    } else {
        scratch.close()?;
        eprintln!("Scratch session removed (use --keep to preserve your experiments).");
    }
    if !status?.success() {
        bail!("teaching session exited unsuccessfully");
    }
    Ok(())
}

fn child() -> Result<()> {
    let root = std::env::current_dir()?.canonicalize()?;
    let supplied = std::env::var_os("THC_DEMO_SANDBOX").map(std::path::PathBuf::from);
    anyhow::ensure!(
        supplied.as_ref() == Some(&root)
            && std::fs::read(root.join(".teaching-sandbox"))? == b"thc-demo-v1\n"
            && std::env::var_os("HOME") == Some(root.join("home").into_os_string())
            && std::env::var_os("THC_CONFIG_DIR") == Some(root.join("config").into_os_string()),
        "demo child requires its isolated parent"
    );
    let paths = Paths {
        vault: root.join("vault"),
        cache: root.join("cache/vault"),
    };
    thc_core::vault::init(&paths.vault, None, None)?;
    thc_core::settings::init(Some(&paths.vault));
    let mut vault = Vault::open(
        paths,
        Actor {
            kind: "human".into(),
            name: None,
        },
        "tui",
    )?;
    thc_tui::teaching::seed(&mut vault)?;
    if let Ok(size) = std::env::var("THC_TUI_SNAPSHOT") {
        let (w, h) = size
            .split_once('x')
            .ok_or_else(|| anyhow::anyhow!("snapshot size is WxH"))?;
        println!(
            "{}",
            thc_tui::teaching_snapshot(
                vault,
                w.parse()?,
                h.parse()?,
                &std::env::var("THC_TUI_KEYS").unwrap_or_default()
            )?
        );
        Ok(())
    } else {
        thc_tui::run_teaching(vault)
    }
}
