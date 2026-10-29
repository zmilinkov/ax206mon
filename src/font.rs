//! Text rendering with the bundled Noto Sans Mono (SIL OFL 1.1, see
//! assets/fonts). Monospace, so layout code can keep counting characters:
//! every glyph occupies a cell of `Font::w()` pixels.

use std::cell::RefCell;
use std::collections::HashMap;

use ab_glyph::{point, Font as _, FontRef, PxScale, ScaleFont};
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::text::Alignment;

use crate::frame::Frame;

static TTF: &[u8] = include_bytes!("../assets/fonts/NotoSansMono-Regular.ttf");

/// Advance width of every glyph, as a fraction of the pixel size.
const ADVANCE: f32 = 0.6;
/// Line box height and baseline position, as fractions of the pixel size.
const LINE_H: f32 = 1.12;
const BASELINE: f32 = 0.86;
/// Height of digits relative to `px`. Note `px` is ab_glyph's PxScale, i.e.
/// the font's full ascent+descent height (1.362 em for Noto Sans Mono), not
/// the em size: digits are 0.714 em = 0.52-0.54 of `px`.
const CAP_H: f32 = 0.54;

thread_local! {
    /// Multiplier on all text sizes (`global.font_scale`). Per thread: the
    /// service renders on its main thread, and tests stay independent.
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

pub fn set_scale(s: f32) {
    SCALE.with(|c| c.set(s));
}

pub fn scale() -> f32 {
    SCALE.with(|c| c.get())
}

/// The four text sizes the dashboard uses, before `font_scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    /// Legends, addresses, footer.
    Small,
    /// List rows, date, sub-rows.
    Mid,
    /// Bar labels and values, titles.
    Big,
}

impl TextSize {
    fn base_px(self) -> f32 {
        match self {
            TextSize::Small => 12.0,
            TextSize::Mid => 16.0,
            TextSize::Big => 20.0,
        }
    }

    pub fn font(self) -> Font {
        Font::px((self.base_px() * scale()).max(6.0))
    }

    /// Cell width in pixels.
    pub fn w(self) -> i32 {
        self.font().w()
    }

