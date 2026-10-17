//! Driver for AX206-protocol USB displays (VID 1908, PID 0102), e.g. the
//! QDtech "USB-Display" 3.5" AIDA64 screens built around a WCH CH32 MCU.
//!
//! The screen speaks a vendor dialect of USB Mass Storage Bulk-Only Transport:
//! every command is a 31-byte CBW on EP 0x01, optionally followed by a data
//! phase, and answered by a 13-byte CSW on EP 0x81.
//!
//! Wire details verified against a USBPcap capture of AIDA64 driving the
//! real hardware:
//!
//! * The CBW tag MUST be the literal bytes `de ad be ef`. A generic mass
//!   storage device just echoes any tag, but this firmware treats any other
//!   tag as an invalid command: it skips data phases, swallows pixel data
//!   without drawing it and stops answering. (Packing 0xDEADBEEF as a
//!   little-endian u32 gives `ef be ad de`, which breaks everything.)
//! * Pixels are RGB565, big-endian (high byte first).
//! * AIDA64 sends a full 480x320 frame as one 262144-byte write followed by
//!   the 45056-byte rest; the screen throttles the transfer while it draws
//!   (about 1.2 s per frame at USB full speed).
//! * Do NOT send SET_CONFIGURATION on an already configured device: the
//!   firmware doesn't reset its data toggles, so the next packet is dropped.

use std::time::Duration;

use log::{debug, info, warn};
use rusb::{DeviceHandle, GlobalContext};

pub const VID: u16 = 0x1908;
pub const PID: u16 = 0x0102;

const EP_OUT: u8 = 0x01;
const EP_IN: u8 = 0x81;
const INTERFACE: u8 = 0;

/// CBW tag exactly as AIDA64/libdpf/lcd4linux send it (byte order matters!).
pub const TAG: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

/// Chunk size used by the Windows driver for frame data.
pub const DEFAULT_CHUNK: usize = 262_144;

const CMD_TIMEOUT: Duration = Duration::from_millis(1000);
const DATA_TIMEOUT: Duration = Duration::from_millis(10_000);
const READ_TIMEOUT: Duration = Duration::from_millis(4000);
const ACK_TIMEOUT: Duration = Duration::from_millis(2000);
const ACK_RETRIES: usize = 2;

/// Direction of a command's data phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

/// Build the 31-byte Command Block Wrapper.
pub fn cbw(cdb: &[u8; 16], data_len: u32, dir: Dir) -> [u8; 31] {
    let mut b = [0u8; 31];
    b[0..4].copy_from_slice(b"USBC");
    b[4..8].copy_from_slice(&TAG);
    b[8..12].copy_from_slice(&data_len.to_le_bytes());
    b[12] = if dir == Dir::In { 0x80 } else { 0x00 };
    b[13] = 0; // LUN
    b[14] = 16; // CDB length
    b[15..31].copy_from_slice(cdb);
    b
}

/// Command blocks (the 16-byte CDB inside the CBW).
pub mod cdb {
    const VENDOR: u8 = 0xcd;
    const EXT: u8 = 0x06;
    const GET_LCD_PARAMS: u8 = 0x02;
    const SET_PROPERTY: u8 = 0x01;
    const PROPERTY_BRIGHTNESS: u16 = 0x0001;
    const BLIT: u8 = 0x12;

    /// Ask for the panel size; answer is 5 bytes: w (LE u16), h (LE u16), 0xff.
    pub fn get_lcd_params() -> [u8; 16] {
        let mut c = [0u8; 16];
        c[0] = VENDOR;
        c[5] = GET_LCD_PARAMS;
        c
    }

    /// Backlight level 0 (off) ..= 7 (max).
    pub fn set_backlight(level: u8) -> [u8; 16] {
        let level = u16::from(level.min(7));
        let mut c = [0u8; 16];
        c[0] = VENDOR;
        c[5] = EXT;
        c[6] = SET_PROPERTY;
        c[7..9].copy_from_slice(&PROPERTY_BRIGHTNESS.to_le_bytes());
        c[9..11].copy_from_slice(&level.to_le_bytes());
        c
    }

