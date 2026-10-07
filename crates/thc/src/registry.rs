//! The command registry: a command lives in its own module and registers here
//! with one line, so adding one never touches cli.rs or main.rs. Each module exports a `SPEC`:
//! its clap arguments, what it needs before it runs (the project board, the vault's settings),
//! the policy verbs it asks for, and how it runs. Commands not moved here yet are still the
//! `Cmd` enum in cli.rs; main parses both from one clap command.

use crate::Ctx;
use anyhow::Result;
use clap::ArgMatches;

/// A registered command.
pub struct Spec {
    pub name: &'static str,
    /// The subcommand with its arguments and help.
    pub command: fn() -> clap::Command,
    /// Runs against the project's work board (`--board`, THC_BOARD, .thc.toml, the capture
    /// target: thc_core::board), not just the current vault.
    pub board: bool,
    /// Reads the vault's layered settings (the capture target, attachments).
    pub settings: bool,
    /// Its arguments are node ids: `acme/k7q2m` runs it in vault acme (vaults.md §1).
    pub ids: bool,
    /// Human listings show the selected vault when it differs from home.
    pub listing: bool,
    /// Runs before any vault is resolved (setup, update, schema, keys…): `run` is never called.
    pub before: Option<fn(&crate::cli::Cli, &ArgMatches) -> Result<()>>,
    /// Runs once the vault's paths are known, without opening it (daemon, config).
    pub with_paths: Option<fn(&crate::cli::Cli, &thc_core::vault::Paths, &ArgMatches) -> Result<()>>,
    /// The policy verbs it needs (policy.md §2): what an agent's tier is checked against.
    pub verbs: fn(&ArgMatches) -> Vec<String>,
    pub run: fn(&mut Ctx, &ArgMatches) -> Result<()>,
}

/// A Spec from a clap `Args` type: `spec!("next", "about…", NextArgs, run)`, then the flags.
#[macro_export]
macro_rules! spec {
    ($name:literal, $about:literal, $args:ty, $run:expr $(, $field:ident: $value:expr)* $(,)?) => {
        $crate::registry::Spec {
            name: $name,
            // The about after the arguments: an Args type's own doc comment would replace it.
            command: || <$args as clap::Args>::augment_args(clap::Command::new($name)).about($about),
            run: |ctx, m| {
                let a = <$args as clap::FromArgMatches>::from_arg_matches(m)?;
                $run(ctx, a)
            },
            $($field: $value,)*
            ..$crate::registry::DEFAULT
        }
    };
}

/// A Spec that runs before any vault: `early!("schema", "about…", SchemaArgs, run)`, with
/// `run(&Cli, SchemaArgs)`.
#[macro_export]
macro_rules! early {
    ($name:literal, $about:literal, $args:ty, $run:expr) => {
        $crate::registry::Spec {
            name: $name,
            command: || <$args as clap::Args>::augment_args(clap::Command::new($name)).about($about),
            before: Some(|cli, m| $run(cli, <$args as clap::FromArgMatches>::from_arg_matches(m)?)),
            ..$crate::registry::DEFAULT
        }
    };
}

/// A Spec that runs with the vault's paths, before opening it: `run(&Cli, &Paths, Args)`.
#[macro_export]
macro_rules! with_paths {
    ($name:literal, $about:literal, $args:ty, $run:expr) => {
        $crate::registry::Spec {
            name: $name,
            command: || <$args as clap::Args>::augment_args(clap::Command::new($name)).about($about),
            with_paths: Some(|cli, paths, m| $run(cli, paths, <$args as clap::FromArgMatches>::from_arg_matches(m)?)),
            ..$crate::registry::DEFAULT
        }
    };
}

/// What a Spec is unless it says otherwise.
pub const DEFAULT: Spec = Spec { name: "", command: || clap::Command::new(""), board: false, settings: false, ids: false, listing: false, before: None, with_paths: None, verbs: |_| vec![], run: |_, _| Ok(()) };

/// Every registered command. One line each.
pub const ALL: &[&Spec] = &[
    &crate::next::SPEC,
    &crate::next::BOARD,
    &crate::msg::SPEC,
    &crate::msg::LIST,
    &crate::watch::SPEC,
    &crate::shot::SPEC,
    &crate::capture_cmd::ADD,
    &crate::capture_cmd::TODO,
    &crate::capture_cmd::REMIND,
    &crate::capture_cmd::ATTACH,
    &crate::alert_cmd::SPEC,
    &crate::view_cmd::SPEC,
    &crate::early::INIT,
    &crate::early::PARSE,
    &crate::early::KEYS,
    &crate::early::ROLE,
    &crate::early::INSTRUCTIONS,
    &crate::early::SETUP,
    &crate::early::UPDATE,
    &crate::early::RELEASE_TOOL,
    &crate::early::SCHEMA,
    &crate::early::VAULT,
    &crate::early::DAEMON,
    &crate::early::CONFIG,
    &crate::status_cmd::SPEC,
    &crate::lifecycle::DONE,
    &crate::lifecycle::REOPEN,
    &crate::lifecycle::SKIP,
    &crate::lifecycle::RM,
    &crate::lifecycle::RESTORE,
    &crate::edits::SET,
    &crate::edits::TEXT,
    &crate::edits::TAG,
    &crate::edits::LINK,
    &crate::edits::UNLINK,
    &crate::reads::TODAY,
    &crate::reads::AGENDA,
    &crate::reads::INBOX,
    &crate::reads::JOURNAL,
    &crate::reads::PAGES,
    &crate::reads::SEARCH,
    &crate::inspect::SHOW,
    &crate::inspect::HISTORY,
    &crate::inspect::DIFF,
    &crate::ocr_cmd::SPEC,
    &crate::ui_cmd::SPEC,
];

pub fn find(name: &str) -> Option<&'static Spec> {
    ALL.iter().copied().find(|s| s.name == name)
}

/// The whole CLI: the `Cmd` enum's commands and the registered ones.
pub fn command() -> clap::Command {
    use clap::CommandFactory;
    ALL.iter().fold(crate::cli::Cli::command(), |c, s| c.subcommand((s.command)()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_commands_are_complete_and_unique() {
        use clap::Subcommand;
        let mut names: Vec<&str> = ALL.iter().map(|s| s.name).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len(), "a command is registered twice");
        for s in ALL {
            assert!(!crate::cli::Cmd::has_subcommand(s.name), "{} is both registered and in Cmd", s.name);
            let c = (s.command)();
            assert_eq!(c.get_name(), s.name);
            assert!(c.get_about().is_some_and(|a| !a.to_string().is_empty()), "{} has no help line", s.name);
        }
        // The whole CLI builds (clap checks every argument: no clashes with the global flags).
        command().debug_assert();
    }
}
