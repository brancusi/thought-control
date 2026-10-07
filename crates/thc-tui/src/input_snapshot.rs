//! Parsing relative-hour tokens consults the clock. Capture only active input text
//! at the runtime boundary; token rendering uses these immutable results.
use crate::app::{App, Overlay, PromptKind};
use chrono::{NaiveDate, NaiveDateTime, Timelike};
use std::collections::{HashMap, HashSet};
use thc_core::capture::{self, Capture};

struct Parsed {
    lenient: Result<(Capture, Vec<String>), String>,
    strict: Result<Capture, String>,
    shadowed: Vec<(usize, usize)>,
    invalid: HashSet<String>,
    dates: HashMap<String, NaiveDate>,
}
#[derive(Default)]
pub(crate) struct InputSnapshot {
    key: Option<(NaiveDate, NaiveDateTime, Vec<String>)>,
    parsed: HashMap<String, Parsed>,
    pub filter_meaning: Option<String>,
}
impl InputSnapshot {
    pub fn lenient(&self, text: &str) -> Result<(Capture, Vec<String>), String> {
        self.parsed.get(text).map(|p| p.lenient.clone()).unwrap_or_else(|| Err("render input not prepared".into()))
    }
    pub fn strict(&self, text: &str) -> Result<Capture, String> {
        self.parsed.get(text).map(|p| p.strict.clone()).unwrap_or_else(|| Err("render input not prepared".into()))
    }
    pub fn shadowed(&self, text: &str) -> Vec<(usize, usize)> {
        self.parsed.get(text).map(|p| p.shadowed.clone()).unwrap_or_default()
    }
    pub fn valid_token(&self, text: &str, word: &str) -> bool {
        self.parsed.get(text).is_some_and(|p| !p.invalid.contains(word))
    }
    pub fn date(&self, text: &str, value: &str) -> Option<NaiveDate> {
        self.parsed.get(text)?.dates.get(value).copied()
    }
}

pub(crate) fn capture(app: &mut App) {
    let mut texts = vec![];
    if let Some(edit) = &app.edit {
        texts.push(edit.input.buf.clone());
    }
    match &app.overlay {
        Some(Overlay::Capture { input, .. }) => texts.push(input.buf.clone()),
        _ => {}
    }
    if let Some(doc) = &app.doc {
        if let Some(line) = doc.blocks().get(doc.caret().line) {
            texts.push(line.text.clone());
        }
    }
    if let Some((_, input)) = &app.prompt {
        texts.push(input.buf.clone());
    }
    let now = thc_core::dates::now_local().with_second(0).unwrap().with_nanosecond(0).unwrap();
    let key = (app.today, now, texts);
    if app.derived.data.input.key.as_ref() != Some(&key) {
        let mut parsed = HashMap::new();
        for text in &key.2 {
            let mut invalid = HashSet::new();
            let mut dates = HashMap::new();
            for word in text.split(' ') {
                if capture::parse(&format!("x {word}"), app.today).is_err() {
                    invalid.insert(word.to_owned());
                }
                if let Some((_, value)) = word.split_once(':') {
                    let value = value.trim_matches('"');
                    if let Ok(date) = thc_core::dates::parse(value, app.today) {
                        dates.insert(value.to_owned(), date.date());
                    }
                }
            }
            parsed.insert(
                text.clone(),
                Parsed {
                    lenient: capture::parse_lenient(text, app.today).map_err(|e| crate::app::friendly_error(&e)),
                    strict: capture::parse(text, app.today).map_err(|e| crate::app::friendly_error(&e)),
                    shadowed: capture::shadowed(text, app.today),
                    invalid,
                    dates,
                },
            );
        }
        app.derived.data.input.key = Some(key);
        app.derived.data.input.parsed = parsed;
    }
    app.derived.data.input.filter_meaning = match &app.prompt {
        Some((PromptKind::Filter, input)) => thc_core::query::explain(&input.buf, &app.vault.store, app.today).ok().map(|e| e.short()),
        _ => None,
    };
}
