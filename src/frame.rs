//! An RGB888 canvas that embedded-graphics can draw on, plus conversion to
//! the panel's big-endian RGB565 format (with rotation) and to PNG.

use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::*;

/// Clockwise rotation from the logical canvas to the physical panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_degrees(d: u16) -> Option<Self> {
        match d {
            0 => Some(Rotation::R0),
            90 => Some(Rotation::R90),
            180 => Some(Rotation::R180),
            270 => Some(Rotation::R270),
            _ => None,
        }
    }

    /// Logical canvas size for a given physical panel size.
    pub fn canvas_size(self, panel_w: u32, panel_h: u32) -> (u32, u32) {
        match self {
            Rotation::R0 | Rotation::R180 => (panel_w, panel_h),
            Rotation::R90 | Rotation::R270 => (panel_h, panel_w),
        }
    }
}

pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<Rgb888>,
}

impl Frame {
    pub fn new(width: u32, height: u32, fill: Rgb888) -> Self {
        Self {
            width,
            height,
            pixels: vec![fill; (width * height) as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn get(&self, x: u32, y: u32) -> Rgb888 {
        self.pixels[(y * self.width + x) as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, c: Rgb888) {
        if x < self.width && y < self.height {
            self.pixels[(y * self.width + x) as usize] = c;
        }
    }

    /// Copy `src` onto this frame at (x, y), scaled up by an integer factor,
    /// skipping pixels equal to `transparent`.
    pub fn blit_scaled(&mut self, src: &Frame, x: i32, y: i32, scale: u32, transparent: Rgb888) {
        for sy in 0..src.height {
            for sx in 0..src.width {
                let c = src.get(sx, sy);
                if c == transparent {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = x + (sx * scale + dx) as i32;
                        let py = y + (sy * scale + dy) as i32;
                        if px >= 0 && py >= 0 {
                            self.set(px as u32, py as u32, c);
                        }
                    }
                }
            }
        }
    }

    /// Convert to the panel's wire format: RGB565 big-endian, rotated so
    /// the logical canvas appears upright on the physical panel.
    pub fn to_panel_rgb565(&self, rot: Rotation, panel_w: u32, panel_h: u32) -> Vec<u8> {
        let (cw, ch) = rot.canvas_size(panel_w, panel_h);
        assert_eq!(
            (cw, ch),
            (self.width, self.height),
            "frame size doesn't match panel"
        );
        let mut out = Vec::with_capacity((panel_w * panel_h * 2) as usize);
        for py in 0..panel_h {
            for px in 0..panel_w {
                let (lx, ly) = match rot {
                    Rotation::R0 => (px, py),
                    Rotation::R90 => (py, panel_w - 1 - px),
                    Rotation::R180 => (panel_w - 1 - px, panel_h - 1 - py),
                    Rotation::R270 => (panel_h - 1 - py, px),
                };
                out.extend_from_slice(&rgb565_be(self.get(lx, ly)));
            }
        }
        out
    }

    pub fn to_rgb_image(&self) -> image::RgbImage {
        image::RgbImage::from_fn(self.width, self.height, |x, y| {
            let c = self.get(x, y);
            image::Rgb([c.r(), c.g(), c.b()])
        })
    }
}

pub fn rgb565_be(c: Rgb888) -> [u8; 2] {
    let v: u16 =
        (u16::from(c.r() & 0xf8) << 8) | (u16::from(c.g() & 0xfc) << 3) | u16::from(c.b() >> 3);
    v.to_be_bytes()
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Frame {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 {
                self.set(p.x as u32, p.y as u32, c);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_byte_order_matches_capture() {
        // From the AIDA64 capture: 0x081f is a blue-ish background.
        assert_eq!(rgb565_be(Rgb888::new(0xff, 0xff, 0xff)), [0xff, 0xff]);
        assert_eq!(rgb565_be(Rgb888::new(0xff, 0, 0)), [0xf8, 0x00]);
        assert_eq!(rgb565_be(Rgb888::new(0, 0, 0xff)), [0x00, 0x1f]);
    }

    #[test]
    fn rotation_maps_corners() {
        // 3x2 panel, 90 deg: canvas is 2x3. Mark canvas top-left red.
        let mut f = Frame::new(2, 3, Rgb888::BLACK);
        f.set(0, 0, Rgb888::RED);
        let out = f.to_panel_rgb565(Rotation::R90, 3, 2);
        // Rotating the canvas clockwise puts its top-left at the panel's top-right.
        let top_right = 2 * 2;
        assert_eq!(&out[top_right..top_right + 2], &[0xf8, 0x00]);
        assert_eq!(out.iter().filter(|&&b| b == 0xf8).count(), 1);
    }

    #[test]
    fn identity_rotation_size() {
        let f = Frame::new(480, 320, Rgb888::BLACK);
        assert_eq!(
            f.to_panel_rgb565(Rotation::R0, 480, 320).len(),
            480 * 320 * 2
        );
    }
}
