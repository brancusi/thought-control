//! Project work boards are independent of the default capture vault (team.md §1).
use crate::{error::invalid, registry::Registry, settings, store::Store, vault::{Paths, PROJECT_CONFIG}};
use anyhow::{Context, Result};
use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};

pub const NO_BOARD: &str = "no board for this project · add board = \"<vault>:¶ <Page>\" to .thc.toml, or pass --board";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Board {
    pub vault: String,
    pub path: PathBuf,
    pub page: Option<Page>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<PathBuf>,
}

impl Board {
    pub fn describe(&self) -> String {
        match self.source.as_str() {
            "flag" => "from --board".into(),
            "env" => "from THC_BOARD".into(),
            "project" => format!("from {} (board)", crate::vault::tilde(self.source_file.as_deref().unwrap())),
            _ => "from the current vault's [capture] target".into(),
        }
    }
}

pub struct Selection {
    pub paths: Paths,
    page: Option<String>,
    source: String,
    source_file: Option<PathBuf>,
}

/// A named board can have a root queue. An implicit board needs a capture page.
pub fn resolve(flag: Option<&str>, current: Option<&Path>) -> Result<Selection> {
    let env = std::env::var("THC_BOARD").ok().filter(|s| !s.trim().is_empty());
    let explicit = flag.filter(|s| !s.trim().is_empty()).map(|s| (s.to_string(), "flag"))
        .or_else(|| env.map(|s| (s, "env")));
    let (raw, source, file) = match explicit {
        Some((raw, source)) => (Some(raw), source, None),
        None => match project_board()? {
            Some((raw, file)) => (Some(raw), "project", Some(file)),
            None => (None, "capture", None),
        },
    };
    let (paths, page) = if let Some(raw) = raw {
        let (vault, page) = raw.split_once(':').map_or((raw.as_str(), None), |(v, p)| (v, Some(p)));
        if vault.trim().is_empty() || page.is_some_and(|p| p.trim().is_empty()) {
            return Err(invalid("board must be <vault> or <vault>:¶ <Page>"));
        }
        let paths = Paths::resolve(Some(Path::new(vault.trim())))?;
        let target = settings::load(Some(&paths.vault)).str("capture.target").map(str::to_string);
        (paths, page.map(str::to_string).or(target))
    } else {
        let paths = Paths::resolve(current).map_err(|e| if e.to_string().contains("no vault found") { invalid(NO_BOARD) } else { e })?;
        let target = settings::load(Some(&paths.vault)).str("capture.target").map(str::to_string).filter(|t| !t.trim().is_empty());
        let target = target.ok_or_else(|| invalid(NO_BOARD))?;
        (paths, Some(target))
    };
    Ok(Selection { paths, page, source: source.into(), source_file: file })
}

impl Selection {
    pub fn finish(&self, store: &Store) -> Result<Board> {
        let page = self.page.as_deref().map(|raw| -> Result<Page> {
            let title = raw.trim().trim_start_matches('¶').trim();
            let id = store.find_root_by_title(title, false)?.ok_or_else(|| invalid(format!("board page ¶ {title} does not exist · create it or choose another --board")))?;
            Ok(Page { id, title: title.into() })
        }).transpose()?;
        Ok(Board { vault: Registry::load().name_for(&self.paths.vault), path: self.paths.vault.clone(), page, source: self.source.clone(), source_file: self.source_file.clone() })
    }
}

fn project_board() -> Result<Option<(String, PathBuf)>> {
    let mut dir = std::env::current_dir()?;
    loop {
        let file = dir.join(PROJECT_CONFIG);
        if file.exists() {
            crate::sandbox::check(&file);
            let cfg: toml::Table = toml::from_str(&std::fs::read_to_string(&file)?).with_context(|| format!("parsing {}", file.display()))?;
            return match cfg.get("board") {
                Some(v) => Ok(Some((v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| invalid("project board must be a nonempty string"))?.into(), file))),
                None => Ok(None),
            };
        }
        if !dir.pop() { return Ok(None); }
    }
}
