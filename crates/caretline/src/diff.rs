//! The least changes from one text to another, for pushing a whole new text into a live
//! document ([`crate::Session::set_text`], the protocol's `text.set`) without disturbing
//! what didn't change: carets, selections, marks and undo steps outside the changes stay.
//!
//! The common start and end are kept, then a line diff (Myers) finds the changed lines and a
//! char diff inside each changed run finds the changed chars. Past a budget of differences
//! a run is replaced whole, so a diff never takes long.

/// How many differing lines the line diff looks for before replacing the whole middle.
const MAX_LINE_EDITS: usize = 2_000;
/// How many differing chars a run's char diff looks for before replacing the whole run.
const MAX_CHAR_EDITS: usize = 1_000;
/// Runs longer than this (old plus new chars) are replaced whole.
const MAX_CHAR_RUN: usize = 40_000;

/// The changes that turn `old` into `new`: `(from, to, text)`, chars `[from, to)` of `old`
/// replaced by `text`, in order and apart. Empty when the texts are equal.
pub fn changes(old: &str, new: &str) -> Vec<(usize, usize, String)> {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()) - pre;
    let suf = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out = Vec::new();
    if am.is_empty() && bm.is_empty() {
        return out;
    }
    if am.is_empty() || bm.is_empty() {
        out.push((pre, pre + am.len(), bm.iter().collect()));
        return out;
    }
    let (la, lb) = (lines(am), lines(bm));
    let eq = |i: usize, j: usize| am[la[i].0..la[i].1] == bm[lb[j].0..lb[j].1];
    let runs = myers(la.len(), lb.len(), eq, MAX_LINE_EDITS).unwrap_or_else(|| vec![(0, la.len(), 0, lb.len())]);
    // Where line `i` starts (the end of the text past the last line).
    let start = |ls: &[(usize, usize)], i: usize, total: usize| ls.get(i).map_or(total, |l| l.0);
    for (a0, a1, b0, b1) in runs {
        let (ca0, ca1) = (start(&la, a0, am.len()), start(&la, a1, am.len()));
        let (cb0, cb1) = (start(&lb, b0, bm.len()), start(&lb, b1, bm.len()));
        chars(&am[ca0..ca1], &bm[cb0..cb1], pre + ca0, &mut out);
    }
    out
}

/// The changes inside one changed run of lines: a char diff, or the whole run past the budget.
fn chars(a: &[char], b: &[char], at: usize, out: &mut Vec<(usize, usize, String)>) {
    let whole = || vec![(0, a.len(), 0, b.len())];
    let runs = if a.is_empty() || b.is_empty() || a.len() + b.len() > MAX_CHAR_RUN {
        whole()
    } else {
        myers(a.len(), b.len(), |i, j| a[i] == b[j], MAX_CHAR_EDITS).unwrap_or_else(whole)
    };
    for (a0, a1, b0, b1) in runs {
        out.push((at + a0, at + a1, b[b0..b1].iter().collect()));
    }
}

/// Each line's chars `[start, end)`, its line break included.
fn lines(t: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in t.iter().enumerate() {
        if *c == '\n' {
            out.push((start, i + 1));
            start = i + 1;
        }
    }
    if start < t.len() {
        out.push((start, t.len()));
    }
    out
}

/// A run of differing items: items `[a0, a1)` of the first sequence become `[b0, b1)` of the
/// second.
type Run = (usize, usize, usize, usize);

const NONE: isize = -1;

/// A shortest edit script between sequences of `n` and `m` items (`eq(i, j)`: item `i` of the
/// first equals item `j` of the second), as its runs of differing items, in order. `None` when
/// it needs more than `max_d` insertions and deletions.
fn myers(n: usize, m: usize, eq: impl Fn(usize, usize) -> bool, max_d: usize) -> Option<Vec<Run>> {
    let (ni, mi) = (n as isize, m as isize);
    // v[k + m]: the furthest x reached on diagonal k (where y = x - k), k in [-m, n].
    let mut v = vec![NONE; n + m + 1];
    let at = |v: &[isize], k: isize| if k < -mi || k > ni { NONE } else { v[(k + mi) as usize] };
    // trace[d][k + d]: v on diagonals [-d, d] before step d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=(max_d.min(n + m) as isize) {
        let row: Vec<isize> = (-d..=d).map(|k| at(&v, k)).collect();
        let lo = (-d).max(-mi);
        let hi = d.min(ni);
        // Step d reaches the diagonals of d's parity.
        let mut k = if (lo + d) % 2 == 0 { lo } else { lo + 1 };
        while k <= hi {
            let start = if d == 0 { Some(0) } else { step(&row, d, k, ni, mi).map(|(x, _)| x) };
            if let Some(mut x) = start {
                let mut y = x - k;
                while x < ni && y < mi && eq(x as usize, y as usize) {
                    x += 1;
                    y += 1;
                }
                v[(k + mi) as usize] = x;
                if x == ni && y == mi {
                    trace.push(row);
                    return Some(backtrack(&trace, d, k, ni, mi));
                }
            }
            k += 2;
        }
        trace.push(row);
    }
    None
}

