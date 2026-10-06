//! Optional first-page PDF thumbnails. Local cache only; no derived files in the vault.
use crate::{attach::Stored, id, sandbox};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn path(cache: &Path, rel: &str) -> PathBuf {
    cache
        .join("thumbs")
        .join(format!("pdf-{}.png", id::from_key(&format!("file:{rel}"))))
}

pub fn cached_thumbnail(cache: &Path, rel: &str) -> Option<PathBuf> {
    let p = path(cache, rel);
    p.is_file().then_some(p)
}

/// Best effort: use an installed Poppler renderer, bounded to three seconds. Tests must
/// explicitly supply a shim; no cargo test ever launches a real renderer.
pub fn pdf_thumbnail(cache: &Path, file: &Stored) -> Option<PathBuf> {
    if file.mime != "application/pdf" {
        return None;
    }
    if let Some(p) = cached_thumbnail(cache, &file.path) {
        return Some(p);
    }
    let renderer = if sandbox::active() {
        let p = PathBuf::from(std::env::var_os("THC_PDF_RENDERER")?);
        sandbox::check(&p);
        p
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join("pdftoppm"))
            .find(|p| p.is_file())?
    };
    let target = path(sandbox::check(cache), &file.path);
    std::fs::create_dir_all(target.parent()?).ok()?;
    // A unique temporary directory prevents two attaches from sharing partial renderer output.
    let dir = target
        .parent()?
        .join(format!(".pdf-{}", crate::id::new_id()));
    std::fs::create_dir(&dir).ok()?;
    let render = || -> Option<PathBuf> {
        let prefix = dir.join("first-page");
        let mut child = Command::new(renderer)
            .args([
                "-f",
                "1",
                "-l",
                "1",
                "-singlefile",
                "-scale-to",
                "640",
                "-png",
            ])
            .arg(sandbox::check(&file.abs))
            .arg(&prefix)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let success = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s.success(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
            }
        };
        if !success {
            return None;
        }
        let png = prefix.with_extension("png");
        let data = std::fs::read(&png).ok()?;
        let (w, h) = crate::attach::dims(&data)?;
        if !data.starts_with(b"\x89PNG\r\n\x1a\n") || w == 0 || h == 0 || w.max(h) > 640 {
            return None;
        }
        std::fs::rename(png, &target).ok()?;
        Some(target.clone())
    };
    let result = render();
    let _ = std::fs::remove_dir_all(dir);
    result
}
