//! Lian Li wireless dongle pair (SLV3TX 0416:8040 sends, SLV3RX 0416:8041 receives).
//!
//! - The RX dongle answers GetDev (0x10) with a 42-byte record per wireless device: MAC, master MAC,
//!   channel, radio slot, device type, fan count, fan types, fan RPM and current PWM.
//! - The TX dongle answers GetMac (0x11) with its own MAC (the "master") on its channel, and sends
//!   240-byte radio frames as four 64-byte USB packets.
//! - Fan speed: radio frame [0x12, 0x10, device MAC, master MAC, rx slot, channel, group index, pwm x4].
//!   Fans fall back to their hardware default when PWM traffic stops, so targets are re-sent every second.
//!
//! Protocol from lian-li-linux (MIT), crates/lianli-devices/src/wireless.

use crate::usb::Bulk;
use anyhow::{Context, Result};
use serde::Serialize;
use std::time::{Duration, Instant};

const TX: (u16, u16) = (0x0416, 0x8040);
const RX: (u16, u16) = (0x0416, 0x8041);

#[derive(Clone, Debug, Serialize)]
pub struct WirelessDevice {
    pub mac: String,
    #[serde(skip)]
    pub mac_raw: [u8; 6],
    #[serde(skip)]
    pub master: [u8; 6],
    pub channel: u8,
    pub rx: u8,
    pub device_type: u8,
    pub fan_count: u8,
    pub fan_types: [u8; 4],
    pub rpm: [u16; 4],
    pub pwm: [u8; 4],
    pub coolant: Option<u8>,
    pub kind: String,
    pub is_aio: bool,
    pub inf_right: bool,
    pub ours: bool,
    /// The lighting effect the device is playing now (matches the upload's index once it applied).
    #[serde(skip)]
    pub effect_index: [u8; 4],
    /// True while the device follows the motherboard's RGB header instead of its own effect.
    pub mb_sync: bool,
    #[serde(skip)]
    pub cmd_seq: u8,
}

impl WirelessDevice {
    fn min_duty_pct(&self) -> u8 {
        if self.is_aio {
            return 10;
        }
        match self.fan_types.iter().copied().find(|&b| b != 0).unwrap_or(0) {
            20..=26 | 59..=62 => 14,
            28..=31 | 36..=39 | 51..=58 => 11,
            63 => 8,
            _ => 10,
        }
    }
    /// LEDs per zone in upload order: the pump ring first on a HydroShift, then each fan.
    pub fn led_zones(&self) -> Vec<usize> {
        let per_fan = if self.is_aio {
            24
        } else {
            match self.fan_types.iter().copied().find(|&b| b != 0).unwrap_or(0) {
                20..=26 => 40,
                27..=35 | 51..=58 => 26,
                36..=39 | 43..=50 => 44,
                40..=42 => 24,
                59..=62 => 52,
                _ => 0,
            }
        };
        if per_fan == 0 || self.fan_count == 0 {
            return Vec::new();
        }
        let mut z = Vec::new();
        if self.is_aio {
            z.push(24);
        }
        z.extend(std::iter::repeat_n(per_fan, self.fan_count as usize));
        z
    }
    fn is_infinity(&self) -> bool {
        matches!(self.fan_types.iter().copied().find(|&b| b != 0).unwrap_or(0), 36..=39 | 43..=50)
    }
    fn is_cl(&self) -> bool {
        matches!(self.fan_types.iter().copied().find(|&b| b != 0).unwrap_or(0), 40..=42 | 126 | 127)
    }
}

fn kind_of(device_type: u8, fan_types: &[u8; 4]) -> (String, bool) {
    match device_type {
        10 => ("HydroShift II Circle".into(), true),
        11 => ("HydroShift II Square".into(), true),
        1..=9 => ("Strimer".into(), false),
        65 => ("Lancool 217".into(), false),
        _ => {
            let b = fan_types.iter().copied().find(|&b| b != 0).unwrap_or(0);
            let k = match b {
                20..=22 => "UNI FAN SL V3",
                23..=26 => "UNI FAN SL V3 LCD",
                27 | 32..=35 => "UNI FAN TL LCD",
                28..=31 => "UNI FAN TL",
                36..=39 => "UNI FAN SL Infinity",
                40..=42 => "UNI FAN CL",
                43..=50 => "UNI FAN SL Infinity V3",
                51..=58 => "UNI FAN TL V3",
                59..=62 => "UNI FAN SL V4",
                _ => "Wireless fan",
            };
            (k.into(), false)
        }
    }
}

