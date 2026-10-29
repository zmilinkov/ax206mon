//! Drawing: dashboard pages, a test pattern and image fitting.
//! Layout adapts to landscape (480x320) and portrait (320x480) canvases.

use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use profont::{PROFONT_12_POINT, PROFONT_14_POINT, PROFONT_18_POINT, PROFONT_24_POINT};

use crate::config::{Global, Kind, Page};
use crate::docker::DockerStatus;
use crate::frame::Frame;
use crate::netif::{self, Iface};
use crate::services::{self, Filter, Unit};
use crate::stats::Snapshot;

pub mod palette {
    use embedded_graphics::pixelcolor::Rgb888;
    pub const BG: Rgb888 = Rgb888::new(0x0e, 0x11, 0x17);
    pub const PANEL: Rgb888 = Rgb888::new(0x1c, 0x21, 0x2b);
    pub const TEXT: Rgb888 = Rgb888::new(0xe8, 0xea, 0xee);
    pub const MUTED: Rgb888 = Rgb888::new(0x8a, 0x93, 0xa3);
    pub const CPU: Rgb888 = Rgb888::new(0x4c, 0x9a, 0xff);
    pub const RAM: Rgb888 = Rgb888::new(0x3c, 0xc8, 0x8c);
    pub const DISK: Rgb888 = Rgb888::new(0xb4, 0x8c, 0xff);
    pub const TEMP: Rgb888 = Rgb888::new(0xff, 0xa0, 0x3c);
    pub const WARN: Rgb888 = Rgb888::new(0xff, 0x55, 0x55);
    pub const DOCKER: Rgb888 = Rgb888::new(0x4f, 0xc3, 0xf7);
}
use palette::*;

/// Wall-clock time to show; passed in so rendering is testable.
#[derive(Default)]
pub struct Clock {
    pub time: String,
    pub date: String,
    /// Minutes since local midnight (for the night schedule).
    pub minute_of_day: u32,
    /// Minutes since the Unix epoch (for time-based header variation).
    pub epoch_minutes: u64,
}

impl Clock {
    pub fn now() -> Self {
        // Local time via libc so we don't need a date/time crate.
        let t = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        Clock {
            minute_of_day: (tm.tm_hour * 60 + tm.tm_min).clamp(0, 1439) as u32,
            epoch_minutes: t.max(0) as u64 / 60,
            time: format!("{:02}:{:02}", tm.tm_hour, tm.tm_min),
            date: format!(
                "{} {} {} {}",
                DAYS[tm.tm_wday.clamp(0, 6) as usize],
                tm.tm_mday,
                MONTHS[tm.tm_mon.clamp(0, 11) as usize],
                tm.tm_year + 1900
            ),
        }
    }
}

fn text(f: &mut Frame, s: &str, pos: Point, font: &MonoFont, color: Rgb888, align: Alignment) {
    let style = MonoTextStyle::new(font, color);
    let ts = TextStyleBuilder::new()
        .alignment(align)
        .baseline(Baseline::Top)
        .build();
    let _ = Text::with_text_style(s, pos, style, ts).draw(f);
}

/// Draw text enlarged by an integer factor (for the big clock).
fn text_scaled(
    f: &mut Frame,
    s: &str,
    center_x: i32,
    top: i32,
    font: &MonoFont,
    color: Rgb888,
    scale: u32,
) {
    let cw = font.character_size.width + font.character_spacing;
    let w = cw * s.chars().count() as u32;
    let h = font.character_size.height;
    let key = Rgb888::new(1, 2, 3); // transparency key
    let mut tmp = Frame::new(w.max(1), h, key);
    text(&mut tmp, s, Point::zero(), font, color, Alignment::Left);
    f.blit_scaled(&tmp, center_x - (w * scale) as i32 / 2, top, scale, key);
}

/// One segment of a (possibly stacked) bar.
#[derive(Clone, Copy)]
struct Seg {
    frac: f32,
    color: Rgb888,
}

/// A labelled bar row. Rows after the first in a widget are drawn smaller.
struct Bar {
    label: String,
    value: String,
    segs: Vec<Seg>,
    /// Colour key shown under the bar in the stacked (portrait) style.
    legend: Vec<(String, Rgb888)>,
    /// Label colour when not hot.
    color: Rgb888,
    /// 90 % or more of the bar is filled.
    hot: bool,
}

impl Bar {
    fn simple(label: impl Into<String>, value: String, frac: f32, color: Rgb888) -> Self {
        let hot = frac >= 0.9;
        Bar {
            label: label.into(),
            value,
            segs: vec![Seg {
                frac,
                color: if hot { WARN } else { color },
            }],
            legend: Vec::new(),
            color,
            hot,
        }
    }
}

