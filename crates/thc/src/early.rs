//! Commands that run before any vault is open (setup, update, keys, schema, instructions,
//! role, parse, init, vault, release-tool) or with only its paths (daemon, config).

use crate::cli::{Cli, DaemonCmd, ReleaseToolCmd, RoleCmd, VaultCmd};
use crate::out::Out;
use crate::{bootstrap, daemon_cmd, instructions, schema, setup, update, vault_cmd, wezterm};
use anyhow::Result;
use serde_json::json;
use std::io::IsTerminal;
use std::path::PathBuf;
use thc_core::error::usage;
use thc_core::vault::{self, Paths};

pub const INIT: crate::registry::Spec = crate::early!("init", "Create a vault (default ./vault) and a .thc.toml pointing at it", InitArgs, init);
pub const PARSE: crate::registry::Spec = crate::early!("parse", "Preview what a capture would save (the daemon/TUI/ThoughtBar lenient parser). Writes nothing", ParseArgs, |cli: &Cli, a: ParseArgs| {
    let mut out = Out::new(cli.json);
    let r = crate::parse_cmd(&mut out, &a.text);
    out.flush();
    r
});
pub const KEYS: crate::registry::Spec = crate::early!("keys", "The TUI's keymap: every key by context (--json for the table, --markdown for the guide's tables, --conflicts for what a remap would be refused for). --edit opens your config at a generated, commented list of every binding to remap", KeysArgs, keys);
pub const ROLE: crate::registry::Spec = crate::early!("role", "Built-in team role cards", RoleArgs, |cli: &Cli, a: RoleArgs| match a.cmd {
    RoleCmd::Show { name } => crate::print_role(&name, cli.json),
});
pub const INSTRUCTIONS: crate::registry::Spec = crate::early!("instructions", "The agent guide built into the binary: thc instructions [topic|all]", InstructionsArgs, instructions_cmd);
pub const SETUP: crate::registry::Spec = crate::early!("setup", "Install or refresh agent files (skill, AGENTS.md block, optional hook) from this binary", SetupArgs, setup_cmd);
pub const UPDATE: crate::registry::Spec = crate::early!("update", "Update thc to the latest release (verified), restarting the daemon on it", UpdateArgs, |cli: &Cli, a: UpdateArgs| {
    let mut out = Out::new(cli.json);
    let r = update::run(&mut out, a.check, a.rollback, a.finish.as_deref());
    out.flush();
    r
});
pub const RELEASE_TOOL: crate::registry::Spec = crate::registry::Spec {
    command: || <ReleaseToolArgs as clap::Args>::augment_args(clap::Command::new("release-tool")).about("Release tooling (CI): make the update key, sign an artifact").hide(true),
    ..crate::early!("release-tool", "Release tooling (CI): make the update key, sign an artifact", ReleaseToolArgs, |_: &Cli, a: ReleaseToolArgs| crate::release_tool(&a.cmd))
};
pub const SCHEMA: crate::registry::Spec = crate::early!("schema", "JSON Schema of a command's input and output (`thc schema --all` for code generation)", SchemaArgs, |_: &Cli, a: SchemaArgs| {
    if a.swift {
        print!("{}", schema::swift_types());
        return Ok(());
    }
    let mut out = Out::new(true);
    out.json(&if a.proto { schema::proto_schema() } else { schema::schema(a.cmd.as_deref(), a.all)? });
    out.flush();
    Ok(())
});
pub const VAULT: crate::registry::Spec = crate::early!("vault", "Vaults: one per project. No argument: the current vault and why (vaults.md)", VaultArgs, |cli: &Cli, a: VaultArgs| {
    let mut out = Out::new(cli.json);
    let r = vault_cmd::run(&mut out, cli.vault.as_deref(), a.cmd);
    out.flush();
    r
});
pub const DAEMON: crate::registry::Spec = crate::with_paths!("daemon", "The background daemon: live updates, alerts, other devices' changes (optional)", DaemonArgs, daemon);
pub const CONFIG: crate::registry::Spec = crate::with_paths!("config", "Settings in effect, layered (yours, the vault's settings.toml, your per-vault override), with where each came from", ConfigArgs, |cli: &Cli, paths: &Paths, a: ConfigArgs| {
    let mut out = Out::new(cli.json);
    let r = crate::config_cmd(&mut out, &paths.vault, a.effective, a.why.as_deref());
    out.flush();
    r
});

