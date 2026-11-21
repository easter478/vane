//! Lighting for the rest of the PC (motherboard, memory, graphics card, strips…) through OpenRGB.
//! Vane connects to OpenRGB's own SDK server (127.0.0.1:6742), puts each device in its Direct mode
//! and streams the same effects the fans use. OpenRGB does the hardware side, so anything it supports works.
//!
//! The "pc" entry in Config.lighting holds the effect; "auto" leaves every device alone.
//! Protocol: OpenRGB network SDK, version 3 asked for (no zone segments, which came in 4).

use crate::engine::Engine;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PORT: u16 = 6742;
const WANT_PROTOCOL: u32 = 3;
pub const KEY: &str = "pc";

#[derive(Clone, Debug, Serialize, Default)]
pub struct PcDevice {
    pub name: String,
    /// "Motherboard", "Memory", "Graphics card", ...
    pub kind: String,
    pub leds: usize,
    /// whether Vane lights it now
    pub on: bool,
    #[serde(skip)]
    pub kind_id: i32,
}

struct Dev {
    index: u32,
    name: String,
    kind: i32,
    zones: Vec<usize>,
    leds: usize,
}

/// Motherboard, memory, graphics card, cooler, LED strip and case are lit unless left out. Everything
/// else (keyboards, mice, a Stream Deck whose keys would lose their icons…) only when switched on.
fn on_by_default(t: i32) -> bool {
    matches!(t, 0 | 1 | 2 | 3 | 4 | 15)
}

/// Whether Vane lights this device, from the user's left-out and added lists.
fn is_on(kind: i32, name: &str, skip: &[String], include: &[String]) -> bool {
    if on_by_default(kind) { !skip.iter().any(|s| s == name) } else { include.iter().any(|s| s == name) }
}

fn kind_name(t: i32) -> &'static str {
    match t {
        0 => "Motherboard",
        1 => "Memory",
        2 => "Graphics card",
        3 => "Cooler",
        4 => "LED strip",
        5 => "Keyboard",
        6 => "Mouse",
        7 => "Mouse mat",
        8 => "Headset",
        9 => "Headset stand",
        10 => "Gamepad",
        11 => "Light",
        12 => "Speaker",
        13 => "Virtual",
        14 => "Storage",
        15 => "Case",
        16 => "Microphone",
        17 => "Accessory",
        18 => "Keypad",
        _ => "Other",
    }
}

pub fn start(e: Arc<Engine>) {
    std::thread::Builder::new().name("openrgb-client".into()).spawn(move || run(e)).ok();
}

fn set_state(e: &Engine, s: &str, devs: Option<Vec<PcDevice>>) {
    let mut st = e.status.write();
    st.pc_rgb = s.to_string();
    if let Some(d) = devs {
        st.pc_devices = d;
    }
}

