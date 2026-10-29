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

/// Decides whether the machine counts as busy. CPU and GPU load are each
/// smoothed with a moving average (`busy.average_seconds`), so a load that
/// merely hovers around the threshold is judged by its average instead of
/// by single samples. Once busy, it stays busy for `hold_seconds` after the
/// smoothed load was last above the threshold.
pub struct BusyDetector {
    cpu: Option<f32>,
    gpu: Option<f32>,
    last: Option<Instant>,
    until: Option<Instant>,
}

impl BusyDetector {
    pub fn new() -> Self {
        Self {
            cpu: None,
            gpu: None,
            last: None,
            until: None,
        }
    }

    /// Feed one load sample (percent) taken at `now`.
    pub fn sample(&mut self, now: Instant, cpu: f32, gpu: f32, cfg: &Config) {
        let tau = cfg.busy.average_seconds;
        let alpha = match self.last {
            Some(t) => 1.0 - (-now.duration_since(t).as_secs_f64() / tau).exp(),
            None => 1.0,
        } as f32;
        let ema = |old: Option<f32>, new: f32| old.map_or(new, |o| o + alpha * (new - o));
        self.cpu = Some(ema(self.cpu, cpu));
        self.gpu = Some(ema(self.gpu, gpu));
        self.last = Some(now);
        if self.load() > cfg.busy.load_threshold {
            self.until = Some(now + Duration::from_secs_f64(cfg.busy.hold_seconds));
        }
    }

    /// Smoothed load of the busier of CPU and GPU.
    pub fn load(&self) -> f32 {
        self.cpu.unwrap_or(0.0).max(self.gpu.unwrap_or(0.0))
    }

    pub fn smoothed(&self) -> (f32, f32) {
        (self.cpu.unwrap_or(0.0), self.gpu.unwrap_or(0.0))
    }

    pub fn is_busy(&self, now: Instant, cfg: &Config) -> bool {
        if !cfg.busy.enabled {
            return false;
        }
        // hold_seconds = 0 means "busy only while above the threshold".
        self.load() > cfg.busy.load_threshold || self.until.is_some_and(|t| now < t)
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
        c.busy.average_seconds = 1.0;
        let t0 = Instant::now();
        let mut d = BusyDetector::new();
        d.sample(t0, 10.0, 0.0, &c);
        assert!(!d.is_busy(t0, &c));
        d.sample(t0 + Duration::from_secs(1), 90.0, 0.0, &c);
        assert!(d.is_busy(t0 + Duration::from_secs(1), &c));
        // Load drops to 0: the smoothed value falls below the threshold but
        // busy is held for 10 s after it was last above.
        for s in 2..=8 {
            d.sample(t0 + Duration::from_secs(s), 0.0, 0.0, &c);
        }
        assert!(d.is_busy(t0 + Duration::from_secs(8), &c), "held");
        d.sample(t0 + Duration::from_secs(20), 0.0, 0.0, &c);
        assert!(!d.is_busy(t0 + Duration::from_secs(20), &c));
        c.busy.enabled = false;
        assert!(!d.is_busy(t0, &c));
    }

    #[test]
    fn load_hovering_around_the_threshold_stays_busy() {
        // Threshold 15, GPU bouncing 14..21 (average ~17): busy throughout,
        // not just at the samples above 15.
        let mut c = cfg(4);
        c.busy.load_threshold = 15.0;
        c.busy.hold_seconds = 0.0;
        c.busy.average_seconds = 15.0;
        let t0 = Instant::now();
        let mut d = BusyDetector::new();
        let samples = [14.0, 21.0, 16.0, 14.0, 20.0, 15.0, 18.0, 14.0, 21.0, 17.0];
        let mut busy_after_warmup = Vec::new();
        for (i, g) in samples.iter().cycle().take(60).enumerate() {
            let t = t0 + Duration::from_secs(2 * i as u64);
            d.sample(t, 5.0, *g, &c);
            if i >= 10 {
                busy_after_warmup.push(d.is_busy(t, &c));
            }
        }
        assert!(busy_after_warmup.iter().all(|&b| b), "flapped: {busy_after_warmup:?}");
    }

    #[test]
    fn a_single_spike_does_not_start_the_busy_schedule() {
        let mut c = cfg(4);
        c.busy.hold_seconds = 0.0;
        c.busy.average_seconds = 15.0;
        let t0 = Instant::now();
        let mut d = BusyDetector::new();
        for i in 0..10 {
            d.sample(t0 + Duration::from_secs(2 * i), 3.0, 3.0, &c);
        }
        let t = t0 + Duration::from_secs(21);
        d.sample(t, 60.0, 3.0, &c);
        assert!(!d.is_busy(t, &c), "average is {}", d.load());
    }
}
