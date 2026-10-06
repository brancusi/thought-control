//! One attachment transaction, shared by `attach` and `shot`.
use crate::Ctx;
use anyhow::Result;
use chrono::NaiveDate;
use serde_json::json;
use std::path::PathBuf;
use thc_core::{attach, capture::Capture, event::Op};

pub enum Destination {
    Under(String),
    Journal(NaiveDate),
}

pub fn attach(ctx: &mut Ctx, id: &str, files: &[PathBuf], caption: Option<&str>) -> Result<()> {
    let parent = ctx.resolve(id)?;
    commit(ctx, Destination::Under(parent), files, caption)
}

pub fn commit(
    ctx: &mut Ctx,
    dest: Destination,
    files: &[PathBuf],
    caption: Option<&str>,
) -> Result<()> {
    if !ctx.dry_run {
        ctx.guard()?;
    }
    let max = attach::max_mb(&thc_core::settings::current());
    // All inputs pass before any vault file or event is written.
    let prepared = files
        .iter()
        .map(|f| attach::prepare_file(f, max))
        .collect::<Result<Vec<_>>>()?;
    let captions: Vec<String> = files
        .iter()
        .map(|f| {
            caption.map(str::to_string).unwrap_or_else(|| {
                f.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .replace(['-', '_'], " ")
            })
        })
        .collect();
    if ctx.dry_run {
        ctx.out.json(&json!({"dry_run": true, "files": files.iter().zip(&prepared).map(|(f, p)| json!({"file": f, "bytes": p.bytes()})).collect::<Vec<_>>() }));
        return Ok(());
    }
    let stored: Vec<_> = prepared
        .iter()
        .map(|f| attach::preview_prepared(&ctx.vault.paths.vault, f, ctx.out.today))
        .collect();
    let mut ocr = Vec::new();
    let mut warnings = Vec::new();
    let mut results = std::collections::BTreeMap::<String, serde_json::Value>::new();
    for (st, file) in stored.iter().zip(&prepared) {
        let value = if attach::is_image(&st.path) {
            let hash = thc_core::ocr::hash(file.data());
            if let Some(value) = results.get(&hash) { Some(value.clone()) } else {
                match thc_core::ocr::recognize(ctx.store(), file.data()) {
                    Ok(value) => {
                        if let Some(v) = &value { results.insert(hash, v.clone()); }
                        value
                    }
                    Err(e) => { warnings.push(format!("OCR skipped for {}: {e}", file.name)); None }
                }
            }
        } else { None };
        ocr.push(value);
    }
    let build = |b: &mut thc_core::builder::TxBuilder| {
        let parent = match &dest {
            Destination::Under(id) => id.clone(),
            Destination::Journal(d) => b.journal(*d)?,
        };
        let mut ids = Vec::new();
        b.plain = true;
        for ((st, caption), ocr) in stored.iter().zip(&captions).zip(&ocr) {
            let aid = b.attachment(&st.path, caption, Some(st))?;
            if let Some(value) = ocr {
                if b.store.props_of(&aid)?.get("ocr") != Some(value) {
                    b.ops.push(Op::NodeSet { id: aid.clone(), props: serde_json::Map::from_iter([("ocr".into(), value.clone())]) });
                }
            }
            let nid = b.create_from_capture(
                Some(parent.clone()),
                &Capture {
                    text: attach::line(caption, &st.path),
                    ..Default::default()
                },
                None,
            )?;
            b.ops.push(Op::EdgeAdd {
                src: nid.clone(),
                rel: "embed".into(),
                dst: aid.clone(),
            });
            ids.push((nid, aid));
        }
        Ok((parent, ids))
    };
    // Preconditions and op policy must also pass before files land in the vault.
    let mut preview = thc_core::builder::TxBuilder::new(ctx.store(), ctx.out.today);
    build(&mut preview)?;
    let ops = preview.finish();
    crate::pre::check(ctx.store(), &ops, &ctx.pre)?;
    ctx.guard()?.ops(&ops)?;
    for file in &prepared {
        attach::store_prepared(&ctx.vault.paths.vault, file, ctx.out.today)?;
    }
    let result = ctx.write(build)?;
    if let Some((ev, (parent, ids))) = result {
        let mut items = Vec::new();
        for (st, (nid, aid)) in stored.iter().zip(ids) {
            let mut item = json!({"id": nid, "attachment": aid, "path": st.path, "abs": st.abs, "bytes": st.bytes, "w": st.w, "h": st.h, "mime": st.mime});
            if let Some(value) = ctx.store().props_of(&aid)?.get("ocr") { item["ocr"] = value.clone(); }
            if let Some(thumb) = attach::pdf_thumbnail(&ctx.vault.paths.cache, st) {
                item["thumbnail"] = json!(thumb);
            }
            items.push(item);
            if !ctx.out.json {
                let dims = match (st.w, st.h) {
                    (Some(w), Some(h)) => format!(" · {w}×{h}"),
                    _ => String::new(),
                };
                ctx.out.line(format!(
                    "attached {}{dims} · {} KB under {}",
                    st.path,
                    st.bytes.div_ceil(1024),
                    ctx.store().short(&parent)
                ));
            }
        }
        if !ctx.out.json { for warning in &warnings { eprintln!("{warning}"); } }
        if ctx.out.json {
            // Keep every single-file field; batches add an array of the same objects.
            let mut output =
                json!({"ok": true, "tx": ev.first().map(|e| &e.tx), "attachments": items, "warnings": warnings});
            if items.len() == 1 {
                for (k, v) in items[0].as_object().unwrap() {
                    output[k] = v.clone();
                }
            }
            ctx.out.json(&output);
        }
    }
    Ok(())
}
