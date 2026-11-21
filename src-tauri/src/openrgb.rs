//! OpenRGB support: Vane runs a small OpenRGB SDK server on this PC only (127.0.0.1, port 6743 by default,
//! so OpenRGB's own server keeps 6742). In OpenRGB, add it under "SDK Client" and the wireless fans and pump
//! show up as devices with a Direct mode; colours set there (or by OpenRGB effects) go to the fans.
//!
//! Protocol: OpenRGB network SDK, protocol version 1 (packet = "ORGB", device index, packet id, data size,
//! all little-endian u32, then the data).

use crate::engine::{DirectRgb, Engine};
use crate::wireless::WirelessDevice;
use parking_lot::Mutex;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const PROTOCOL: u32 = 1;
static VERSION: AtomicU64 = AtomicU64::new(1);

pub fn start(e: Arc<Engine>) {
    let (on, port) = {
        let c = e.cfg.read();
        (c.openrgb_server, c.openrgb_port)
    };
    if !on {
        e.status.write().openrgb = "Off".into();
        return;
    }
    std::thread::Builder::new()
        .name("openrgb".into())
        .spawn(move || match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                e.status.write().openrgb = format!("Ready on 127.0.0.1:{port}");
                for s in l.incoming().flatten() {
                    let e = e.clone();
                    std::thread::spawn(move || {
                        e.status.write().openrgb_clients += 1;
                        let _ = client(&e, s);
                        let mut st = e.status.write();
                        st.openrgb_clients = st.openrgb_clients.saturating_sub(1);
                    });
                }
            }
            Err(err) => e.status.write().openrgb = format!("Couldn't use port {port}: {err}"),
        })
        .ok();
}

/// Devices OpenRGB sees, in a stable order.
fn devices(e: &Engine) -> Vec<WirelessDevice> {
    let mut v: Vec<WirelessDevice> = e.status.read().wireless.iter().filter(|d| !d.led_zones().is_empty()).cloned().collect();
    v.sort_by(|a, b| a.mac.cmp(&b.mac));
    v
}

fn header(dev: u32, id: u32, len: usize) -> Vec<u8> {
    let mut h = b"ORGB".to_vec();
    h.extend(dev.to_le_bytes());
    h.extend(id.to_le_bytes());
    h.extend((len as u32).to_le_bytes());
    h
}

fn send(w: &Mutex<TcpStream>, dev: u32, id: u32, data: &[u8]) -> std::io::Result<()> {
    let mut p = header(dev, id, data.len());
    p.extend_from_slice(data);
    w.lock().write_all(&p)
}

fn client(e: &Arc<Engine>, mut s: TcpStream) -> std::io::Result<()> {
    s.set_nodelay(true)?;
    let w = Arc::new(Mutex::new(s.try_clone()?));
    let done = Arc::new(AtomicBool::new(false));
    // tell OpenRGB when devices appear or go (the dongle can show up after OpenRGB connects)
    {
        let (e, w, done) = (e.clone(), w.clone(), done.clone());
        std::thread::spawn(move || {
            let mut known: Vec<String> = devices(&e).iter().map(|d| d.mac.clone()).collect();
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(2));
                let now: Vec<String> = devices(&e).iter().map(|d| d.mac.clone()).collect();
                if now != known {
                    known = now;
                    if send(&w, 0, 100, &[]).is_err() {
                        break;
                    }
                }
            }
        });
    }
    let r = (|| -> std::io::Result<()> {
        loop {
            let mut h = [0u8; 16];
            s.read_exact(&mut h)?;
            if &h[0..4] != b"ORGB" {
                return Err(std::io::Error::other("not an OpenRGB client"));
            }
            let dev = u32::from_le_bytes(h[4..8].try_into().unwrap());
            let id = u32::from_le_bytes(h[8..12].try_into().unwrap());
            let len = u32::from_le_bytes(h[12..16].try_into().unwrap()) as usize;
            if len > 1 << 20 {
                return Err(std::io::Error::other("packet too large"));
            }
            let mut data = vec![0u8; len];
            s.read_exact(&mut data)?;
            let list = devices(e);
            match id {
                0 => send(&w, 0, 0, &(list.len() as u32).to_le_bytes())?,
                1 => {
                    if let Some(d) = list.get(dev as usize) {
                        send(&w, dev, 1, &describe(e, d))?;
                    }
                }
                40 => send(&w, 0, 40, &PROTOCOL.to_le_bytes())?,
                1050 => {
                    // u32 size, u16 count, colours
                    if let Some(d) = list.get(dev as usize) {
                        let cols = colours(&data, 4);
                        set(e, d, |buf| {
                            for (i, c) in cols.iter().enumerate().take(buf.len()) {
                                buf[i] = *c;
                            }
                        });
                    }
                }
                1051 => {
                    // u32 size, u32 zone, u16 count, colours
                    if let (Some(d), true) = (list.get(dev as usize), data.len() >= 8) {
                        let zone = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
                        let zones = d.led_zones();
                        if zone < zones.len() {
                            let start: usize = zones[..zone].iter().sum();
                            let cols = colours(&data, 8);
                            set(e, d, |buf| {
                                for (i, c) in cols.iter().enumerate().take(zones[zone]) {
                                    buf[start + i] = *c;
                                }
                            });
                        }
                    }
                }
                1052 => {
                    // i32 led, u32 colour
                    if let (Some(d), true) = (list.get(dev as usize), data.len() >= 8) {
                        let i = i32::from_le_bytes(data[0..4].try_into().unwrap());
                        let c = u32::from_le_bytes(data[4..8].try_into().unwrap());
                        set(e, d, |buf| {
                            if i >= 0 && (i as usize) < buf.len() {
                                buf[i as usize] = rgb(c);
                            }
                        });
                    }
                }
                // set client name, resize zone, set custom mode, update mode, save mode: nothing to do
                _ => {}
            }
        }
    })();
    done.store(true, Ordering::Relaxed);
    r
}

