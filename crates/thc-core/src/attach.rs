//! Attachments (docs/design/attachments.md): files in the vault, Markdown in the text.
//!
//! A file is copied once into `<vault>/files/<yyyy>/<mm>/<hash>-<name>.<ext>` (content-addressed:
//! the same bytes land at the same path, and nothing there is ever edited), and a note refers to
//! it with a Markdown image, `![caption](files/2026/10/k3m9q-shot.png)`. There's no new op: it's
//! text plus a file, synced with the vault like the log.

pub use crate::attachment_thumbnail::{cached_thumbnail, pdf_thumbnail};
use crate::error::invalid;
use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use std::path::{Path, PathBuf};

/// The default per-file limit (`[attachments] max_mb` in the vault's settings).
pub const DEFAULT_MAX_MB: u64 = 20;

/// A file stored in the vault.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Stored {
    /// Relative to the vault: `files/2026/10/k3m9q-shot.png`.
    pub path: String,
    pub abs: PathBuf,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    pub mime: &'static str,
}

/// A validated file, held in memory until every file in a batch has passed preparation.
pub struct Prepared {
    pub name: String,
    data: Vec<u8>,
}

impl Prepared {
    pub fn bytes(&self) -> u64 { self.data.len() as u64 }
    pub fn data(&self) -> &[u8] { &self.data }
}

pub fn prepare_file(src: &Path, max_mb: u64) -> Result<Prepared> {
    let size = std::fs::metadata(crate::sandbox::check(src)).map_err(|e| invalid(format!("can't read {}: {e}", src.display())))?.len();
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    check_size(size, name, max_mb)?;
    let data = std::fs::read(src).map_err(|e| invalid(format!("can't read {}: {e}", src.display())))?;
    prepare_bytes(&data, name, max_mb)
}

fn prepare_bytes(data: &[u8], name: &str, max_mb: u64) -> Result<Prepared> {
    check_size(data.len() as u64, name, max_mb)?;
    let data = crate::attachment_media::prepare(data, crate::attachment_media::max_dimension(&crate::settings::current()))?;
    check_size(data.len() as u64, name, max_mb)?;
    Ok(Prepared { name: name.to_string(), data })
}

/// Copy `src` into the vault's `files/` (content-addressed), refusing one over `max_mb`.
pub fn store(vault: &Path, src: &Path, today: NaiveDate, max_mb: u64) -> Result<Stored> {
    store_prepared(vault, &prepare_file(src, max_mb)?, today)
}

/// [`store`] for bytes already in hand (a clipboard image), named `name`.
pub fn store_bytes(vault: &Path, data: &[u8], name: &str, today: NaiveDate, max_mb: u64) -> Result<Stored> {
    store_prepared(vault, &prepare_bytes(data, name, max_mb)?, today)
}

pub fn store_prepared(vault: &Path, file: &Prepared, today: NaiveDate) -> Result<Stored> {
    crate::sandbox::check(vault);
    let data = file.data.as_slice();
    let stored = preview_prepared(vault, file, today);
    let abs = &stored.abs;
    if !abs.exists() {
        if let Some(dir) = abs.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = abs.with_extension("tmp-thc");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &abs)?;
    }
    Ok(stored)
}

/// Metadata/path for preflight checks, without touching the vault.
pub fn preview_prepared(vault: &Path, file: &Prepared, today: NaiveDate) -> Stored {
    let (stem, ext) = split_name(&file.name);
    let rel = format!("files/{:04}/{:02}/{}-{}{}", today.year(), today.month(), short_hash(&file.data), slug(&stem), ext.map(|e| format!(".{e}")).unwrap_or_default());
    describe(vault, &rel, Some(&file.data))
}

fn check_size(size: u64, name: &str, max_mb: u64) -> Result<()> {
    let limit = max_mb.saturating_mul(1024 * 1024);
    if size > limit {
        let mb = size.div_ceil(1024 * 1024);
        let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
        return Err(invalid(format!("{stem} is {mb} MB · the limit is {max_mb} MB ([attachments] max_mb)")));
    }
    Ok(())
}

/// What a stored file is: its size, image dimensions and type (read from its header).
pub fn describe(vault: &Path, rel: &str, data: Option<&[u8]>) -> Stored {
    let abs = vault.join(rel);
    let head: Vec<u8> = match data {
        Some(d) => d[..d.len().min(256 * 1024)].to_vec(),
        None => read_head(&abs, 256 * 1024),
    };
    let bytes = data.map(|d| d.len() as u64).or_else(|| std::fs::metadata(&abs).ok().map(|m| m.len())).unwrap_or(0);
    let (w, h) = dims(&head).map_or((None, None), |(w, h)| (Some(w), Some(h)));
    let ext = Path::new(rel).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    Stored { path: rel.to_string(), abs, bytes, w, h, mime: mime(&ext) }
}

