use std::time::{Duration, Instant};

pub const DEFAULT_QUIET: Duration = Duration::from_millis(1500);
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Debounce {
    quiet: Duration,
    max_wait: Duration,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Debounce {
    #[must_use]
    pub const fn new(quiet: Duration, max_wait: Duration) -> Self {
        Self {
            quiet,
            max_wait,
            first: None,
            last: None,
        }
    }

    pub fn touched(&mut self, at: Instant) {
        self.first.get_or_insert(at);
        self.last = Some(at);
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.last.is_some()
    }

    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        let (first, last) = (self.first?, self.last?);
        Some((last + self.quiet).min(first + self.max_wait))
    }

    #[must_use]
    pub fn is_due(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }

    pub fn taken(&mut self) {
        self.first = None;
        self.last = None;
    }
}

impl Default for Debounce {
    fn default() -> Self {
        Self::new(DEFAULT_QUIET, DEFAULT_MAX_WAIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet_after(millis: u64) -> Debounce {
        Debounce::new(Duration::from_millis(millis), Duration::from_secs(10))
    }

    #[test]
    fn nothing_is_due_before_anything_changed() {
        let debounce = quiet_after(100);
        assert!(!debounce.is_pending());
        assert_eq!(debounce.deadline(), None);
        assert!(!debounce.is_due(Instant::now()));
    }

    #[test]
    fn a_change_is_not_due_until_the_folder_goes_quiet() {
        let start = Instant::now();
        let mut debounce = quiet_after(100);
        debounce.touched(start);

        assert!(!debounce.is_due(start + Duration::from_millis(99)));
        assert!(debounce.is_due(start + Duration::from_millis(100)));
    }

    #[test]
    fn a_burst_of_changes_pushes_the_deadline_back() {
        let start = Instant::now();
        let mut debounce = quiet_after(100);

        debounce.touched(start);
        debounce.touched(start + Duration::from_millis(80));
        debounce.touched(start + Duration::from_millis(160));

        assert!(
            !debounce.is_due(start + Duration::from_millis(200)),
            "the last write was at 160, so 260 is the earliest"
        );
        assert!(debounce.is_due(start + Duration::from_millis(260)));
    }

    #[test]
    fn a_folder_that_never_goes_quiet_still_syncs_at_the_ceiling() {
        let start = Instant::now();
        let mut debounce = Debounce::new(Duration::from_millis(100), Duration::from_millis(250));

        for step in 0..10 {
            debounce.touched(start + Duration::from_millis(step * 50));
        }

        assert_eq!(
            debounce.deadline(),
            Some(start + Duration::from_millis(250)),
            "the ceiling wins over a deadline the writes keep pushing"
        );
        assert!(debounce.is_due(start + Duration::from_millis(250)));
    }

    #[test]
    fn taking_the_work_clears_the_deadline() {
        let start = Instant::now();
        let mut debounce = quiet_after(100);
        debounce.touched(start);
        debounce.taken();

        assert!(!debounce.is_pending());
        assert_eq!(debounce.deadline(), None);
        assert!(!debounce.is_due(start + Duration::from_secs(60)));
    }

    #[test]
    fn the_ceiling_is_measured_from_the_first_change_of_the_batch() {
        let start = Instant::now();
        let mut debounce = Debounce::new(Duration::from_millis(100), Duration::from_millis(250));

        debounce.touched(start);
        debounce.taken();
        debounce.touched(start + Duration::from_secs(5));

        assert_eq!(
            debounce.deadline(),
            Some(start + Duration::from_secs(5) + Duration::from_millis(100)),
            "a new batch starts its own clock"
        );
    }
}