/// Where step `d` starts on diagonal `k`, from `row` (diagonals `[-d, d]` before it): the x
/// one move reaches, and the diagonal the move came from. A move down (an insertion) needs
/// room below, a move right (a deletion) room to the right; the furthest one wins.
fn step(row: &[isize], d: isize, k: isize, n: isize, m: isize) -> Option<(isize, isize)> {
    let get = |k: isize| if k < -d || k > d { NONE } else { row[(k + d) as usize] };
    let down = get(k + 1);
    let down = (down != NONE && down - k <= m).then_some((down, k + 1));
    let right = get(k - 1);
    let right = (right != NONE && right < n).then_some((right + 1, k - 1));
    match (down, right) {
        (Some(dn), Some(r)) => Some(if r.0 > dn.0 { r } else { dn }),
        (dn, r) => dn.or(r),
    }
}

/// The runs of differing items along the path that ends at step `d` on diagonal `k`.
fn backtrack(trace: &[Vec<isize>], d_end: isize, k_end: isize, n: isize, m: isize) -> Vec<Run> {
    // The path's diagonal stretches (equal items), as (x, y, len), last first.
    let mut snakes: Vec<(isize, isize, isize)> = Vec::new();
    let (mut x, mut k) = (n, k_end);
    for d in (0..=d_end).rev() {
        let (sx, from) = if d == 0 { (0, 0) } else { step(&trace[d as usize], d, k, n, m).expect("a step on the path") };
        if x > sx {
            snakes.push((sx, sx - k, x - sx));
        }
        if d > 0 {
            x = trace[d as usize][(from + d) as usize];
            k = from;
        }
    }
    let mut runs = Vec::new();
    let (mut a, mut b) = (0isize, 0isize);
    for &(sx, sy, len) in snakes.iter().rev() {
        if sx > a || sy > b {
            runs.push((a as usize, sx as usize, b as usize, sy as usize));
        }
        a = sx + len;
        b = sy + len;
    }
    if a < n || b < m {
        runs.push((a as usize, n as usize, b as usize, m as usize));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, ch: &[(usize, usize, String)]) -> String {
        let a: Vec<char> = old.chars().collect();
        let mut out = String::new();
        let mut at = 0;
        for (f, t, s) in ch {
            assert!(*f >= at && f <= t, "{ch:?}");
            out.extend(&a[at..*f]);
            out.push_str(s);
            at = *t;
        }
        out.extend(&a[at..]);
        out
    }

    #[test]
    fn finds_the_least_changes() {
        assert!(changes("same", "same").is_empty());
        assert_eq!(changes("Hey there, \nnext\n", "Hey there, \n- note\nnext\n"), vec![(12, 12, "- note\n".into())]);
        // Two changes far apart stay two changes: a caret between them is left alone.
        let old = "one\ntwo\nthree\nfour\nfive\n";
        let new = "ONE\ntwo\nthree\nfour\nfive!\n";
        assert_eq!(changes(old, new), vec![(0, 3, "ONE".into()), (23, 23, "!".into())]);
        assert_eq!(changes("abc", ""), vec![(0, 3, String::new())]);
        assert_eq!(changes("", "abc"), vec![(0, 0, "abc".into())]);
    }

    #[test]
    fn random_texts_round_trip() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let pieces = ["a", "b", "\n", "日", "xy", " "];
        for _ in 0..3000 {
            let (l1, l2) = (next(30), next(30));
            let old: String = (0..l1).map(|_| pieces[next(pieces.len() as u64) as usize]).collect();
            let new: String = (0..l2).map(|_| pieces[next(pieces.len() as u64) as usize]).collect();
            let ch = changes(&old, &new);
            assert_eq!(apply(&old, &ch), new, "{old:?} -> {new:?}: {ch:?}");
            for w in ch.windows(2) {
                assert!(w[0].1 <= w[1].0, "{ch:?}");
            }
        }
    }
}
