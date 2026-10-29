//! GPU load for Radeon (amdgpu sysfs) and Intel Arc (i915 PMU).
//!
//! * Radeon: `gpu_busy_percent` (graphics pipe), `mem_busy_percent` (memory
//!   controller activity, newer kernels) or VRAM fill as fallback.
//! * Intel: the i915 perf PMU exposes busy time per engine. The main bar is
//!   the busier of the render (rcs) and compute (ccs, Arc) engines, with
//!   extra rows for compute, the blitter (bcs) and video (vcs/vecs). Opening a PMU counter
//!   needs CAP_PERFMON (the systemd unit grants it) or perf_event_paranoid <= 0.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct GpuStat {
    /// DRM card name, e.g. "card1".
    pub card: String,
    /// Main load, 0..=100: graphics pipe (Radeon) or busiest render/compute engine (Intel).
    pub load: Option<f32>,
    /// Secondary bars drawn under the main one.
    pub rows: Vec<GpuRow>,
    pub vram: Option<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub struct GpuRow {
    pub label: &'static str,
    pub pct: Option<f32>,
}

pub struct Probe {
    card: String,
    name: String,
    dev: PathBuf,
    kind: Kind,
}

enum Kind {
    Amd,
    Intel(Vec<Engine>),
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
                let engines = open_i915(&slot);
                log::info!(
                    "{card}: i915 engines {:?}",
                    engines.iter().map(|e| e.name.as_str()).collect::<Vec<_>>()
                );
                Probe {
                    name: "Intel".into(),
                    kind: Kind::Intel(engines),
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
            ..Default::default()
        };
        match &mut self.kind {
            Kind::Amd => {
                s.load = read_num(&self.dev.join("gpu_busy_percent"));
                let used = read_num::<u64>(&self.dev.join("mem_info_vram_used"));
                let total = read_num::<u64>(&self.dev.join("mem_info_vram_total"));
                s.vram = used.zip(total).filter(|&(_, t)| t > 0);
                let mem = read_num(&self.dev.join("mem_busy_percent"));
                s.rows.push(match mem {
                    Some(m) => GpuRow { label: "MEM", pct: Some(m) },
                    None => GpuRow {
                        label: "VRAM",
                        pct: s.vram.map(|(u, t)| 100.0 * u as f32 / t as f32),
                    },
                });
            }
            Kind::Intel(engines) => {
                let by_class = read_classes(engines);
                let get = |c: &str| by_class.iter().find(|(k, _)| k == c).map(|(_, v)| *v);
                let (render, compute, blitter) = (get("rcs"), get("ccs"), get("bcs"));
                // Video decode/encode (vcs) and video enhance (vecs) engines.
                let video = [get("vcs"), get("vecs")].into_iter().flatten().reduce(f32::max);
                s.load = [render, compute].into_iter().flatten().reduce(f32::max);
                if let Some(c) = compute {
                    s.rows.push(GpuRow { label: "CMP", pct: Some(c) });
                }
                if let Some(b) = blitter {
                    s.rows.push(GpuRow { label: "BLT", pct: Some(b) });
                }
                if let Some(v) = video {
                    s.rows.push(GpuRow { label: "VID", pct: Some(v) });
                }
            }
        }
        s
    }

    /// One line per engine/counter with its current load, for `ax206mon gpu`.
    pub fn report(&mut self) -> Vec<String> {
        let mut out = vec![format!("{} ({})", self.card, self.name)];
        match &mut self.kind {
            Kind::Amd => {
                let s = self.read();
                out.push(format!("  graphics busy: {:?}", s.load));
                for r in &s.rows {
                    out.push(format!("  {}: {:?}", r.label, r.pct));
                }
            }
            Kind::Intel(engines) => {
                if engines.is_empty() {
                    out.push("  no engine counters could be opened".into());
                }
                for e in engines.iter_mut() {
                    out.push(format!("  {}: {:?}", e.name, e.counter.busy_pct()));
                }
            }
        }
        out
    }
}

/// Busiest engine per class ("rcs", "ccs", ...); engines whose counter
/// can't be read yet are skipped.
fn read_classes(engines: &mut [Engine]) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    for e in engines {
        let Some(p) = e.counter.busy_pct() else { continue };
        match out.iter_mut().find(|(c, _)| *c == e.class) {
            Some((_, v)) => *v = v.max(p),
            None => out.push((e.class.clone(), p)),
        }
    }
    out
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

/// One i915 engine ("rcs0", "ccs1", ...) and its busy counter.
struct Engine {
    name: String,
    /// Engine class without the instance number ("rcs", "ccs", "bcs", "vcs", "vecs").
    class: String,
    counter: Counter,
}

fn engine_class(name: &str) -> String {
    name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()
}

/// Open a busy counter for every engine the i915 PMU advertises.
fn open_i915(slot: &str) -> Vec<Engine> {
    // The first i915 device is plain "i915"; others are "i915_0000_03_00.0".
    let specific = format!("i915_{}", slot.replace(':', "_"));
    let base = [specific.as_str(), "i915"]
        .iter()
        .map(|n| Path::new("/sys/bus/event_source/devices").join(n))
        .find(|p| p.join("type").exists());
    let Some(base) = base else {
        log::warn!("i915 PMU not available; Intel GPU load disabled");
        return Vec::new();
    };
    let Some(pmu_type) = read_num::<u32>(&base.join("type")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = fs::read_dir(base.join("events"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix("-busy").map(str::to_string))
        .collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let spec = read_trim(&base.join("events").join(format!("{name}-busy")));
        let Some(config) = spec
            .strip_prefix("config=0x")
            .and_then(|h| u64::from_str_radix(h, 16).ok())
        else {
            continue;
        };
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
                "perf_event_open({name}-busy) failed: {} (needs CAP_PERFMON or perf_event_paranoid <= 0)",
                std::io::Error::last_os_error()
            );
            continue;
        }
        let mut counter = Counter { fd, prev: None };
        counter.busy_pct(); // prime
        out.push(Engine { class: engine_class(&name), name, counter });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_attr_is_ver5_sized() {
        assert_eq!(std::mem::size_of::<PerfAttr>(), 112);
    }

    #[test]
    fn engine_classes() {
        assert_eq!(engine_class("rcs0"), "rcs");
        assert_eq!(engine_class("ccs3"), "ccs");
        assert_eq!(engine_class("vecs1"), "vecs");
    }

    #[test]
    fn discovery_does_not_panic() {
        let _ = discover();
    }
}
