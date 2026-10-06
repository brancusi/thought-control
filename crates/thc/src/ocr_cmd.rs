//! Explicit image backfill. Never invoked from save, replay, a listing, or TUI input.
use crate::{Ctx, out::node_json};
use anyhow::Result;
use serde_json::{Value, json};
use thc_core::{event::Op, model::Node, ocr};

pub const SPEC: crate::registry::Spec = crate::spec!("ocr", "Read text in existing images locally (preview; --yes to backfill)", OcrArgs, |ctx: &mut Ctx, _: OcrArgs| run(ctx), verbs: |_| vec!["set".into()]);

#[derive(clap::Args, Debug)]
pub struct OcrArgs {}

fn run(ctx: &mut Ctx) -> Result<()> {
    let pending = ocr::pending(ctx.store())?;
    let available = ocr::available();
    if ctx.dry_run || !ctx.yes {
        if ctx.out.json {
            ctx.out.json(&json!({"dry_run": true, "available": available, "pending": pending.len(), "images": pending.iter().map(|(id,path)| json!({"id":id,"path":path})).collect::<Vec<_>>() }));
        } else {
            ctx.out.line(format!(
                "{} images await text recognition · {}",
                pending.len(),
                if available {
                    "thc ocr --yes to backfill"
                } else {
                    "macOS Vision unavailable; attachments remain usable"
                }
            ));
        }
        return Ok(());
    }
    ctx.guard()?;
    let mut values = std::collections::BTreeMap::<String, Value>::new();
    let mut updates = Vec::new();
    let mut skipped = Vec::new();
    let root = ctx.vault.paths.vault.canonicalize()?;
    for (id, path) in &pending {
        let result = (|| -> Result<Option<Value>> {
            let file = ctx.vault.paths.vault.join(path).canonicalize()?;
            anyhow::ensure!(
                file.starts_with(root.join("files")),
                "image path is outside vault files"
            );
            let data = std::fs::read(thc_core::sandbox::check(&file))?;
            let hash = ocr::hash(&data);
            if let Some(value) = values.get(&hash) {
                return Ok(Some(value.clone()));
            }
            let value = ocr::recognize(ctx.store(), &data)?;
            if let Some(value) = &value {
                values.insert(hash, value.clone());
            }
            Ok(value)
        })();
        match result {
            Ok(Some(value)) => updates.push((id.clone(), value)),
            Ok(None) => {
                skipped.push(json!({"id": id, "path": path, "reason": "macOS Vision unavailable"}))
            }
            Err(e) => skipped.push(json!({"id": id, "path": path, "reason": e.to_string()})),
        }
    }
    let mut tx = None;
    if !updates.is_empty() {
        if let Some((events, ())) = ctx.write(|b| {
            for (id, value) in &updates {
                b.ops.push(Op::NodeSet {
                    id: id.clone(),
                    props: serde_json::Map::from_iter([("ocr".into(), value.clone())]),
                });
            }
            Ok(())
        })? {
            tx = events.first().map(|e| e.tx.clone());
        }
    }
    if ctx.out.json {
        ctx.out.json(&json!({"ok": true, "tx": tx, "recognized": updates.len(), "skipped": skipped, "available": available}));
    } else {
        ctx.out.line(format!(
            "read text in {} images · {} skipped",
            updates.len(),
            skipped.len()
        ));
        for skip in skipped {
            ctx.out.line(format!(
                "{}: {}",
                skip["path"].as_str().unwrap_or_default(),
                skip["reason"].as_str().unwrap_or_default()
            ));
        }
    }
    Ok(())
}

pub fn emit_search(ctx: &mut Ctx, nodes: &[Node], terms: &str) -> Result<()> {
    let matches = nodes
        .iter()
        .map(|n| ocr::search_images(ctx.store(), &n.id, terms))
        .collect::<Result<Vec<_>>>()?;
    if ctx.out.json {
        let items = nodes
            .iter()
            .zip(&matches)
            .map(|(n, matches)| {
                let mut value = node_json(ctx.store(), n);
                if !matches.is_empty() {
                    value["image_matches"] = json!(matches);
                }
                value
            })
            .collect::<Vec<_>>();
        ctx.out
            .json(&json!({"count": items.len(), "items": items, "context": ctx.context_json()}));
    } else if nodes.is_empty() {
        ctx.out.line("Results: nothing here");
    } else {
        ctx.out.heading("Results");
        for (n, matches) in nodes.iter().zip(matches) {
            let line = ctx.out.node_line(ctx.store(), n, 0, true);
            ctx.out.line(line);
            for image in matches {
                ctx.out.line(format!(
                    "  in image: {} — {}",
                    image["caption"].as_str().unwrap_or("image"),
                    image["snippet"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    Ok(())
}
