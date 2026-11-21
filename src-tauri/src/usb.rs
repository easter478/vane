//! USB drivers for the Lian Li devices in this PC. All of them use the Microsoft WinUSB driver,
//! so libusb (rusb, vendored) talks to them directly: interface 0, bulk OUT 0x01, bulk IN 0x81.
//!
//! Protocol details come from the open-source lian-li-linux project (MIT, github.com/sgtaziz/lian-li-linux),
//! which worked them out by reverse engineering for interoperability.
//!
//! - `FanLcd`: UNI FAN SL V3 wireless LCD screen, USB 1CBE:0005, 400x400 JPEG. Uses the older
//!   512-byte DES header and a fixed 102,400-byte frame packet.
//! - `CoolerLcd`: HydroShift II LCD-S screen, USB 1CBE:A034, 480x480 JPEG. Uses the "WinUSB" header
//!   (500-byte plaintext, A1 1A trailer) and variable-length packets.

use anyhow::{bail, Context, Result};
use cbc::Encryptor;
use des::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use rusb::{DeviceHandle, GlobalContext};
use std::time::{Duration, Instant};

pub const EP_OUT: u8 = 0x01;
pub const EP_IN: u8 = 0x81;
const KEY: [u8; 8] = *b"slv3tuzx";

pub const VID_LCD: u16 = 0x1CBE;
pub const PID_FAN_LCD: u16 = 0x0005;
pub const PID_COOLER_LCD: u16 = 0xA034;

/// Builds the encrypted 512-byte command headers.
pub struct Packets {
    start: Instant,
    last: u32,
}

impl Packets {
    pub fn new() -> Self {
        Self { start: Instant::now(), last: 0 }
    }
    fn ts(&mut self) -> u32 {
        let raw = self.start.elapsed().as_millis() as u32;
        let t = if raw <= self.last { self.last + 1 } else { raw };
        self.last = t;
        t
    }
    /// Older header (SL V3 fan screens): 504 bytes plaintext -> 512 encrypted.
    pub fn legacy(&mut self, cmd: u8, params: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; 512];
        buf[0] = cmd;
        buf[2] = 0x1A;
        buf[3] = 0x6D;
        let ts = self.ts();
        buf[4..8].copy_from_slice(&ts.to_le_bytes());
        let n = params.len().min(496);
        buf[8..8 + n].copy_from_slice(&params[..n]);
        Encryptor::<des::Des>::new_from_slices(&KEY, &KEY)
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut buf, 504)
            .unwrap()
            .to_vec()
    }
    /// WinUSB header (cooler screen and newer panels).
    pub fn winusb(&mut self, cmd: u8, params: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; 508];
        buf[0] = cmd;
        buf[2] = 0x1A;
        buf[3] = 0x6D;
        let ts = self.ts();
        buf[4..8].copy_from_slice(&ts.to_le_bytes());
        let n = params.len().min(492);
        buf[8..8 + n].copy_from_slice(&params[..n]);
        let enc = Encryptor::<des::Des>::new_from_slices(&KEY, &KEY)
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut buf, 500)
            .unwrap()
            .to_vec();
        let mut out = vec![0u8; 512];
        out[..504].copy_from_slice(&enc);
        out[510] = 0xA1;
        out[511] = 0x1A;
        out
    }
}

pub struct Bulk {
    pub h: DeviceHandle<GlobalContext>,
}

/// libusb's Windows backend misbehaves when several threads list or open devices at the same time
/// (all screen workers and the dongle start together), so those calls take turns.
static USB_LOCK: parking_lot::Mutex<()> = parking_lot::const_mutex(());

