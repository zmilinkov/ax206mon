//! Configuration file (TOML): a `[global]` table plus one `[[page]]` table
//! per dashboard page. Every key is optional; see packaging/ax206mon.conf
//! for the documented defaults.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::de::{Deserializer, Error as _};
use serde::Deserialize;

pub const DEFAULT_PATH: &str = "/etc/ax206mon/ax206mon.conf";

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub global: Global,
    pub busy: Busy,
    pub burnin: Burnin,
    /// Dashboard pages, shown in order. Defaults to the built-in pages.
    #[serde(rename = "page")]
    pub pages: Vec<Page>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Global {
    /// Clockwise rotation of the picture: 0, 90, 180 or 270.
    pub rotate: u16,
    /// Backlight level 0-7 set when the display is opened.
    pub backlight: u8,
    /// Multiplier on all text sizes (0.6 - 1.3).
    pub font_scale: f32,
    /// Turn the backlight off when the service stops.
    pub backlight_off_on_exit: bool,
    /// Seconds between redraws of the current page (a frame takes ~1.2 s to send).
    pub refresh_interval: f64,
    /// Seconds to stay on a page before moving to the next one.
    pub page_duration: f64,
    /// Filesystem used by `disk` widgets that don't set their own `path`.
    pub disk_path: String,
    /// Network interface for rx/tx rates; empty = all except loopback.
    pub net_interface: String,
    /// Temperature file in millidegrees C; empty = auto-detect.
    pub temp_file: String,
    /// Docker Engine API socket.
    pub docker_socket: String,
    /// Panel size as "WxH"; empty = ask the display.
    pub size: String,
    /// Frame data chunk size in bytes (Windows driver uses 262144).
    pub chunk_size: usize,
}

/// Page schedule while the machine is busy.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Busy {
    /// Use the busy schedule at all.
    pub enabled: bool,
    /// CPU (processing) or GPU load in percent above which the machine counts as busy.
    pub load_threshold: f32,
    /// Seconds page 1 stays up in the busy sequence.
    pub main_page_seconds: f64,
    /// Seconds every other page stays up in the busy sequence.
    pub other_page_seconds: f64,
    /// Seconds the load must stay below the threshold before leaving busy mode.
    pub hold_seconds: f64,
}

impl Default for Busy {
    fn default() -> Self {
        Self {
            enabled: true,
            load_threshold: 30.0,
            main_page_seconds: 12.0,
            other_page_seconds: 8.0,
            hold_seconds: 10.0,
        }
    }
}

/// Screen-protection options: night backlight, header variation, idle refresh.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Burnin {
    /// Night window start "HH:MM" (local time); empty = no night schedule.
    pub night_start: String,
    /// Night window end "HH:MM".
    pub night_end: String,
    /// Backlight level during the night window: 0 = off, 1-7 = dimmed.
    pub night_backlight: u8,
    /// How the clock header varies: off, page, time or both.
    pub header_variation: crate::burnin::Variation,
    /// Minutes between header changes for `time` / `both`.
    pub header_variation_minutes: f64,
    /// Hours between full-screen refresh cycles; 0 = never.
    pub idle_refresh_hours: f64,
    /// Seconds a refresh cycle lasts.
    pub idle_refresh_seconds: f64,
}

impl Default for Burnin {
    fn default() -> Self {
        Self {
            night_start: String::new(),
            night_end: String::new(),
            night_backlight: 0,
            header_variation: Default::default(),
            header_variation_minutes: 30.0,
            idle_refresh_hours: 0.0,
            idle_refresh_seconds: 10.0,
        }
    }
}

