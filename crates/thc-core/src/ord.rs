//! Fractional-index order keys (base-62). Keys sort lexically; inserting between
//! two keys never requires renumbering siblings. Keys never end in '0'.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn idx(c: u8) -> usize {
    DIGITS.iter().position(|&d| d == c).expect("valid order digit")
}

/// A key strictly between `a` and `b` (None = open end).
///
/// Siblings can share a key: two devices appending offline both pick the key after the same
/// last sibling, and siblings then sort by (key, id). Nothing fits between equal keys, so when
/// `b` isn't above `a` the key goes after `a` (it sorts after the tied pair). Never panics.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> String {
    let a = a.unwrap_or("");
    let b = b.filter(|b| a < *b);
    midpoint(a.as_bytes(), b.map(str::as_bytes))
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> String {
    if let Some(b) = b {
        // Shared prefix (treating a as zero-padded) is carried through.
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let rest_a = if n < a.len() { &a[n..] } else { &[][..] };
            let mut out = String::from_utf8(b[..n].to_vec()).unwrap();
            out.push_str(&midpoint(rest_a, Some(&b[n..])));
            return out;
        }
    }
    let da = a.first().map(|&c| idx(c)).unwrap_or(0);
    let db = b.map(|b| idx(b[0])).unwrap_or(DIGITS.len());
    if db - da > 1 {
        return (DIGITS[(da + db) / 2] as char).to_string();
    }
    // Adjacent digits.
    if let Some(b) = b {
        if b.len() > 1 {
            return (b[0] as char).to_string();
        }
    }
    let mut out = (DIGITS[da] as char).to_string();
    let rest_a = if a.len() > 1 { &a[1..] } else { &[][..] };
    out.push_str(&midpoint(rest_a, None));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_prepends_stay_ordered() {
        let mut keys = vec![key_between(None, None)];
        for _ in 0..200 {
            let last = keys.last().unwrap().clone();
            keys.push(key_between(Some(&last), None));
        }
        for _ in 0..200 {
            let first = keys[0].clone();
            keys.insert(0, key_between(None, Some(&first)));
        }
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn repeated_insert_between_stays_ordered() {
        let mut lo = key_between(None, None);
        let hi = key_between(Some(&lo), None);
        for _ in 0..500 {
            let mid = key_between(Some(&lo), Some(&hi));
            assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
            assert!(!mid.ends_with('0'));
            lo = mid;
        }
        let mut hi2 = hi.clone();
        let lo2 = key_between(None, None);
        for _ in 0..500 {
            let mid = key_between(Some(&lo2), Some(&hi2));
            assert!(lo2 < mid && mid < hi2, "{lo2} < {mid} < {hi2}");
            hi2 = mid;
        }
    }
}

#[cfg(test)]
mod tie_tests {
    use super::key_between;

    #[test]
    fn equal_or_reversed_bounds_never_panic() {
        let k = key_between(Some("s"), Some("s"));
        assert!(k.as_str() > "s");
        let k = key_between(Some("t"), Some("s"));
        assert!(k.as_str() > "t");
    }
}
