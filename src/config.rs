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
    /// `network`: also list loopback interfaces (default false).
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
            pages: vec![
                Page {
                    name: "System".into(),
                    duration: None,
                    widgets: w(&[Cpu, Ram, Disk, Temp, Gpu, Status]),
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
        if !(0.1..=3600.0).contains(&g.refresh_interval) {
            bail!("global.refresh_interval must be between 0.1 and 3600 seconds");
        }
        if !(1.0..=86_400.0).contains(&g.page_duration) {
            bail!("global.page_duration must be between 1 and 86400 seconds");
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