impl Burnin {
    /// Night window as minutes-since-midnight, if configured (validated).
    pub fn night_window(&self) -> Option<(u32, u32)> {
        let s = crate::burnin::parse_hhmm(&self.night_start)?;
        let e = crate::burnin::parse_hhmm(&self.night_end)?;
        Some((s, e))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
    /// Shown in the page header when there is more than one page.
    pub name: String,
    /// Seconds to stay on this page; overrides `global.page_duration`.
    pub duration: Option<f64>,
    /// Widgets from top to bottom. `status` widgets always sit at the bottom.
    pub widgets: Vec<Widget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// CPU load bar, processing vs. I/O wait.
    Cpu,
    /// Memory bar: apps / system / buffers+cache.
    Ram,
    /// Filesystem usage bar.
    Disk,
    /// CPU temperature bar.
    Temp,
    /// GPU load bar plus blitter / memory activity bar (Radeon, Intel Arc).
    Gpu,
    /// systemd services, named as `systemctl list-units -t service` shows them.
    Services,
    /// Interfaces that have an IPv4 or IPv6 address.
    Network,
    /// Footer line: hostname, uptime, load, network rates.
    Status,
    /// Docker daemon reachability and container counts.
    DockerStatus,
    /// Running docker compose stacks.
    DockerStacks,
}

/// A widget is either just its kind (`"cpu"`) or a table with options
/// (`{ type = "disk", path = "/data", label = "DATA" }`).
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub kind: Kind,
    pub label: Option<String>,
    /// `disk`: filesystem to show.
    pub path: Option<String>,
    /// `gpu`: DRM card name such as "card1"; default = first GPU found.
    pub card: Option<String>,
    /// `services`: units to show, in this order (".service" is optional).
    pub units: Option<Vec<String>>,
    /// `services` without `units`: "running" (default), "active", "failed" or "all".
    pub filter: Option<String>,
    /// `network`: interfaces to show, in this order (trailing `*` = prefix
    /// match); none or empty = all.
    pub interfaces: Option<Vec<String>>,
    /// `network`: show IPv4 addresses (default true).
    pub ipv4: Option<bool>,
    /// `network`: show IPv6 addresses (default true).
    pub ipv6: Option<bool>,
    /// `cpu` / `gpu`: show the temperature under the load bar, or next to the
    /// load when there is no room (default true).
    pub temp: Option<bool>,
    /// `ram`: show a swap bar under the memory bar when swap exists (default true).
    pub swap: Option<bool>,
    /// `network`: show each interface's MAC address under its IPs (default false).
    pub mac: Option<bool>,
    /// `network` without `interfaces`: also list loopback (default false).
    pub loopback: Option<bool>,
}

impl Widget {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            label: None,
            path: None,
            card: None,
            units: None,
            filter: None,
            interfaces: None,
            ipv4: None,
            ipv6: None,
            temp: None,
            swap: None,
            mac: None,
            loopback: None,
        }
    }
}

impl<'de> Deserialize<'de> for Widget {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Full {
            #[serde(rename = "type")]
            kind: Kind,
            label: Option<String>,
            path: Option<String>,
            card: Option<String>,
            units: Option<Vec<String>>,
            filter: Option<String>,
            interfaces: Option<Vec<String>>,
            ipv4: Option<bool>,
            ipv6: Option<bool>,
            temp: Option<bool>,
            swap: Option<bool>,
            mac: Option<bool>,
            loopback: Option<bool>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            Full(Full),
        }
        match Raw::deserialize(d)? {
            Raw::Full(f) => Ok(Widget {
                kind: f.kind,
                label: f.label,
                path: f.path,
                card: f.card,
                units: f.units,
                filter: f.filter,
                interfaces: f.interfaces,
                ipv4: f.ipv4,
                ipv6: f.ipv6,
                temp: f.temp,
                swap: f.swap,
                mac: f.mac,
                loopback: f.loopback,
            }),
            Raw::Name(s) => {
                let kind = Kind::deserialize(serde::de::value::StrDeserializer::<D::Error>::new(&s))
                    .map_err(|_| {
                        D::Error::custom(format!(
                            "unknown widget {s:?} (cpu, ram, disk, temp, gpu, services, \
                             network, status, docker_status, docker_stacks)"
                        ))
                    })?;
                Ok(Widget::new(kind))
            }
        }
    }
}