#[derive(clap::Args, Debug)]
pub struct InitArgs {
    #[arg(default_value = "vault")]
    path: PathBuf,
    /// Local cache dir recorded in .thc.toml (relative to the current dir).
    #[arg(long, default_value = ".thc-cache")]
    cache: String,
    /// Make it this user's default vault (~/.config/thought/config.toml) instead of writing a
    /// .thc.toml here. Works on an existing vault too.
    #[arg(long)]
    global: bool,
    }

#[derive(clap::Args, Debug)]
pub struct ParseArgs {
    text: Vec<String>,
    }

#[derive(clap::Args, Debug)]
pub struct KeysArgs {
    #[arg(long)]
    markdown: bool,
    #[arg(long)]
    conflicts: bool,
    /// Open config.toml in $EDITOR at the keys block (made or refreshed first), then check it.
    #[arg(long)]
    edit: bool,
    /// With --edit: open at this context's table (write, list, today, …).
    #[arg(long)]
    context: Option<String>,
    /// With --edit: print the block instead of opening the editor.
    #[arg(long)]
    print: bool,
    }

#[derive(clap::Args, Debug)]
pub struct RoleArgs {
    #[command(subcommand)]
    cmd: RoleCmd,
}

#[derive(clap::Args, Debug)]
pub struct InstructionsArgs {
    topic: Option<String>,
    name: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct SetupArgs {
    /// claude or codex: that agent's files (this repo's, or the user's with --user); wezterm:
    /// macOS editing keys for WezTerm (thc's own thc_keys.lua; your wezterm.lua isn't
    /// touched). Without it, set up the machine: vault, the thc command, the login item and
    /// agent skills.
    agent: Option<String>,
    /// The agent's user-level files (~/.claude/skills/thc, ~/.codex/AGENTS.md), not this repo's.
    #[arg(long)]
    user: bool,
    /// Set up everything without asking (install.sh: `thc setup --all --yes`).
    #[arg(long)]
    all: bool,
    /// One step: vault, path, login or agents (the Mac app ticks its lines with these).
    #[arg(long)]
    step: Option<String>,
    /// With --step agents: only these (comma-separated), instead of every agent found.
    #[arg(long, value_delimiter = ',')]
    agents: Option<Vec<String>>,
    /// What setup added, where it is and whether it's current (from ~/.config/thought/setup.json).
    #[arg(long)]
    status: bool,
    /// Remove everything setup added. Never the vault.
    #[arg(long)]
    undo: bool,
    /// Record a thc link made outside setup (the app's Install for all users…), so --undo
    /// knows about it.
    #[arg(long, hide = true)]
    record_link: Option<PathBuf>,
    /// Show what would change without writing (default when stdin isn't a terminal).
    #[arg(long)]
    dry_run: bool,
    /// Exit 1 if the files are stale for this binary.
    #[arg(long)]
    check: bool,
    /// Also add a SessionStart hook that runs `thc prime` (claude).
    #[arg(long)]
    hook: bool,
    /// Take over from Thought Central.app: this binary becomes ~/.local/bin/thc, the app's
    /// login item goes, thc's own runs the daemon (install.sh does this when it finds the app).
    #[arg(long)]
    migrate_from_app: bool,
    }

#[derive(clap::Args, Debug)]
pub struct UpdateArgs {
    /// Only check: print whether an update is available.
    #[arg(long)]
    check: bool,
    /// Put the previous version (thc.prev) back.
    #[arg(long, conflicts_with = "check")]
    rollback: bool,
    /// After a swap, the new binary finishes the update (restarts the daemon, refreshes the
    /// agent files) with its own logic; the value is the version it should be. Internal.
    #[arg(long, hide = true, value_name = "VERSION")]
    finish: Option<String>,
    }

#[derive(clap::Args, Debug)]
pub struct ReleaseToolArgs {
    #[command(subcommand)]
    cmd: ReleaseToolCmd,
    }

#[derive(clap::Args, Debug)]
pub struct SchemaArgs {
    cmd: Option<String>,
    #[arg(long)]
    all: bool,
    /// The daemon socket protocol (methods, events), for clients like ThoughtBar.
    #[arg(long)]
    proto: bool,
    /// With --proto: Swift Codable models instead of JSON Schema (ThoughtBar's Generated.swift).
    #[arg(long, requires = "proto")]
    swift: bool,
    }

#[derive(clap::Args, Debug)]
pub struct VaultArgs {
    #[command(subcommand)]
    cmd: Option<VaultCmd>,
    }

#[derive(clap::Args, Debug)]
pub struct DaemonArgs {
    #[command(subcommand)]
    cmd: DaemonCmd,
}

#[derive(clap::Args, Debug)]
pub struct ConfigArgs {
    /// Every setting in effect and its source (the default).
    #[arg(long)]
    effective: bool,
    /// Every layer that sets one key, e.g. tui.focus.preset.
    #[arg(long, value_name = "KEY")]
    why: Option<String>,
    }

fn init(cli: &Cli, a: InitArgs) -> Result<()> {
    let InitArgs { path, cache, global } = a;
    let cwd = std::env::current_dir()?;
    let vault_path = if path.is_absolute() { path.clone() } else { cwd.join(&path) };
    let config = if global {
        vault::init(&vault_path, None, None)?;
        vault::set_global_vault(&vault_path)?
    } else {
        vault::init(&vault_path, Some(&cwd), Some(&cache))?;
        cwd.join(vault::PROJECT_CONFIG)
    };
    let mut out = Out::new(cli.json);
    if cli.json {
        out.json(&json!({ "ok": true, "vault": vault_path, "config": config }));
    } else {
        out.line(format!("vault ready at {}", vault_path.display()));
        out.line(format!("config: {}", config.display()));
    }
    out.flush();
    Ok(())
}

fn keys(cli: &Cli, a: KeysArgs) -> Result<()> {
    if a.edit || a.print {
        return crate::keys_edit_cmd(a.context.as_deref(), a.print);
    }
    let mut out = Out::new(false);
    if a.conflicts {
        let c = thc_tui::keys_conflicts();
        if cli.json {
            out.line(json!({"conflicts": c}).to_string());
        } else if c.is_empty() {
            out.line("no conflicts");
        } else {
            for l in &c {
                out.line(l);
            }
        }
        out.flush();
        if !c.is_empty() {
            return Err(thc_core::error::invalid(format!("{} keymap conflict{}", c.len(), if c.len() == 1 { "" } else { "s" })));
        }
        return Ok(());
    }
    if cli.json {
        out.line(serde_json::to_string_pretty(&thc_tui::keys_json())?);
    } else if a.markdown {
        out.line(thc_tui::keys_markdown().trim_end());
    } else {
        out.line(thc_tui::keys_text().trim_end());
    }
    out.flush();
    Ok(())
}

fn instructions_cmd(cli: &Cli, a: InstructionsArgs) -> Result<()> {
    let InstructionsArgs { topic, name } = a;
    if topic.as_deref() == Some("role") {
        return crate::print_role(name.as_deref().ok_or_else(|| usage("thc instructions role <name>"))?, cli.json);
    }
    if name.is_some() {
        return Err(usage("only thc instructions role takes a name"));
    }
    let mut out = Out::new(false);
    match topic.as_deref() {
        None => out.line(instructions::index()),
        Some("all") => out.line(instructions::all()),
        Some(t) => out.line(instructions::topic(t).ok_or_else(|| thc_core::error::not_found(format!("no topic {t} · thc instructions lists them")))?),
    }
    out.flush();
    Ok(())
}

fn setup_cmd(cli: &Cli, a: SetupArgs) -> Result<()> {
    let SetupArgs { agent, user, all, step, agents, status, undo, record_link, dry_run, check, hook, migrate_from_app } = a;
    let (agent, user, all, step, agents, status, undo, record_link, dry_run, check, hook, migrate_from_app) = (&agent, &user, &all, &step, &agents, &status, &undo, &record_link, &dry_run, &check, &hook, &migrate_from_app);
    let yes = cli.yes;
    let mut out = Out::new(cli.json);
    let r = (|| -> Result<()> {
        if *migrate_from_app {
            return bootstrap::migrate_from_app(&mut out);
        }
        // Not an agent: WezTerm's macOS editing keys (keymap.md §12.5), and --undo for them.
        if agent.as_deref() == Some("wezterm") {
            return wezterm::setup(&mut out, yes, *dry_run, *undo);
        }
        if *status {
            return bootstrap::status(&mut out);
        }
        if let Some(link) = record_link {
            return bootstrap::record_link(&mut out, link);
        }
        if *undo {
            // `thc setup claude --user --undo` removes only that agent's files (a Settings toggle).
            return bootstrap::undo(&mut out, *dry_run, agent.as_deref());
        }
        if let Some(s) = step {
            if !bootstrap::STEPS.contains(&s.as_str()) {
                return Err(usage(format!("unknown step {s} (vault, path, login, agents)")));
            }
            let r = bootstrap::run(&mut out, &[s.as_str()], agents.clone())?;
            return if r.iter().any(|s| s.state == "failed") { Err(anyhow::anyhow!(daemon_cmd::Quiet)) } else { Ok(()) };
        }
        match agent {
            Some(a) if *user => {
                if *check {
                    return bootstrap::check_user(&mut out, a);
                }
                let r = bootstrap::run(&mut out, &["agents"], Some(vec![a.clone()]))?;
                if r.iter().any(|s| s.state == "failed") { Err(anyhow::anyhow!(daemon_cmd::Quiet)) } else { Ok(()) }
            }
            Some(a) => {
                let dry = *dry_run || (!yes && !std::io::stdin().is_terminal());
                setup::run(&mut out, a, dry, *check, *hook, !*dry_run && dry)
            }
            None => {
                // The whole machine (setup.md §3.3). Interactive runs ask before teaching agents.
                let interactive = !*all && !yes && std::io::stdin().is_terminal();
                let chosen = if interactive { bootstrap::ask_agents()? } else { None };
                let r = bootstrap::run(&mut out, &bootstrap::STEPS, chosen)?;
                if interactive {
                    wezterm::offer(&mut out)?;
                }
                if r.iter().any(|s| s.state == "failed") { Err(anyhow::anyhow!(daemon_cmd::Quiet)) } else { Ok(()) }
            }
        }
    })();
    out.flush();
    r
}

fn daemon(cli: &Cli, paths: &Paths, a: DaemonArgs) -> Result<()> {
    if matches!(a.cmd, DaemonCmd::Run) {
        return thc_daemon::run(paths.clone(), thc_daemon::RunOpts { json: cli.json, channel: None, label: None, host: true });
    }
    if !paths.vault.join(vault::VAULT_MARKER).exists() {
        return Err(usage(format!("no vault at {} · thc init {} to create one, or set THC_VAULT", vault::tilde(&paths.vault), vault::tilde(&paths.vault))));
    }
    let mut out = Out::new(cli.json);
    let r = daemon_cmd::run(&mut out, paths, &a.cmd);
    out.flush();
    r
}