    /// Line height in pixels.
    pub fn h(self) -> i32 {
        self.font().h()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Font {
    px: f32,
}

impl Font {
    pub fn px(px: f32) -> Self {
        Font { px }
    }

    pub fn w(self) -> i32 {
        ((self.px * ADVANCE).round() as i32).max(1)
    }

    pub fn h(self) -> i32 {
        (self.px * LINE_H).round() as i32
    }

    /// Height of digits/capitals in pixels.
    pub fn cap_h(self) -> i32 {
        (self.px * CAP_H).round() as i32
    }

    /// Distance from the top of the line box to the top of digits/capitals.
    pub fn cap_offset(self) -> i32 {
        self.baseline() - self.cap_h()
    }

    fn baseline(self) -> i32 {
        (self.px * BASELINE).round() as i32
    }
}

/// Largest pixel size at which `chars` characters fit in `width` pixels.
pub fn fit_px(width: i32, chars: usize, max_px: f32) -> f32 {
    let by_width = width as f32 / (ADVANCE * chars.max(1) as f32);
    // Whole pixel sizes keep the rounded cell width stable.
    by_width.min(max_px).floor().max(6.0)
}

struct Glyph {
    /// Offset of the bitmap's top-left from the pen position on the baseline.
    left: i32,
    top: i32,
    w: usize,
    h: usize,
    coverage: Vec<u8>,
    /// Unrounded advance, used to centre the glyph in its cell.
    advance: f32,
}

thread_local! {
    static FONT: FontRef<'static> = FontRef::try_from_slice(TTF).expect("bundled font parses");
    /// Rasterised glyphs by (char, pixel size in tenths).
    static CACHE: RefCell<HashMap<(char, u32), std::rc::Rc<Glyph>>> = RefCell::new(HashMap::new());
}

fn glyph(c: char, px: f32) -> std::rc::Rc<Glyph> {
    let key = (c, (px * 10.0).round() as u32);
    if let Some(g) = CACHE.with(|m| m.borrow().get(&key).cloned()) {
        return g;
    }
    let g = std::rc::Rc::new(FONT.with(|font| {
        let scaled = font.as_scaled(PxScale::from(px));
        let id = font.glyph_id(c);
        let advance = scaled.h_advance(id);
        let g = id.with_scale_and_position(PxScale::from(px), point(0.0, 0.0));
        match font.outline_glyph(g) {
            Some(o) => {
                let b = o.px_bounds();
                let (w, h) = (b.width().ceil() as usize, b.height().ceil() as usize);
                let mut coverage = vec![0u8; w * h];
                o.draw(|x, y, c| {
                    let (x, y) = (x as usize, y as usize);
                    if x < w && y < h {
                        // Slight gamma boost: Regular weight looks thin on a small LCD.
                        coverage[y * w + x] = (c.clamp(0.0, 1.0).powf(0.7) * 255.0).round() as u8;
                    }
                });
                Glyph {
                    left: b.min.x.floor() as i32,
                    top: b.min.y.floor() as i32,
                    w,
                    h,
                    coverage,
                    advance,
                }
            }
            // Space and other glyphs without an outline.
            None => Glyph { left: 0, top: 0, w: 0, h: 0, coverage: Vec::new(), advance },
        }
    }));
    CACHE.with(|m| m.borrow_mut().insert(key, g.clone()));
    g
}

fn blend(bg: Rgb888, fg: Rgb888, a: u8) -> Rgb888 {
    let mix = |b: u8, f: u8| ((u32::from(b) * (255 - u32::from(a)) + u32::from(f) * u32::from(a) + 127) / 255) as u8;
    Rgb888::new(mix(bg.r(), fg.r()), mix(bg.g(), fg.g()), mix(bg.b(), fg.b()))
}

/// Draw `s` with its line box top at `y`; `x` is the left, centre or right
/// edge depending on `align`.
pub fn draw(f: &mut Frame, s: &str, x: i32, y: i32, font: Font, color: Rgb888, align: Alignment) {
    let cell = font.w();
    let total = cell * s.chars().count() as i32;
    let start = match align {
        Alignment::Left => x,
        Alignment::Center => x - total / 2,
        Alignment::Right => x - total,
    };
    let baseline = y + font.baseline();
    for (i, c) in s.chars().enumerate() {
        if c == ' ' {
            continue;
        }
        let g = glyph(c, font.px);
        let pen = start + i as i32 * cell + ((cell as f32 - g.advance) / 2.0).round() as i32;
        for gy in 0..g.h {
            for gx in 0..g.w {
                let a = g.coverage[gy * g.w + gx];
                if a == 0 {
                    continue;
                }
                let (px, py) = (pen + g.left + gx as i32, baseline + g.top + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < f.width() && (py as u32) < f.height() {
                    let bg = f.get(px as u32, py as u32);
                    f.set(px as u32, py as u32, blend(bg, color, a));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_is_monospace_with_expected_advance() {
        FONT.with(|font| {
            let units = font.units_per_em().unwrap();
            let adv = |c| font.h_advance_unscaled(font.glyph_id(c)) / units;
            assert!((adv('M') - ADVANCE).abs() < 0.01, "{}", adv('M'));
            assert_eq!(adv('M'), adv('i'));
            assert_eq!(adv('M'), adv('0'));
        });
    }

    #[test]
    fn covers_the_characters_the_dashboard_uses() {
        FONT.with(|font| {
            for c in "0123456789:/%.-~+!\u{b0}abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
                assert_ne!(font.glyph_id(c).0, 0, "missing {c:?}");
            }
        });
    }

    #[test]
    fn cap_height_constant_matches_the_font() {
        FONT.with(|font| {
            let g = font
                .glyph_id('0')
                .with_scale_and_position(PxScale::from(1000.0), point(0.0, 0.0));
            let h = font.outline_glyph(g).unwrap().px_bounds().height();
            // Digits are a touch taller than capitals (overshoot); allow 3 %.
            assert!((h / 1000.0 - CAP_H).abs() < 0.03, "digit height {}", h / 1000.0);
        });
    }

    #[test]
    fn metrics_scale() {
        let f = Font::px(20.0);
        assert_eq!((f.w(), f.h()), (12, 22));
        assert!(TextSize::Small.w() < TextSize::Mid.w() && TextSize::Mid.w() < TextSize::Big.w());
    }

    #[test]
    fn fit_px_respects_width_and_max() {
        // 5 chars in 288 px: 96 px would fit, capped by max.
        assert_eq!(fit_px(288, 5, 76.0), 76.0);
        assert_eq!(fit_px(100, 5, 76.0), 33.0);
    }

    #[test]
    fn draws_antialiased_pixels_inside_the_line_box() {
        let bg = Rgb888::BLACK;
        let mut f = Frame::new(60, 40, bg);
        let font = TextSize::Big.font();
        draw(&mut f, "A", 5, 5, font, Rgb888::WHITE, Alignment::Left);
        let lit: Vec<(u32, u32)> = (0..40)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .filter(|&(x, y)| f.get(x, y) != bg)
            .collect();
        assert!(lit.len() > 20, "glyph drew {} pixels", lit.len());
        let (minx, maxx) = (lit.iter().map(|p| p.0).min().unwrap(), lit.iter().map(|p| p.0).max().unwrap());
        let (miny, maxy) = (lit.iter().map(|p| p.1).min().unwrap(), lit.iter().map(|p| p.1).max().unwrap());
        assert!(minx >= 5 && maxx < 5 + font.w() as u32 + 1, "x {minx}..{maxx}");
        assert!(miny >= 5 && maxy < 5 + font.h() as u32, "y {miny}..{maxy}");
        // Some edge pixel is a partial blend (anti-aliasing).
        assert!(lit.iter().any(|&(x, y)| f.get(x, y) != Rgb888::WHITE));
    }

    #[test]
    fn alignment_places_text_relative_to_x() {
        let bg = Rgb888::BLACK;
        let extent = |align| {
            let mut f = Frame::new(200, 30, bg);
            draw(&mut f, "000", 100, 2, TextSize::Mid.font(), Rgb888::WHITE, align);
            let xs: Vec<u32> = (0..200).filter(|&x| (0..30).any(|y| f.get(x, y) != bg)).collect();
            (xs[0], *xs.last().unwrap())
        };
        let (l0, _) = extent(Alignment::Left);
        let (_, r1) = extent(Alignment::Right);
        let (c0, c1) = extent(Alignment::Center);
        assert!((100..106).contains(&l0));
        assert!(r1 <= 100 && r1 > 92);
        assert!(c0 < 100 && c1 > 100);
    }
}