impl Bulk {
    pub fn open(dev: &rusb::Device<GlobalContext>) -> Result<Self> {
        let _turn = USB_LOCK.lock();
        let h = dev.open().context("opening USB device (is L-Connect still running?)")?;
        h.claim_interface(0).context("claiming USB interface (is L-Connect still running?)")?;
        Ok(Self { h })
    }
    pub fn write(&self, data: &[u8], timeout: Duration) -> Result<()> {
        let mut off = 0;
        while off < data.len() {
            let n = self.h.write_bulk(EP_OUT, &data[off..], timeout)?;
            if n == 0 {
                bail!("USB write made no progress");
            }
            off += n;
        }
        Ok(())
    }
    pub fn read(&self, buf: &mut [u8], timeout: Duration) -> Option<usize> {
        self.h.read_bulk(EP_IN, buf, timeout).ok()
    }
    pub fn flush(&self) {
        let mut b = [0u8; 512];
        for _ in 0..16 {
            if self.h.read_bulk(EP_IN, &mut b, Duration::from_millis(5)).is_err() {
                break;
            }
        }
    }
    pub fn exchange(&self, pkt: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        self.write(pkt, Duration::from_millis(2000))?;
        let mut b = vec![0u8; 512];
        let n = self.read(&mut b, timeout).unwrap_or(0);
        b.truncate(n);
        Ok(b)
    }
}

impl Drop for Bulk {
    fn drop(&mut self) {
        let _ = self.h.release_interface(0);
    }
}

/// Stable name for a device from the USB port it sits in. Reading the serial would need the device
/// opened, and WinUSB allows only one open handle, so a screen already in use can't be asked again.
pub fn port_key(dev: &rusb::Device<GlobalContext>) -> String {
    let ports: Vec<String> = dev.port_numbers().unwrap_or_default().iter().map(|p| p.to_string()).collect();
    format!("fan-{}-{}", dev.bus_number(), ports.join("."))
}

pub fn find(vid: u16, pid: u16) -> Vec<rusb::Device<GlobalContext>> {
    let _turn = USB_LOCK.lock();
    rusb::devices()
        .map(|l| {
            l.iter()
                .filter(|d| d.device_descriptor().is_ok_and(|x| x.vendor_id() == vid && x.product_id() == pid))
                .collect()
        })
        .unwrap_or_default()
}

fn fw_string(r: &[u8]) -> Option<String> {
    if r.len() <= 8 {
        return None;
    }
    let s = &r[8..r.len().min(40)];
    let e = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    let t = String::from_utf8_lossy(&s[..e]).trim().to_string();
    (!t.is_empty()).then_some(t)
}

// ---------------------------------------------------------------- SL V3 fan screen

pub struct FanLcd {
    bulk: Bulk,
    pk: Packets,
    pub firmware: Option<String>,
    ready: bool,
    /// last brightness asked for; sent again after every re-init, because the panel
    /// goes back to full brightness whenever it resets
    bright: Option<u8>,
}

pub const FAN_LCD_SIZE: u32 = 400;
const FAN_FRAME_PACKET: usize = 102_400;

impl FanLcd {
    pub fn open(dev: &rusb::Device<GlobalContext>) -> Result<Self> {
        let bulk = Bulk::open(dev)?;
        Ok(Self { bulk, pk: Packets::new(), firmware: None, ready: false, bright: None })
    }

    fn init(&mut self) -> Result<()> {
        if self.ready {
            return Ok(());
        }
        let mut buf = [0u8; 512];
        // hardware revision check (the vendor app waits 10 ms and moves on without a reply)
        let rev = self.pk.winusb(0x80, &[]);
        self.bulk.write(&rev, Duration::from_millis(500))?;
        let _ = self.bulk.read(&mut buf, Duration::from_millis(10));
        // the vendor's fixed frame rate for wireless LCDs
        let fr = self.pk.legacy(0x0F, &[120]);
        self.bulk.write(&fr, Duration::from_millis(500))?;
        let _ = self.bulk.read(&mut buf, Duration::from_millis(1000));
        let ver = self.pk.legacy(0x0A, &[]);
        self.bulk.write(&ver, Duration::from_millis(500))?;
        if let Some(n) = self.bulk.read(&mut buf, Duration::from_millis(1000)) {
            if n > 8 && buf[0] == 0x0A {
                self.firmware = fw_string(&buf[..n]);
            }
        }
        self.ready = true;
        if let Some(b) = self.bright {
            let h = self.pk.legacy(0x0E, &[fan_brightness_lut(b)]);
            self.bulk.write(&h, Duration::from_millis(500))?;
            let n = self.bulk.read(&mut buf, Duration::from_millis(1000));
            crate::config::log_to("screens.log", &format!("fan re-init: brightness {b}% -> {} again, reply {}", fan_brightness_lut(b), reply_hex(&buf, n)));
        }
        Ok(())
    }