enum Item<'a> {
    Bars(Vec<Bar>),
    DockerStatus(&'a DockerStatus),
    DockerStacks(&'a DockerStatus),
    Services {
        title: String,
        error: Option<&'a str>,
        units: Vec<Unit>,
    },
    Network {
        title: String,
        ifaces: Vec<Iface>,
        mac: bool,
    },
    Status,
}

/// How a bar row is laid out.
#[derive(Clone, Copy)]
enum Style {
    /// Label and value above the bar, legend below (portrait).
    Stacked,
    /// Label, bar and value on one line.
    Line {
        label: &'static MonoFont<'static>,
        value: &'static MonoFont<'static>,
        bar_h: u32,
    },
}

const LINE_MAIN: Style = Style::Line {
    label: &PROFONT_18_POINT,
    value: &PROFONT_14_POINT,
    bar_h: 12,
};
const LINE_SUB: Style = Style::Line {
    label: &PROFONT_14_POINT,
    value: &PROFONT_12_POINT,
    bar_h: 8,
};
const BAR_H: u32 = 12;
const STATUS_H: i32 = 26;

fn style_for(portrait: bool, row: usize) -> Style {
    match (row, portrait) {
        (0, true) => Style::Stacked,
        (0, false) => LINE_MAIN,
        _ => LINE_SUB,
    }
}

fn char_w(font: &MonoFont) -> i32 {
    (font.character_size.width + font.character_spacing) as i32
}

fn char_h(font: &MonoFont) -> i32 {
    font.character_size.height as i32
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('~');
        t
    }
}

/// Normal and tight vertical padding below a bar row; tight is used when
/// the page would otherwise overflow (e.g. a GPU widget with many rows).
const GAP: i32 = 8;
const GAP_TIGHT: i32 = 3;

fn bar_height(b: &Bar, style: Style, gap: i32) -> i32 {
    match style {
        Style::Stacked => {
            let legend = if b.legend.is_empty() {
                0
            } else {
                3 + char_h(&PROFONT_12_POINT)
            };
            char_h(&PROFONT_18_POINT) + 4 + BAR_H as i32 + legend + gap
        }
        Style::Line { label, bar_h, .. } => char_h(label).max(bar_h as i32) + gap,
    }
}

fn bars_height(bars: &[Bar], portrait: bool, gap: i32) -> i32 {
    bars.iter()
        .enumerate()
        .map(|(i, b)| bar_height(b, style_for(portrait, i), gap))
        .sum()
}

fn bar(f: &mut Frame, x: i32, y: i32, w: u32, h: u32, segs: &[Seg]) {
    let r = Size::new(h / 2, h / 2);
    let _ =
        RoundedRectangle::with_equal_corners(Rectangle::new(Point::new(x, y), Size::new(w, h)), r)
            .into_styled(PrimitiveStyle::with_fill(PANEL))
            .draw(f);
    // Paint cumulative fills from the longest to the shortest, so each
    // segment's colour ends where the next one begins.
    let mut cum = 0.0f32;
    let ends: Vec<(f32, Rgb888)> = segs
        .iter()
        .map(|s| {
            cum = (cum + s.frac.max(0.0)).min(1.0);
            (cum, s.color)
        })
        .collect();
    for &(end, color) in ends.iter().rev() {
        let fw = ((w as f32) * end).round() as u32;
        if fw >= h {
            let _ = RoundedRectangle::with_equal_corners(
                Rectangle::new(Point::new(x, y), Size::new(fw, h)),
                r,
            )
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(f);
        } else if fw > 0 {
            let _ = Rectangle::new(Point::new(x, y), Size::new(fw, h))
                .into_styled(PrimitiveStyle::with_fill(color))
                .draw(f);
        }
    }
}