fn run(e: Arc<Engine>) {
    let mut launched = false;
    loop {
        let (on, effect, launch) = {
            let c = e.cfg.read();
            (c.openrgb_client, c.lighting.get(KEY).map(|l| l.effect.clone()).unwrap_or_else(|| "auto".into()), c.openrgb_launch)
        };
        if !on {
            set_state(&e, "Off", Some(vec![]));
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        let addr: SocketAddr = ([127, 0, 0, 1], PORT).into();
        match TcpStream::connect_timeout(&addr, Duration::from_millis(800)) {
            Ok(s) => {
                if let Err(err) = session(&e, s) {
                    set_state(&e, &format!("Lost OpenRGB: {err}"), Some(vec![]));
                }
            }
            Err(_) => {
                let exe = find_openrgb();
                // starting OpenRGB is only worth it once there's an effect to show
                if launch && !launched && effect != "auto" {
                    if let Some(p) = &exe {
                        launched = true;
                        let _ = start_hidden(p);
                        set_state(&e, "Starting OpenRGB…", None);
                        std::thread::sleep(Duration::from_secs(8));
                        continue;
                    }
                }
                let msg = match (&exe, launched) {
                    (None, _) => "OpenRGB isn't installed",
                    (Some(_), true) => "OpenRGB is running without its SDK server. Turn on the server in OpenRGB's SDK Server tab.",
                    (Some(_), false) => "OpenRGB isn't running",
                };
                set_state(&e, msg, Some(vec![]));
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Where winget, the installer and the portable zip usually put OpenRGB.
pub fn find_openrgb() -> Option<std::path::PathBuf> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let mut c: Vec<std::path::PathBuf> = vec![
        format!("{}\\OpenRGB\\OpenRGB.exe", env("ProgramFiles")).into(),
        format!("{}\\OpenRGB\\OpenRGB.exe", env("ProgramFiles(x86)")).into(),
        format!("{}\\Programs\\OpenRGB\\OpenRGB.exe", env("LOCALAPPDATA")).into(),
        format!("{}\\Microsoft\\WinGet\\Links\\OpenRGB.exe", env("LOCALAPPDATA")).into(),
    ];
    if let Ok(rd) = std::fs::read_dir(format!("{}\\Microsoft\\WinGet\\Packages", env("LOCALAPPDATA"))) {
        for d in rd.flatten().filter(|d| d.file_name().to_string_lossy().starts_with("OpenRGB.OpenRGB")) {
            c.push(d.path().join("OpenRGB.exe"));
            if let Ok(sub) = std::fs::read_dir(d.path()) {
                c.extend(sub.flatten().map(|s| s.path().join("OpenRGB.exe")));
            }
        }
    }
    c.into_iter().find(|p| p.is_file())
}

fn start_hidden(p: &std::path::Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(p);
    cmd.args(["--server", "--startminimized"]);
    if let Some(dir) = p.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map(|_| ())
}

struct Conn {
    s: TcpStream,
    proto: u32,
    /// OpenRGB said its device list changed; read it again
    stale: bool,
}

impl Conn {
    fn send(&mut self, dev: u32, id: u32, data: &[u8]) -> std::io::Result<()> {
        let mut p = b"ORGB".to_vec();
        p.extend(dev.to_le_bytes());
        p.extend(id.to_le_bytes());
        p.extend((data.len() as u32).to_le_bytes());
        p.extend_from_slice(data);
        self.s.write_all(&p)
    }
    fn recv(&mut self, want: u32) -> std::io::Result<Vec<u8>> {
        loop {
            let (id, d) = self.recv_any()?;
            if id == want {
                return Ok(d);
            }
        }
    }
    /// One packet, whatever it is. Device-list changes (100) mark the list stale.
    fn recv_any(&mut self) -> std::io::Result<(u32, Vec<u8>)> {
        {
            let mut h = [0u8; 16];
            self.s.read_exact(&mut h)?;
            if &h[0..4] != b"ORGB" {
                return Err(std::io::Error::other("not an OpenRGB reply"));
            }
            let id = u32::from_le_bytes(h[8..12].try_into().unwrap());
            let len = u32::from_le_bytes(h[12..16].try_into().unwrap()) as usize;
            if len > 8 << 20 {
                return Err(std::io::Error::other("reply too big"));
            }
            let mut d = vec![0u8; len];
            self.s.read_exact(&mut d)?;
            if id == 100 {
                self.stale = true;
            }
            Ok((id, d))
        }
    }
}

/// Reads OpenRGB's controller description, keeping what Vane needs.
struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}
impl Rd<'_> {
    fn u16(&mut self) -> Option<u16> {
        let v = u16::from_le_bytes(self.b.get(self.i..self.i + 2)?.try_into().ok()?);
        self.i += 2;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_le_bytes(self.b.get(self.i..self.i + 4)?.try_into().ok()?);
        self.i += 4;
        Some(v)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        (self.i + n <= self.b.len()).then(|| self.i += n)
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(String::from_utf8_lossy(s).trim_end_matches('\0').to_string())
    }
}

/// (type, name, description, location, zone LED counts, LED count)
fn parse(b: &[u8], proto: u32) -> Option<(i32, String, String, String, Vec<usize>, usize)> {
    let mut r = Rd { b, i: 0 };
    r.u32()?; // size
    let kind = r.u32()? as i32;
    let name = r.str()?;
    if proto >= 1 {
        r.str()?; // vendor
    }
    let desc = r.str()?;
    r.str()?; // version
    r.str()?; // serial
    let loc = r.str()?;
    let modes = r.u16()?;
    r.u32()?; // active mode
    for _ in 0..modes {
        r.str()?; // mode name
        r.skip(4 * 4)?; // value, flags, speed min/max
        if proto >= 3 {
            r.skip(4 * 2)?; // brightness min/max
        }
        r.skip(4 * 3)?; // colours min/max, speed
        if proto >= 3 {
            r.skip(4)?; // brightness
        }
        r.skip(4 * 2)?; // direction, colour mode
        let nc = r.u16()? as usize;
        r.skip(nc * 4)?;
    }
    let nz = r.u16()?;
    let mut zones = Vec::new();
    for _ in 0..nz {
        r.str()?;
        r.skip(4 * 3)?; // type, leds min, leds max
        zones.push(r.u32()? as usize);
        let ml = r.u16()? as usize;
        if ml > 0 {
            r.skip(ml)?; // matrix: height, width and the map; ml is its size in bytes
        }
    }
    let leds = r.u16()? as usize;
    Some((kind, name, desc, loc, zones, leds))
}

fn enumerate(c: &mut Conn) -> std::io::Result<(Vec<Dev>, Vec<PcDevice>)> {
    c.send(0, 0, &[])?;
    let n = c.recv(0)?;
    let count = u32::from_le_bytes(n.get(0..4).and_then(|s| s.try_into().ok()).unwrap_or([0; 4])).min(256);
    let mut devs = Vec::new();
    let mut names = Vec::new();
    for i in 0..count {
        c.send(i, 1, &c.proto.to_le_bytes())?;
        let d = c.recv(1)?;
        let Some((kind, mut name, desc, loc, zones, leds)) = parse(&d, c.proto) else { continue };
        // two memory sticks are both "ENE DRAM": number repeats so each can be switched on or off
        let same = names.iter().filter(|p: &&PcDevice| p.name == name || p.name.starts_with(&format!("{name} "))).count();
        if same > 0 {
            name = format!("{name} {}", same + 1);
        }
        // Vane's own fans, when OpenRGB is also connected to Vane's server: never loop them back
        if desc.contains("through Vane") || loc.contains("Vane") {
            continue;
        }
        crate::config::log_to("openrgb.log", &format!("device {i}: {} \"{name}\", {leds} LEDs, zones {zones:?}", kind_name(kind)));
        names.push(PcDevice { kind: kind_name(kind).into(), name: name.clone(), leds, on: true, kind_id: kind });
        devs.push(Dev { index: i, name, kind, zones: if zones.iter().sum::<usize>() == leds { zones } else { vec![leds] }, leds });
    }
    Ok((devs, names))
}

fn session(e: &Engine, s: TcpStream) -> std::io::Result<()> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(4)))?;
    let mut c = Conn { s, proto: 0, stale: false };
    c.send(0, 40, &WANT_PROTOCOL.to_le_bytes())?;
    // OpenRGB older than 0.5 never answers this; it then speaks protocol 0
    c.proto = match c.recv(40) {
        Ok(v) => u32::from_le_bytes(v.get(0..4).and_then(|s| s.try_into().ok()).unwrap_or([0; 4])).min(WANT_PROTOCOL),
        Err(_) => 0,
    };
    c.send(0, 50, b"Vane\0")?;
    let mut devs: Vec<Dev> = Vec::new();
    let mut listed_at = Instant::now() - Duration::from_secs(60);
    let mut driving: Vec<u32> = Vec::new(); // devices already switched to Direct
    let mut last_frame: Vec<(u32, usize, u64)> = Vec::new(); // (device, frame, signature)
    let mut last_sent = Instant::now() - Duration::from_secs(60);
    let t0 = Instant::now();
    loop {
        if c.stale || listed_at.elapsed() > Duration::from_secs(30) {
            c.stale = false;
            let (d, list) = enumerate(&mut c)?;
            devs = d;
            listed_at = Instant::now();
            driving.clear();
            last_frame.clear();
            let (skip, include) = {
                let c = e.cfg.read();
                (c.openrgb_skip.clone(), c.openrgb_include.clone())
            };
            let list: Vec<PcDevice> = list.into_iter().map(|mut p| {
                p.on = is_on(p.kind_id, &p.name, &skip, &include);
                p
            }).collect();
            let n = list.len();
            set_state(e, &format!("Connected to OpenRGB, {n} device{}", if n == 1 { "" } else { "s" }), Some(list));
        }
        let (l, skip, include) = {
            let cfg = e.cfg.read();
            (cfg.lighting.get(KEY).cloned().unwrap_or_default(), cfg.openrgb_skip.clone(), cfg.openrgb_include.clone())
        };
        // update the on/off flags without waiting for the next listing
        {
            let mut st = e.status.write();
            st.pc_devices.iter_mut().for_each(|p| p.on = is_on(p.kind_id, &p.name, &skip, &include));
        }
        if l.effect == "auto" || l.effect == "openrgb" {
            driving.clear();
            last_frame.clear();
            // keep the connection alive and notice device changes
            std::thread::sleep(Duration::from_millis(500));
            if last_sent.elapsed() > Duration::from_secs(10) {
                c.send(0, 0, &[])?;
                c.recv(0)?;
                last_sent = Instant::now();
            }
            continue;
        }
        let temp = (l.effect == "temperature").then(|| e.sensors.read().get("vane.hottest").map(|r| (r.value / 3.0).round() * 3.0)).flatten();
        let sig = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            l.hash(&mut h);
            temp.map(|t| t as i64).hash(&mut h);
            h.finish()
        };
        let now_ms = t0.elapsed().as_millis() as u64;
        let mut wait_ms = 500u64;
        for d in devs.iter().filter(|d| d.leds > 0 && is_on(d.kind, &d.name, &skip, &include)) {
            let Some(anim) = crate::lighting::build_zones(&d.zones, &l, temp, 1) else { continue };
            let n = anim.frames.len().max(1);
            let iv = anim.interval_ms.max(10) as u64;
            // stream no faster than 30 frames a second; still effects are refreshed every 2 s
            let step = iv.max(33);
            let frame = ((now_ms / step) as usize * (step / iv).max(1) as usize) % n;
            wait_ms = wait_ms.min(if n > 1 { step } else { 500 });
            let prev = last_frame.iter().position(|x| x.0 == d.index);
            let unchanged = prev.map(|p| last_frame[p].1 == frame && last_frame[p].2 == sig).unwrap_or(false);
            if unchanged && (n > 1 || last_sent.elapsed() < Duration::from_secs(2)) {
                continue;
            }
            if !driving.contains(&d.index) {
                c.send(d.index, 1100, &[])?; // RGBCONTROLLER_SETCUSTOMMODE
                driving.push(d.index);
                crate::config::log_to("openrgb.log", &format!("lighting \"{}\" with {}", d.name, l.effect));
            }
            let colors = &anim.frames[frame];
            let mut data = Vec::with_capacity(6 + d.leds * 4);
            data.extend(((6 + d.leds * 4) as u32).to_le_bytes());
            data.extend((d.leds as u16).to_le_bytes());
            for i in 0..d.leds {
                let c = colors.get(i).copied().unwrap_or([0, 0, 0]);
                data.extend([c[0], c[1], c[2], 0]);
            }
            c.send(d.index, 1050, &data)?; // RGBCONTROLLER_UPDATELEDS
            match prev {
                Some(p) => last_frame[p] = (d.index, frame, sig),
                None => last_frame.push((d.index, frame, sig)),
            }
            last_sent = Instant::now();
        }
        // OpenRGB doesn't answer LED updates; drain anything it sent (device-list changes)
        c.s.set_nonblocking(true)?;
        let mut peek = [0u8; 16];
        let pending = matches!(c.s.peek(&mut peek), Ok(n) if n > 0);
        c.s.set_nonblocking(false)?;
        if pending {
            c.recv_any()?;
        }
        std::thread::sleep(Duration::from_millis(wait_ms.clamp(15, 500)));
    }
}