impl Default for Global {
    fn default() -> Self {
        Self {
            rotate: 270,
            backlight: 7,
            font_scale: 1.0,
            backlight_off_on_exit: true,
            refresh_interval: 2.0,
            page_duration: 15.0,
            disk_path: "/".into(),
            net_interface: String::new(),
            temp_file: String::new(),
            docker_socket: crate::docker::DEFAULT_SOCKET.into(),
            size: String::new(),
            chunk_size: crate::ax206::DEFAULT_CHUNK,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        use Kind::*;
        let w = |kinds: &[Kind]| kinds.iter().map(|&k| Widget::new(k)).collect();
        Self {
            global: Global::default(),
            busy: Busy::default(),
            burnin: Burnin::default(),
            pages: vec![
                Page {
                    name: "System".into(),
                    duration: None,
                    widgets: w(&[Cpu, Ram, Disk, Gpu, Status]),
                },
                Page {
                    name: "Services".into(),
                    duration: None,
                    widgets: w(&[Services]),
                },
                Page {
                    name: "Network".into(),
                    duration: None,
                    widgets: w(&[Network]),
                },
                Page {
                    name: "Compose".into(),
                    duration: None,
                    widgets: w(&[DockerStacks]),
                },
            ],
        }
    }
}

impl Config {
    /// Load `path`; a missing file at the default location means defaults.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let (path, explicit) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => (PathBuf::from(DEFAULT_PATH), false),
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let c: Config =
                    toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
                c.validate()
                    .with_context(|| format!("in {}", path.display()))?;
                log::debug!("loaded config from {}", path.display());
                Ok(c)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
                Ok(Config::default())
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let g = &self.global;
        if crate::frame::Rotation::from_degrees(g.rotate).is_none() {
            bail!("global.rotate must be 0, 90, 180 or 270 (got {})", g.rotate);
        }
        if g.backlight > 7 {
            bail!("global.backlight must be 0-7 (got {})", g.backlight);
        }
        if !(0.6..=1.3).contains(&g.font_scale) {
            bail!("global.font_scale must be between 0.6 and 1.3 (got {})", g.font_scale);
        }
        if !(0.1..=3600.0).contains(&g.refresh_interval) {
            bail!("global.refresh_interval must be between 0.1 and 3600 seconds");
        }
        if !(1.0..=86_400.0).contains(&g.page_duration) {
            bail!("global.page_duration must be between 1 and 86400 seconds");
        }
        let busy = &self.busy;
        if !(1.0..=100.0).contains(&busy.load_threshold) {
            bail!("busy.load_threshold must be between 1 and 100 percent");
        }
        for (name, v) in [
            ("main_page_seconds", busy.main_page_seconds),
            ("other_page_seconds", busy.other_page_seconds),
        ] {
            if !(1.0..=86_400.0).contains(&v) {
                bail!("busy.{name} must be between 1 and 86400 seconds");
            }
        }
        if !(0.0..=3600.0).contains(&busy.hold_seconds) {
            bail!("busy.hold_seconds must be between 0 and 3600 seconds");
        }
        let b = &self.burnin;
        match (b.night_start.is_empty(), b.night_end.is_empty()) {
            (true, true) => {}
            (false, false) => {
                let (s, e) = (
                    crate::burnin::parse_hhmm(&b.night_start),
                    crate::burnin::parse_hhmm(&b.night_end),
                );
                match (s, e) {
                    (Some(s), Some(e)) if s != e => {}
                    (Some(_), Some(_)) => bail!("burnin.night_start and night_end must differ"),
                    _ => bail!("burnin.night_start/night_end must look like \"23:00\""),
                }
            }
            _ => bail!("burnin.night_start and night_end must be set together"),
        }
        if b.night_backlight > 7 {
            bail!("burnin.night_backlight must be 0-7 (got {})", b.night_backlight);
        }
        if !(1.0..=1440.0).contains(&b.header_variation_minutes) {
            bail!("burnin.header_variation_minutes must be between 1 and 1440");
        }
        if b.idle_refresh_hours != 0.0 && !(0.1..=168.0).contains(&b.idle_refresh_hours) {
            bail!("burnin.idle_refresh_hours must be 0 (off) or between 0.1 and 168");
        }
        if !(1.0..=600.0).contains(&b.idle_refresh_seconds) {
            bail!("burnin.idle_refresh_seconds must be between 1 and 600");
        }
        if self.pages.is_empty() {
            bail!("at least one [[page]] is required");
        }
        for (i, p) in self.pages.iter().enumerate() {
            if let Some(d) = p.duration {
                if !(1.0..=86_400.0).contains(&d) {
                    bail!("page {}: duration must be between 1 and 86400 seconds", i + 1);
                }
            }
            if p.widgets.is_empty() {
                bail!("page {}: no widgets", i + 1);
            }
            for w in &p.widgets {
                if w.kind == Kind::Network && !w.ipv4.unwrap_or(true) && !w.ipv6.unwrap_or(true) {
                    bail!("page {}: network widget has ipv4 and ipv6 both disabled", i + 1);
                }
                if let Some(f) = &w.filter {
                    if crate::services::Filter::parse(f).is_none() {
                        bail!("page {}: filter must be running, active, failed or all (got {f:?})", i + 1);
                    }
                }
            }
        }
        parse_size(&g.size)?;
        Ok(())
    }

