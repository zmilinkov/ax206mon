//! Minimal Docker Engine API client over the unix socket (no extra deps
//! beyond serde_json). Only what the Docker pages need: container list
//! grouped into compose stacks, plus the daemon version.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

/// Tried in order when no socket is configured. Podman's API is
/// Docker-compatible, so a podman-only host (e.g. AlmaLinux) works too.
pub const AUTO_SOCKETS: &[&str] = &["/var/run/docker.sock", "/run/podman/podman.sock"];
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_BODY: u64 = 16 << 20;

#[derive(Clone, Debug, Default)]
pub struct Stack {
    pub name: String,
    pub running: u32,
    pub total: u32,
    pub unhealthy: u32,
}

#[derive(Clone, Debug, Default)]
pub struct DockerStatus {
    /// Why the daemon couldn't be queried; None when it answered.
    pub error: Option<String>,
    pub version: String,
    pub total: u32,
    pub running: u32,
    pub paused: u32,
    pub restarting: u32,
    pub unhealthy: u32,
    /// Compose stacks with at least one running container, sorted by name.
    pub stacks: Vec<Stack>,
}

impl DockerStatus {
    pub fn up(&self) -> bool {
        self.error.is_none()
    }
}

/// Query the engine at `socket`; empty = try [`AUTO_SOCKETS`].
pub fn query(socket: &str) -> DockerStatus {
    match pick_socket(socket).and_then(|p| try_query(&p).map_err(|e| explain(&e, &p))) {
        Ok(s) => s,
        Err(e) => DockerStatus {
            error: Some(e),
            ..Default::default()
        },
    }
}

fn pick_socket(configured: &str) -> Result<String, String> {
    if !configured.is_empty() {
        return Ok(configured.to_string());
    }
    AUTO_SOCKETS
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
        .ok_or_else(|| format!("no engine socket found (tried {})", AUTO_SOCKETS.join(", ")))
}

/// Human-readable failure, with the fix for the common permission case.
fn explain(e: &anyhow::Error, socket: &str) -> String {
    let root = e.root_cause().to_string();
    let low = root.to_lowercase();
    if low.contains("permission denied") {
        format!("permission denied on {socket}; give the service SupplementaryGroups=docker (see README)")
    } else if low.contains("no such file") || low.contains("connection refused") {
        format!("engine not running ({socket})")
    } else {
        format!("{socket}: {root}")
    }
}

fn try_query(socket: &str) -> Result<DockerStatus> {
    let version: Version = serde_json::from_slice(&get(socket, "/version")?)?;
    let containers: Vec<Container> =
        serde_json::from_slice(&get(socket, "/containers/json?all=1")?)?;
    let mut st = summarize(&containers);
    st.version = version.version;
    Ok(st)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Version {
    #[serde(default)]
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Container {
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
}

fn summarize(containers: &[Container]) -> DockerStatus {
    let mut st = DockerStatus::default();
    let mut stacks: BTreeMap<String, Stack> = BTreeMap::new();
    for c in containers {
        let running = matches!(c.state.as_str(), "running" | "restarting");
        let unhealthy = c.status.contains("(unhealthy)");
        st.total += 1;
        st.running += u32::from(c.state == "running");
        st.paused += u32::from(c.state == "paused");
        st.restarting += u32::from(c.state == "restarting");
        st.unhealthy += u32::from(unhealthy);
        if let Some(p) = c
            .labels
            .as_ref()
            .and_then(|l| l.get("com.docker.compose.project"))
        {
            let s = stacks.entry(p.clone()).or_insert_with(|| Stack {
                name: p.clone(),
                ..Default::default()
            });
            s.total += 1;
            // A restarting container is not healthy but is not "up" either.
            s.running += u32::from(c.state == "running");
            s.unhealthy += u32::from(unhealthy || (running && c.state == "restarting"));
        }
    }
    st.stacks = stacks
        .into_values()
        .filter(|s| s.running > 0 || s.unhealthy > 0)
        .collect();
    st
}

/// GET `path` and return the response body. HTTP/1.0 keeps the daemon from
/// chunking, but chunked replies are still decoded.
fn get(socket: &str, path: &str) -> Result<Vec<u8>> {
    let mut s = UnixStream::connect(socket).with_context(|| format!("connecting to {socket}"))?;
    s.set_read_timeout(Some(TIMEOUT))?;
    s.set_write_timeout(Some(TIMEOUT))?;
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: docker\r\n\r\n").as_bytes())?;
    let mut raw = Vec::new();
    s.take(MAX_BODY).read_to_end(&mut raw)?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<Vec<u8>> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
    let body = &raw[split + 4..];
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| anyhow!("malformed HTTP status line"))?;
    if status != 200 {
        bail!("docker API returned HTTP {status}");
    }
    if head.contains("transfer-encoding: chunked") {
        dechunk(body)
    } else {
        Ok(body.to_vec())
    }
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("bad chunked body"))?;
        let size = usize::from_str_radix(
            std::str::from_utf8(&b[..eol])?.split(';').next().unwrap_or("").trim(),
            16,
        )?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size + 2 {
            bail!("truncated chunked body");
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn containers(json: &str) -> Vec<Container> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn groups_compose_stacks() {
        let c = containers(
            r#"[
            {"State":"running","Status":"Up 2 hours","Labels":{"com.docker.compose.project":"web"}},
            {"State":"running","Status":"Up 2 hours (unhealthy)","Labels":{"com.docker.compose.project":"web"}},
            {"State":"exited","Status":"Exited (0)","Labels":{"com.docker.compose.project":"web"}},
            {"State":"exited","Status":"Exited (0)","Labels":{"com.docker.compose.project":"old"}},
            {"State":"running","Status":"Up 1 minute","Labels":null}
            ]"#,
        );
        let st = summarize(&c);
        assert_eq!((st.total, st.running, st.unhealthy), (5, 3, 1));
        assert_eq!(st.stacks.len(), 1, "stopped stacks are hidden");
        let web = &st.stacks[0];
        assert_eq!((web.name.as_str(), web.running, web.total, web.unhealthy), ("web", 2, 3, 1));
    }

    #[test]
    fn http_plain_and_chunked() {
        let plain = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n[1]";
        assert_eq!(parse_response(plain).unwrap(), b"[1]");
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n[1\r\n1\r\n]\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked).unwrap(), b"[1]");
        assert!(parse_response(b"HTTP/1.0 500 Oops\r\n\r\n").is_err());
    }

    #[test]
    fn talks_to_a_unix_socket() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("ax206mon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("d.sock");
        let l = UnixListener::bind(&sock).unwrap();
        let t = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = [0u8; 512];
                let n = c.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.contains("/version") {
                    r#"{"Version":"27.1.1"}"#
                } else {
                    r#"[{"State":"running","Status":"Up","Labels":{"com.docker.compose.project":"p"}}]"#
                };
                write!(c, "HTTP/1.0 200 OK\r\n\r\n{body}").unwrap();
            }
        });
        let st = query(sock.to_str().unwrap());
        t.join().unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert!(st.up(), "{:?}", st.error);
        assert_eq!((st.version.as_str(), st.running, st.stacks.len()), ("27.1.1", 1, 1));
    }

    #[test]
    fn missing_socket_is_reported() {
        let st = query("/nonexistent/docker.sock");
        assert!(st.error.unwrap().contains("/nonexistent/docker.sock"));
    }

    #[test]
    fn permission_error_explains_the_fix() {
        let e = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            .context("connecting");
        let m = explain(&e, "/var/run/docker.sock");
        assert!(m.contains("SupplementaryGroups=docker"), "{m}");
    }
}
