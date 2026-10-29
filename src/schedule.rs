//! Which page is on screen. Normally pages rotate in order, each for its
//! own duration. While the machine is busy (CPU or GPU load above a
//! threshold) page 1 alternates with every other page in turn:
//! 1, 2, 1, 3, 1, 4, 1, 2, ...

use std::time::{Duration, Instant};

use crate::config::Config;

pub struct Schedule {
    busy: bool,
    page: usize,
    /// Position in the busy sequence (even = main page, odd = another page).
    pos: usize,
    since: Instant,
}

/// Page shown at position `pos` of the busy sequence.
fn busy_page(pos: usize, pages: usize) -> usize {
    if pages <= 1 || pos % 2 == 0 {
        0
    } else {
        1 + (pos / 2) % (pages - 1)
    }
}

impl Schedule {
    pub fn new(now: Instant) -> Self {
        Self {
            busy: false,
            page: 0,
            pos: 0,
            since: now,
        }
    }

    /// Advance the schedule to `now` and return the page to show.
    pub fn update(&mut self, now: Instant, busy: bool, cfg: &Config) -> usize {
        let n = cfg.pages.len();
        if busy != self.busy {
            self.busy = busy;
            self.since = now;
            if busy {
                // Start the sequence with the main page right away.
                self.pos = 0;
                self.page = 0;
            }
            return self.page;
        }
        let dwell = if self.busy {
            if self.pos % 2 == 0 {
                cfg.busy.main_page_seconds
            } else {
                cfg.busy.other_page_seconds
            }
        } else {
            cfg.page_duration(self.page)
        };
        if now.duration_since(self.since) >= Duration::from_secs_f64(dwell) {
            if self.busy {
                self.pos += 1;
                self.page = busy_page(self.pos, n);
            } else {
                self.page = (self.page + 1) % n;
            }
            self.since = now;
        }
        self.page
    }
}

/// Keeps "busy" on for `hold` seconds after the load last exceeded the
/// threshold, so a load that hovers around it doesn't flip the schedule.
pub struct BusyDetector {
    until: Option<Instant>,
}

impl BusyDetector {
    pub fn new() -> Self {
        Self { until: None }
    }

    pub fn update(&mut self, now: Instant, load: f32, cfg: &Config) -> bool {
        let b = &cfg.busy;
        if !b.enabled {
            return false;
        }
        if load > b.load_threshold {
            self.until = Some(now + Duration::from_secs_f64(b.hold_seconds));
        }
        // The hold is inclusive so hold_seconds = 0 means "busy only while above".
        match self.until {
            Some(t) => now < t || load > b.load_threshold,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pages: usize) -> Config {
        let mut c = Config::default();
        c.pages.truncate(pages);
        c.global.page_duration = 15.0;
        c.busy.main_page_seconds = 12.0;
        c.busy.other_page_seconds = 8.0;
        c
    }

    #[test]
    fn busy_sequence_alternates_main_with_each_other_page() {
        let seq: Vec<usize> = (0..8).map(|p| busy_page(p, 4)).collect();
        assert_eq!(seq, [0, 1, 0, 2, 0, 3, 0, 1]);
        assert_eq!(busy_page(1, 1), 0);
        assert_eq!(busy_page(3, 2), 1);
    }

    #[test]
    fn normal_mode_rotates_with_page_duration() {
        let c = cfg(4);
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        assert_eq!(s.update(t0, false, &c), 0);
        assert_eq!(s.update(t0 + Duration::from_secs(14), false, &c), 0);
        assert_eq!(s.update(t0 + Duration::from_secs(15), false, &c), 1);
        assert_eq!(s.update(t0 + Duration::from_secs(30), false, &c), 2);
    }

    #[test]
    fn busy_mode_uses_12_and_8_seconds() {
        let c = cfg(4);
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        // Move to page 3 in normal mode, then turn busy: jumps to page 1.
        s.update(t0 + Duration::from_secs(15), false, &c);
        s.update(t0 + Duration::from_secs(30), false, &c);
        let t = t0 + Duration::from_secs(40);
        assert_eq!(s.update(t, true, &c), 0);
        assert_eq!(s.update(t + Duration::from_secs(11), true, &c), 0);
        assert_eq!(s.update(t + Duration::from_secs(12), true, &c), 1); // main 12 s
        assert_eq!(s.update(t + Duration::from_secs(19), true, &c), 1);
        assert_eq!(s.update(t + Duration::from_secs(20), true, &c), 0); // other 8 s
        assert_eq!(s.update(t + Duration::from_secs(32), true, &c), 2);
        assert_eq!(s.update(t + Duration::from_secs(40), true, &c), 0);
        assert_eq!(s.update(t + Duration::from_secs(52), true, &c), 3);
    }

    #[test]
    fn leaving_busy_resumes_normal_rotation() {
        let c = cfg(4);
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        s.update(t0, true, &c);
        s.update(t0 + Duration::from_secs(12), true, &c); // on page 2
        let t = t0 + Duration::from_secs(13);
        assert_eq!(s.update(t, false, &c), 1);
        assert_eq!(s.update(t + Duration::from_secs(15), false, &c), 2);
    }

    #[test]
    fn single_page_stays_put() {
        let c = cfg(1);
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        s.update(t0, true, &c);
        assert_eq!(s.update(t0 + Duration::from_secs(100), true, &c), 0);
    }

    #[test]
    fn detector_holds_busy_after_load_drops() {
        let mut c = cfg(4);
        c.busy.hold_seconds = 10.0;
        let t0 = Instant::now();
        let mut d = BusyDetector::new();
        assert!(!d.update(t0, 10.0, &c));
        assert!(d.update(t0, 50.0, &c));
        assert!(d.update(t0 + Duration::from_secs(5), 5.0, &c), "held");
        assert!(!d.update(t0 + Duration::from_secs(11), 5.0, &c));
        c.busy.enabled = false;
        assert!(!d.update(t0, 99.0, &c));
    }
}
