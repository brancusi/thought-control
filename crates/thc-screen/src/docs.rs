//! Pages and journal days as text a person edits: thc's `$EDITOR` round trip
//! (thc_core::edit::render / apply), with the `^id`s kept out of sight. The text shown is the
//! render without its ids and help comment; on save, the edited lines are lined up with the last
//! render's (lines that stayed, then changed lines in order) and get their ids back, so
//! `edit::apply` keeps every node's identity. No second serializer: the format is edit.rs's.

use thc_core::edit::split_id;

/// The store key of a page or a day: `page/<id>`, `day/<2026-10-11>`.
pub enum Doc {
    Page(String),
    Day(chrono::NaiveDate),
}

impl Doc {
    pub fn parse(key: &str) -> Option<Doc> {
        if let Some(id) = key.strip_prefix("page/") {
            return (!id.is_empty()).then(|| Doc::Page(id.to_string()));
        }
        let d = key.strip_prefix("day/")?;
        chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().map(Doc::Day)
    }
}

/// A rendered document's lines, without the help comment: (shown text, its `^id`).
fn lines(rendered: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut comment = false;
    for raw in rendered.lines() {
        let t = raw.trim();
        if comment {
            comment = !t.contains("-->");
            continue;
        }
        if t.starts_with("<!--") {
            comment = !t.contains("-->");
            continue;
        }
        let (body, id) = split_id(raw);
        match id {
            Some(id) => out.push((body, Some(id))),
            None => out.push((raw.trim_end().to_string(), None)),
        }
    }
    while out.last().is_some_and(|(l, _)| l.is_empty()) {
        out.pop();
    }
    out
}

/// What the editor shows: the render without ids or the help comment.
pub fn shown(rendered: &str) -> String {
    let mut s: String = lines(rendered).into_iter().map(|(l, _)| l + "\n").collect();
    if s.is_empty() {
        s.push('\n');
    }
    s
}

/// The edited text with ids put back for `edit::apply`: a line that's still there keeps its id
/// (the longest common run of lines), and between those, changed lines take the ids of the
/// lines they replaced, in order. Extra new lines are new nodes; old lines left over are gone.
pub fn with_ids(rendered: &str, edited: &str) -> String {
    let old = lines(rendered);
    let new: Vec<&str> = edited.lines().collect();
    let (n, m) = (old.len(), new.len());
    // Longest common subsequence of lines, by their shown text.
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i].0 == new[j].trim_end() { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let mut ids: Vec<Option<String>> = vec![None; m];
    let (mut i, mut j) = (0, 0);
    let (mut hunk_old, mut hunk_new): (Vec<usize>, Vec<usize>) = (vec![], vec![]);
    let flush = |ho: &mut Vec<usize>, hn: &mut Vec<usize>, ids: &mut Vec<Option<String>>| {
        // Changed lines take the replaced lines' ids, in order (blank lines carry none).
        let olds = ho.drain(..).filter_map(|k| old[k].1.clone());
        let news = hn.drain(..).filter(|&k| !new[k].trim().is_empty()).collect::<Vec<_>>();
        for (id, k) in olds.zip(news) {
            ids[k] = Some(id);
        }
    };
    while i < n || j < m {
        if i < n && j < m && old[i].0 == new[j].trim_end() {
            flush(&mut hunk_old, &mut hunk_new, &mut ids);
            ids[j] = old[i].1.clone();
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            hunk_new.push(j);
            j += 1;
        } else {
            hunk_old.push(i);
            i += 1;
        }
    }
    flush(&mut hunk_old, &mut hunk_new, &mut ids);
    new.iter()
        .zip(ids)
        .map(|(l, id)| match id {
            Some(id) => format!("{} ^{id}\n", l.trim_end()),
            None => format!("{l}\n"),
        })
        .collect()
}

/// Every line of a render with the node it shows (full ids): what a document has shown, so a
/// line that comes back (an undo after a save) can be the node it was.
pub fn seen(rendered: &thc_core::edit::Rendered) -> Vec<(String, String)> {
    lines(&rendered.text)
        .into_iter()
        .filter_map(|(l, id)| {
            let id = id?;
            let full = rendered.ids.get(&id).cloned().or_else(|| rendered.ids.values().find(|f| f.starts_with(&id)).cloned())?;
            Some((l.trim().to_string(), full))
        })
        .collect()
}

/// The lines of an edited text (with ids put back) that have no id: new to the last render.
pub fn new_lines(edited: &str) -> Vec<String> {
    edited.lines().filter(|l| !l.trim().is_empty() && split_id(l).1.is_none()).map(|l| l.trim().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: &str = "# Home ^aaaa1\n\n- [ ] Water plants ^bbbb2\n  - [ ] Buy a can ^cccc3\n- Wifi is on the fridge ^dddd4\n\n<!-- thc edit: help\n     more -->\n";

    #[test]
    fn shown_drops_ids_and_help() {
        assert_eq!(shown(R), "# Home\n\n- [ ] Water plants\n  - [ ] Buy a can\n- Wifi is on the fridge\n");
    }

    #[test]
    fn ids_come_back_on_kept_changed_and_moved_lines() {
        // Unchanged: every id back.
        assert_eq!(with_ids(R, &shown(R)), "# Home ^aaaa1\n\n- [ ] Water plants ^bbbb2\n  - [ ] Buy a can ^cccc3\n- Wifi is on the fridge ^dddd4\n");
        // A ticked box and a new line: the changed line keeps its id, the new one has none.
        let e = "# Home\n\n- [x] Water plants\n  - [ ] Buy a can\n- Call the plumber\n- Wifi is on the fridge\n";
        assert_eq!(with_ids(R, e), "# Home ^aaaa1\n\n- [x] Water plants ^bbbb2\n  - [ ] Buy a can ^cccc3\n- Call the plumber\n- Wifi is on the fridge ^dddd4\n");
        // A deleted line: the others keep theirs.
        let e = "# Home\n\n- [ ] Water plants\n- Wifi is on the fridge\n";
        assert_eq!(with_ids(R, e), "# Home ^aaaa1\n\n- [ ] Water plants ^bbbb2\n- Wifi is on the fridge ^dddd4\n");
        assert!(Doc::parse("day/2026-10-11").is_some() && Doc::parse("page/").is_none() && Doc::parse("x/1").is_none());
    }
}
