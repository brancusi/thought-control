//! macOS area capture, using the same attachment transaction as a supplied file.
use crate::{
    Ctx,
    attachment_cmd::{self, Destination},
};
use anyhow::Result;
use serde_json::json;
use thc_core::error::invalid;

pub const SPEC: crate::registry::Spec = crate::spec!("shot", "Capture a macOS screen area and attach it (default: today's journal)", ShotArgs, |ctx: &mut Ctx, a: ShotArgs| run(ctx, a.under.as_deref(), a.journal.as_deref(), a.caption.as_deref()), settings: true, verbs: |_| vec!["attach".into(), "add".into()]);

#[derive(clap::Args, Debug)]
pub struct ShotArgs {
    #[arg(long, conflicts_with = "journal")]
    under: Option<String>,
    #[arg(long)]
    journal: Option<String>,
    #[arg(long)]
    caption: Option<String>,
}

pub fn run(
    ctx: &mut Ctx,
    under: Option<&str>,
    journal: Option<&str>,
    caption: Option<&str>,
) -> Result<()> {
    let dest = match under {
        Some(id) => Destination::Under(ctx.resolve(id)?),
        None => Destination::Journal(
            thc_core::dates::parse(journal.unwrap_or("today"), ctx.out.today)?.date(),
        ),
    };
    if ctx.dry_run {
        ctx.out.json(&json!({"dry_run": true, "capture": "macOS area screenshot", "under": under, "journal": journal.unwrap_or("today")}));
        return Ok(());
    }
    // Refusal must precede the capture UI, not just the eventual transaction.
    ctx.guard()?;
    let temp = tempfile::Builder::new().prefix("thc-shot-").tempdir()?;
    let file = temp.path().join("screenshot.png");
    if thc_core::sandbox::active() {
        // Fixture seam: no test can invoke screencapture, even accidentally.
        let fixture = std::env::var_os("THC_SHOT_IMAGE").ok_or_else(|| {
            invalid("THC_TEST: shot requires THC_SHOT_IMAGE; capture UI is disabled")
        })?;
        let fixture = std::path::PathBuf::from(fixture);
        thc_core::sandbox::check(&fixture);
        if fixture.as_os_str().is_empty() {
            return cancelled(ctx);
        }
        std::fs::copy(&fixture, &file)
            .map_err(|e| invalid(format!("can't read screenshot fixture: {e}")))?;
    } else {
        if !cfg!(target_os = "macos") {
            return Err(invalid(
                "thc shot requires macOS · use thc attach ID FILE on this platform",
            ));
        }
        let status = std::process::Command::new("screencapture")
            .args(["-i", "-s", "-x", "-t", "png"])
            .arg(&file)
            .status()
            .map_err(|e| invalid(format!("can't run screencapture: {e}")))?;
        // Escape commonly returns success without a file. A failed process is an error unless
        // it was interrupted; permission failures should remain visible to the user.
        if !status.success() {
            return Err(invalid(format!("screencapture failed ({status})")));
        }
    }
    if !file.is_file() || std::fs::metadata(&file)?.len() == 0 {
        return cancelled(ctx);
    }
    let caption = caption
        .map(str::to_string)
        .unwrap_or_else(|| format!("screenshot {}", chrono::Local::now().format("%H:%M")));
    attachment_cmd::commit(ctx, dest, &[file], Some(&caption))
}

fn cancelled(ctx: &mut Ctx) -> Result<()> {
    if ctx.out.json {
        ctx.out.json(&json!({"ok": true, "cancelled": true}));
    } else {
        ctx.out.line("screenshot cancelled · nothing attached");
    }
    Ok(())
}
