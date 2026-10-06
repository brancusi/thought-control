//! Optional, on-device image text. Only explicit CLI attachment/backfill calls run Vision.
//! Replay and document saving never call the recognizer. One atomic prop holds the result.
use crate::store::Store;
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde_json::Value;
#[cfg(target_os = "macos")]
use serde_json::json;
use sha2::{Digest, Sha256};

pub const ENGINE: &str = "macos-vision-v1";

pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        native::available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// SHA-256 of the actual stored bytes; filenames and dates do not affect reuse.
pub fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

pub fn cached(store: &Store, hash: &str) -> Result<Option<Value>> {
    let value: Option<String> = store.conn.query_row(
        "SELECT value FROM props WHERE key='ocr' AND json_extract(value,'$.hash')=?1 AND json_extract(value,'$.engine')=?2 AND json_type(value,'$.text')='text' LIMIT 1",
        [hash, ENGINE], |r| r.get(0)).optional()?;
    Ok(value.and_then(|v| serde_json::from_str(&v).ok()))
}

/// Empty text is a successful result. Unavailable service/errors never get a success marker.
pub fn recognize(store: &Store, data: &[u8]) -> Result<Option<Value>> {
    let hash = hash(data);
    if let Some(value) = cached(store, &hash)? {
        return Ok(Some(value));
    }
    if !available() {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        // Vision's supported encoded formats vary by OS. Decode GIF/WebP to a first-frame PNG;
        // decoding also prevents a corrupt attachment from reaching Objective-C image services.
        let image = image::load_from_memory(data)?;
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png)?;
        let text = native::recognize(png.get_ref())?;
        return Ok(Some(json!({"hash": hash, "engine": ENGINE, "text": text})));
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

/// A substring term matches the image itself or a live image the visible note embeds.
pub fn text_sql(parameter: &str) -> String {
    // Uncorrelated sets are computed once, not two metadata probes per candidate note.
    format!(
        "(n.text LIKE {parameter} OR n.title LIKE {parameter} OR n.id IN (SELECT p.node FROM props p WHERE p.key='ocr' AND json_extract(p.value,'$.text') LIKE {parameter}) OR n.id IN (SELECT e.src FROM props p JOIN edges e ON e.dst=p.node AND e.rel='embed' JOIN nodes image ON image.id=p.node AND image.deleted=0 WHERE p.key='ocr' AND json_extract(p.value,'$.text') LIKE {parameter}))"
    )
}

/// Recognition is intentionally not performed when merely looking for backfill candidates.
pub fn pending(store: &Store) -> Result<Vec<(String, String)>> {
    let mut st = store.conn.prepare("SELECT n.id,json_extract(path.value,'$') FROM nodes n JOIN props kind ON kind.node=n.id AND kind.key='kind' AND kind.value='\"image\"' JOIN props system ON system.node=n.id AND system.key='system' AND system.value='\"attachment\"' JOIN props path ON path.node=n.id AND path.key='path' WHERE n.deleted=0 AND NOT EXISTS(SELECT 1 FROM props o WHERE o.node=n.id AND o.key='ocr' AND json_extract(o.value,'$.engine')=?1 AND json_type(o.value,'$.text')='text') ORDER BY n.id")?;
    Ok(st
        .query_map([ENGINE], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?)
}

/// Image provenance for an FTS result. Text lives only in the image's prop and derived index.
pub fn search_images(store: &Store, id: &str, terms: &str) -> Result<Vec<Value>> {
    let fts = terms
        .split_whitespace()
        .map(crate::query::fts_terms)
        .collect::<Vec<_>>()
        .join(" OR ");
    if fts.is_empty() {
        return Ok(vec![]);
    }
    let mut st = store.conn.prepare("SELECT image.id,image.title,p.value FROM edges e JOIN nodes image ON image.id=e.dst AND image.deleted=0 JOIN props p ON p.node=image.id AND p.key='ocr' WHERE e.src=?1 AND e.rel='embed' AND image.id IN (SELECT id FROM nodes_fts WHERE nodes_fts MATCH ?2) ORDER BY image.id")?;
    Ok(st.query_map([id, &fts], |r| {
        let value: String = r.get(2)?;
        let value = serde_json::from_str::<Value>(&value).unwrap_or(Value::Null);
        let text = value["text"].as_str().unwrap_or_default();
        let words = terms.split_whitespace().map(|w| w.trim_matches('"').to_lowercase()).collect::<Vec<_>>();
        let snippet = text.lines().find(|line| words.iter().any(|w| line.to_lowercase().contains(w))).unwrap_or(text).chars().take(160).collect::<String>();
        Ok(serde_json::json!({"id": r.get::<_,String>(0)?, "caption": r.get::<_,Option<String>>(1)?, "snippet": snippet}))
    })?.collect::<Result<_, _>>()?)
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2::{
        msg_send,
        rc::{Allocated, Retained, autoreleasepool},
        runtime::{AnyClass, AnyObject},
    };
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSString};
    use std::sync::OnceLock;

    pub fn available() -> bool {
        static LOADED: OnceLock<bool> = OnceLock::new();
        *LOADED.get_or_init(|| {
            // Load only the Apple system framework, keeping it loaded for the process lifetime.
            // Unlike a strong framework link this lets older macOS launch without Vision.
            let loaded = unsafe {
                !libc::dlopen(
                    c"/System/Library/Frameworks/Vision.framework/Vision".as_ptr(),
                    libc::RTLD_LAZY | libc::RTLD_LOCAL,
                )
                .is_null()
            };
            loaded
                && AnyClass::get(c"VNRecognizeTextRequest").is_some()
                && AnyClass::get(c"VNImageRequestHandler").is_some()
        })
    }

    pub fn recognize(data: &[u8]) -> Result<String> {
        // SAFETY: availability checks both system classes. These are the documented Vision
        // selectors and Foundation collection types, with Accurate=0 (NSInteger). Objects are
        // owned by Retained inside an autorelease pool; no callbacks or borrowed data escape.
        autoreleasepool(|_| unsafe {
            let request: Retained<AnyObject> =
                msg_send![AnyClass::get(c"VNRecognizeTextRequest").unwrap(), new];
            let _: () = msg_send![&request, setRecognitionLevel: 0isize];
            let _: () = msg_send![&request, setUsesLanguageCorrection: true];
            let alloc: Allocated<AnyObject> =
                msg_send![AnyClass::get(c"VNImageRequestHandler").unwrap(), alloc];
            let bytes = NSData::with_bytes(data);
            let options = NSDictionary::<NSString, AnyObject>::new();
            let handler: Retained<AnyObject> =
                msg_send![alloc, initWithData: &*bytes, options: &*options];
            let requests = NSArray::from_slice(&[&*request]);
            let mut error: *mut NSError = std::ptr::null_mut();
            let ok: bool = msg_send![&handler, performRequests: &*requests, error: &mut error];
            anyhow::ensure!(
                ok,
                "{}",
                error
                    .as_ref()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "Vision text recognition unavailable".into())
            );
            // A missing results array is retryable; an empty array is a successful no-text image.
            let results: Option<Retained<NSArray<AnyObject>>> = msg_send![&request, results];
            let results = results.ok_or_else(|| anyhow::anyhow!("Vision returned no recognition results"))?;
            let mut lines = Vec::new();
            for i in 0..results.count() {
                let candidates: Retained<NSArray<AnyObject>> =
                    msg_send![&*results.objectAtIndex(i), topCandidates: 1usize];
                if let Some(candidate) = candidates.firstObject() {
                    let text: Retained<NSString> = msg_send![&*candidate, string];
                    lines.push(text.to_string());
                }
            }
            Ok(lines.join("\n"))
        })
    }
}