pub fn hex(m: &[u8; 6]) -> String {
    hex_mac(m)
}

fn hex_mac(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

pub struct Dongle {
    tx: Bulk,
    rx: Bulk,
    pub master: [u8; 6],
    pub master_channel: u8,
    pub devices: Vec<WirelessDevice>,
    last_frame: Option<([u8; 6], u8, Instant)>,
    pages: u8,
}

impl Dongle {
    pub fn open() -> Result<Self> {
        let tx_dev = crate::usb::find(TX.0, TX.1).into_iter().next().context("wireless TX dongle not found")?;
        let rx_dev = crate::usb::find(RX.0, RX.1).into_iter().next().context("wireless RX dongle not found")?;
        let tx = Bulk::open(&tx_dev)?;
        let rx = Bulk::open(&rx_dev)?;
        rx.flush();
        let mut d = Self { tx, rx, master: [0; 6], master_channel: 8, devices: Vec::new(), last_frame: None, pages: 1 };
        d.discover_master()?;
        Ok(d)
    }

    fn discover_master(&mut self) -> Result<()> {
        let channels = std::iter::once(8u8).chain((2..=38).filter(|c| c % 2 == 0 && *c != 8)).chain((1..=39).filter(|c| c % 2 == 1));
        for ch in channels {
            let mut cmd = [0u8; 64];
            cmd[0] = 0x11;
            cmd[1] = ch;
            let mut stale = [0u8; 64];
            for _ in 0..16 {
                if self.tx.h.read_bulk(crate::usb::EP_IN, &mut stale, Duration::from_millis(5)).is_err() {
                    break;
                }
            }
            if self.tx.h.write_bulk(crate::usb::EP_OUT, &cmd, Duration::from_millis(1000)).is_err() {
                continue;
            }
            let mut r = [0u8; 64];
            let Ok(n) = self.tx.h.read_bulk(crate::usb::EP_IN, &mut r, Duration::from_millis(500)) else { continue };
            if n >= 13 && r[0] == 0x11 && r[1..7].iter().any(|&b| b != 0) && u32::from_be_bytes([r[7], r[8], r[9], r[10]]) > 1 {
                self.master.copy_from_slice(&r[1..7]);
                self.master_channel = ch;
                return Ok(());
            }
        }
        anyhow::bail!("the wireless TX dongle did not report its address")
    }

    /// Ask the RX dongle what it hears. Read-only.
    pub fn poll(&mut self) -> Result<()> {
        let mut cmd = [0u8; 64];
        cmd[0] = 0x10;
        cmd[1] = self.pages;
        self.rx.flush();
        self.rx.write(&cmd, Duration::from_millis(2000))?;
        let mut resp = vec![0u8; 26 * 512];
        let mut total = 0;
        let mut timeout = Duration::from_millis(400);
        while total + 512 <= resp.len() {
            match self.rx.h.read_bulk(crate::usb::EP_IN, &mut resp[total..total + 512], timeout) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    timeout = Duration::from_millis(30);
                    if total >= 4 + resp[1] as usize * 42 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        anyhow::ensure!(total >= 4 && resp[0] == 0x10, "no reply from the wireless RX dongle");
        let count = resp[1] as usize;
        self.pages = (count as u8).div_ceil(10).clamp(1, 26);
        let mut out = Vec::new();
        for i in 0..count.min(self.pages as usize * 10) {
            let o = 4 + i * 42;
            if o + 42 > total {
                break;
            }
            let d = &resp[o..o + 42];
            if d[41] != 0x1C || d[18] == 0xFF || d[0..6].iter().all(|&b| b == 0) {
                continue;
            }
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&d[0..6]);
            let mut master = [0u8; 6];
            master.copy_from_slice(&d[6..12]);
            let raw_count = d[19];
            let (fan_count, inf_right) = if raw_count >= 10 { ((raw_count - 10).min(4), true) } else { (raw_count.min(4), false) };
            let mut fan_types = [0u8; 4];
            fan_types.copy_from_slice(&d[24..28]);
            let rpm = [
                u16::from_be_bytes([d[28] & 0x0F, d[29]]),
                u16::from_be_bytes([d[30] & 0x0F, d[31]]),
                u16::from_be_bytes([d[32] & 0x0F, d[33]]),
                u16::from_be_bytes([d[34] & 0x0F, d[35]]),
            ];
            let mut pwm = [0u8; 4];
            pwm.copy_from_slice(&d[36..40]);
            let (kind, is_aio) = kind_of(d[18], &fan_types);
            out.push(WirelessDevice {
                mac: hex_mac(&mac),
                mac_raw: mac,
                master,
                channel: d[12],
                rx: d[13],
                device_type: d[18],
                fan_count,
                fan_types,
                rpm,
                pwm,
                coolant: (is_aio && d[27] > 0).then_some(d[27]),
                kind,
                is_aio,
                inf_right,
                ours: master == self.master,
                effect_index: [d[20], d[21], d[22], d[23]],
                mb_sync: d[28] & 0x40 != 0,
                cmd_seq: d[40],
            });
        }
        // keep devices that skipped one poll, so a missed record doesn't make a group vanish
        for old in &self.devices {
            if !out.iter().any(|n| n.mac_raw == old.mac_raw) {
                out.push(old.clone());
            }
        }
        out.retain(|d| d.ours);
        self.devices = out;
        Ok(())
    }

    /// Radio group index: 1-based position among the devices bound to this dongle.
    fn slot_of(&self, mac: &[u8; 6]) -> u8 {
        let mut n = 1u8;
        for d in self.devices.iter().filter(|d| d.master == self.master) {
            if &d.mac_raw == mac {
                return n;
            }
            n = n.saturating_add(1);
        }
        n
    }

    /// Set fan duties (0-255) for one device; slot 3 is the pump on a HydroShift.
    /// Returns the duties as the device will report them back once it took them.
    pub fn set_pwm(&mut self, mac: &[u8; 6], duty: [u8; 4]) -> Result<[u8; 4]> {
        let dev = self.devices.iter().find(|d| &d.mac_raw == mac).cloned().context("device not in the wireless list")?;
        let mut pwm = duty;
        let min = ((dev.min_duty_pct() as f32 / 100.0) * 255.0) as u8;
        for (i, v) in pwm.iter_mut().enumerate() {
            let pump_slot = i == 3 && dev.is_aio;
            if i as u8 >= dev.fan_count && !pump_slot {
                *v = 0;
                continue;
            }
            if *v > 0 && *v < min {
                *v = min;
            }
            if dev.is_cl() {
                *v = match *v {
                    153 | 154 => 152,
                    155 => 156,
                    x => x,
                };
            }
        }
        if dev.inf_right && dev.fan_count > 1 {
            pwm[..dev.fan_count as usize].reverse();
        }
        let mut rf = [0u8; 240];
        rf[0] = 0x12;
        rf[1] = 0x10;
        rf[2..8].copy_from_slice(mac);
        rf[8..14].copy_from_slice(&self.master);
        rf[14] = dev.rx;
        let channel = if (1..=39).contains(&dev.channel) { dev.channel } else { self.master_channel };
        rf[15] = channel;
        rf[16] = self.slot_of(mac);
        rf[17..21].copy_from_slice(&pwm);
        self.send_rf(mac, channel, dev.rx, &rf)?;
        Ok(pwm)
    }

    /// One line per device for the diagnostics file.
    pub fn describe(&self) -> String {
        self.devices
            .iter()
            .map(|d| format!("{} {} ch={} rx={} slot={} fans={} pwm={:?} rpm={:?} mb_sync={} effect={:02x?} seq={}", d.mac, d.kind, d.channel, d.rx, self.slot_of(&d.mac_raw), d.fan_count, d.pwm, d.rpm, d.mb_sync, d.effect_index, d.cmd_seq))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Turn the device's "follow motherboard RGB" off so it plays our effect. Returns true if a request was sent.
    pub fn set_mb_sync_off(&mut self, mac: &[u8; 6]) -> Result<bool> {
        let dev = self.devices.iter().find(|d| &d.mac_raw == mac).cloned().context("device not in the wireless list")?;
        if !dev.mb_sync {
            return Ok(false);
        }
        let slave_index = self.devices.iter().filter(|d| d.master == self.master && d.device_type != 0xFF).position(|d| &d.mac_raw == mac).unwrap_or(0) as u8;
        let mut rf = [0u8; 240];
        rf[0] = 0x12;
        rf[1] = 0x27;
        rf[2..8].copy_from_slice(mac);
        rf[8..14].copy_from_slice(&self.master);
        rf[14] = dev.rx;
        rf[15] = self.master_channel;
        rf[16] = slave_index;
        rf[17] = if dev.cmd_seq >= 254 { 1 } else { dev.cmd_seq + 1 };
        rf[20] = 0;
        let channel = if (1..=39).contains(&dev.channel) { dev.channel } else { self.master_channel };
        self.send_rf(mac, channel, dev.rx, &rf)?;
        Ok(true)
    }

    /// Upload a looping lighting animation (frames of RGB per LED, in `led_zones` order).
    /// Returns the effect index the device reports once it is playing it.
    pub fn upload_rgb(&mut self, mac: &[u8; 6], frames: &[Vec<[u8; 3]>], interval_ms: u32) -> Result<[u8; 4]> {
        const CHUNK: usize = 220;
        let dev = self.devices.iter().find(|d| &d.mac_raw == mac).cloned().context("device not in the wireless list")?;
        let zones = dev.led_zones();
        let leds: usize = zones.iter().sum();
        anyhow::ensure!(leds > 0 && leds <= 255, "this device has no lighting Vane knows");
        anyhow::ensure!(!frames.is_empty() && frames.len() <= 4096 && frames.iter().all(|f| f.len() == leds), "lighting frames don't match the device");
        let reverse = (dev.inf_right && dev.is_infinity()).then_some(zones.last().copied().unwrap_or(44));
        let mut raw = Vec::with_capacity(frames.len() * leds * 3);
        for f in frames {
            match reverse {
                Some(n) => f.chunks(n).rev().flatten().for_each(|c| raw.extend_from_slice(c)),
                None => f.iter().for_each(|c| raw.extend_from_slice(c)),
            }
        }
        let comp = crate::tinyuz::compress(&raw)?;
        anyhow::ensure!(comp.len() <= 12_288, "the lighting animation is too big for the fans; try a slower or shorter effect");
        let hundredths = interval_ms.clamp(1, 40_959) * 160;
        let ticks = (hundredths / 100) as u16;
        let fraction = (hundredths % 100) as u8;
        let frame_count = frames.len() as u16;
        let mut hash = 0x811c_9dc5u32;
        for b in comp.iter().copied().chain(ticks.to_be_bytes()).chain([fraction]).chain(frame_count.to_be_bytes()).chain([leds as u8]).chain([0, 0, 0, 0, 0]) {
            hash = (hash ^ b as u32).wrapping_mul(0x0100_0193);
        }
        let mut idx = hash.to_be_bytes();
        idx[0] |= 0x80;
        let packets = comp.len().div_ceil(CHUNK) + 1;
        let channel = if (1..=39).contains(&dev.channel) { dev.channel } else { self.master_channel };
        let started = Instant::now();
        for i in 0..packets {
            anyhow::ensure!(started.elapsed() < Duration::from_secs(3), "the lighting upload took too long");
            let mut p = [0u8; 240];
            p[0] = 0x12;
            p[1] = 0x20;
            p[2..8].copy_from_slice(mac);
            p[8..14].copy_from_slice(&self.master);
            p[14..18].copy_from_slice(&idx);
            p[18] = i as u8;
            p[19] = packets as u8;
            if i == 0 {
                p[20..24].copy_from_slice(&(comp.len() as u32).to_be_bytes());
                p[25..27].copy_from_slice(&frame_count.to_be_bytes());
                p[27] = leds as u8;
                p[32..34].copy_from_slice(&ticks.to_be_bytes());
                p[34] = fraction;
            } else {
                let o = (i - 1) * CHUNK;
                let data = &comp[o..comp.len().min(o + CHUNK)];
                p[20..20 + data.len()].copy_from_slice(data);
            }
            let repeats = if i == 0 { 2 } else { 1 };
            for r in 0..repeats {
                self.send_rf(mac, channel, dev.rx, &p)?;
                if r + 1 < repeats {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        Ok(idx)
    }

    fn send_rf(&mut self, target: &[u8; 6], channel: u8, rx: u8, rf: &[u8; 240]) -> Result<()> {
        if let Some((last, op, at)) = self.last_frame {
            if &last != target || op != rf[1] {
                // the radio needs a rest between frames for different devices or commands (longer after lighting)
                let rest = if op == 0x20 { 10 } else { 5 };
                let gap = Duration::from_millis(rest).saturating_sub(at.elapsed());
                std::thread::sleep(gap);
            }
        }
        for chunk in 0..4u8 {
            let mut p = [0u8; 64];
            p[0] = 0x10;
            p[1] = chunk;
            p[2] = channel;
            p[3] = rx;
            let s = chunk as usize * 60;
            p[4..64].copy_from_slice(&rf[s..s + 60]);
            self.tx.write(&p, Duration::from_millis(2000)).context("sending to the wireless TX dongle")?;
            std::thread::sleep(Duration::from_millis(1));
        }
        self.last_frame = Some((*target, rf[1], Instant::now()));
        Ok(())
    }
}
