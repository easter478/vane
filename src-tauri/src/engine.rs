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
        });
        // sensors, once a second
        {
            let e = e.clone();
            std::thread::Builder::new().name("sensors".into()).spawn(move || {
                let mut cpu = sensors::CpuMeter::default();
                loop {
                    sensors::poll(&e.sensors, &mut cpu);
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
            let pm = render::render(&mut fonts, &mut state, &cfg, &sensors, size, !is_cooler, started.elapsed().as_secs_f32());
            drop(sensors);
            let mut jpeg = render::jpeg(&pm, max_payload);

            let off = !send;
            if off && (cooler.is_some() || fan.is_some()) {
                // screens switched off: show black
                let black = tiny_skia::Pixmap::new(size, size).unwrap();
                jpeg = render::jpeg(&black, max_payload);
            }
            if !paused {
                if last_bright != Some(cfg.brightness) {
                    let r = if let Some(c) = cooler.as_mut() { c.set_brightness(cfg.brightness) } else if let Some(f) = fan.as_mut() { f.set_brightness(cfg.brightness) } else { Ok(()) };
                    if r.is_ok() && (cooler.is_some() || fan.is_some()) {
                        last_bright = Some(cfg.brightness);
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
        loop {
            let t0 = Instant::now();
            if self.paused.load(Ordering::Relaxed) {
                dongle = None;
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
            if let Some(d) = dongle.as_mut() {
                match d.poll() {
                    Ok(()) => {
                        self.publish_wireless(&d.devices);
                        self.apply_fans(d);
                    }
                    Err(e) => {
                        self.status.write().wireless_error = Some(format!("{e:#}"));
                        dongle = None;
                    }
                }
            }
            if let Some(rest) = Duration::from_millis(1000).checked_sub(t0.elapsed()) {
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
                        g.follow = "gpu.temp".into();
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

    fn apply_fans(&self, d: &mut Dongle) {
        let cfg = self.cfg.read().fans.clone();
        let sensors = self.sensors.read();
        let mut sends: Vec<([u8; 6], [u8; 4])> = Vec::new();
        for dev in &d.devices {
            let Some(g) = cfg.get(&dev.mac) else { continue };
            if g.mode == "auto" {
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
            let _ = d.set_pwm(&mac, pwm);
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
