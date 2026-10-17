//! System statistics read straight from /proc and /sys (no extra deps).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub hostname: String,
    /// Overall CPU busy, 0..=100.
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub temp_c: Option<f32>,
    pub load1: f32,
    pub uptime_s: u64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Filesystem whose usage is reported.
    pub disk_path: String,
    /// Network interface to watch; None = sum of all non-loopback.
    pub net_iface: Option<String>,
    /// Explicit temperature file (millidegrees), else auto-detect.
    pub temp_file: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            disk_path: "/".into(),
            net_iface: None,
            temp_file: None,
        }
    }
}

pub struct Collector {
    opts: Options,
    temp_file: Option<PathBuf>,
    prev_cpu: Option<(u64, u64)>,
    prev_net: Option<(Instant, u64, u64)>,
}

impl Collector {
    pub fn new(opts: Options) -> Self {
        let temp_file = opts.temp_file.clone().or_else(find_cpu_temp_sensor);
        if let Some(p) = &temp_file {
            log::info!("using temperature sensor {}", p.display());
        } else {
            log::info!("no temperature sensor found");
        }
        let mut c = Self {
            opts,
            temp_file,
            prev_cpu: None,
            prev_net: None,
        };
        // Prime the delta-based counters so the first snapshot is meaningful.
        let _ = c.cpu();
        let _ = c.net();
        c
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let (mem_used, mem_total) = meminfo().unwrap_or((0, 0));
        let (disk_used, disk_total) = disk_usage(&self.opts.disk_path).unwrap_or((0, 0));
        let (net_rx_bps, net_tx_bps) = self.net().unwrap_or((0.0, 0.0));
        Snapshot {
            hostname: hostname(),
            cpu_pct: self.cpu().unwrap_or(0.0),
            mem_used,
            mem_total,
            disk_used,
            disk_total,
            temp_c: self.temp_file.as_deref().and_then(read_millideg),
            load1: loadavg().unwrap_or(0.0),
            uptime_s: uptime().unwrap_or(0),
            net_rx_bps,
            net_tx_bps,
        }
    }

    fn cpu(&mut self) -> Option<f32> {
        let s = fs::read_to_string("/proc/stat").ok()?;
        let line = s.lines().find(|l| l.starts_with("cpu "))?;
        let v: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() < 4 {
            return None;
        }
        // user nice system idle iowait irq softirq steal ...
        let idle = v[3] + v.get(4).copied().unwrap_or(0);
        let total: u64 = v.iter().take(8).sum();
        let pct = match self.prev_cpu {
            Some((pt, pi)) if total > pt => {
                let dt = (total - pt) as f32;
                let di = idle.saturating_sub(pi) as f32;
                (100.0 * (1.0 - di / dt)).clamp(0.0, 100.0)
            }
            _ => 0.0,
        };
        self.prev_cpu = Some((total, idle));
        Some(pct)
    }

    fn net(&mut self) -> Option<(f64, f64)> {
        let s = fs::read_to_string("/proc/net/dev").ok()?;
        let (mut rx, mut tx) = (0u64, 0u64);
        for line in s.lines().skip(2) {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            let wanted = match &self.opts.net_iface {
                Some(i) => name == i,
                None => name != "lo",
            };
            if !wanted {
                continue;
            }
            let f: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            if f.len() >= 9 {
                rx += f[0];
                tx += f[8];
            }
        }
        let now = Instant::now();
        let rates = match self.prev_net {
            Some((t, prx, ptx)) => {
                let dt = now.duration_since(t).as_secs_f64().max(0.001);
                (
                    rx.saturating_sub(prx) as f64 / dt,
                    tx.saturating_sub(ptx) as f64 / dt,
                )
            }
            None => (0.0, 0.0),
        };
        self.prev_net = Some((now, rx, tx));
        Some(rates)
    }
}

fn meminfo() -> Option<(u64, u64)> {
    let s = fs::read_to_string("/proc/meminfo").ok()?;
    let get = |key: &str| -> Option<u64> {
        let line = s.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    let total = get("MemTotal:")?;
    let avail = get("MemAvailable:")?;
    Some((total.saturating_sub(avail), total))
}

fn disk_usage(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    let total = st.f_blocks as u64 * frsize;
    let free = st.f_bavail as u64 * frsize;
    Some((total.saturating_sub(free), total))
}

fn loadavg() -> Option<f32> {
    fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn uptime() -> Option<u64> {
    let s = fs::read_to_string("/proc/uptime").ok()?;
    s.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|f| f as u64)
}

fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "?".into())
}

fn read_millideg(p: &Path) -> Option<f32> {
    let v: f32 = fs::read_to_string(p).ok()?.trim().parse().ok()?;
    Some(v / 1000.0)
}

/// Find a CPU package temperature: known hwmon drivers first, then
/// thermal zones (x86_pkg_temp, cpu-thermal, ...), then any thermal zone.
fn find_cpu_temp_sensor() -> Option<PathBuf> {
    const CPU_HWMON: &[&str] = &[
        "coretemp",
        "k10temp",
        "zenpower",
        "cpu_thermal",
        "soc_thermal",
    ];
    if let Ok(dir) = fs::read_dir("/sys/class/hwmon") {
        let mut hwmons: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
        hwmons.sort();
        for h in &hwmons {
            let name = fs::read_to_string(h.join("name")).unwrap_or_default();
            if !CPU_HWMON.contains(&name.trim()) {
                continue;
            }
            // Prefer the input labelled Package/Tctl/Tdie, else temp1.
            let mut fallback = None;
            for i in 1..=16 {
                let input = h.join(format!("temp{i}_input"));
                if !input.exists() {
                    continue;
                }
                let label =
                    fs::read_to_string(h.join(format!("temp{i}_label"))).unwrap_or_default();
                let label = label.trim();
                if label.starts_with("Package") || label == "Tctl" || label == "Tdie" {
                    return Some(input);
                }
                fallback.get_or_insert(input);
            }
            if fallback.is_some() {
                return fallback;
            }
        }
    }
    if let Ok(dir) = fs::read_dir("/sys/class/thermal") {
        let mut zones: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("temp").exists())
            .collect();
        zones.sort();
        let ty = |z: &PathBuf| fs::read_to_string(z.join("type")).unwrap_or_default();
        if let Some(z) = zones.iter().find(|z| {
            let t = ty(z);
            t.contains("x86_pkg") || t.contains("cpu") || t.contains("soc")
        }) {
            return Some(z.join("temp"));
        }
        return zones.first().map(|z| z.join("temp"));
    }
    None
}
