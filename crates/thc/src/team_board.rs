//! First-run board setup: deterministic creation behind explicit consent.
use crate::{cli::Cli, out::Out, team_cmd::Member};
use anyhow::Result;
use std::{
    io::{IsTerminal, Write},
    path::Path,
};
use thc_core::{
    board::{self, Board},
    builder::TxBuilder,
    capture::Capture,
    error::invalid,
    registry::Registry,
    vault::{Paths, Vault},
};

pub fn resolve(cli: &Cli) -> Result<(Vault, Board)> {
    let selection = board::resolve(cli.board.as_deref(), cli.vault.as_deref())?;
    let vault = Vault::open(
        selection.paths.clone(),
        crate::parse_actor(cli.actor.as_deref()),
        "cli",
    )?;
    let board = selection.finish(&vault.store)?;
    Ok((vault, board))
}

pub fn prepare(
    cli: &Cli,
    root: &Path,
    members: &[Member],
    new: Option<&str>,
    out: &mut Out,
) -> Result<Option<(Vault, Board, bool)>> {
    let resolution = resolve(cli);
    if new.is_none() {
        match resolution {
            Ok((v, b)) => return Ok(Some((v, b, false))),
            Err(e) if e.to_string().contains("no board for this project") => {}
            Err(e) => return Err(e),
        }
    } else if resolution.is_ok() || cli.board.is_some() || std::env::var_os("THC_BOARD").is_some() {
        return Err(invalid(
            "a board is already configured · choose --board, or create a board in a new project",
        ));
    }
    let mut reg = Registry::load();
    let suggested = thc_core::registry::name_from_path(root);
    let matched = reg
        .vaults
        .iter()
        .find(|e| {
            e.name == suggested
                || thc_core::settings::load(Some(&e.path))
                    .str("vault.name_short")
                    .is_some_and(|n| n == suggested)
        })
        .map(|e| e.name.clone());
    let mut name = new.map(str::to_string).or(matched).unwrap_or(suggested);
    if cli.dry_run {
        out.json(&serde_json::json!({"dry_run":true,"new_board":name,"project":root,"roles":members.iter().map(|m| &m.role).collect::<Vec<_>>()}));
        return Ok(None);
    }
    if new.is_none() {
        if !std::io::stdin().is_terminal()
            || cli.json
            || crate::parse_actor(cli.actor.as_deref()).kind != "human"
        {
            return Err(invalid(
                "no board · run thc team up in a terminal, or pass --board <vault[:¶ Page]> or --new-board <name>",
            ));
        }
        let verb = if reg.find(&name).is_some() {
            "use"
        } else {
            "create"
        };
        eprint!(
            "no board for this project\n{verb} vault {name}, board ¶ Issues? y yes · n no · or type another name: "
        );
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let answer = answer.trim();
        if answer.is_empty()
            || answer.eq_ignore_ascii_case("n")
            || answer.eq_ignore_ascii_case("no")
        {
            return Err(invalid("board setup cancelled · nothing created"));
        }
        if !answer.eq_ignore_ascii_case("y") && !answer.eq_ignore_ascii_case("yes") {
            name = answer.into();
        }
    }
    if !thc_core::registry::valid_name(&name) {
        return Err(invalid(
            "new board needs a valid vault name (lowercase letters, digits and -)",
        ));
    }
    let lead = members.iter().find(|m| matches!(m.role.as_str(),"pm"|"lead")).ok_or_else(|| invalid("a new board needs a pm or lead · add pm (thc team up pm engineer …), or point at an existing board"))?;
    let config = root.join(thc_core::vault::PROJECT_CONFIG);
    thc_core::sandbox::check(&config);
    let mut project: toml::Table = if config.exists() {
        toml::from_str(&std::fs::read_to_string(&config)?)?
    } else {
        toml::Table::new()
    };
    if project.contains_key("board") {
        return Err(invalid("project already has a board · nothing replaced"));
    }
    // Validate existing settings before creating pages or recording any board transaction.
    let settings_path = reg
        .find(&name)
        .map(|e| e.path.join(thc_core::settings::VAULT_FILE));
    let mut table: toml::Table = match settings_path.as_ref().filter(|p| p.exists()) {
        Some(p) => toml::from_str(&std::fs::read_to_string(p)?)?,
        None => toml::Table::new(),
    };
    if table.get("capture").is_some_and(|v| !v.is_table()) {
        return Err(invalid("capture settings must be a table"));
    }
    let entry = match reg.find(&name) {
        Some(e) => e.clone(),
        None => {
            let entry = thc_core::registry::create_vault(&mut reg, &name, None)?.0;
            table = toml::from_str(&std::fs::read_to_string(
                entry.path.join(thc_core::settings::VAULT_FILE),
            )?)?;
            entry
        }
    };
    let paths = Paths::resolve(Some(&entry.path))?;
    let mut vault = Vault::open(paths, crate::parse_actor(cli.actor.as_deref()), "team")?;
    let fresh = vault.store.find_root_by_title("Issues", false)?.is_none();
    let readme = ["README.md", "readme.md", "README"]
        .iter()
        .find_map(|n| std::fs::read_to_string(root.join(n)).ok())
        .unwrap_or_default();
    let title = readme
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Project")
        });
    let paragraph = readme
        .split("\n\n")
        .find(|p| !p.trim().is_empty() && !p.trim().starts_with('#'))
        .unwrap_or("")
        .trim();
    let today = thc_core::dates::today();
    let (_, issue) = vault.transact(|store| {
        let mut b = TxBuilder::new(store, today);
        b.plain = true;
        let issues = b.page("Issues", true)?.unwrap();
        if fresh {
            let id = b.create_from_capture(
                Some(issues.clone()),
                &Capture {
                    text: "First plan".into(),
                    status: Some("todo".into()),
                    ..Default::default()
                },
                Some(thc_core::id::from_key("team-first-plan")),
            )?;
            b.ops.push(thc_core::event::Op::NodeSet {
                id,
                props: serde_json::json!({"owner":lead.actor,"role":lead.role})
                    .as_object()
                    .unwrap()
                    .clone(),
            });
        }
        if store.find_root_by_title("About", false)?.is_none() {
            let about = b.page("About", true)?.unwrap();
            for text in [
                title.to_string(),
                format!("{paragraph}\n(from README.md · the lead refines this)"),
                "## Members".into(),
            ] {
                b.create_from_capture(
                    Some(about.clone()),
                    &Capture {
                        text,
                        ..Default::default()
                    },
                    None,
                )?;
            }
            for m in members {
                b.create_from_capture(
                    Some(about.clone()),
                    &Capture {
                        text: format!("{} · {}", m.actor, m.role),
                        ..Default::default()
                    },
                    None,
                )?;
            }
        }
        let roles = b.page("Roles", true)?.unwrap();
        let present: Vec<_> = store
            .children(&roles)?
            .iter()
            .map(|n| n.text.clone())
            .collect();
        for role in members
            .iter()
            .map(|m| &m.role)
            .collect::<std::collections::BTreeSet<_>>()
        {
            if !present
                .iter()
                .any(|r| r.lines().next() == Some(role.as_str()))
            {
                b.create_from_capture(
                    Some(roles.clone()),
                    &Capture {
                        text: role.clone(),
                        ..Default::default()
                    },
                    None,
                )?;
            }
        }
        if thc_core::views::find(store, "issues")?.is_none() {
            thc_core::views::add(
                &mut b,
                "issues",
                &format!("under:{issues} is:task status:open sort:order"),
                Some("Issues"),
                None,
                true,
                None,
            )?;
        }
        Ok((b.finish(), issues))
    })?;
    let settings = entry.path.join(thc_core::settings::VAULT_FILE);
    table
        .entry("capture")
        .or_insert(toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| invalid("capture settings must be a table"))?
        .insert("target".into(), toml::Value::String("¶ Issues".into()));
    std::fs::write(settings, toml::to_string_pretty(&table)?)?;
    project
        .entry("vault")
        .or_insert(toml::Value::String(name.clone()));
    project.insert(
        "board".into(),
        toml::Value::String(format!("{name}:¶ Issues")),
    );
    std::fs::write(&config, toml::to_string_pretty(&project)?)?;
    let board = Board {
        vault: name,
        path: entry.path,
        page: Some(board::Page {
            id: issue,
            title: "Issues".into(),
        }),
        source: "project".into(),
        source_file: Some(config),
    };
    if !cli.json {
        out.line(format!(
            "board ready · {} · ¶ Issues · ¶ About from README.md",
            board.vault
        ));
    }
    Ok(Some((vault, board, fresh)))
}
