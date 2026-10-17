mod ax206;
mod config;
mod frame;
mod render;
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
    /// Config file [default: /etc/ax206mon/config.toml if it exists]
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
    /// Set the backlight level 0-7 and exit
    Brightness { level: u8 },
    /// Render to a PNG file instead of the display (no hardware needed)
    Preview {
        /// Output PNG
        out: PathBuf,
        /// What to render
        #[arg(long, value_enum, default_value_t = PreviewKind::Dashboard)]
        kind: PreviewKind,
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
        cfg.rotate = r;
    }
    if let Some(b) = cli.backlight {
        cfg.backlight = b;
    }
    if let Some(s) = &cli.size {
        cfg.size = s.clone();
    }
    cfg.validate()?;
    let rot = Rotation::from_degrees(cfg.rotate).expect("validated");

    let open_opts = |backlight: Option<u8>| OpenOptions {
        size: cfg.size(),
        backlight,
        chunk: cfg.chunk_size,
    };

    match cli.command {
        Command::Preview { out, kind } => {
            let (pw, ph) = cfg.size().unwrap_or((480, 320));
            let (w, h) = rot.canvas_size(pw.into(), ph.into());
            let frame = match kind {
                PreviewKind::Dashboard => {
                    let mut c = stats::Collector::new(stats_opts(&cfg));
                    std::thread::sleep(Duration::from_millis(500)); // get real CPU/net deltas
                    render::dashboard(w, h, &c.snapshot(), &render::Clock::now())
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
        Command::Info => {
            let d = Display::open(&open_opts(None))?;
            println!("panel: {}x{}", d.width(), d.height());
            let (w, h) = rot.canvas_size(d.width().into(), d.height().into());
            println!("canvas with rotate={}: {w}x{h}", cfg.rotate);
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
            let mut d = Display::open(&open_opts(Some(cfg.backlight)))?;
            let f = canvas_for(&d, rot, render::test_pattern);
            show(&mut d, &f, rot)
        }
        Command::Image { path } => {
            let img = image::open(&path).with_context(|| format!("opening {}", path.display()))?;
            let mut d = Display::open(&open_opts(Some(cfg.backlight)))?;
            let f = canvas_for(&d, rot, |w, h| render::fit_image(&img, w, h));
            show(&mut d, &f, rot)
        }
        Command::Run => run(&cfg, rot, open_opts(Some(cfg.backlight))),
    }
}

fn stats_opts(cfg: &Config) -> stats::Options {
    stats::Options {
        disk_path: cfg.disk_path.clone(),
        net_iface: Some(cfg.net_interface.clone()).filter(|s| !s.is_empty()),
        temp_file: Some(PathBuf::from(&cfg.temp_file)).filter(|p| !p.as_os_str().is_empty()),
    }
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
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).context("installing signal handler")?;
    }

    let interval = Duration::from_secs_f64(cfg.interval);
    let mut collector = stats::Collector::new(stats_opts(cfg));
    let mut reported_missing = false;

    while !stop.load(Ordering::Relaxed) {
        let mut d = match Display::open(&opts) {
            Ok(d) => {
                reported_missing = false;
                d
            }
            Err(e) => {
                if !reported_missing {
                    warn!("{e}; waiting for the display");
                    reported_missing = true;
                }
                sleep_unless_stopped(Duration::from_secs(3), &stop);
                continue;
            }
        };

        let (w, h) = rot.canvas_size(d.width().into(), d.height().into());
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            let frame = render::dashboard(w, h, &collector.snapshot(), &render::Clock::now());
            if let Err(e) = show(&mut d, &frame, rot) {
                warn!("lost the display: {e:#}");
                break;
            }
            log::debug!("frame sent in {:?}", started.elapsed());
            sleep_unless_stopped(interval.saturating_sub(started.elapsed()), &stop);
        }

        if stop.load(Ordering::Relaxed) && cfg.backlight_off_on_exit {
            let _ = d.set_backlight(0);
        }
    }
    info!("stopped");
    Ok(())
}

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(100);
    let end = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= end {
            break;
        }
        std::thread::sleep(step.min(end - now));
    }
}