/// Draw one bar row in `x..x+w` starting at `y`; returns its height.
fn draw_bar_row(f: &mut Frame, x: i32, y: i32, w: i32, b: &Bar, style: Style, gap: i32) -> i32 {
    let label_color = if b.hot { WARN } else { b.color };
    match style {
        Style::Stacked => {
            let font = &PROFONT_18_POINT;
            text(f, &b.label, Point::new(x, y), font, label_color, Alignment::Left);
            text(f, &b.value, Point::new(x + w, y), font, TEXT, Alignment::Right);
            let bar_y = y + char_h(font) + 4;
            bar(f, x, bar_y, w as u32, BAR_H, &b.segs);
            let mut lx = x;
            let lfont = &PROFONT_12_POINT;
            let ly = bar_y + BAR_H as i32 + 3;
            for (name, color) in &b.legend {
                let _ = Rectangle::new(Point::new(lx, ly + 4), Size::new(6, 6))
                    .into_styled(PrimitiveStyle::with_fill(*color))
                    .draw(f);
                text(f, name, Point::new(lx + 9, ly), lfont, MUTED, Alignment::Left);
                lx += 9 + char_w(lfont) * name.chars().count() as i32 + 10;
            }
        }
        Style::Line { label, value, bar_h } => {
            let label_w = char_w(label) * 5;
            let value_w = char_w(value) * b.value.chars().count() as i32;
            let h = char_h(label).max(bar_h as i32);
            text(f, &b.label, Point::new(x, y + (h - char_h(label)) / 2), label, label_color, Alignment::Left);
            text(f, &b.value, Point::new(x + w, y + (h - char_h(value)) / 2), value, TEXT, Alignment::Right);
            let bx = x + label_w + 6;
            let bw = (w - label_w - 6 - value_w - 8).max(h);
            bar(f, bx, y + (h - bar_h as i32) / 2, bw as u32, bar_h, &b.segs);
        }
    }
    bar_height(b, style, gap)
}

pub fn human_bytes(b: u64) -> String {
    const U: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 || i == 0 {
        format!("{v:.0}{}", U[i])
    } else {
        format!("{v:.1}{}", U[i])
    }
}

fn human_rate(bps: f64) -> String {
    format!("{}/s", human_bytes(bps.max(0.0) as u64))
}

fn human_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else {
        format!("{h}h {m:02}m")
    }
}

fn frac(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used as f32 / total as f32
    }
}

// Blue shades for CPU, green for RAM; lighter = less "real" load.
const CPU_IOWAIT: Rgb888 = Rgb888::new(0xa9, 0xd2, 0xff);
const RAM_SYSTEM: Rgb888 = Rgb888::new(0x1f, 0x8a, 0x5e);
const RAM_CACHE: Rgb888 = Rgb888::new(0xa6, 0xe8, 0xcf);
const GPU: Rgb888 = Rgb888::new(0xff, 0x6f, 0xb5);

fn build_items<'a>(page: &Page, global: &Global, s: &'a Snapshot) -> Vec<Item<'a>> {
    let mut items = Vec::new();
    for w in &page.widgets {
        let label = |default: &str| w.label.clone().unwrap_or_else(|| default.to_string());
        match w.kind {
            Kind::Cpu => {
                let c = s.cpu;
                let mut b = Bar::simple(label("CPU"), format!("{:.0}%", c.total()), c.total() / 100.0, CPU);
                b.hot = c.total() >= 90.0;
                b.segs = vec![
                    Seg { frac: c.busy / 100.0, color: CPU },
                    Seg { frac: c.iowait / 100.0, color: CPU_IOWAIT },
                ];
                b.legend = vec![
                    (format!("proc {:.0}%", c.busy), CPU),
                    (format!("iowait {:.0}%", c.iowait), CPU_IOWAIT),
                ];
                items.push(Item::Bars(vec![b]));
            }
            Kind::Ram => {
                let m = s.mem;
                let t = m.total.max(1) as f32;
                let used_frac = frac(m.used(), m.total);
                let mut b = Bar::simple(
                    label("RAM"),
                    format!("{} / {}", human_bytes(m.used()), human_bytes(m.total)),
                    used_frac,
                    RAM,
                );
                b.hot = used_frac >= 0.9;
                b.segs = vec![
                    Seg { frac: m.apps as f32 / t, color: RAM },
                    Seg { frac: m.kernel as f32 / t, color: RAM_SYSTEM },
                    Seg { frac: m.cache as f32 / t, color: RAM_CACHE },
                ];
                b.legend = vec![
                    (format!("apps {}", human_bytes(m.apps)), RAM),
                    (format!("sys {}", human_bytes(m.kernel)), RAM_SYSTEM),
                    (format!("cache {}", human_bytes(m.cache)), RAM_CACHE),
                ];
                items.push(Item::Bars(vec![b]));
            }
            Kind::Disk => {
                let path = w.path.as_deref().unwrap_or(&global.disk_path);
                let (used, total) = s.disks.get(path).copied().unwrap_or((0, 0));
                items.push(Item::Bars(vec![Bar::simple(
                    label("DISK"),
                    format!("{} / {}", human_bytes(used), human_bytes(total)),
                    frac(used, total),
                    DISK,
                )]));
            }
            Kind::Temp => {
                if let Some(t) = s.temp_c {
                    items.push(Item::Bars(vec![Bar::simple(
                        label("TEMP"),
                        format!("{t:.0}\u{b0}C"),
                        t / 100.0,
                        TEMP,
                    )]));
                }
            }
            Kind::Gpu => {
                let g = match &w.card {
                    Some(c) => s.gpus.iter().find(|g| &g.card == c),
                    None => s.gpus.first(),
                };
                if let Some(g) = g {
                    let pct = |v: Option<f32>| v.map_or("n/a".to_string(), |v| format!("{v:.0}%"));
                    let mut main = Bar::simple(label("GPU"), pct(g.load), g.load.unwrap_or(0.0) / 100.0, GPU);
                    if g.load.is_none() {
                        main.hot = false;
                    }
                    let mut rows = vec![main];
                    for r in &g.rows {
                        rows.push(Bar::simple(r.label, pct(r.pct), r.pct.unwrap_or(0.0) / 100.0, GPU));
                    }
                    items.push(Item::Bars(rows));
                }
            }
            Kind::Services => {
                if let Some(l) = &s.services {
                    let filter = w.filter.as_deref().and_then(Filter::parse).unwrap_or(Filter::Running);
                    items.push(Item::Services {
                        title: label("SERVICES"),
                        error: l.error.as_deref(),
                        units: services::select(&l.units, w.units.as_deref(), filter),
                    });
                }
            }
            Kind::Network => {
                if let Some(i) = &s.interfaces {
                    let sel = netif::Select {
                        names: w.interfaces.as_deref(),
                        ipv4: w.ipv4.unwrap_or(true),
                        ipv6: w.ipv6.unwrap_or(true),
                        loopback: w.loopback.unwrap_or(false),
                    };
                    items.push(Item::Network {
                        title: label("NETWORK"),
                        ifaces: netif::select(i, &sel),
                        mac: w.mac.unwrap_or(false),
                    });
                }
            }
            Kind::Status => items.push(Item::Status),
            Kind::DockerStatus | Kind::DockerStacks => {
                if let Some(d) = &s.docker {
                    items.push(if w.kind == Kind::DockerStatus {
                        Item::DockerStatus(d)
                    } else {
                        Item::DockerStacks(d)
                    });
                }
            }
        }
    }
    items
}

