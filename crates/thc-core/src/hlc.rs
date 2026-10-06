//! Hybrid logical clock. Events are totally ordered by (ms, counter, device, eid).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct Hlc(pub u64, pub u32);

impl Hlc {
    /// Next clock value strictly greater than `last`, tracking wall time when it is ahead.
    pub fn tick(last: Hlc) -> Hlc {
        Hlc::tick_at(last, chrono::Utc::now().timestamp_millis().max(0) as u64)
    }

    /// `tick` against a given wall time (fixtures pin it).
    pub fn tick_at(last: Hlc, now: u64) -> Hlc {
        if now > last.0 { Hlc(now, 0) } else { Hlc(last.0, last.1 + 1) }
    }

    pub fn ms(&self) -> u64 {
        self.0
    }
}

/// Lexically sortable total-order key shared by every device.
pub fn order_key(hlc: Hlc, dev: &str, eid: &str) -> String {
    format!("{:013}.{:06}.{}.{}", hlc.0, hlc.1, dev, eid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_monotonic_even_if_clock_is_behind() {
        let future = Hlc(u64::MAX / 2, 7);
        let next = Hlc::tick(future);
        assert!(next > future);
        assert_eq!(next, Hlc(u64::MAX / 2, 8));
    }

    #[test]
    fn order_keys_sort_like_hlcs() {
        let a = order_key(Hlc(5, 1), "dev-b", "01");
        let b = order_key(Hlc(5, 2), "dev-a", "00");
        let c = order_key(Hlc(10, 0), "dev-a", "00");
        assert!(a < b && b < c);
    }
}