fn rgb(c: u32) -> [u8; 3] {
    [(c & 0xFF) as u8, (c >> 8 & 0xFF) as u8, (c >> 16 & 0xFF) as u8]
}

fn colours(data: &[u8], at: usize) -> Vec<[u8; 3]> {
    if data.len() < at + 2 {
        return Vec::new();
    }
    let n = u16::from_le_bytes([data[at], data[at + 1]]) as usize;
    data[at + 2..].chunks_exact(4).take(n).map(|c| rgb(u32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect()
}

/// Change a device's OpenRGB colours. The first colours from OpenRGB switch that device's effect to "OpenRGB".
fn set(e: &Engine, d: &WirelessDevice, f: impl FnOnce(&mut Vec<[u8; 3]>)) {
    let total: usize = d.led_zones().iter().sum();
    {
        let mut m = e.rgb_direct.lock();
        let x = m.entry(d.mac.clone()).or_insert_with(|| DirectRgb { colors: vec![[0, 0, 0]; total], version: 0 });
        x.colors.resize(total, [0, 0, 0]);
        f(&mut x.colors);
        x.version = VERSION.fetch_add(1, Ordering::Relaxed);
    }
    let mut c = e.cfg.write();
    let l = c.lighting.entry(d.mac.clone()).or_default();
    if l.effect != "openrgb" {
        l.effect = "openrgb".into();
        drop(c);
        e.cfg_changed.store(true, Ordering::Relaxed);
    }
}

fn s16(b: &mut Vec<u8>, s: &str) {
    b.extend(((s.len() + 1) as u16).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
    b.push(0);
}

fn zone_names(d: &WirelessDevice) -> Vec<String> {
    let zones = d.led_zones();
    let mut names = Vec::new();
    for i in 0..zones.len() {
        names.push(if d.is_aio && i == 0 { "Pump".to_string() } else { format!("Fan {}", if d.is_aio { i } else { i + 1 }) });
    }
    names
}

/// Controller description, protocol 1.
fn describe(e: &Engine, d: &WirelessDevice) -> Vec<u8> {
    let zones = d.led_zones();
    let names = zone_names(d);
    let total: usize = zones.iter().sum();
    let current = e.rgb_direct.lock().get(&d.mac).map(|x| x.colors.clone()).unwrap_or_default();
    let mut b = Vec::new();
    b.extend(3i32.to_le_bytes()); // device type: cooler
    s16(&mut b, &format!("Lian Li {}", if d.is_aio { "HydroShift II" } else { d.kind.trim_start_matches("UNI FAN ") }));
    s16(&mut b, "Lian Li");
    s16(&mut b, "Wireless, through Vane");
    s16(&mut b, env!("CARGO_PKG_VERSION"));
    s16(&mut b, &d.mac);
    s16(&mut b, "Vane wireless dongle");
    // one mode: Direct (per-LED colour)
    b.extend(1u16.to_le_bytes());
    b.extend(0i32.to_le_bytes());
    s16(&mut b, "Direct");
    b.extend(0i32.to_le_bytes()); // value
    b.extend((1u32 << 5).to_le_bytes()); // flags: per-LED colour
    for v in [0u32, 0, 0, 0, 0, 0] {
        b.extend(v.to_le_bytes()); // speed min/max, colours min/max, speed, direction
    }
    b.extend(1u32.to_le_bytes()); // colour mode: per LED
    b.extend(0u16.to_le_bytes()); // mode colours
    // zones
    b.extend((zones.len() as u16).to_le_bytes());
    for (z, &n) in zones.iter().enumerate() {
        s16(&mut b, &names[z]);
        b.extend(1i32.to_le_bytes()); // linear
        for v in [n as u32, n as u32, n as u32] {
            b.extend(v.to_le_bytes());
        }
        b.extend(0u16.to_le_bytes()); // no matrix
    }
    // LEDs
    b.extend((total as u16).to_le_bytes());
    for (z, &n) in zones.iter().enumerate() {
        for i in 0..n {
            s16(&mut b, &format!("{} LED {}", names[z], i + 1));
            b.extend(0u32.to_le_bytes());
        }
    }
    // colours
    b.extend((total as u16).to_le_bytes());
    for i in 0..total {
        let c = current.get(i).copied().unwrap_or([0, 0, 0]);
        b.extend((c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16).to_le_bytes());
    }
    let mut out = ((b.len() + 4) as u32).to_le_bytes().to_vec();
    out.extend(b);
    out
}