    pub fn size(&self) -> Option<(u16, u16)> {
        parse_size(&self.global.size).ok().flatten()
    }

    /// How long page `i` stays on screen.
    pub fn page_duration(&self, i: usize) -> f64 {
        self.pages[i].duration.unwrap_or(self.global.page_duration)
    }
}

pub fn parse_size(s: &str) -> Result<Option<(u16, u16)>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let (w, h) = s
        .to_ascii_lowercase()
        .split_once('x')
        .map(|(w, h)| (w.trim().parse::<u16>(), h.trim().parse::<u16>()))
        .with_context(|| format!("size must look like 480x320 (got {s:?})"))?;
    match (w, h) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => Ok(Some((w, h))),
        _ => bail!("size must look like 480x320 (got {s:?})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_config_parses_and_matches_defaults() {
        let s = include_str!("../packaging/ax206mon.conf");
        let c: Config = toml::from_str(s).unwrap();
        c.validate().unwrap();
        let d = Config::default();
        assert_eq!(c.global.rotate, d.global.rotate);
        assert_eq!(c.pages.len(), d.pages.len());
        for (a, b) in c.pages.iter().zip(&d.pages) {
            assert_eq!(a.widgets, b.widgets);
        }
    }

    #[test]
    fn rejects_bad_rotation() {
        let c: Config = toml::from_str("[global]\nrotate = 45").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn widget_forms() {
        let c: Config = toml::from_str(
            r#"
            [[page]]
            widgets = ["cpu", { type = "disk", path = "/data", label = "DATA" }]
            "#,
        )
        .unwrap();
        assert_eq!(c.pages[0].widgets[0], Widget::new(Kind::Cpu));
        assert_eq!(c.pages[0].widgets[1].path.as_deref(), Some("/data"));
    }

    #[test]
    fn network_options_parse_and_validate() {
        let c: Config = toml::from_str(
            r#"
            [[page]]
            widgets = [{ type = "network", interfaces = ["eth0", "br-*"], ipv6 = false, mac = true }]
            "#,
        )
        .unwrap();
        c.validate().unwrap();
        let w = &c.pages[0].widgets[0];
        assert_eq!(w.interfaces.as_deref().unwrap().len(), 2);
        assert_eq!(w.ipv6, Some(false));
        assert_eq!(w.mac, Some(true));
        let bad: Config = toml::from_str(
            "[[page]]\nwidgets = [{ type = \"network\", ipv4 = false, ipv6 = false }]",
        )
        .unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn busy_defaults_and_validation() {
        let d = Config::default();
        assert_eq!(
            (d.busy.load_threshold, d.busy.main_page_seconds, d.busy.other_page_seconds),
            (30.0, 12.0, 8.0)
        );
        for bad in ["[busy]\nload_threshold = 0", "[busy]\nmain_page_seconds = 0", "[busy]\nhold_seconds = -1"] {
            let c: Config = toml::from_str(bad).unwrap();
            assert!(c.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn burnin_defaults_and_validation() {
        let d = Config::default();
        d.validate().unwrap();
        assert!(d.burnin.night_window().is_none());
        let c: Config = toml::from_str(
            "[burnin]\nnight_start = \"23:00\"\nnight_end = \"07:00\"\nheader_variation = \"page\"",
        )
        .unwrap();
        c.validate().unwrap();
        assert_eq!(c.burnin.night_window(), Some((23 * 60, 7 * 60)));
        for bad in [
            "[burnin]\nnight_start = \"23:00\"",
            "[burnin]\nnight_start = \"25:00\"\nnight_end = \"07:00\"",
            "[burnin]\nnight_start = \"07:00\"\nnight_end = \"07:00\"",
            "[burnin]\nidle_refresh_hours = 0.01",
        ] {
            let c: Config = toml::from_str(bad).unwrap();
            assert!(c.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn unknown_widget_is_an_error() {
        let e = toml::from_str::<Config>("[[page]]\nwidgets = [\"cpuu\"]").unwrap_err();
        assert!(e.to_string().contains("unknown widget"), "{e}");
    }

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("480x320").unwrap(), Some((480, 320)));
        assert_eq!(parse_size("").unwrap(), None);
        assert!(parse_size("480").is_err());
    }
}
