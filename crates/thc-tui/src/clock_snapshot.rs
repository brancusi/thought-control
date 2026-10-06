//! Historical timezone offsets are runtime inputs, never consulted by drawing.
use crate::app::{App, Overlay, Row};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, Offset};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct ClockSnapshot {
    offsets: HashMap<i64, FixedOffset>,
    zone: Option<(Option<String>, i32)>,
}
impl ClockSnapshot {
    fn time(&self, ms: i64) -> Option<DateTime<FixedOffset>> {
        Some(DateTime::from_timestamp_millis(ms)?.with_timezone(self.offsets.get(&ms)?))
    }
    pub fn format(&self, ms: i64, format: &str) -> String {
        self.time(ms).map(|t| t.format(format).to_string()).unwrap_or_default()
    }
    pub fn when_words(&self, ms: i64, today: NaiveDate) -> String {
        self.time(ms).map(|t| words(t, today)).unwrap_or_default()
    }
}
fn words(t: DateTime<FixedOffset>, today: NaiveDate) -> String {
    t.format(match (today - t.date_naive()).num_days() {
        0 => "%H:%M",
        1..=6 => "%a %H:%M",
        _ => "%b %-d %H:%M",
    })
    .to_string()
}
/// Used only by runtime reload/undo summaries, outside the view path.
pub(crate) fn when_words(ms: i64, today: NaiveDate) -> String {
    DateTime::from_timestamp_millis(ms).map(|t| words(t.with_timezone(&Local).fixed_offset(), today)).unwrap_or_default()
}
pub(crate) fn capture(app: &mut App) {
    let mut times = vec![0]; // capture preview nodes have a synthetic creation time
    times.extend(app.derived.data.rows.timestamps());
    for row in app.rows.iter().filter(|_| app.doc.is_none()) {
        if let Some(n) = row.node() {
            times.push(n.created_ms);
        }
        if let Row::Tx { entries, .. } = row {
            times.extend(entries.iter().map(|e| e.ms));
        }
    }
    times.extend(app.history.entries.iter().map(|p| p.ms));
    let detail = &app.derived.data.detail;
    if let Some(node) = &detail.node {
        times.extend(node.data.nodes.values().map(|n| n.created_ms));
        times.extend(node.history.iter().map(|e| e.ms));
    }
    if let Some(item) = &detail.review {
        times.push(item.ms);
        times.extend(item.later.iter().map(|later| later.ms));
    }
    times.extend(detail.review_data.nodes.values().chain(detail.compare_data.nodes.values()).map(|n| n.created_ms));
    times.extend(detail.compare_added.iter().map(|e| e.ms));
    if let Some(Overlay::Compare { detail }) = &app.overlay {
        times.extend(detail.current.iter().chain(&detail.other).map(|v| v.ms));
    }
    let clock = &mut app.derived.data.clock;
    let zone = (std::env::var("TZ").ok(), Local::now().offset().local_minus_utc());
    if clock.zone.as_ref() != Some(&zone) {
        clock.offsets.clear();
        clock.zone = Some(zone);
    }
    for ms in times {
        clock.offsets.entry(ms).or_insert_with(|| {
            DateTime::from_timestamp_millis(ms)
                .map(|t| t.with_timezone(&Local).offset().fix())
                .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
        });
    }
}
