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

fn hex_mac(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

pub struct Dongle {
    tx: Bulk,
    rx: Bulk,
    pub master: [u8; 6],
    pub master_channel: u8,
    pub devices: Vec<WirelessDevice>,
    last_frame: Option<([u8; 6], Instant)>,
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
    pub fn set_pwm(&mut self, mac: &[u8; 6], duty: [u8; 4]) -> Result<()> {
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
        self.send_rf(mac, channel, dev.rx, &rf)
    }

    fn send_rf(&mut self, target: &[u8; 6], channel: u8, rx: u8, rf: &[u8; 240]) -> Result<()> {
        if let Some((last, at)) = self.last_frame {
            if &last != target {
                let gap = Duration::from_millis(5).saturating_sub(at.elapsed());
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
        self.last_frame = Some((*target, Instant::now()));
        Ok(())
    }
}
