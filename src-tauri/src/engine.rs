//! The background engine: finds the devices, drives each screen at its frame rate, polls the wireless
//! fans and applies fan curves, and keeps the sensor hub fresh. Runs whether or not the window is open.

use crate::config::{curve_at, Config, FanGroup, ScreenConfig};
use crate::render::{self, Fonts, ScreenState};
use crate::sensors::{self, Sensors};
use crate::usb::{self, CoolerLcd, FanLcd};
use crate::wireless::{Dongle, WirelessDevice};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Default)]
pub struct ScreenStatus {
    pub key: String,
    pub name: String,
    pub kind: String, // "cooler" | "fan"
    pub size: u32,
    pub round: bool,
    pub connected: bool,
    pub firmware: Option<String>,
    pub error: Option<String>,
    pub fps: f32,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Status {
    pub screens: Vec<ScreenStatus>,
    pub wireless: Vec<WirelessDevice>,
    pub wireless_error: Option<String>,
    pub lconnect_running: bool,
    pub memory_mb: f64,
    pub paused: bool,
    /// lighting state per wireless MAC, in words ("Playing", or what went wrong)
    pub lighting: BTreeMap<String, String>,
    /// OpenRGB server state, in words
    pub openrgb: String,
    pub openrgb_clients: u32,
    /// OpenRGB connection for the rest of the PC's lighting, in words, and what it found
    pub pc_rgb: String,
    pub pc_devices: Vec<crate::openrgb_client::PcDevice>,
}

/// Colours OpenRGB sent for one device, with a change counter.
#[derive(Clone, Default)]
pub struct DirectRgb {
    pub colors: Vec<[u8; 3]>,
    pub version: u64,
}

struct LightState {
    sig: u64,
    idx: [u8; 4],
    at: Instant,
    tries: u8,
    mb_off_at: Option<Instant>,
    confirmed: bool,
}

/// One line in %APPDATA%\Vane\lighting.log (kept short), so lighting problems can be traced.
fn light_log(msg: &str) {
    use std::io::Write;
    let p = crate::config::dir().join("lighting.log");
    if std::fs::metadata(&p).map(|m| m.len() > 200_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

pub struct Engine {
    pub cfg: Arc<RwLock<Config>>,
    pub sensors: Sensors,
    pub status: Arc<RwLock<Status>>,
    pub previews: Arc<RwLock<HashMap<String, Arc<Vec<u8>>>>>,
    pub identify: Arc<Mutex<HashMap<String, (u8, Instant)>>>,
    /// While true every device is let go of (L-Connect turned back on).
    pub paused: Arc<AtomicBool>,
    pub cfg_changed: Arc<AtomicBool>,
    pub rgb_direct: Arc<Mutex<HashMap<String, DirectRgb>>>,
}

fn is_static(c: &ScreenConfig) -> bool {
    matches!(c.background.as_str(), "carbon" | "solid") || (c.background == "image" && !c.image.to_lowercase().ends_with(".gif"))
}

impl Engine {
    pub fn start() -> Arc<Engine> {
        let e = Arc::new(Engine {
            cfg: Arc::new(RwLock::new(Config::load())),
            sensors: Arc::new(RwLock::new(Default::default())),
            status: Arc::new(RwLock::new(Status::default())),
            previews: Arc::new(RwLock::new(HashMap::new())),
            identify: Arc::new(Mutex::new(HashMap::new())),
            paused: Arc::new(AtomicBool::new(false)),
            cfg_changed: Arc::new(AtomicBool::new(false)),
            rgb_direct: Arc::new(Mutex::new(HashMap::new())),
        });
        // sensors, once a second
        {
            let e = e.clone();
            std::thread::Builder::new().name("sensors".into()).spawn(move || {
                let mut cpu = sensors::CpuMeter::default();
                let mut tools_at: Option<Instant> = None;
                loop {
                    sensors::poll(&e.sensors, &mut cpu);
                    // MSI Afterburner gives the CPU temperature; start it if it was closed (at most every 5 minutes)
                    let ab = e.sensors.read().sources.afterburner;
                    if !ab && e.cfg.read().start_afterburner && tools_at.map_or(true, |t| t.elapsed() > Duration::from_secs(300)) {
                        tools_at = Some(Instant::now());
                        crate::system::start_monitoring_tools();
                    }
                    {
                        let mut st = e.status.write();
                        st.memory_mb = crate::system::memory_mb();
                        st.lconnect_running = crate::system::lconnect_running();
                        st.paused = e.paused.load(Ordering::Relaxed);
                    }
                    if e.cfg_changed.swap(false, Ordering::Relaxed) {
                        e.cfg.read().save();
                    }
                    std::thread::sleep(Duration::from_millis(1000));
                }
            }).ok();
        }
        // screen manager: finds screens and starts one worker per screen
        {
            let e = e.clone();
            std::thread::Builder::new().name("screens".into()).spawn(move || e.screen_manager()).ok();
        }
        // wireless fans
        {
            let e = e.clone();
            std::thread::Builder::new().name("wireless".into()).spawn(move || e.wireless_loop()).ok();
        }
        crate::openrgb::start(e.clone());
        crate::openrgb_client::start(e.clone());
        e
    }

    fn set_screen_status(&self, s: ScreenStatus) {
        let mut st = self.status.write();
        if let Some(x) = st.screens.iter_mut().find(|x| x.key == s.key) {
            *x = s;
        } else {
            st.screens.push(s);
            st.screens.sort_by(|a, b| (a.kind != "cooler", a.name.clone()).cmp(&(b.kind != "cooler", b.name.clone())));
        }
    }

    fn screen_manager(self: Arc<Self>) {
        let running: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        // the cooler worker always runs so its preview works before the device shows up
        self.spawn_worker("cooler".into(), running.clone());
        loop {
            if !self.paused.load(Ordering::Relaxed) {
                let mut fans: Vec<(String, Vec<u8>)> = usb::find(usb::VID_LCD, usb::PID_FAN_LCD)
                    .iter()
                    .map(|d| (usb::port_key(d), d.port_numbers().unwrap_or_default()))
                    .collect();
                fans.sort_by(|a, b| a.1.cmp(&b.1));
                {
                    let mut cfg = self.cfg.write();
                    let mut changed = false;
                    for (i, (serial, _)) in fans.iter().enumerate() {
                        if !cfg.fan_screen_names.contains_key(serial) {
                            cfg.fan_screen_names.insert(serial.clone(), format!("LCD fan {}", i + 1));
                            changed = true;
                        }
                    }
                    if changed {
                        self.cfg_changed.store(true, Ordering::Relaxed);
                    }
                }
                for (serial, _) in fans {
                    if !running.lock().contains(&serial) {
                        self.spawn_worker(serial, running.clone());
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    }

    fn spawn_worker(self: &Arc<Self>, key: String, running: Arc<Mutex<HashSet<String>>>) {
        running.lock().insert(key.clone());
        let e = self.clone();
        std::thread::Builder::new()
            .name(format!("screen-{key}"))
            .spawn(move || {
                e.screen_worker(&key);
                running.lock().remove(&key);
            })
            .ok();
    }

    fn screen_worker(&self, key: &str) {
        let is_cooler = key == "cooler";
        let (size, max_payload) = if is_cooler { (usb::COOLER_LCD_SIZE, 150_000) } else { (usb::FAN_LCD_SIZE, 101_000) };
        let mut fonts = Fonts::load();
        let mut state = ScreenState::new(key.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211)));
        let mut cooler: Option<CoolerLcd> = None;
        let mut fan: Option<FanLcd> = None;
        let mut last_open_try = Instant::now() - Duration::from_secs(60);
        let mut last_bright: Option<u8> = None;
        let mut bright_at = Instant::now();
        // test hook: %APPDATA%\Vane\fan-brightness-test.txt holding 0-100 overrides the fan backlight
        // (and turns off the picture dimming) so the panel's own backlight can be checked by eye
        let mut test_at = Instant::now() - Duration::from_secs(5);
        let mut test_bright: Option<u8> = None;
        let mut last_telemetry = Instant::now();
        let mut error: Option<String> = None;
        let started = Instant::now();
        let mut frames = 0u32;
        let mut fps_window = Instant::now();
        let mut fps_measured = 0.0f32;
        let mut misses = 0u32;
        loop {
            let t0 = Instant::now();
            let paused = self.paused.load(Ordering::Relaxed);
            let cfg_all = self.cfg.read().clone();
            let cfg = cfg_all.screen(key, "matrix-rain");
            let name = if is_cooler { "Cooler screen".to_string() } else { cfg_all.fan_screen_names.get(key).cloned().unwrap_or_else(|| "LCD fan".into()) };
            let send = cfg_all.screens_on && cfg.enabled && !paused;

            // open or drop the device
            if paused {
                cooler = None;
                fan = None;
            } else if cooler.is_none() && fan.is_none() && last_open_try.elapsed() > Duration::from_secs(5) {
                last_open_try = Instant::now();
                let pid = if is_cooler { usb::PID_COOLER_LCD } else { usb::PID_FAN_LCD };
                let dev = usb::find(usb::VID_LCD, pid).into_iter().find(|d| is_cooler || usb::port_key(d) == key);
                match dev {
                    None if !is_cooler => return, // fan screen unplugged; the manager restarts us when it's back
                    None => error = Some("Not found on USB".into()),
                    Some(d) => {
                        let r = if is_cooler { CoolerLcd::open(&d).map(|c| cooler = Some(c)) } else { FanLcd::open(&d).map(|f| fan = Some(f)) };
                        match r {
                            Ok(()) => {
                                error = None;
                                last_bright = None;
                            }
                            Err(err) => error = Some(if crate::system::lconnect_running() { "In use by L-Connect".into() } else { format!("{err:#}") }),
                        }
                    }
                }
            }

            if let Some((n, at)) = self.identify.lock().remove(key) {
                state.identify = Some((n, at));
            }

            let sensors = self.sensors.read();
            let pm = render::render(&mut fonts, &mut state, &cfg, &sensors, size, !is_cooler, started.elapsed().as_secs_f32(), Some(&cfg_all.alerts));
            if !is_cooler && test_at.elapsed() > Duration::from_secs(1) {
                test_at = Instant::now();
                test_bright = std::fs::read_to_string(crate::config::dir().join("fan-brightness-test.txt")).ok().and_then(|s| s.trim().parse::<u8>().ok()).map(|v| v.min(100));
            }
            let want_bright = test_bright.unwrap_or(cfg.brightness);
            // (no picture darkening here: on these single-backlight panels it only lowers contrast,
            // because black stays at the backlight's glow; the backlight command does the dimming)
            drop(sensors);
            let mut jpeg = render::jpeg(&pm, max_payload);

            let off = !send;
            if off && (cooler.is_some() || fan.is_some()) {
                // screens switched off: show black
                let black = tiny_skia::Pixmap::new(size, size).unwrap();
                jpeg = render::jpeg(&black, max_payload);
            }
            if !paused {
                // sent again now and then too: a panel that reset itself comes back at full brightness
                if last_bright != Some(want_bright) || bright_at.elapsed() > Duration::from_secs(20) {
                    bright_at = Instant::now();
                    let changed = last_bright != Some(want_bright);
                    let r = if let Some(c) = cooler.as_mut() {
                        c.set_brightness(want_bright)
                    } else if let Some(f) = fan.as_mut() {
                        match f.set_brightness(want_bright) {
                            Ok(note) => {
                                if changed {
                                    crate::config::log_to("screens.log", &format!("{key}: brightness {note}{}", if test_bright.is_some() { " (test)" } else { "" }));
                                }
                                Ok(())
                            }
                            Err(err) => {
                                crate::config::log_to("screens.log", &format!("{key}: brightness {want_bright}% failed: {err:#}"));
                                Err(err)
                            }
                        }
                    } else {
                        Ok(())
                    };
                    if r.is_ok() && (cooler.is_some() || fan.is_some()) {
                        last_bright = Some(want_bright);
                    }
                }
                let r = if let Some(c) = cooler.as_mut() { Some(c.send_jpeg(&jpeg)) } else { fan.as_mut().map(|f| f.send_jpeg(&jpeg)) };
                match r {
                    Some(Ok(())) => {
                        misses = 0;
                        frames += 1;
                    }
                    Some(Err(err)) => {
                        misses += 1;
                        if misses >= 5 {
                            error = Some(format!("Stopped answering: {err:#}"));
                            cooler = None;
                            fan = None;
                            misses = 0;
                        }
                    }
                    None => {}
                }
                // cooler telemetry over USB every 2 s
                if last_telemetry.elapsed() > Duration::from_secs(2) {
                    last_telemetry = Instant::now();
                    if let Some(tel) = cooler.as_mut().and_then(|c| c.telemetry()) {
                        let mut s = self.sensors.write();
                        sensors::put(&mut s.readings, "lianli.coolant", "Coolant", "Lian Li", tel.coolant as f64, "°C");
                        sensors::put(&mut s.readings, "lianli.pump", "Pump", "Lian Li", tel.pump_rpm as f64, "RPM");
                        let avg = tel.fan_rpm.iter().map(|&r| r as f64).sum::<f64>() / 3.0;
                        sensors::put(&mut s.readings, "lianli.fans.radiator", "Radiator fans", "Lian Li", avg.round(), "RPM");
                    }
                }
            }
            if self.previews.read().get(key).is_none() || frames % 2 == 0 || off {
                self.previews.write().insert(key.to_string(), Arc::new(if off { render::jpeg(&pm, max_payload) } else { jpeg }));
            }

            if fps_window.elapsed() > Duration::from_secs(2) {
                fps_measured = frames as f32 / fps_window.elapsed().as_secs_f32();
                frames = 0;
                fps_window = Instant::now();
            }
            let firmware = cooler.as_ref().and_then(|c| c.firmware.clone()).or_else(|| fan.as_ref().and_then(|f| f.firmware.clone()));
            self.set_screen_status(ScreenStatus {
                key: key.into(),
                name,
                kind: if is_cooler { "cooler".into() } else { "fan".into() },
                size,
                round: !is_cooler,
                connected: cooler.is_some() || fan.is_some(),
                firmware,
                error: error.clone(),
                fps: (fps_measured * 10.0).round() / 10.0,
            });

            let target_fps = if off || is_static(&cfg) && matches!(cfg.layout.as_str(), "none") { 1.0 } else if is_static(&cfg) { 2.0 } else { cfg.fps.clamp(1, 30) as f32 };
            let frame = Duration::from_secs_f32(1.0 / target_fps);
            if let Some(rest) = frame.checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }

    fn wireless_loop(self: Arc<Self>) {
        let mut dongle: Option<Dongle> = None;
        let mut last_try = Instant::now() - Duration::from_secs(60);
        let mut last_poll = Instant::now() - Duration::from_secs(60);
        let mut lights: HashMap<String, LightState> = HashMap::new();
        // fan targets: (logical duty, duty the device should report, last sent)
        let mut targets: HashMap<[u8; 6], ([u8; 4], [u8; 4], Instant)> = HashMap::new();
        let mut last_diag = Instant::now() - Duration::from_secs(60);
        loop {
            let t0 = Instant::now();
            if self.paused.load(Ordering::Relaxed) {
                dongle = None;
                lights.clear();
            } else if dongle.is_none() && last_try.elapsed() > Duration::from_secs(10) {
                last_try = Instant::now();
                match Dongle::open() {
                    Ok(d) => {
                        dongle = Some(d);
                        self.status.write().wireless_error = None;
                    }
                    Err(e) => {
                        let msg = if crate::system::lconnect_running() { "In use by L-Connect".to_string() } else { format!("{e:#}") };
                        self.status.write().wireless_error = Some(msg);
                    }
                }
            }
            // poll and fan speeds once a second; lighting is checked every tick so OpenRGB feels live
            if let Some(d) = dongle.as_mut() {
                if last_poll.elapsed() >= Duration::from_millis(1000) {
                    last_poll = Instant::now();
                    match d.poll() {
                        Ok(()) => {
                            self.publish_wireless(&d.devices);
                            self.apply_fans(d, &mut targets);
                            if last_diag.elapsed() > Duration::from_secs(10) {
                                last_diag = Instant::now();
                                let _ = std::fs::write(crate::config::dir().join("wireless.txt"), format!("{}\nmaster ch={}\n{}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), d.master_channel, d.describe()));
                            }
                        }
                        Err(e) => {
                            self.status.write().wireless_error = Some(format!("{e:#}"));
                            dongle = None;
                            lights.clear();
                        }
                    }
                }
            }
            if let Some(d) = dongle.as_mut() {
                Self::converge_fans(d, &mut targets);
                self.apply_lighting(d, &mut lights);
            }
            if let Some(rest) = Duration::from_millis(100).checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }

    fn publish_wireless(&self, devs: &[WirelessDevice]) {
        {
            let mut cfg = self.cfg.write();
            let mut changed = false;
            for d in devs {
                if !cfg.fans.contains_key(&d.mac) {
                    let mut g = FanGroup { name: d.kind.clone(), mode: "curve".into(), ..Default::default() };
                    if d.is_aio {
                        g.name = "Radiator fans and pump".into();
                        g.follow = "coolant".into();
                        g.curve = [(28., 30.), (32., 40.), (36., 60.), (40., 85.), (44., 100.)].iter().map(|&(t, p)| crate::config::CurvePoint { t, p }).collect();
                    } else {
                        g.follow = "hottest".into();
                    }
                    cfg.fans.insert(d.mac.clone(), g);
                    changed = true;
                }
            }
            if changed {
                self.cfg_changed.store(true, Ordering::Relaxed);
            }
        }
        let mut s = self.sensors.write();
        for d in devs {
            if d.is_aio {
                if let Some(c) = d.coolant {
                    sensors::put(&mut s.readings, "lianli.coolant", "Coolant", "Lian Li", c as f64, "°C");
                }
                sensors::put(&mut s.readings, "lianli.pump", "Pump", "Lian Li", d.rpm[3] as f64, "RPM");
                let n = d.fan_count.max(1) as usize;
                let avg = d.rpm[..n].iter().map(|&r| r as f64).sum::<f64>() / n as f64;
                sensors::put(&mut s.readings, "lianli.fans.radiator", "Radiator fans", "Lian Li", avg.round(), "RPM");
            } else if d.kind.contains("LCD") {
                let n = d.fan_count.max(1) as usize;
                let avg = d.rpm[..n].iter().map(|&r| r as f64).sum::<f64>() / n as f64;
                sensors::put(&mut s.readings, "lianli.fans.lcd", "LCD fans", "Lian Li", avg.round(), "RPM");
            }
            let n = d.fan_count.max(1) as usize;
            let avg = d.rpm[..n].iter().map(|&r| r as f64).sum::<f64>() / n as f64;
            sensors::put(&mut s.readings, &format!("lianli.fan.{}", d.mac), &d.kind, "Lian Li", avg.round(), "RPM");
        }
        drop(s);
        self.status.write().wireless = devs.to_vec();
    }

    /// Like L-Connect, keep re-sending a fan speed until the device reports it back (radio frames get lost).
    fn converge_fans(d: &mut Dongle, targets: &mut HashMap<[u8; 6], ([u8; 4], [u8; 4], Instant)>) {
        let devices = d.devices.clone();
        for dev in &devices {
            let Some((duty, want, at)) = targets.get(&dev.mac_raw).copied() else { continue };
            let off = want.iter().zip(dev.pwm.iter()).any(|(a, b)| a.abs_diff(*b) > 5);
            if off && at.elapsed() >= Duration::from_millis(300) {
                if let Ok(w) = d.set_pwm(&dev.mac_raw, duty) {
                    targets.insert(dev.mac_raw, (duty, w, Instant::now()));
                }
            }
        }
    }

    fn apply_fans(&self, d: &mut Dongle, targets: &mut HashMap<[u8; 6], ([u8; 4], [u8; 4], Instant)>) {
        let cfg = self.cfg.read().fans.clone();
        let sensors = self.sensors.read();
        let mut sends: Vec<([u8; 6], [u8; 4])> = Vec::new();
        for dev in &d.devices {
            let Some(g) = cfg.get(&dev.mac) else { continue };
            if g.mode == "auto" {
                targets.remove(&dev.mac_raw);
                continue;
            }
            let pct = if g.mode == "fixed" {
                g.fixed as f32
            } else {
                match sensors.get(&g.follow) {
                    Some(r) => curve_at(&g.curve, r.value as f32),
                    None => 60.0, // sensor missing: a safe middle speed
                }
            }
            .clamp(0.0, 100.0);
            let duty = (pct / 100.0 * 255.0).round() as u8;
            let mut pwm = [duty; 4];
            if dev.is_aio {
                let pump = if g.pump_mode == "fixed" { g.pump_fixed as f32 } else { pct.max(60.0) };
                pwm[3] = (pump.clamp(30.0, 100.0) / 100.0 * 255.0).round() as u8;
            }
            sends.push((dev.mac_raw, pwm));
        }
        drop(sensors);
        for (mac, pwm) in sends {
            match d.set_pwm(&mac, pwm) {
                Ok(w) => {
                    targets.insert(mac, (pwm, w, Instant::now()));
                }
                Err(e) => light_log(&format!("fan speed for {} not sent: {e:#}", crate::wireless::hex(&mac))),
            }
        }
    }

    fn apply_lighting(&self, d: &mut Dongle, st: &mut HashMap<String, LightState>) {
        use std::hash::{Hash, Hasher};
        let cfg = self.cfg.read().lighting.clone();
        let hottest = self.sensors.read().get("vane.hottest").map(|r| r.value);
        let devices = d.devices.clone();
        let mut notes: Vec<(String, Option<String>)> = Vec::new();
        for dev in &devices {
            let Some(l) = cfg.get(&dev.mac).cloned() else { continue };
            if l.effect == "auto" {
                st.remove(&dev.mac);
                notes.push((dev.mac.clone(), None));
                continue;
            }
            if dev.led_zones().is_empty() {
                notes.push((dev.mac.clone(), Some("Vane doesn't know this device's lights yet".into())));
                continue;
            }
            // temperature colour moves in 3 °C steps, so the fans aren't re-sent all the time
            let temp = (l.effect == "temperature").then(|| hottest.map(|t| (t / 3.0).round() * 3.0)).flatten();
            let direct = if l.effect == "openrgb" { self.rgb_direct.lock().get(&dev.mac).cloned() } else { None };
            if l.effect == "openrgb" && direct.is_none() {
                notes.push((dev.mac.clone(), Some("Waiting for colours from OpenRGB".into())));
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            l.hash(&mut h);
            temp.map(|t| t as i64).hash(&mut h);
            direct.as_ref().map(|x| x.version).hash(&mut h);
            dev.led_zones().hash(&mut h);
            let sig = h.finish();
            let s = st.entry(dev.mac.clone()).or_insert(LightState { sig: 0, idx: [0; 4], at: Instant::now() - Duration::from_secs(60), tries: 0, mb_off_at: None, confirmed: false });
            let changed = s.sig != sig;
            // the device stopped playing ours (power cycle, or the upload was missed): send it again a few times
            let lost = !changed && dev.effect_index != s.idx && s.at.elapsed() > Duration::from_secs(6) && s.tries < 4;
            if !changed && !lost {
                if dev.effect_index == s.idx {
                    s.tries = 0;
                    if !s.confirmed {
                        s.confirmed = true;
                        light_log(&format!("{} is playing {} ({:02x?})", dev.mac, l.effect, s.idx));
                    }
                }
                continue;
            }
            let min_gap = match l.effect.as_str() {
                "openrgb" => Duration::from_millis(100),
                "temperature" => Duration::from_secs(8),
                _ => Duration::from_millis(300),
            };
            // after failures, back off so a device that won't take it isn't flooded
            let min_gap = if s.tries > 0 { min_gap.max(Duration::from_secs(5 * s.tries.min(6) as u64)) } else { min_gap };
            if s.at.elapsed() < min_gap {
                continue;
            }
            if dev.mb_sync {
                // the fans follow the motherboard's RGB header; switch that off first
                if s.mb_off_at.is_none_or(|t| t.elapsed() > Duration::from_millis(2500)) {
                    light_log(&format!("{} follows motherboard RGB; asking it to stop", dev.mac));
                    let _ = d.set_mb_sync_off(&dev.mac_raw);
                    s.mb_off_at = Some(Instant::now());
                }
                notes.push((dev.mac.clone(), Some("Turning off motherboard RGB sync".into())));
                continue;
            }
            let mut result: Result<[u8; 4], String> = Err("nothing to send".into());
            for div in [1usize, 2, 4, 8] {
                let anim = match &direct {
                    Some(x) => {
                        let total: usize = dev.led_zones().iter().sum();
                        let mut c = x.colors.clone();
                        c.resize(total, [0, 0, 0]);
                        Some(crate::lighting::Anim { frames: vec![c], interval_ms: 1000 })
                    }
                    None => crate::lighting::build(dev, &l, temp, div),
                };
                let Some(anim) = anim else { break };
                match d.upload_rgb(&dev.mac_raw, &anim.frames, anim.interval_ms) {
                    Ok(idx) => {
                        result = Ok(idx);
                        break;
                    }
                    Err(e) if e.to_string().contains("too big") && anim.frames.len() > 2 => continue,
                    Err(e) => {
                        result = Err(format!("{e:#}"));
                        break;
                    }
                }
            }
            s.at = Instant::now();
            match result {
                Ok(idx) => {
                    if l.effect != "openrgb" {
                        light_log(&format!("{} sent {} ({:02x?}); device had {:02x?}", dev.mac, l.effect, idx, dev.effect_index));
                    }
                    s.confirmed = false;
                    s.idx = idx;
                    s.sig = sig;
                    s.tries = if lost { s.tries + 1 } else { 0 };
                    notes.push((dev.mac.clone(), Some("Playing".into())));
                }
                Err(e) => {
                    light_log(&format!("{} {} failed: {e}", dev.mac, l.effect));
                    s.tries = s.tries.saturating_add(1);
                    notes.push((dev.mac.clone(), Some(e)));
                }
            }
        }
        if !notes.is_empty() {
            let mut status = self.status.write();
            for (mac, n) in notes {
                match n {
                    Some(n) => {
                        status.lighting.insert(mac, n);
                    }
                    None => {
                        status.lighting.remove(&mac);
                    }
                }
            }
        }
    }

    pub fn identify_all(&self) {
        let names = self.cfg.read().fan_screen_names.clone();
        let mut id = self.identify.lock();
        let now = Instant::now();
        id.insert("cooler".into(), (0, now));
        for (serial, name) in names {
            let n: u8 = name.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
            id.insert(serial, (n, now));
        }
    }

    pub fn snapshot_sensors(&self) -> serde_json::Value {
        let s = self.sensors.read();
        let smart: Vec<serde_json::Value> = sensors::SMART
            .iter()
            .map(|(id, label, unit, _)| {
                let r = s.get(id);
                serde_json::json!({"id": id, "label": label, "unit": r.as_ref().map(|r| r.unit.clone()).unwrap_or_else(|| unit.to_string()), "value": r.map(|r| r.value), "history": s.history.get(*id)})
            })
            .collect();
        let raw: Vec<&sensors::Reading> = s.readings.values().filter(|r| !r.id.starts_with("win.") && !r.id.starts_with("rtss.")).collect();
        let mut groups: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
        for r in raw {
            groups.entry(r.group.clone()).or_default().push(serde_json::json!({"id": r.id, "label": r.label, "unit": r.unit, "value": r.value}));
        }
        serde_json::json!({"smart": smart, "groups": groups, "sources": s.sources})
    }
}