    pub fn send_jpeg(&mut self, jpeg: &[u8]) -> Result<()> {
        if jpeg.len() > FAN_FRAME_PACKET - 512 {
            bail!("frame too large for the fan screen ({} bytes)", jpeg.len());
        }
        self.init()?;
        let header = self.pk.legacy(0x65, &(jpeg.len() as u32).to_be_bytes());
        let mut pkt = vec![0u8; FAN_FRAME_PACKET];
        pkt[..512].copy_from_slice(&header);
        pkt[512..512 + jpeg.len()].copy_from_slice(jpeg);
        if let Err(e) = self.bulk.write(&pkt, Duration::from_millis(2000)) {
            self.ready = false;
            return Err(e);
        }
        let mut b = [0u8; 512];
        let _ = self.bulk.read(&mut b, Duration::from_millis(1000));
        Ok(())
    }

    /// Returns the panel's reply, for the diagnostics log.
    pub fn set_brightness(&mut self, pct: u8) -> Result<String> {
        self.bright = Some(pct);
        self.init()?;
        let h = self.pk.legacy(0x0E, &[fan_brightness_lut(pct)]);
        self.bulk.write(&h, Duration::from_millis(500))?;
        let mut b = [0u8; 512];
        let n = self.bulk.read(&mut b, Duration::from_millis(1000));
        Ok(format!("{pct}% -> {}, reply {}", fan_brightness_lut(pct), reply_hex(&b, n)))
    }
}

