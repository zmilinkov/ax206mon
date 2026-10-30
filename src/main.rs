mod ax206;
mod burnin;
mod config;
mod docker;
mod font;
mod frame;
mod gpu;
mod netif;
mod render;
mod schedule;
mod services;
mod stats;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use log::{error, info, warn};

use crate::ax206::{Display, OpenOptions};
use crate::config::Config;
use crate::frame::{Frame, Rotation};

#[derive(Parser)]
#[command(
    version,
    about = "Status dashboard for AX206-protocol USB mini displays (1908:0102)"
)]
struct Cli {
    /// Config file [default: /etc/ax206mon/ax206mon.conf if it exists]
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Override rotation (0, 90, 180, 270)
    #[arg(long, global = true)]
    rotate: Option<u16>,

    /// Override backlight level (0-7) set at start-up
    #[arg(long, global = true)]
    backlight: Option<u8>,

    /// Override panel size, e.g. 480x320 (skips the size query)
    #[arg(long, global = true)]
    size: Option<String>,

    /// More logging (-v debug incl. raw USB headers, -vv trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the live dashboard (what the systemd service does)
    Run,
    /// Print the panel size reported by the display
    Info,
    /// Show colour bars and an orientation marker
    Test,
    /// Show an image file (PNG or JPEG), scaled to fit
    Image { path: PathBuf },
    /// Print the detected GPUs and their engine loads over one second
    Gpu,
    /// Set the backlight level 0-7 and exit
    Brightness { level: u8 },
    /// Render to a PNG file instead of the display (no hardware needed)
    Preview {
        /// Output PNG
        out: PathBuf,
        /// What to render
        #[arg(long, value_enum, default_value_t = PreviewKind::Dashboard)]
        kind: PreviewKind,
        /// Which page to render (1-based)
        #[arg(long, default_value_t = 1)]
        page: usize,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum PreviewKind {
    Dashboard,
    Test,
}

fn main() {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    if let Err(e) = real_main(cli) {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let mut b = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level));
    // Under systemd the journal adds timestamps itself.
    if std::env::var_os("JOURNAL_STREAM").is_some() {
        b.format_timestamp(None);
    }
    b.init();
}

fn real_main(cli: Cli) -> Result<()> {
    let mut cfg = Config::load(cli.config.as_deref())?;
    if let Some(r) = cli.rotate {
        cfg.global.rotate = r;
    }
    if let Some(b) = cli.backlight {
        cfg.global.backlight = b;
    }
    if let Some(s) = &cli.size {
        cfg.global.size = s.clone();
    }
    cfg.validate()?;
    font::set_scale(cfg.global.font_scale);
    let rot = Rotation::from_degrees(cfg.global.rotate).expect("validated");

    let open_opts = |backlight: Option<u8>| OpenOptions {
        size: cfg.size(),
        backlight,
        chunk: cfg.global.chunk_size,
    };

    match cli.command {
        Command::Preview { out, kind, page } => {
            if page == 0 || page > cfg.pages.len() {
                bail!("page must be 1-{}", cfg.pages.len());
            }
            let (pw, ph) = cfg.size().unwrap_or((480, 320));
            let (w, h) = rot.canvas_size(pw.into(), ph.into());
            let frame = match kind {
                PreviewKind::Dashboard => {
                    let mut c = stats::Collector::new(stats_opts(&cfg));
                    let needs = needs_for(&cfg, page - 1);
                    c.snapshot(&needs); // prime GPU counters
                    std::thread::sleep(Duration::from_millis(500)); // get real CPU/net deltas
                    let clock = render::Clock::now();
                    let info = render::PageInfo {
                        page: &cfg.pages[page - 1],
                        index: page - 1,
                        count: cfg.pages.len(),
                        variant: variant_for(&cfg, page - 1, &clock),
                    };
                    render::render_page(w, h, &info, &cfg.global, &c.snapshot(&needs), &clock)
                }
                PreviewKind::Test => render::test_pattern(w, h),
            };
            frame
                .to_rgb_image()
                .save(&out)
                .with_context(|| format!("writing {}", out.display()))?;
            info!(
                "wrote {}x{} preview to {}",
                frame.width(),
                frame.height(),
                out.display()
            );
            Ok(())
        }
        Command::Gpu => {
            let mut probes = gpu::discover();
            if probes.is_empty() {
                println!("no supported GPU found (Radeon: amdgpu, Intel: i915)");
            }
            for p in &mut probes {
                p.report(); // prime the delta counters
            }
            std::thread::sleep(Duration::from_secs(1));
            for p in &mut probes {
                for line in p.report() {
                    println!("{line}");
                }
            }
            Ok(())
        }
        Command::Info => {
            let d = Display::open(&open_opts(None))?;
            println!("panel: {}x{}", d.width(), d.height());
            let (w, h) = rot.canvas_size(d.width().into(), d.height().into());
            println!("canvas with rotate={}: {w}x{h}", cfg.global.rotate);
            Ok(())
        }
        Command::Brightness { level } => {
            if level > 7 {
                bail!("level must be 0-7");
            }
            Display::open(&open_opts(Some(level)))?;
            Ok(())
        }
        Command::Test => {
            let mut d = Display::open(&open_opts(Some(cfg.global.backlight)))?;
            let f = canvas_for(&d, rot, render::test_pattern);
            show(&mut d, &f, rot)
        }
        Command::Image { path } => {
            let img = image::open(&path).with_context(|| format!("opening {}", path.display()))?;
            let mut d = Display::open(&open_opts(Some(cfg.global.backlight)))?;
            let f = canvas_for(&d, rot, |w, h| render::fit_image(&img, w, h));
            show(&mut d, &f, rot)
        }
        Command::Run => run(&cfg, rot, open_opts(Some(cfg.global.backlight))),
    }
}

fn stats_opts(cfg: &Config) -> stats::Options {
    let g = &cfg.global;
    stats::Options {
        docker_socket: g.docker_socket.clone(),
        net_iface: Some(g.net_interface.clone()).filter(|s| !s.is_empty()),
        temp_file: Some(PathBuf::from(&g.temp_file)).filter(|p| !p.as_os_str().is_empty()),
        services_cache: Duration::from_secs_f64(g.services_cache_seconds),
        docker_cache: Duration::from_secs_f64(g.docker_cache_seconds),
    }
}

/// Data sources needed while page `idx` is showing. Filesystems and GPU
/// counters are collected for every page (GPU load is a delta that must not
/// span a page's absence); the docker API, systemctl and interface list are only queried on their own pages.
fn needs_for(cfg: &Config, idx: usize) -> stats::Needs {
    use config::Kind;
    let mut n = stats::Needs::default();
    for p in &cfg.pages {
        for w in &p.widgets {
            match w.kind {
                Kind::Disk => {
                    let path = w.path.clone().unwrap_or_else(|| cfg.global.disk_path.clone());
                    if !n.disk_paths.contains(&path) {
                        n.disk_paths.push(path);
                    }
                }
                Kind::Gpu => n.gpu = true,
                _ => {}
            }
        }
    }
    // The busy schedule watches GPU load even when no page shows it.
    n.gpu |= cfg.busy.enabled;
    let on_page = |f: fn(Kind) -> bool| cfg.pages[idx].widgets.iter().any(|w| f(w.kind));
    n.docker = on_page(|k| matches!(k, Kind::DockerStatus | Kind::DockerStacks));
    n.services = on_page(|k| k == Kind::Services);
    n.interfaces = on_page(|k| k == Kind::Network);
    n
}

fn canvas_for(d: &Display, rot: Rotation, draw: impl FnOnce(u32, u32) -> Frame) -> Frame {
    let (w, h) = rot.canvas_size(d.width().into(), d.height().into());
    draw(w, h)
}

fn show(d: &mut Display, f: &Frame, rot: Rotation) -> Result<()> {
    let data = f.to_panel_rgb565(rot, d.width().into(), d.height().into());
    d.blit_full(&data)?;
    Ok(())
}

/// Service loop: keep the dashboard on screen, reopening the display
/// whenever it is unplugged or not there yet.
fn run(cfg: &Config, rot: Rotation, opts: OpenOptions) -> Result<()> {
    let stop = Stop::install()?;

    let interval = Duration::from_secs_f64(cfg.global.refresh_interval);
    let mut collector = stats::Collector::new(stats_opts(cfg));
    let mut reported_missing = false;
    let mut schedule = schedule::Schedule::new(Instant::now());
    let mut detector = schedule::BusyDetector::new();
    let mut was_busy = false;
    let b = &cfg.busy;
    info!(
        "busy schedule {}: threshold {:.0}%, page 1 for {:.0}s then others for {:.0}s, \
         average {:.0}s, hold {:.0}s",
        if b.enabled { "enabled" } else { "disabled" },
        b.load_threshold,
        b.main_page_seconds,
        b.other_page_seconds,
        b.average_seconds,
        b.hold_seconds
    );
    let refresh_every = (cfg.burnin.idle_refresh_hours > 0.0)
        .then(|| Duration::from_secs_f64(cfg.burnin.idle_refresh_hours * 3600.0));
    let mut last_refresh = Instant::now();

    while !stop.is_set() {
        let mut level = backlight_for(cfg, render::Clock::now().minute_of_day);
        let open = OpenOptions {
            backlight: Some(level),
            ..opts.clone()
        };
        let mut d = match Display::open(&open) {
            Ok(d) => {
                reported_missing = false;
                d
            }
            Err(e) => {
                if !reported_missing {
                    warn!("{e}; waiting for the display");
                    reported_missing = true;
                }
                stop.sleep(Duration::from_secs(3));
                continue;
            }
        };

        let (w, h) = rot.canvas_size(d.width().into(), d.height().into());
        // Last frame sent in the panel's format; an identical frame is not
        // sent again (it would only repeat a ~1.2 s USB transfer). Reset on
        // every (re)open, so a replugged display always gets a full frame.
        let mut sent: Vec<u8> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        while !stop.is_set() {
            let started = Instant::now();
            let clock = render::Clock::now();

            // Night schedule: dim, or switch off and stop redrawing.
            let want = backlight_for(cfg, clock.minute_of_day);
            if want != level {
                info!("backlight {level} -> {want}");
                if let Err(e) = d.set_backlight(want) {
                    warn!("lost the display: {e}");
                    break;
                }
                level = want;
            }

            // Periodic full-screen colour cycle against image retention.
            if refresh_every.is_some_and(|every| last_refresh.elapsed() >= every) {
                info!("running idle refresh cycle");
                if let Err(e) = refresh_cycle(&mut d, rot, w, h, cfg.burnin.idle_refresh_seconds, &stop) {
                    warn!("lost the display: {e:#}");
                    break;
                }
                last_refresh = Instant::now();
                sent.clear(); // the screen no longer shows the last page
                continue;
            }

            if night_dark(cfg, clock.minute_of_day) {
                stop.sleep(Duration::from_secs(10));
                continue;
            }

            // The page is chosen from the samples taken so far (one refresh behind).
            let busy = detector.is_busy(started, cfg);
            if busy != was_busy {
                let (cpu, gpu) = detector.smoothed();
                info!(
                    "busy schedule {} (smoothed load cpu {cpu:.0}%, gpu {gpu:.0}%, threshold {:.0}%)",
                    if busy { "on" } else { "off" },
                    cfg.busy.load_threshold
                );
                was_busy = busy;
            }
            let page = schedule.update(started, busy, cfg);
            let info = render::PageInfo {
                page: &cfg.pages[page],
                index: page,
                count: cfg.pages.len(),
                variant: variant_for(cfg, page, &clock),
            };
            let snap = collector.snapshot(&needs_for(cfg, page));
            // CPU as shown on the bar (processing + I/O wait), busiest GPU.
            let gpu = snap.gpus.iter().filter_map(|g| g.load).fold(0.0, f32::max);
            detector.sample(Instant::now(), snap.cpu.total(), gpu, cfg);
            log::debug!("load sample: cpu {:.0}% gpu {gpu:.0}%", snap.cpu.total());
            let frame = render::render_page(w, h, &info, &cfg.global, &snap, &clock);
            frame.to_panel_rgb565_into(rot, d.width().into(), d.height().into(), &mut buf);
            if buf == sent {
                log::debug!("page {} unchanged, not sent", page + 1);
            } else {
                if let Err(e) = d.blit_full(&buf) {
                    warn!("lost the display: {e:#}");
                    break;
                }
                std::mem::swap(&mut buf, &mut sent);
                log::debug!("page {} sent in {:?}", page + 1, started.elapsed());
            }
            stop.sleep(interval.saturating_sub(started.elapsed()));
        }

        if stop.is_set() && cfg.global.backlight_off_on_exit {
            let _ = d.set_backlight(0);
        }
    }
    info!("stopped");
    Ok(())
}

/// Backlight level wanted at `minute_of_day`: the night level inside the
/// configured night window, otherwise the normal one.
fn backlight_for(cfg: &Config, minute_of_day: u32) -> u8 {
    match cfg.burnin.night_window() {
        Some((s, e)) if burnin::in_window(s, e, minute_of_day) => cfg.burnin.night_backlight,
        _ => cfg.global.backlight,
    }
}

/// In the night window with the backlight off: nothing visible to draw.
fn night_dark(cfg: &Config, minute_of_day: u32) -> bool {
    cfg.burnin.night_backlight == 0
        && cfg
            .burnin
            .night_window()
            .is_some_and(|(s, e)| burnin::in_window(s, e, minute_of_day))
}

fn variant_for(cfg: &Config, page: usize, clock: &render::Clock) -> usize {
    burnin::header_variant(
        cfg.burnin.header_variation,
        page,
        clock.epoch_minutes,
        cfg.burnin.header_variation_minutes,
    )
}

/// Flash black, white and the primaries full-screen for `secs` seconds.
fn refresh_cycle(
    d: &mut Display,
    rot: Rotation,
    w: u32,
    h: u32,
    secs: f64,
    stop: &Stop,
) -> Result<()> {
    use embedded_graphics::pixelcolor::Rgb888;
    use embedded_graphics::prelude::RgbColor;
    let colors = [Rgb888::BLACK, Rgb888::WHITE, Rgb888::RED, Rgb888::GREEN, Rgb888::BLUE];
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut i = 0;
    while Instant::now() < end && !stop.is_set() {
        show(d, &Frame::new(w, h, colors[i % colors.len()]), rot)?;
        i += 1;
    }
    Ok(())
}

/// Stop request from SIGTERM/SIGINT. Besides setting a flag, the signal
/// handler writes to a socket pair, so sleeps wait on that socket and wake
/// at once on a signal instead of polling the flag.
struct Stop {
    flag: Arc<AtomicBool>,
    wake: std::os::unix::net::UnixStream,
}

impl Stop {
    fn install() -> Result<Self> {
        let flag = Arc::new(AtomicBool::new(false));
        let (wake, notify) = std::os::unix::net::UnixStream::pair().context("creating signal pipe")?;
        for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
            signal_hook::flag::register(sig, Arc::clone(&flag)).context("installing signal handler")?;
            signal_hook::low_level::pipe::register(sig, notify.try_clone()?)
                .context("installing signal handler")?;
        }
        Ok(Stop { flag, wake })
    }

    fn is_set(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Sleep for `total`, returning early when a stop signal arrives.
    fn sleep(&self, total: Duration) {
        use std::os::fd::AsRawFd;
        let end = Instant::now() + total;
        while !self.is_set() {
            let now = Instant::now();
            if now >= end {
                break;
            }
            // Round up so a sub-millisecond remainder doesn't spin.
            let ms = (end - now).as_micros().div_ceil(1000).min(i32::MAX as u128) as i32;
            let mut pfd = libc::pollfd {
                fd: self.wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // EINTR or a wake-up just loops and re-checks the flag.
            unsafe { libc::poll(&mut pfd, 1, ms) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_sleep_wakes_on_signal_and_times_out_otherwise() {
        let stop = Stop::install().unwrap();
        let t = Instant::now();
        stop.sleep(Duration::from_millis(50));
        assert!(t.elapsed() >= Duration::from_millis(50) && !stop.is_set());

        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
        });
        let t = Instant::now();
        stop.sleep(Duration::from_secs(10));
        assert!(stop.is_set());
        assert!(t.elapsed() < Duration::from_secs(2), "woke after {:?}", t.elapsed());
    }
}