pub struct PageInfo<'a> {
    pub page: &'a Page,
    pub index: usize,
    pub count: usize,
    /// Header look, 0..burnin::VARIANTS (see burnin::header_variant).
    pub variant: usize,
}

/// Largest vertical shift a header variant applies; always reserved so the
/// widgets below never move.
const HEADER_DY_MAX: i32 = 6;

/// Clock colours cycled by header variants; all stay light for contrast.
const CLOCK_COLORS: [Rgb888; crate::burnin::VARIANTS] = [
    TEXT,
    Rgb888::new(0xbc, 0xd8, 0xff),
    Rgb888::new(0xff, 0xe3, 0xb8),
    Rgb888::new(0xc6, 0xf3, 0xdd),
];

/// (dx, dy, colour) for header variant `v`; dx is limited by the free
/// horizontal space `slack` so nothing leaves the screen.
fn header_look(v: usize, portrait: bool, slack: i32) -> (i32, i32, Rgb888) {
    let v = v % crate::burnin::VARIANTS;
    let unit = slack.clamp(0, 12);
    let dx = if portrait {
        [0, -unit, 0, unit][v]
    } else {
        [0, unit / 2, unit, unit / 2][v]
    };
    (dx, [0, 2, 4, 6][v], CLOCK_COLORS[v])
}

/// Draw the clock/date header; returns its height.
fn draw_header(f: &mut Frame, info: &PageInfo, clock: &Clock, portrait: bool) -> i32 {
    let w = f.width() as i32;
    let margin = 16;
    let font = &PROFONT_24_POINT;
    let cw = char_w(font) * clock.time.chars().count() as i32;
    let avail = if portrait { w - 2 * margin } else { w / 2 - margin };
    let scale = [3u32, 2, 1]
        .into_iter()
        .find(|&sc| cw * sc as i32 <= avail)
        .unwrap_or(1);
    let clock_h = char_h(font) * scale as i32;
    let indicator = if info.count > 1 {
        format!("{}/{}", info.index + 1, info.count)
    } else {
        String::new()
    };
    let name = truncate(&info.page.name, 14);
    let slack = avail - cw * scale as i32;
    let (dx, dy, clock_color) = header_look(info.variant, portrait, slack / 2);
    if portrait {
        text_scaled(f, &clock.time, w / 2 + dx, 8 + dy, font, clock_color, scale);
        let sub_y = 8 + dy + clock_h + 2;
        text(f, &clock.date, Point::new(w / 2 + dx, sub_y), &PROFONT_14_POINT, MUTED, Alignment::Center);
        if info.count > 1 {
            let small = &PROFONT_12_POINT;
            let y = sub_y + (char_h(&PROFONT_14_POINT) - char_h(small)) / 2;
            text(f, &name, Point::new(margin, y), small, MUTED, Alignment::Left);
            text(f, &indicator, Point::new(w - margin, y), small, MUTED, Alignment::Right);
        }
        8 + HEADER_DY_MAX + clock_h + 2 + char_h(&PROFONT_14_POINT) + 8
    } else {
        text_scaled(f, &clock.time, margin + dx + cw * scale as i32 / 2, 8 + dy, font, clock_color, scale);
        text(f, &clock.date, Point::new(w - margin, 12), &PROFONT_14_POINT, MUTED, Alignment::Right);
        if info.count > 1 {
            let small = &PROFONT_12_POINT;
            let y = 12 + char_h(&PROFONT_14_POINT) + 6;
            text(f, &format!("{name}  {indicator}"), Point::new(w - margin, y), small, MUTED, Alignment::Right);
        }
        8 + HEADER_DY_MAX + clock_h + 6
    }
}

