//! systemd service list, as `systemctl list-units -t service` reports it.
//! Runs systemctl (read-only, works for unprivileged users over D-Bus).

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Unit name as systemctl prints it, e.g. "docker.service".
    pub name: String,
    /// ACTIVE column: active, inactive, failed, ...
    pub active: String,
    /// SUB column: running, exited, dead, failed, ...
    pub sub: String,
}

#[derive(Clone, Debug, Default)]
pub struct ServiceList {
    pub error: Option<String>,
    pub units: Vec<Unit>,
}

pub fn query() -> ServiceList {
    match run() {
        Ok(units) => ServiceList { error: None, units },
        Err(e) => ServiceList {
            error: Some(e),
            units: Vec::new(),
        },
    }
}

fn run() -> Result<Vec<Unit>, String> {
    let (tx, rx) = mpsc::channel();
    // A thread so a wedged systemctl can't freeze the display loop.
    std::thread::spawn(move || {
        let out = Command::new("systemctl")
            .args(["list-units", "--type=service", "--all", "--plain", "--no-legend", "--no-pager", "--full"])
            .env("LC_ALL", "C")
            .env("SYSTEMD_COLORS", "0")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let out = rx
        .recv_timeout(TIMEOUT)
        .map_err(|_| "systemctl timed out".to_string())?
        .map_err(|e| format!("systemctl: {e}"))?;
    if !out.status.success() {
        return Err("systemctl failed".into());
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `UNIT LOAD ACTIVE SUB DESCRIPTION...` lines.
pub fn parse(s: &str) -> Vec<Unit> {
    s.lines()
        .filter_map(|l| {
            let mut f = l.trim_start_matches(['●', '*', ' ']).split_whitespace();
            let (name, _load, active, sub) = (f.next()?, f.next()?, f.next()?, f.next()?);
            name.ends_with(".service").then(|| Unit {
                name: name.into(),
                active: active.into(),
                sub: sub.into(),
            })
        })
        .collect()
}

/// Which units a `services` widget shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Running,
    Active,
    Failed,
    All,
}

impl Filter {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Filter::Running),
            "active" => Some(Filter::Active),
            "failed" => Some(Filter::Failed),
            "all" => Some(Filter::All),
            _ => None,
        }
    }

    fn matches(self, u: &Unit) -> bool {
        match self {
            Filter::Running => u.sub == "running",
            Filter::Active => u.active == "active",
            Filter::Failed => u.active == "failed",
            Filter::All => true,
        }
    }
}

/// Units to show, in display order. An explicit `units` list is shown in the
/// order given (units systemd doesn't know appear as "not-found"); otherwise
/// the filtered list is sorted failed-first, then by name.
pub fn select(all: &[Unit], units: Option<&[String]>, filter: Filter) -> Vec<Unit> {
    if let Some(names) = units {
        return names
            .iter()
            .map(|n| {
                let name = if n.contains('.') { n.clone() } else { format!("{n}.service") };
                all.iter().find(|u| u.name == name).cloned().unwrap_or(Unit {
                    name,
                    active: "inactive".into(),
                    sub: "not-found".into(),
                })
            })
            .collect();
    }
    let mut v: Vec<Unit> = all.iter().filter(|u| filter.matches(u)).cloned().collect();
    v.sort_by(|a, b| {
        (a.active != "failed", &a.name).cmp(&(b.active != "failed", &b.name))
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
  auditd.service        loaded active   running Security Auditing Service
● foo.service           loaded failed   failed  Foo
  docker.service        loaded active   running Docker Application Container Engine
  dnf-makecache.service loaded inactive dead    dnf makecache
  sshd.service          loaded active   running OpenSSH server daemon
";

    #[test]
    fn parses_list_units() {
        let u = parse(SAMPLE);
        assert_eq!(u.len(), 5);
        assert_eq!(u[1].name, "foo.service");
        assert_eq!((u[1].active.as_str(), u[1].sub.as_str()), ("failed", "failed"));
    }

    #[test]
    fn filters_and_sorts_failed_first() {
        let u = parse(SAMPLE);
        let names = |v: Vec<Unit>| v.into_iter().map(|u| u.name).collect::<Vec<_>>();
        assert_eq!(
            names(select(&u, None, Filter::All))[..2],
            ["foo.service", "auditd.service"]
        );
        assert_eq!(
            names(select(&u, None, Filter::Running)),
            ["auditd.service", "docker.service", "sshd.service"]
        );
    }

    #[test]
    fn explicit_units_keep_order_and_flag_unknown() {
        let u = parse(SAMPLE);
        let want = vec!["sshd".to_string(), "nope.service".to_string()];
        let s = select(&u, Some(&want), Filter::Running);
        assert_eq!(s[0].name, "sshd.service");
        assert_eq!(s[0].sub, "running");
        assert_eq!(s[1].sub, "not-found");
    }
}
