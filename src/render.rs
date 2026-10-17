//! Drawing: the status dashboard, a test pattern and image fitting.
//! Layout adapts to landscape (480x320) and portrait (320x480) canvases.

use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use profont::{PROFONT_12_POINT, PROFONT_14_POINT, PROFONT_18_POINT, PROFONT_24_POINT};

use crate::frame::Frame;
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
}
use palette::*;

/// Wall-clock time to show; passed in so rendering is testable.
pub struct Clock {
    pub time: String,
    pub date: String,
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

fn bar(f: &mut Frame, x: i32, y: i32, w: u32, h: u32, frac: f32, color: Rgb888) {
    let r = Size::new(h / 2, h / 2);
    let _ =
        RoundedRectangle::with_equal_corners(Rectangle::new(Point::new(x, y), Size::new(w, h)), r)
            .into_styled(PrimitiveStyle::with_fill(PANEL))
            .draw(f);
    let fw = ((w as f32) * frac.clamp(0.0, 1.0)).round() as u32;
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

struct Row {
    label: &'static str,
    value: String,
    frac: f32,
    color: Rgb888,
}

pub fn dashboard(width: u32, height: u32, s: &Snapshot, clock: &Clock) -> Frame {
    let mut f = Frame::new(width, height, BG);
    let w = width as i32;
    let h = height as i32;
    let margin = 16;
    let portrait = height > width;

    // --- header: big clock, date, host -----------------------------------
    let clock_scale = if portrait { 3 } else { 2 };
    text_scaled(
        &mut f,
        &clock.time,
        w / 2,
        8,
        &PROFONT_24_POINT,
        TEXT,
        clock_scale,
    );
    let clock_h = (PROFONT_24_POINT.character_size.height * clock_scale) as i32;
    let sub_y = 8 + clock_h + 2;
    text(
        &mut f,
        &clock.date,
        Point::new(w / 2, sub_y),
        &PROFONT_14_POINT,
        MUTED,
        Alignment::Center,
    );

    // --- metric rows ------------------------------------------------------
    let mem_frac = frac(s.mem_used, s.mem_total);
    let disk_frac = frac(s.disk_used, s.disk_total);
    let mut rows = vec![
        Row {
            label: "CPU",
            value: format!("{:.0}%", s.cpu_pct),
            frac: s.cpu_pct / 100.0,
            color: CPU,
        },
        Row {
            label: "RAM",
            value: format!("{} / {}", human_bytes(s.mem_used), human_bytes(s.mem_total)),
            frac: mem_frac,
            color: RAM,
        },
        Row {
            label: "DISK",
            value: format!(
                "{} / {}",
                human_bytes(s.disk_used),
                human_bytes(s.disk_total)
            ),
            frac: disk_frac,
            color: DISK,
        },
    ];
    if let Some(t) = s.temp_c {
        rows.push(Row {
            label: "TEMP",
            value: format!("{t:.0}\u{b0}C"),
            frac: t / 100.0,
            color: TEMP,
        });
    }

    let footer_h = 26;
    let top = sub_y + 30;
    let avail = h - top - footer_h - 6;
    let row_h = (avail / rows.len() as i32).min(if portrait { 84 } else { 64 });
    let label_font = &PROFONT_18_POINT;
    let bar_h = 12u32;
    for (i, r) in rows.iter().enumerate() {
        let y = top + i as i32 * row_h;
        let color = if r.frac >= 0.9 { WARN } else { r.color };
        text(
            &mut f,
            r.label,
            Point::new(margin, y),
            label_font,
            color,
            Alignment::Left,
        );
        text(
            &mut f,
            &r.value,
            Point::new(w - margin, y),
            label_font,
            TEXT,
            Alignment::Right,
        );
        let bar_y = y + label_font.character_size.height as i32 + 4;
        bar(
            &mut f,
            margin,
            bar_y,
            (w - 2 * margin) as u32,
            bar_h,
            r.frac,
            color,
        );
    }

    // --- footer -------------------------------------------------------------
    let fy = h - footer_h + 4;
    let _ = Rectangle::new(
        Point::new(0, h - footer_h),
        Size::new(width, footer_h as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(PANEL))
    .draw(&mut f);
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
    text(
        &mut f,
        &left,
        Point::new(10, fy),
        &PROFONT_12_POINT,
        MUTED,
        Alignment::Left,
    );
    text(
        &mut f,
        &right,
        Point::new(w - 10, fy),
        &PROFONT_12_POINT,
        MUTED,
        Alignment::Right,
    );
    f
}

fn frac(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used as f32 / total as f32
    }
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
    fn dashboard_renders_both_orientations() {
        let s = Snapshot {
            cpu_pct: 42.0,
            mem_total: 1,
            temp_c: Some(55.0),
            ..Default::default()
        };
        let c = Clock {
            time: "12:34".into(),
            date: "Sat 17 Oct 2026".into(),
        };
        assert_eq!(dashboard(480, 320, &s, &c).width(), 480);
        assert_eq!(dashboard(320, 480, &s, &c).height(), 480);
    }
}