fn read_head(p: &Path, n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.take(n as u64).read_to_end(&mut buf);
    }
    buf
}

/// An image the TUI can draw (the rest are chips only).
pub fn is_image(path: &str) -> bool {
    matches!(mime(&Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()), "image/png" | "image/jpeg" | "image/gif" | "image/webp")
}

pub fn mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The attachments a text refers to, in order: (caption, path) for each `![caption](files/…)`.
pub fn refs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("![") {
        let after = &rest[i + 2..];
        let Some(close) = after.find("](") else { break };
        let alt = &after[..close];
        let tail = &after[close + 2..];
        let Some(end) = tail.find(')') else { break };
        let path = tail[..end].trim();
        if path.starts_with("files/") && !alt.contains('\n') {
            out.push((alt.to_string(), path.to_string()));
        }
        rest = &tail[end..];
    }
    out
}

/// The Markdown line for a stored file.
pub fn line(caption: &str, path: &str) -> String {
    let c = caption.replace(['[', ']', '\n'], " ");
    format!("![{}]({path})", c.trim())
}

/// Every file under the vault's `files/` (orphans moved aside excluded), as relative paths.
pub fn all_files(vault: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                walk(&p, base, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(&vault.join("files"), vault, &mut out);
    out.sort();
    out
}

fn split_name(name: &str) -> (String, Option<String>) {
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file").to_string();
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase());
    (stem, ext)
}

/// A file-name-safe version of a name: lowercase letters, digits and dashes, at most 40.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    let out: String = out.chars().take(40).collect();
    if out.is_empty() { "file".into() } else { out }
}

/// Five base32 characters of the content's FNV-1a hash.
fn short_hash(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    const A: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    (0..5).map(|i| A[((h >> (i * 5)) & 31) as usize] as char).collect()
}

/// An image's width and height from its header: PNG, GIF, JPEG, WebP.
pub fn dims(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| -> Option<u32> { Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let le16 = |i: usize| -> Option<u32> { Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as u32) };
    let be16 = |i: usize| -> Option<u32> { Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?) as u32) };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8X" => {
                let w = 1 + (b.get(24)?.to_owned() as u32 | (*b.get(25)? as u32) << 8 | (*b.get(26)? as u32) << 16);
                let h = 1 + (b.get(27)?.to_owned() as u32 | (*b.get(28)? as u32) << 8 | (*b.get(29)? as u32) << 16);
                Some((w, h))
            }
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let v = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            _ => None,
        };
    }
    if b.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = b[i + 1];
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            let len = be16(i + 2)? as usize;
            i += 2 + len;
        }
    }
    None
}

/// The vault's limit: `[attachments] max_mb`, else 20.
pub fn max_mb(eff: &crate::settings::Effective) -> u64 {
    eff.get("attachments.max_mb").and_then(|v| v.as_integer()).filter(|n| *n > 0).map(|n| n as u64).unwrap_or(DEFAULT_MAX_MB)
}

