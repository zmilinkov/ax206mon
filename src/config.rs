//! Configuration file (TOML). Every key is optional; see
//! packaging/config.toml for the documented defaults.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub const DEFAULT_PATH: &str = "/etc/ax206mon/config.toml";

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Clockwise rotation of the picture: 0, 90, 180 or 270.
    pub rotate: u16,
    /// Backlight level 0-7 set when the display is opened.
    pub backlight: u8,
    /// Turn the backlight off when the service stops.
    pub backlight_off_on_exit: bool,
    /// Seconds between dashboard refreshes (a frame takes ~1.2 s to send).
    pub interval: f64,
    /// Filesystem whose usage is shown.
    pub disk_path: String,
    /// Network interface for rx/tx rates; empty = all except loopback.
    pub net_interface: String,
    /// Temperature file in millidegrees C; empty = auto-detect.
    pub temp_file: String,
    /// Panel size as "WxH"; empty = ask the display.
    pub size: String,
    /// Frame data chunk size in bytes (Windows driver uses 262144).
    pub chunk_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rotate: 0,
            backlight: 7,
            backlight_off_on_exit: true,
            interval: 2.0,
            disk_path: "/".into(),
            net_interface: String::new(),
            temp_file: String::new(),
            size: String::new(),
            chunk_size: crate::ax206::DEFAULT_CHUNK,
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
                c.validate()?;
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
        if crate::frame::Rotation::from_degrees(self.rotate).is_none() {
            bail!("rotate must be 0, 90, 180 or 270 (got {})", self.rotate);
        }
        if self.backlight > 7 {
            bail!("backlight must be 0-7 (got {})", self.backlight);
        }
        if !(0.1..=3600.0).contains(&self.interval) {
            bail!("interval must be between 0.1 and 3600 seconds");
        }
        parse_size(&self.size)?;
        Ok(())
    }

    pub fn size(&self) -> Option<(u16, u16)> {
        parse_size(&self.size).ok().flatten()
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
    fn packaged_config_parses() {
        let s = include_str!("../packaging/config.toml");
        let c: Config = toml::from_str(s).unwrap();
        c.validate().unwrap();
    }

    #[test]
    fn rejects_bad_rotation() {
        let c: Config = toml::from_str("rotate = 45").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("480x320").unwrap(), Some((480, 320)));
        assert_eq!(parse_size("").unwrap(), None);
        assert!(parse_size("480").is_err());
    }
}
