//! System statistics read straight from /proc and /sys (no extra deps).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::docker::{self, DockerStatus};
use crate::gpu::{self, GpuStat};
use crate::netif::{self, Iface};
use crate::services::{self, ServiceList};

/// CPU time split since the previous snapshot, each 0..=100.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuUsage {
    /// Actually executing (user, nice, system, irq, steal).
    pub busy: f32,
    /// Idle but waiting for I/O.
    pub iowait: f32,
}

impl CpuUsage {
    pub fn total(&self) -> f32 {
        self.busy + self.iowait
    }
}

/// Memory split in bytes. `apps + kernel + cache + free <= total`.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemUsage {
    pub total: u64,
    /// Process memory (anonymous, shmem, ...).
    pub apps: u64,
    /// Kernel allocations that can't be reclaimed (slab, stacks, page tables).
    pub kernel: u64,
    /// Page cache, buffers and reclaimable slab.
    pub cache: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

impl MemUsage {
    pub fn used(&self) -> u64 {
        self.apps + self.kernel
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub hostname: String,
    pub cpu: CpuUsage,
    pub mem: MemUsage,
    /// (used, total) bytes per configured filesystem path.
    pub disks: HashMap<String, (u64, u64)>,
    pub gpus: Vec<GpuStat>,
    /// Only filled when the current page has a docker widget.
    pub docker: Option<DockerStatus>,
    /// Only filled when the current page has a services widget.
    pub services: Option<ServiceList>,
    /// Only filled when the current page has a network widget.
    pub interfaces: Option<Vec<Iface>>,
    pub temp_c: Option<f32>,
    pub load1: f32,
    pub uptime_s: u64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
}

/// What the page being drawn needs; keeps unneeded sources unread.
#[derive(Clone, Debug, Default)]
pub struct Needs {
    pub disk_paths: Vec<String>,
    /// GPU counters are deltas, so this must cover every page, not just the current one.
    pub gpu: bool,
    pub docker: bool,
    pub services: bool,
    pub interfaces: bool,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Docker Engine API socket.
    pub docker_socket: String,
    /// Network interface to watch; None = sum of all non-loopback.
    pub net_iface: Option<String>,
    /// Explicit temperature file (millidegrees), else auto-detect.
    pub temp_file: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            docker_socket: docker::DEFAULT_SOCKET.into(),
            net_iface: None,
            temp_file: None,
        }
    }
}

pub struct Collector {
    opts: Options,
    temp_file: Option<PathBuf>,
    prev_cpu: Option<[u64; 3]>,
    gpus: Option<Vec<gpu::Probe>>,
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
            gpus: None,
            prev_net: None,
        };
        // Prime the delta-based counters so the first snapshot is meaningful.
        let _ = c.cpu();
        let _ = c.net();
        c
    }

    pub fn snapshot(&mut self, needs: &Needs) -> Snapshot {
        let (net_rx_bps, net_tx_bps) = self.net().unwrap_or((0.0, 0.0));
        let gpus = if needs.gpu {
            self.gpus
                .get_or_insert_with(gpu::discover)
                .iter_mut()
                .map(gpu::Probe::read)
                .collect()
        } else {
            Vec::new()
        };
        Snapshot {
            hostname: hostname(),
            cpu: self.cpu().unwrap_or_default(),
            mem: meminfo().unwrap_or_default(),
            disks: needs
                .disk_paths
                .iter()
                .filter_map(|p| Some((p.clone(), disk_usage(p)?)))
                .collect(),
            gpus,
            docker: needs
                .docker
                .then(|| docker::query(&self.opts.docker_socket)),
            services: needs.services.then(services::query),
            interfaces: needs.interfaces.then(netif::list),
            temp_c: self.temp_file.as_deref().and_then(read_millideg),
            load1: loadavg().unwrap_or(0.0),
            uptime_s: uptime().unwrap_or(0),
            net_rx_bps,
            net_tx_bps,
        }
    }

    fn cpu(&mut self) -> Option<CpuUsage> {
        let s = fs::read_to_string("/proc/stat").ok()?;
        let (total, busy, iowait) = parse_cpu_line(s.lines().find(|l| l.starts_with("cpu "))?)?;
        let cur = [total, busy, iowait];
        let usage = match self.prev_cpu {
            Some([pt, pb, pw]) if total > pt => {
                let dt = (total - pt) as f32;
                CpuUsage {
                    busy: (100.0 * busy.saturating_sub(pb) as f32 / dt).clamp(0.0, 100.0),
                    iowait: (100.0 * iowait.saturating_sub(pw) as f32 / dt).clamp(0.0, 100.0),
                }
            }
            _ => CpuUsage::default(),
        };
        self.prev_cpu = Some(cur);
        Some(usage)
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

/// (total, busy, iowait) jiffies from the aggregate `cpu ` line of /proc/stat.
fn parse_cpu_line(line: &str) -> Option<(u64, u64, u64)> {
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal ...
    let g = |i: usize| v.get(i).copied().unwrap_or(0);
    let total: u64 = v.iter().take(8).sum();
    let busy = g(0) + g(1) + g(2) + g(5) + g(6) + g(7);
    Some((total, busy, g(4)))
}

fn meminfo() -> Option<MemUsage> {
    parse_meminfo(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn parse_meminfo(s: &str) -> Option<MemUsage> {
    let get = |key: &str| -> Option<u64> {
        let line = s.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    let total = get("MemTotal:")?;
    let free = get("MemFree:")?;
    let or0 = |k| get(k).unwrap_or(0);
    // Shmem is counted inside Cached but can't be dropped, so it is app memory.
    let cache = (or0("Buffers:") + or0("Cached:") + or0("SReclaimable:"))
        .saturating_sub(or0("Shmem:"));
    let in_use = total.saturating_sub(free).saturating_sub(cache);
    let kernel = (or0("SUnreclaim:") + or0("KernelStack:") + or0("PageTables:") + or0("Percpu:"))
        .min(in_use);
    Some(MemUsage {
        total,
        apps: in_use - kernel,
        kernel,
        cache,
        swap_total: or0("SwapTotal:"),
        swap_used: or0("SwapTotal:").saturating_sub(or0("SwapFree:")),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_line_splits_busy_and_iowait() {
        // user nice system idle iowait irq softirq steal
        let (total, busy, iowait) = parse_cpu_line("cpu  10 5 20 100 30 1 2 3 0 0").unwrap();
        assert_eq!((total, busy, iowait), (171, 41, 30));
    }

    #[test]
    fn meminfo_split() {
        let m = parse_meminfo(
            "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 300 kB\n\
             SReclaimable: 50 kB\nShmem: 100 kB\nSUnreclaim: 40 kB\nKernelStack: 10 kB\n\
             PageTables: 10 kB\nPercpu: 0 kB\nSwapTotal: 400 kB\nSwapFree: 300 kB\n",
        )
        .unwrap();
        assert_eq!(m.total, 1000 * 1024);
        assert_eq!(m.cache, 300 * 1024);
        assert_eq!(m.kernel, 60 * 1024);
        assert_eq!(m.apps, (1000 - 100 - 300 - 60) * 1024);
        assert!(m.used() + m.cache <= m.total);
        assert_eq!((m.swap_total, m.swap_used), (400 * 1024, 100 * 1024));
    }
}
