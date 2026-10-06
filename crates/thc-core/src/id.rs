//! Node/alert IDs: 60 random bits, Crockford base32, 12 lowercase chars.

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

pub const ID_LEN: usize = 12;
pub const MIN_SHORT: usize = 5;
pub const MIN_PREFIX: usize = 4;

pub fn new_id() -> String {
    let mut v: u64 = rand::random::<u64>() >> 4;
    let mut out = [0u8; ID_LEN];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

/// Deterministic id for an idempotency key (same key, same id, on every device).
pub fn from_key(key: &str) -> String {
    // FNV-1a 64 over a namespaced key, folded to 60 bits.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("thc-key:{key}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let mut v = h >> 4;
    let mut out = [0u8; ID_LEN];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

/// Normalize user-typed IDs: lowercase, Crockford confusables (i/l -> 1, o -> 0), strip `^`.
pub fn normalize(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('^')
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            'i' | 'l' => '1',
            'o' => '0',
            c => c,
        })
        .collect()
}

pub fn looks_like_id(s: &str) -> bool {
    let s = normalize(s);
    s.len() >= MIN_PREFIX && s.len() <= ID_LEN && s.bytes().all(|b| ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_well_formed() {
        for _ in 0..1000 {
            let id = new_id();
            assert_eq!(id.len(), ID_LEN);
            assert!(looks_like_id(&id));
        }
    }

    #[test]
    fn keys_map_to_stable_ids() {
        assert_eq!(from_key("meeting-2026-10-03-1"), from_key("meeting-2026-10-03-1"));
        assert_ne!(from_key("a"), from_key("b"));
        assert!(looks_like_id(&from_key("x")));
    }

    #[test]
    fn normalizes_confusables() {
        assert_eq!(normalize("^K3F9A"), "k3f9a");
        assert_eq!(normalize("Io1L"), "1011");
    }
}