fn draw_status(f: &mut Frame, y: i32, s: &Snapshot, portrait: bool) {
    let w = f.width() as i32;
    let _ = Rectangle::new(Point::new(0, y), Size::new(f.width(), STATUS_H as u32))
        .into_styled(PrimitiveStyle::with_fill(PANEL))
        .draw(f);
    let ty = y + 4;
    let left = format!("{}  up {}", s.hostname, human_uptime(s.uptime_s));
    let right = if portrait {
        format!("ld {:.2}", s.load1)
    } else {
        format!(
            "ld {:.2}  rx {} tx {}",
            s.load1,
            human_rate(s.net_rx_bps),
            human_rate(s.net_tx_bps)
        )
    };
    let font = &PROFONT_12_POINT;
    text(f, &left, Point::new(10, ty), font, MUTED, Alignment::Left);
    text(f, &right, Point::new(w - 10, ty), font, MUTED, Alignment::Right);
}

const DOCKER_STATUS_H: i32 = 22 + 4 + 2 * 19 + 10;

fn draw_docker_status(f: &mut Frame, x: i32, y: i32, w: i32, d: &DockerStatus) {
    let title = &PROFONT_18_POINT;
    let small = &PROFONT_14_POINT;
    let (state, color) = if d.up() {
        ("UP", RAM)
    } else {
        ("DOWN", WARN)
    };
    text(f, "DOCKER", Point::new(x, y), title, if d.up() { DOCKER } else { WARN }, Alignment::Left);
    text(f, state, Point::new(x + w, y), title, color, Alignment::Right);
    let y1 = y + char_h(title) + 4;
    let lh = char_h(small) + 3;
    if let Some(e) = &d.error {
        text(f, &truncate(e, (w / char_w(small)) as usize), Point::new(x, y1), small, WARN, Alignment::Left);
        return;
    }
    let v = if d.version.is_empty() { String::new() } else { format!("engine {}", d.version) };
    text(f, &format!("{} / {} containers running", d.running, d.total), Point::new(x, y1), small, TEXT, Alignment::Left);
    let mut issues = Vec::new();
    if d.unhealthy > 0 {
        issues.push(format!("{} unhealthy", d.unhealthy));
    }
    if d.restarting > 0 {
        issues.push(format!("{} restarting", d.restarting));
    }
    if d.paused > 0 {
        issues.push(format!("{} paused", d.paused));
    }
    let (line, c) = if issues.is_empty() {
        (v, MUTED)
    } else {
        (issues.join(", "), TEMP)
    };
    text(f, &truncate(&line, (w / char_w(small)) as usize), Point::new(x, y1 + lh), small, c, Alignment::Left);
}

fn unit_color(u: &Unit) -> Rgb888 {
    match (u.active.as_str(), u.sub.as_str()) {
        ("failed", _) => WARN,
        (_, "running") => RAM,
        (_, "exited") | (_, "dead") | ("inactive", _) => MUTED,
        _ => TEMP,
    }
}