    /// Draw a rectangle; coordinates are inclusive. This firmware only
    /// reliably accepts full-screen blits.
    pub fn blit(x0: u16, y0: u16, x1: u16, y1: u16) -> [u8; 16] {
        let mut c = [0u8; 16];
        c[0] = VENDOR;
        c[5] = EXT;
        c[6] = BLIT;
        c[7..9].copy_from_slice(&x0.to_le_bytes());
        c[9..11].copy_from_slice(&y0.to_le_bytes());
        c[11..13].copy_from_slice(&x1.to_le_bytes());
        c[13..15].copy_from_slice(&y1.to_le_bytes());
        c
    }
}

#[derive(Debug)]
pub enum Error {
    /// No matching device on the bus.
    NotFound,
    /// The device vanished (unplugged, reset). Reopen to continue.
    Disconnected,
    Usb(rusb::Error),
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => write!(f, "display {VID:04x}:{PID:04x} not found"),
            Error::Disconnected => write!(f, "display disconnected"),
            Error::Usb(rusb::Error::Access) => write!(
                f,
                "permission denied opening the display (is the udev rule installed?)"
            ),
            Error::Usb(e) => write!(f, "USB error: {e}"),
            Error::Protocol(s) => write!(f, "protocol error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusb::Error> for Error {
    fn from(e: rusb::Error) -> Self {
        match e {
            rusb::Error::NoDevice => Error::Disconnected,
            other => Error::Usb(other),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct OpenOptions {
    /// Skip the size query and assume this panel size.
    pub size: Option<(u16, u16)>,
    /// Backlight level to set right after opening (None = leave as is).
    pub backlight: Option<u8>,
    /// Frame data chunk size in bytes.
    pub chunk: usize,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            size: None,
            backlight: Some(7),
            chunk: DEFAULT_CHUNK,
        }
    }
}

pub struct Display {
    handle: DeviceHandle<GlobalContext>,
    width: u16,
    height: u16,
    chunk: usize,
    missing_acks: u64,
}

impl Display {
    /// Open the first matching display and run the same start-up sequence as
    /// AIDA64: size query, then backlight.
    pub fn open(opts: &OpenOptions) -> Result<Self> {
        let handle = rusb::open_device_with_vid_pid(VID, PID).ok_or(Error::NotFound)?;
        if let Ok(true) = handle.kernel_driver_active(INTERFACE) {
            handle.detach_kernel_driver(INTERFACE)?;
        }
        // Intentionally no set_active_configuration(): see module docs.
        handle.claim_interface(INTERFACE)?;

        let mut d = Display {
            handle,
            width: 480,
            height: 320,
            chunk: opts.chunk.max(64) / 64 * 64,
            missing_acks: 0,
        };

        match opts.size {
            Some((w, h)) => {
                d.width = w;
                d.height = h;
            }
            None => {
                let (w, h) = d.query_size()?;
                d.width = w;
                d.height = h;
            }
        }
        info!("display opened, panel {}x{}", d.width, d.height);

        if let Some(level) = opts.backlight {
            d.set_backlight(level)?;
        }
        Ok(d)
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Panel size as reported by the firmware.
    pub fn query_size(&mut self) -> Result<(u16, u16)> {
        let mut buf = [0u8; 5];
        self.command_in(&cdb::get_lcd_params(), &mut buf)?;
        let w = u16::from_le_bytes([buf[0], buf[1]]);
        let h = u16::from_le_bytes([buf[2], buf[3]]);
        if w == 0 || h == 0 || w > 2048 || h > 2048 {
            return Err(Error::Protocol(format!(
                "implausible size reply {buf:02x?}"
            )));
        }
        Ok((w, h))
    }

    pub fn set_backlight(&mut self, level: u8) -> Result<()> {
        self.command_out(&cdb::set_backlight(level), &[])
    }

    /// Send a full frame of big-endian RGB565 pixels (width*height*2 bytes).
    pub fn blit_full(&mut self, rgb565_be: &[u8]) -> Result<()> {
        let expected = usize::from(self.width) * usize::from(self.height) * 2;
        if rgb565_be.len() != expected {
            return Err(Error::Protocol(format!(
                "frame is {} bytes, panel needs {expected}",
                rgb565_be.len()
            )));
        }
        let cmd = cdb::blit(0, 0, self.width - 1, self.height - 1);
        self.command_out(&cmd, rgb565_be)
    }

    // ---- transport -------------------------------------------------------

    fn command_out(&mut self, cdb: &[u8; 16], data: &[u8]) -> Result<()> {
        let header = cbw(cdb, data.len() as u32, Dir::Out);
        debug!("cbw {}", hex(&header));
        self.write_all(&header, CMD_TIMEOUT)?;
        for chunk in data.chunks(self.chunk) {
            self.write_all(chunk, DATA_TIMEOUT)?;
        }
        self.read_ack()
    }

    fn command_in(&mut self, cdb: &[u8; 16], buf: &mut [u8]) -> Result<()> {
        let header = cbw(cdb, buf.len() as u32, Dir::In);
        debug!("cbw {}", hex(&header));
        self.write_all(&header, CMD_TIMEOUT)?;
        let n = self.handle.read_bulk(EP_IN, buf, READ_TIMEOUT)?;
        debug!("data {}", hex(&buf[..n]));
        if n != buf.len() {
            return Err(Error::Protocol(format!(
                "short read: {n} of {} bytes",
                buf.len()
            )));
        }
        self.read_ack()
    }

    fn write_all(&mut self, data: &[u8], timeout: std::time::Duration) -> Result<()> {
        let n = self.handle.write_bulk(EP_OUT, data, timeout)?;
        if n != data.len() {
            return Err(Error::Protocol(format!(
                "short write: {n} of {} bytes",
                data.len()
            )));
        }
        Ok(())
    }

    /// Read the 13-byte CSW. A missing ACK is tolerated (logged), because
    /// some units occasionally skip it yet keep working; a vanished device
    /// is not.
    fn read_ack(&mut self) -> Result<()> {
        let mut csw = [0u8; 13];
        for attempt in 1..=ACK_RETRIES {
            match self.handle.read_bulk(EP_IN, &mut csw, ACK_TIMEOUT) {
                Ok(13) if csw[0..4] == *b"USBS" => {
                    debug!("csw {}", hex(&csw));
                    if csw[4..8] != TAG {
                        warn!("status reply has unexpected tag {}", hex(&csw[4..8]));
                    }
                    if csw[12] != 0 {
                        warn!("display reported command status {}", csw[12]);
                    }
                    return Ok(());
                }
                Ok(n) => debug!("odd status reply ({n} bytes): {}", hex(&csw[..n])),
                Err(rusb::Error::NoDevice) => return Err(Error::Disconnected),
                Err(e) => debug!("status read attempt {attempt} failed: {e}"),
            }
        }
        self.missing_acks += 1;
        if self.missing_acks == 1 || self.missing_acks % 100 == 0 {
            warn!(
                "display did not send a status reply ({} so far); continuing",
                self.missing_acks
            );
        }
        Ok(())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(INTERFACE);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|h| u8::from_str_radix(h, 16).unwrap())
            .collect()
    }

    // Golden values copied from the USBPcap capture of AIDA64 on Windows.

    #[test]
    fn size_query_matches_aida64() {
        let got = cbw(&cdb::get_lcd_params(), 5, Dir::In);
        // AIDA64 sends flags 0x00 here; libdpf/lcd4linux send 0x80. The
        // firmware accepts both, we use the spec-correct 0x80.
        let mut want = parse_hex(
            "55 53 42 43 de ad be ef 05 00 00 00 00 00 10 cd 00 00 00 00 02 00 00 00 00 00 00 00 00 00 00",
        );
        want[12] = 0x80;
        assert_eq!(got.to_vec(), want);
    }

    #[test]
    fn backlight_matches_aida64() {
        let got = cbw(&cdb::set_backlight(7), 0, Dir::Out);
        let want = parse_hex(
            "55 53 42 43 de ad be ef 00 00 00 00 00 00 10 cd 00 00 00 00 06 01 01 00 07 00 00 00 00 00 00",
        );
        assert_eq!(got.to_vec(), want);
    }

    #[test]
    fn full_frame_blit_matches_aida64() {
        let got = cbw(&cdb::blit(0, 0, 479, 319), 480 * 320 * 2, Dir::Out);
        let want = parse_hex(
            "55 53 42 43 de ad be ef 00 b0 04 00 00 00 10 cd 00 00 00 00 06 12 00 00 00 00 df 01 3f 01 00",
        );
        assert_eq!(got.to_vec(), want);
    }

    #[test]
    fn tag_is_literal_bytes_not_little_endian_u32() {
        let got = cbw(&cdb::get_lcd_params(), 0, Dir::Out);
        assert_eq!(&got[4..8], &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn backlight_is_clamped() {
        assert_eq!(cdb::set_backlight(200)[9], 7);
    }
}
