use super::blocks::Grade;

/// Sliding window counter: the current count plus the previous one's, weighted by overlap.
pub(crate) struct Window {
    length: i64,
    start: i64,
    current: u32,
    previous: u32,
    /// A decision is made again only above it, or once it ran out.
    pub(crate) decided: Option<(Grade, i64)>,
}

impl Window {
    /// `length` in microseconds.
    pub(crate) fn new(length: i64, at: i64) -> Self {
        Self {
            length: length.max(1),
            start: at,
            current: 0,
            previous: 0,
            decided: None,
        }
    }

    /// A signal stamped slightly before the window, as a late thread sends, counts in it.
    pub(crate) fn add(&mut self, at: i64) -> u32 {
        self.slide(at);
        self.current = self.current.saturating_add(1);
        self.estimate(at)
    }

    fn slide(&mut self, at: i64) {
        let passed = (at - self.start).div_euclid(self.length);
        if passed <= 0 {
            return;
        }
        self.previous = if passed == 1 { self.current } else { 0 };
        self.current = 0;
        self.start += passed * self.length;
    }

    fn estimate(&self, at: i64) -> u32 {
        let into = (at - self.start).clamp(0, self.length);
        let left = i64::from(self.previous) * (self.length - into) / self.length;
        self.current
            .saturating_add(u32::try_from(left).unwrap_or(u32::MAX))
    }

    pub(crate) fn is_idle(&self, now: i64) -> bool {
        now - self.start >= 2 * self.length && self.decided.is_none_or(|(_, until)| until <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000_000;

    #[test]
    fn the_previous_window_counts_by_the_share_still_covered() {
        let mut window = Window::new(MINUTE, 0);
        for _ in 0..10 {
            window.add(0);
        }

        assert_eq!(
            window.add(MINUTE + MINUTE / 2),
            6,
            "half of 10, and this one"
        );
        assert_eq!(
            window.add(2 * MINUTE + MINUTE / 2),
            1,
            "half of 1 rounds down"
        );
        assert!(!window.is_idle(3 * MINUTE));
        assert!(window.is_idle(4 * MINUTE));
    }

    #[test]
    fn a_window_past_both_buckets_starts_over() {
        let mut window = Window::new(MINUTE, 0);
        window.add(0);

        assert_eq!(window.add(5 * MINUTE), 1);
        assert_eq!(window.add(5 * MINUTE - 1), 2, "a late signal counts too");
    }
}