/// Services that fit in the available height; the rest are dropped.
fn draw_services(f: &mut Frame, (x, y, w, h): (i32, i32, i32, i32), title: &str, error: Option<&str>, units: &[Unit]) {
    let big = &PROFONT_18_POINT;
    let small = &PROFONT_14_POINT;
    text(f, title, Point::new(x, y), big, DOCKER, Alignment::Left);
    let count = if error.is_some() { "-".to_string() } else { units.len().to_string() };
    text(f, &count, Point::new(x + w, y), big, TEXT, Alignment::Right);
    let row_h = char_h(small) + 8;
    let mut ry = y + char_h(big) + 6;
    if let Some(e) = error {
        text(f, &truncate(e, (w / char_w(small)) as usize), Point::new(x, ry), small, WARN, Alignment::Left);
        return;
    }
    if units.is_empty() {
        text(f, "no matching services", Point::new(x, ry), small, MUTED, Alignment::Left);
        return;
    }
    let fit = ((y + h - ry) / row_h).max(0) as usize;
    for u in units.iter().take(fit) {
        let color = unit_color(u);
        let _ = Rectangle::new(Point::new(x, ry + 4), Size::new(8, 8))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(f);
        let sub = truncate(&u.sub, 9);
        let max = ((w - 16) / char_w(small)) as usize - sub.chars().count() - 1;
        text(f, &truncate(&u.name, max), Point::new(x + 16, ry), small, TEXT, Alignment::Left);
        text(f, &sub, Point::new(x + w, ry), small, color, Alignment::Right);
        ry += row_h;
    }
}

/// Interfaces with their addresses; whatever doesn't fit is dropped.
fn draw_network(f: &mut Frame, (x, y, w, h): (i32, i32, i32, i32), title: &str, ifaces: &[Iface], mac: bool) {
    let big = &PROFONT_18_POINT;
    let name_font = &PROFONT_14_POINT;
    let addr_font = &PROFONT_12_POINT;
    text(f, title, Point::new(x, y), big, DOCKER, Alignment::Left);
    text(f, &ifaces.len().to_string(), Point::new(x + w, y), big, TEXT, Alignment::Right);
    let mut ry = y + char_h(big) + 6;
    let end = y + h;
    if ifaces.is_empty() {
        text(f, "no addresses", Point::new(x, ry), name_font, MUTED, Alignment::Left);
        return;
    }
    let name_h = char_h(name_font) + 4;
    let addr_h = char_h(addr_font) + 3;
    for i in ifaces {
        // Need the name row and at least one address, or stop.
        if ry + name_h + addr_h > end {
            break;
        }
        let color = if i.up { RAM } else { MUTED };
        let _ = Rectangle::new(Point::new(x, ry + 4), Size::new(8, 8))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(f);
        text(f, &truncate(&i.name, 16), Point::new(x + 16, ry), name_font, TEXT, Alignment::Left);
        text(f, if i.up { "up" } else { "down" }, Point::new(x + w, ry), name_font, color, Alignment::Right);
        ry += name_h;
        for a in &i.addrs {
            if ry + addr_h > end {
                break;
            }
            let max = ((w - 16) / char_w(addr_font)) as usize;
            text(f, &truncate(&a.to_string(), max), Point::new(x + 16, ry), addr_font, MUTED, Alignment::Left);
            ry += addr_h;
        }
        if let Some(m) = i.mac.as_ref().filter(|_| mac && ry + addr_h <= end) {
            text(f, &format!("mac {m}"), Point::new(x + 16, ry), addr_font, DOCKER, Alignment::Left);
            ry += addr_h;
        }
        ry += 5;
    }
}

fn draw_docker_stacks(f: &mut Frame, x: i32, y: i32, w: i32, h: i32, d: &DockerStatus) {
    let title = &PROFONT_18_POINT;
    let small = &PROFONT_14_POINT;
    text(f, "STACKS", Point::new(x, y), title, DOCKER, Alignment::Left);
    let count = if d.up() { format!("{}", d.stacks.len()) } else { "-".into() };
    text(f, &count, Point::new(x + w, y), title, TEXT, Alignment::Right);
    let row_h = char_h(small) + 8;
    let mut ry = y + char_h(title) + 6;
    if !d.up() || d.stacks.is_empty() {
        let msg = if d.up() { "no compose stacks running" } else { "docker unavailable" };
        text(f, msg, Point::new(x, ry), small, MUTED, Alignment::Left);
        return;
    }
    let fit = (((y + h - ry) / row_h).max(1)) as usize;
    let shown = if d.stacks.len() > fit { fit - 1 } else { fit };
    for s in d.stacks.iter().take(shown) {
        let color = if s.running == 0 || s.unhealthy > 0 {
            WARN
        } else if s.running < s.total {
            TEMP
        } else {
            RAM
        };
        let status = if s.unhealthy > 0 {
            format!("{}/{} !{}", s.running, s.total, s.unhealthy)
        } else {
            format!("{}/{} up", s.running, s.total)
        };
        let _ = Rectangle::new(Point::new(x, ry + 4), Size::new(8, 8))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(f);
        let max = ((w - 16) / char_w(small)) as usize - status.chars().count() - 1;
        text(f, &truncate(&s.name, max), Point::new(x + 16, ry), small, TEXT, Alignment::Left);
        text(f, &status, Point::new(x + w, ry), small, color, Alignment::Right);
        ry += row_h;
    }
    if shown < d.stacks.len() {
        text(f, &format!("+{} more", d.stacks.len() - shown), Point::new(x, ry), small, MUTED, Alignment::Left);
    }
}