/// Where orphans go (`thc doctor --fix`): never deleted.
pub fn orphans_dir(vault: &Path) -> PathBuf {
    vault.join("files/.orphans")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_dims_and_store() {
        assert_eq!(refs("see ![shift bug](files/2026/10/abcde-shot.png) and ![x](http://no)"), vec![("shift bug".to_string(), "files/2026/10/abcde-shot.png".to_string())]);
        // A 3×2 PNG header.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(3u32.to_be_bytes());
        png.extend(2u32.to_be_bytes());
        png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(dims(&png), Some((3, 2)));
        let gif = b"GIF89a\x05\x00\x07\x00".to_vec();
        assert_eq!(dims(&gif), Some((5, 7)));
        let dir = std::env::temp_dir().join(format!("thc-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let a = store_bytes(&dir, &png, "WezTerm Shift.PNG", d, 20).unwrap();
        assert!(a.path.starts_with("files/2026/10/") && a.path.ends_with("-wezterm-shift.png"), "{}", a.path);
        assert_eq!((a.w, a.h, a.mime), (Some(3), Some(2), "image/png"));
        // The same bytes again: the same file.
        assert_eq!(store_bytes(&dir, &png, "WezTerm Shift.PNG", d, 20).unwrap().path, a.path);
        assert_eq!(all_files(&dir), vec![a.path.clone()]);
        // Over the limit: refused, with the size and the setting.
        let big = vec![0u8; 2 * 1024 * 1024 + 1];
        let e = store_bytes(&dir, &big, "screenshot.png", d, 2).unwrap_err().to_string();
        assert!(e.contains("screenshot is 3 MB · the limit is 2 MB ([attachments] max_mb)"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- attachment nodes (FORMAT.md "Attachments") ---------------------------------

/// Where a note's text and its `embed` edges disagree, and attachment nodes nothing shows.
#[derive(Debug, Default)]
pub struct Drift {
    /// Notes whose `![…](files/…)` references aren't exactly their embed edges.
    pub notes: Vec<String>,
    /// Attachment nodes with no embed from a note that's there.
    pub orphans: Vec<String>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.orphans.is_empty()
    }
}

/// The attachment node a path is (keyed: the same on every device).
pub fn node_id(path: &str) -> String {
    crate::id::from_key(&format!("file:{path}"))
}

/// Check every note's text against its embed edges.
pub fn drift(store: &crate::store::Store) -> Result<Drift> {
    let mut d = Drift::default();
    let notes = store.nodes_where("n.deleted=0 AND (n.text LIKE '%](files/%' OR n.id IN (SELECT src FROM edges WHERE rel='embed'))", &[])?;
    for n in &notes {
        let mut want: Vec<String> = refs(&n.text).into_iter().filter(|(_, p)| p.starts_with("files/")).map(|(_, p)| node_id(&p)).collect();
        want.sort();
        want.dedup();
        let mut have: Vec<String> = store.edges_from(&n.id)?.into_iter().filter(|(r, _)| r == "embed").map(|(_, d)| d).collect();
        have.sort();
        let present = want.iter().all(|w| store.node(w).ok().flatten().is_some_and(|x| !x.deleted));
        if want != have || !present {
            d.notes.push(n.id.clone());
        }
    }
    let mut st = store.conn.prepare(
        "SELECT p.node FROM props p JOIN nodes n ON n.id=p.node WHERE p.key='system' AND p.value='\"attachment\"' AND n.deleted=0 \
         AND NOT EXISTS (SELECT 1 FROM edges e JOIN nodes s ON s.id=e.src WHERE e.dst=p.node AND e.rel='embed' AND s.deleted=0)",
    )?;
    d.orphans = st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    Ok(d)
}

/// Bring the drifting notes' embeds in line with their text, in one transaction's builder:
/// attachment nodes made (or brought back) with what's known about their files, edges added and
/// removed. Never touches any note's text.
pub fn backfill(b: &mut crate::builder::TxBuilder, vault: &Path, notes: &[String]) -> Result<usize> {
    let mut changed = 0;
    for id in notes {
        let Some(n) = b.store.node(id)? else { continue };
        let mut want = Vec::new();
        for (caption, path) in refs(&n.text) {
            if path.starts_with("files/") {
                let st = describe(vault, &path, None);
                let aid = b.attachment(&path, &caption, Some(&st).filter(|s| s.abs.exists()))?;
                if !want.contains(&aid) {
                    want.push(aid);
                }
            }
        }
        let have: Vec<String> = b.store.edges_from(id)?.into_iter().filter(|(r, _)| r == "embed").map(|(_, d)| d).collect();
        for h in have.iter().filter(|h| !want.contains(h)) {
            b.ops.push(crate::event::Op::EdgeRemove { src: id.clone(), rel: "embed".into(), dst: h.clone() });
        }
        for w in want.iter().filter(|w| !have.contains(w)) {
            b.ops.push(crate::event::Op::EdgeAdd { src: id.clone(), rel: "embed".into(), dst: w.clone() });
        }
        changed += 1;
    }
    Ok(changed)
}

/// Once per vault (a cache marker), and whenever asked (`thc doctor --fix`): one transaction by
/// `thc` (an agent: reviewable, `thc undo`-able as a whole) that gives today's attachment lines
/// their nodes and edges. Idempotent: nothing to do writes nothing. Returns the notes fixed.
pub fn upgrade(vault: &mut crate::vault::Vault, force: bool) -> Result<usize> {
    let marker = vault.paths.cache.join("attachments-v1");
    if !force && marker.exists() {
        return Ok(0);
    }
    let d = drift(&vault.store)?;
    let mut n = 0;
    if !d.notes.is_empty() {
        let who = std::mem::replace(&mut vault.actor, crate::event::Actor { kind: "agent".into(), name: Some("thc".into()) });
        let dir = vault.paths.vault.clone();
        let today = crate::dates::today();
        let r = vault.transact(|s| {
            let mut b = crate::builder::TxBuilder::new(s, today);
            let k = backfill(&mut b, &dir, &d.notes)?;
            Ok((b.finish(), k))
        });
        vault.actor = who;
        n = r?.1;
    }
    let _ = std::fs::write(&marker, "");
    Ok(n)
}
