//! GPU load for Radeon (amdgpu sysfs) and Intel Arc (i915 PMU).
//!
//! * Radeon: `gpu_busy_percent` (graphics pipe), `mem_busy_percent` (memory
//!   controller activity, newer kernels) or VRAM fill as fallback.
//! * Intel: the i915 perf PMU exposes busy time per engine; we show the
//!   render engine and the blitter (copy) engine. Opening a PMU counter needs
//!   CAP_PERFMON (the systemd unit grants it) or perf_event_paranoid <= 0.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct GpuStat {
    /// DRM card name, e.g. "card1".
    pub card: String,
    /// Graphics/render engine load, 0..=100.
    pub load: Option<f32>,
    /// Second bar: blitter load (Intel) or memory activity / VRAM fill (Radeon).
    pub aux: Option<f32>,
    pub aux_label: &'static str,
    pub vram: Option<(u64, u64)>,
}

pub struct Probe {
    card: String,
    name: String,
    dev: PathBuf,
    kind: Kind,
}

enum Kind {
    Amd,
    Intel { render: Option<Counter>, blitter: Option<Counter> },
}

pub fn discover() -> Vec<Probe> {
    let mut cards: Vec<String> = fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.strip_prefix("card").is_some_and(|d| d.chars().all(|c| c.is_ascii_digit())))
        .collect();
    cards.sort_by_key(|c| c[4..].parse::<u32>().unwrap_or(0));
    let mut out = Vec::new();
    for card in cards {
        let dev = Path::new("/sys/class/drm").join(&card).join("device");
        let vendor = read_trim(&dev.join("vendor"));
        let driver = fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let probe = match (vendor.as_str(), driver.as_str()) {
            ("0x1002", "amdgpu") | ("0x1002", "radeon") => Probe {
                name: "Radeon".into(),
                kind: Kind::Amd,
                card,
                dev,
            },
            ("0x8086", "i915") => {
                let slot = fs::canonicalize(&dev)
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_default();
                let (render, blitter) = open_i915(&slot);
                Probe {
                    name: "Intel".into(),
                    kind: Kind::Intel { render, blitter },
                    card,
                    dev,
                }
            }
            ("0x8086", "xe") => {
                log::warn!("{card}: Intel GPU uses the xe driver; engine load is not supported yet");
                continue;
            }
            _ => continue,
        };
        log::info!("GPU {} ({}) found", probe.card, probe.name);
        out.push(probe);
    }
    out
}

impl Probe {
    pub fn read(&mut self) -> GpuStat {
        let mut s = GpuStat {
            card: self.card.clone(),
            aux_label: "MEM",
            ..Default::default()
        };
        match &mut self.kind {
            Kind::Amd => {
                s.load = read_num(&self.dev.join("gpu_busy_percent"));
                let used = read_num::<u64>(&self.dev.join("mem_info_vram_used"));
                let total = read_num::<u64>(&self.dev.join("mem_info_vram_total"));
                s.vram = used.zip(total).filter(|&(_, t)| t > 0);
                s.aux = read_num(&self.dev.join("mem_busy_percent"));
                if s.aux.is_none() {
                    s.aux = s.vram.map(|(u, t)| 100.0 * u as f32 / t as f32);
                    s.aux_label = "VRAM";
                }
            }
            Kind::Intel { render, blitter } => {
                s.aux_label = "BLT";
                s.load = render.as_mut().and_then(Counter::busy_pct);
                s.aux = blitter.as_mut().and_then(Counter::busy_pct);
            }
        }
        s
    }
}

fn read_trim(p: &Path) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn read_num<T: std::str::FromStr>(p: &Path) -> Option<T> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

// --- i915 perf PMU ----------------------------------------------------------

/// Cumulative busy nanoseconds of one engine.
struct Counter {
    fd: i32,
    prev: Option<(Instant, u64)>,
}

impl Drop for Counter {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

impl Counter {
    fn busy_pct(&mut self) -> Option<f32> {
        let mut v = 0u64;
        let n = unsafe { libc::read(self.fd, (&mut v as *mut u64).cast(), 8) };
        if n != 8 {
            return None;
        }
        let now = Instant::now();
        let pct = self.prev.and_then(|(t, pv)| {
            let wall = now.duration_since(t).as_nanos() as f64;
            (wall > 0.0).then(|| (100.0 * v.saturating_sub(pv) as f64 / wall).clamp(0.0, 100.0) as f32)
        });
        self.prev = Some((now, v));
        pct
    }
}

/// `struct perf_event_attr` (PERF_ATTR_SIZE_VER5, 112 bytes); only the first
/// three fields matter for a PMU counting event.
#[repr(C)]
struct PerfAttr {
    type_: u32,
    size: u32,
    config: u64,
    rest: [u64; 12],
}

fn open_i915(slot: &str) -> (Option<Counter>, Option<Counter>) {
    // The first i915 device is plain "i915"; others are "i915_0000_03_00.0".
    let specific = format!("i915_{}", slot.replace(':', "_"));
    let base = [specific.as_str(), "i915"]
        .iter()
        .map(|n| Path::new("/sys/bus/event_source/devices").join(n))
        .find(|p| p.join("type").exists());
    let Some(base) = base else {
        log::warn!("i915 PMU not available; Intel GPU load disabled");
        return (None, None);
    };
    let Some(pmu_type) = read_num::<u32>(&base.join("type")) else {
        return (None, None);
    };
    let open = |event: &str| -> Option<Counter> {
        let spec = read_trim(&base.join("events").join(event));
        let config = u64::from_str_radix(spec.strip_prefix("config=0x")?, 16).ok()?;
        let attr = PerfAttr {
            type_: pmu_type,
            size: std::mem::size_of::<PerfAttr>() as u32,
            config,
            rest: [0; 12],
        };
        // pid = -1, cpu = 0: system-wide uncore counter; flags = PERF_FLAG_FD_CLOEXEC.
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &attr, -1, 0, -1, 8u64) } as i32;
        if fd < 0 {
            log::warn!(
                "perf_event_open({event}) failed: {} (needs CAP_PERFMON or perf_event_paranoid <= 0)",
                std::io::Error::last_os_error()
            );
            return None;
        }
        let mut c = Counter { fd, prev: None };
        c.busy_pct(); // prime
        Some(c)
    };
    (open("rcs0-busy"), open("bcs0-busy"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_attr_is_ver5_sized() {
        assert_eq!(std::mem::size_of::<PerfAttr>(), 112);
    }

    #[test]
    fn discovery_does_not_panic() {
        let _ = discover();
    }
}