/// Draw `info.page` on a `width` x `height` canvas. Portrait canvases get
/// stacked bars with legends; landscape ones get one-line bars and a
/// side-by-side header.
pub fn render_page(
    width: u32,
    height: u32,
    info: &PageInfo,
    global: &Global,
    s: &Snapshot,
    clock: &Clock,
) -> Frame {
    let mut f = Frame::new(width, height, BG);
    let w = width as i32;
    let h = height as i32;
    let margin = 16;
    let cw = w - 2 * margin;
    let portrait = height > width;

    let header_h = draw_header(&mut f, info, clock, portrait);

    let items = build_items(info.page, global, s);
    let status_count = items.iter().filter(|i| matches!(i, Item::Status)).count() as i32;
    let body: Vec<&Item> = items.iter().filter(|i| !matches!(i, Item::Status)).collect();

    let avail = h - header_h - status_count * STATUS_H - 4;
    let natural = |i: &Item, gap: i32| match i {
        Item::Bars(b) => bars_height(b, portrait, gap),
        Item::DockerStatus(_) => DOCKER_STATUS_H,
        _ => 0,
    };
    let is_flex = |i: &Item| matches!(i, Item::DockerStacks(_) | Item::Services { .. } | Item::Network { .. });
    let flex = body.iter().filter(|i| is_flex(i)).count() as i32;
    const MIN_FLEX: i32 = 60;
    let total = |gap: i32| -> i32 { body.iter().map(|i| natural(i, gap)).sum() };
    let gap = if total(GAP) + MIN_FLEX * flex > avail { GAP_TIGHT } else { GAP };
    let fixed = total(gap);
    let scale = if flex > 0 {
        ((avail - MIN_FLEX * flex) as f32 / fixed.max(1) as f32).min(1.0)
    } else {
        (avail as f32 / fixed.max(1) as f32).min(1.4)
    }
    .max(0.0);
    let flex_h = if flex > 0 {
        ((avail as f32 - fixed as f32 * scale) / flex as f32) as i32
    } else {
        0
    };

    let mut y = header_h;
    for item in &body {
        match item {
            Item::Bars(bars) => {
                let nat = bars_height(bars, portrait, gap);
                let slot = (nat as f32 * scale) as i32;
                // Centre the rows in their slot so spare space is spread evenly.
                let mut ry = y + (slot - nat).max(0) / 2;
                for (i, b) in bars.iter().enumerate() {
                    ry += draw_bar_row(&mut f, margin, ry, cw, b, style_for(portrait, i), gap);
                }
                y += slot.max(nat);
            }
            Item::DockerStatus(d) => {
                draw_docker_status(&mut f, margin, y, cw, d);
                y += (DOCKER_STATUS_H as f32 * scale) as i32;
            }
            Item::DockerStacks(d) => {
                draw_docker_stacks(&mut f, margin, y, cw, flex_h, d);
                y += flex_h;
            }
            Item::Services { title, error, units } => {
                draw_services(&mut f, (margin, y, cw, flex_h), title, *error, units);
                y += flex_h;
            }
            Item::Network { title, ifaces, mac } => {
                draw_network(&mut f, (margin, y, cw, flex_h), title, ifaces, *mac);
                y += flex_h;
            }
            Item::Status => {}
        }
    }

    for k in 0..status_count {
        draw_status(&mut f, h - (k + 1) * STATUS_H, s, portrait);
    }
    f
}

pub fn test_pattern(width: u32, height: u32) -> Frame {
    let mut f = Frame::new(width, height, Rgb888::BLACK);
    let colors = [
        Rgb888::RED,
        Rgb888::GREEN,
        Rgb888::BLUE,
        Rgb888::YELLOW,
        Rgb888::CYAN,
        Rgb888::MAGENTA,
        Rgb888::WHITE,
        Rgb888::BLACK,
    ];
    let bw = width as f32 / colors.len() as f32;
    for (i, c) in colors.iter().enumerate() {
        let x0 = (i as f32 * bw) as i32;
        let x1 = ((i + 1) as f32 * bw) as i32;
        let _ = Rectangle::new(Point::new(x0, 0), Size::new((x1 - x0) as u32, height))
            .into_styled(PrimitiveStyle::with_fill(*c))
            .draw(&mut f);
    }
    // Corner marker shows orientation: top-left should be the white square.
    let _ = Rectangle::new(Point::new(0, 0), Size::new(24, 24))
        .into_styled(PrimitiveStyle::with_fill(Rgb888::WHITE))
        .draw(&mut f);
    let label = format!("{width}x{height}  TOP-LEFT");
    let _ = Rectangle::new(
        Point::new(30, 2),
        Size::new(12 * label.len() as u32 + 8, 26),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb888::BLACK))
    .draw(&mut f);
    text(
        &mut f,
        &label,
        Point::new(34, 4),
        &PROFONT_18_POINT,
        Rgb888::WHITE,
        Alignment::Left,
    );
    f
}