fn reply_hex(b: &[u8], n: Option<usize>) -> String {
    match n {
        None => "none".into(),
        Some(n) => format!("{n} bytes {}", b[..n.min(16)].iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")),
    }
}

/// The SL V3 panel's brightness response is not linear; these anchors match the vendor app.
fn fan_brightness_lut(value: u8) -> u8 {
    const A: &[(u8, u8)] = &[(0, 0), (25, 10), (50, 30), (75, 40), (100, 100)];
    let v = value.min(100);
    for w in A.windows(2) {
        let ((li, lo), (hi, ho)) = (w[0], w[1]);
        if v <= hi {
            let span = (hi - li) as u32;
            return lo + (((v - li) as u32 * (ho - lo) as u32 + span / 2) / span) as u8;
        }
    }
    100
}

// ---------------------------------------------------------------- HydroShift II cooler screen

pub struct CoolerLcd {
    bulk: Bulk,
    pk: Packets,
    pub firmware: Option<String>,
    ready: bool,
    layers_cleared: bool,
    /// when the panel last confirmed the transparent overlay layer, and when it was set up
    cleared_at: Option<std::time::Instant>,
    ready_at: std::time::Instant,
}

pub const COOLER_LCD_SIZE: u32 = 480;
const COOLER_MAX_PAYLOAD: usize = 153_600;

#[derive(Clone, Copy, Debug, Default)]
pub struct CoolerTelemetry {
    pub coolant: u8,
    pub pump_rpm: u16,
    pub fan_rpm: [u16; 3],
}

impl CoolerLcd {
    pub fn open(dev: &rusb::Device<GlobalContext>) -> Result<Self> {
        let bulk = Bulk::open(dev)?;
        Ok(Self { bulk, pk: Packets::new(), firmware: None, ready: false, layers_cleared: false, cleared_at: None, ready_at: std::time::Instant::now() })
    }

    fn cmd(&mut self, cmd: u8, params: &[u8]) -> Vec<u8> {
        let p = self.pk.winusb(cmd, params);
        let r = self.bulk.exchange(&p, Duration::from_millis(2000)).unwrap_or_default();
        self.bulk.flush();
        r
    }

    fn init(&mut self) -> Result<()> {
        if self.ready {
            return Ok(());
        }
        self.bulk.flush();
        let _ = self.cmd(0x7B, &[]); // StopPlay: end the firmware's own theme playback
        std::thread::sleep(Duration::from_millis(150));
        let r = self.cmd(0x0A, &[]); // GetVer
        if r.first() != Some(&0x0A) {
            // still starting up (right after Windows starts): try the whole setup again next frame,
            // so StopPlay isn't lost and the panel's own screen doesn't stay up
            crate::config::log_to("screens.log", "cooler: no answer yet, setting up again");
            std::thread::sleep(Duration::from_millis(500));
            bail!("the cooler screen isn't answering yet");
        }
        self.firmware = fw_string(&r);
        let _ = self.cmd(0x0F, &[30]); // frame rate
        let now = chrono::Local::now();
        use chrono::{Datelike, Timelike};
        let y = now.year() as u16;
        let clock = [(y >> 8) as u8, y as u8, now.month() as u8, now.day() as u8, now.hour() as u8, now.minute() as u8, now.second() as u8, 2];
        let _ = self.cmd(0x33, &clock); // SyncClock
        let _ = self.cmd(0x34, &[0]); // StopClock
        self.ready = true;
        self.ready_at = std::time::Instant::now();
        self.cleared_at = None;
        crate::config::log_to("screens.log", &format!("cooler: set up, firmware {}", self.firmware.as_deref().unwrap_or("?")));
        Ok(())
    }

    /// A transparent PNG layer stops the firmware's own coolant/pump overlay; a black JPEG clears the base layer.
    /// Returns whether the panel confirmed it.
    fn clear_layers(&mut self) -> bool {
        use image::{ImageBuffer, Rgba};
        let s = COOLER_LCD_SIZE;
        let mut png = Vec::new();
        if ImageBuffer::from_pixel(s, s, Rgba([0u8, 0, 0, 0]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
        {
            let mut pkt = self.pk.winusb(0x66, &(png.len() as u32).to_be_bytes());
            pkt.extend_from_slice(&png);
            let ok = self.bulk.exchange(&pkt, Duration::from_millis(2000)).map(|r| r.first() == Some(&0x66)).unwrap_or(false);
            self.bulk.flush();
            return ok;
        }
        false
    }

    pub fn send_jpeg(&mut self, jpeg: &[u8]) -> Result<()> {
        if jpeg.len() > COOLER_MAX_PAYLOAD {
            bail!("frame too large for the cooler screen ({} bytes)", jpeg.len());
        }
        self.init()?;
        // the panel turns its coolant/pump overlay back on after a reset or while it's still starting,
        // so the transparent layer goes again every 2 s for the first half-minute, then every 10 s
        let every = if self.ready_at.elapsed() < Duration::from_secs(30) { 2 } else { 10 };
        let wait = if self.layers_cleared { every } else { 1 };
        if self.cleared_at.is_none_or(|t| t.elapsed() > Duration::from_secs(wait)) {
            let ok = self.clear_layers();
            if ok != self.layers_cleared || self.cleared_at.is_none() {
                crate::config::log_to("screens.log", &format!("cooler: overlay layer {}", if ok { "confirmed" } else { "not confirmed, retrying" }));
            }
            self.layers_cleared = ok;
            self.cleared_at = Some(std::time::Instant::now());
        }
        let mut pkt = self.pk.winusb(0x65, &(jpeg.len() as u32).to_be_bytes());
        pkt.extend_from_slice(jpeg);
        let reply = match self.bulk.exchange(&pkt, Duration::from_millis(2000)) {
            Ok(r) => r,
            Err(e) => {
                self.ready = false;
                self.layers_cleared = false;
                let _ = self.bulk.h.clear_halt(EP_OUT);
                let _ = self.bulk.h.clear_halt(EP_IN);
                return Err(e);
            }
        };
        self.bulk.flush();
        // byte 8 of the ack is the panel's buffer level; let it drain if it backs up
        if reply.first() == Some(&0x65) && reply.get(8).is_some_and(|&l| l > 3) {
            for _ in 0..40 {
                let r = self.cmd(0x7A, &[]);
                if r.get(8).is_none_or(|&l| l <= 2) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    pub fn set_brightness(&mut self, pct: u8) -> Result<()> {
        self.init()?;
        let _ = self.cmd(0x0E, &[pct.min(100)]);
        Ok(())
    }

    /// Coolant temperature, pump and radiator fan speeds (GetH2Params).
    pub fn telemetry(&mut self) -> Option<CoolerTelemetry> {
        let r = self.cmd(0xFA, &[]);
        if r.len() < 32 || r[0] != 0xFA {
            return None;
        }
        let be = |i: usize| u16::from_be_bytes([r[i], r[i + 1]]);
        Some(CoolerTelemetry { coolant: r[13], pump_rpm: be(20), fan_rpm: [be(14), be(16), be(18)] })
    }
}