/// Scale an image to fit the canvas (letterboxed, aspect preserved).
pub fn fit_image(img: &image::DynamicImage, width: u32, height: u32) -> Frame {
    let scaled = img
        .resize(width, height, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut f = Frame::new(width, height, Rgb888::BLACK);
    let ox = (width - scaled.width()) / 2;
    let oy = (height - scaled.height()) / 2;
    for (x, y, p) in scaled.enumerate_pixels() {
        f.set(ox + x, oy + y, Rgb888::new(p[0], p[1], p[2]));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(1536), "1.5K");
        assert_eq!(human_bytes(32 * 1024 * 1024 * 1024), "32.0G");
    }

    #[test]
    fn every_page_renders_at_every_rotation_size() {
        use crate::docker::Stack;
        use crate::stats::{CpuUsage, MemUsage};
        let s = Snapshot {
            cpu: CpuUsage { busy: 30.0, iowait: 12.0 },
            mem: MemUsage { total: 16 << 30, apps: 4 << 30, kernel: 1 << 30, cache: 6 << 30 },
            temp_c: Some(55.0),
            disks: [("/".to_string(), (50u64 << 30, 100u64 << 30))].into(),
            gpus: vec![crate::gpu::GpuStat {
                card: "card1".into(),
                load: Some(40.0),
                rows: vec![
                    crate::gpu::GpuRow { label: "CMP", pct: Some(70.0) },
                    crate::gpu::GpuRow { label: "BLT", pct: Some(10.0) },
                    crate::gpu::GpuRow { label: "VID", pct: Some(35.0) },
                ],
                vram: None,
            }],
            interfaces: Some(crate::netif::list()),
            services: Some(crate::services::ServiceList {
                error: None,
                units: (0..40)
                    .map(|i| Unit { name: format!("svc-{i}.service"), active: "active".into(), sub: "running".into() })
                    .collect(),
            }),
            docker: Some(DockerStatus {
                version: "27.1".into(),
                total: 20,
                running: 18,
                unhealthy: 1,
                stacks: (0..30)
                    .map(|i| Stack { name: format!("stack-with-a-long-name-{i}"), running: 2, total: 3, unhealthy: 0 })
                    .collect(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let c = Clock { time: "12:34".into(), date: "Sat 17 Oct 2026".into(), ..Default::default() };
        let cfg = crate::config::Config::default();
        // AX206MON_PREVIEW_DIR=dir cargo test dumps the pages as PNGs.
        let dump = std::env::var_os("AX206MON_PREVIEW_DIR");
        for (i, page) in cfg.pages.iter().enumerate() {
            let info = PageInfo { page, index: i, count: cfg.pages.len(), variant: i };
            for (cw, ch) in [(480, 320), (320, 480)] {
                let f = render_page(cw, ch, &info, &cfg.global, &s, &c);
                assert_eq!((f.width(), f.height()), (cw, ch));
                if let Some(d) = &dump {
                    f.to_rgb_image()
                        .save(std::path::Path::new(d).join(format!("page{}_{cw}x{ch}.png", i + 1)))
                        .unwrap();
                }
            }
        }
        // Every header variant renders at both sizes (dumped for a visual check).
        for v in 0..crate::burnin::VARIANTS {
            for (cw, ch) in [(480, 320), (320, 480)] {
                let info = PageInfo { page: &cfg.pages[0], index: 0, count: cfg.pages.len(), variant: v };
                let f = render_page(cw, ch, &info, &cfg.global, &s, &c);
                if let Some(d) = &dump {
                    f.to_rgb_image()
                        .save(std::path::Path::new(d).join(format!("variant{v}_{cw}x{ch}.png")))
                        .unwrap();
                }
            }
        }
        // Docker unreachable / nothing collected must not panic either.
        let none = Snapshot::default();
        for i in 0..cfg.pages.len() {
            let info = PageInfo { page: &cfg.pages[i], index: i, count: cfg.pages.len(), variant: i };
            render_page(320, 480, &info, &cfg.global, &none, &c);
        }
    }
}
