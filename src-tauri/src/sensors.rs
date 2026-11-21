//! Sensor hub. Collects readings once a second from:
//! - RivaTuner Statistics Server shared memory (FPS and frame time of the game in front),
//! - HWiNFO: full shared memory when "Shared Memory Support" is on, otherwise the values the user
//!   ticked for "Report value in Gadget" (registry HKCU\Software\HWiNFO64\VSB),
//! - Windows itself (CPU load, RAM use),
//! - Lian Li devices (coolant, pump and fan speeds), pushed in by the device manager.
//!
//! Every reading has a stable id. Screens and fan curves store ids; the friendly "smart" ids
//! (`fps`, `gpu.temp`, `cpu.temp`, ...) resolve to the best matching raw reading at read time.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use parking_lot::RwLock;

#[derive(Clone, Debug, Serialize)]
pub struct Reading {
    pub id: String,
    pub label: String,
    pub group: String,
    pub value: f64,
    pub unit: String,
}

#[derive(Default)]
pub struct SensorState {
    pub readings: BTreeMap<String, Reading>,
    pub sources: Sources,
    /// Recent history of each smart id, for sparklines and graph widgets (60 samples).
    pub history: BTreeMap<String, Vec<f64>>,
}

#[derive(Default, Clone, Serialize)]
pub struct Sources {
    pub rtss: bool,
    pub hwinfo_shared_memory: bool,
    pub hwinfo_gadget: usize,
    pub game: Option<String>,
    /// NVIDIA GPU read straight from the driver (NVML), like the Stream Deck "System Vitals" plugin
    pub nvidia: Option<String>,
}

pub type Sensors = Arc<RwLock<SensorState>>;

/// Friendly ids shown first in every sensor picker. Each maps to label patterns tried in order.
pub const SMART: &[(&str, &str, &str, &[&str])] = &[
    // id, label, unit, patterns ("group|label" substrings, lowercase; first match wins)
    ("fps", "FPS", "FPS", &["rtss.fps", "rtss|framerate"]),
    ("frametime", "Frame time", "ms", &["rtss.frametime", "rtss|frame time"]),
    ("fps.low1", "1% low FPS", "FPS", &["rtss|framerate 1% low", "rtss|1% low"]),
    ("cpu.temp", "CPU temperature", "°C", &["|cpu (tctl/tdie)", "|cpu package", "|cpu die (average)", "|core temperatures", "|cpu temperature"]),
    ("cpu.load", "CPU load", "%", &["win.cpu.load", "|total cpu usage"]),
    ("cpu.clock", "CPU clock", "MHz", &["|core clocks (avg)", "|average effective clock", "|core 0 clock"]),
    ("cpu.power", "CPU power", "W", &["|cpu package power", "|cpu ppt"]),
    ("gpu.temp", "GPU temperature", "°C", &["gpu|gpu temperature"]),
    ("gpu.hotspot", "GPU hot spot", "°C", &["gpu|gpu hot spot temperature", "gpu|hot spot"]),
    ("gpu.load", "GPU load", "%", &["gpu|gpu core load", "gpu|gpu utilization", "gpu|gpu bus load"]),
    ("gpu.clock", "GPU clock", "MHz", &["gpu|gpu clock"]),
    ("gpu.voltage", "GPU voltage", "V", &["gpu|gpu core voltage", "gpu|gpu voltage"]),
    ("gpu.power", "GPU power", "W", &["gpu|gpu power", "gpu|total board power"]),
    ("gpu.vram", "VRAM used", "MB", &["gpu|gpu memory allocated", "gpu|gpu d3d memory dedicated", "gpu|gpu memory used"]),
    ("gpu.fan", "GPU fan", "RPM", &["gpu|gpu fan1", "gpu|gpu fan"]),
    ("ram.load", "RAM used", "%", &["win.ram.load"]),
    ("ram.used", "RAM used", "GB", &["win.ram.used"]),
    ("coolant", "Coolant", "°C", &["lianli.coolant"]),
    ("pump", "Pump speed", "RPM", &["lianli.pump"]),
    ("fan.radiator", "Radiator fans", "RPM", &["lianli.fans.radiator"]),
    ("fan.lcd", "LCD fans", "RPM", &["lianli.fans.lcd"]),
    ("drive.temp", "Drive temperature", "°C", &["|drive temperature"]),
    ("hottest", "Hottest of CPU and GPU", "°C", &["vane.hottest"]),
];

pub fn smart_meta(id: &str) -> Option<(&'static str, &'static str)> {
    SMART.iter().find(|s| s.0 == id).map(|s| (s.1, s.2))
}

impl SensorState {
    /// Value for a smart id or a raw id.
    pub fn get(&self, id: &str) -> Option<Reading> {
        if let Some(r) = self.readings.get(id) {
            return Some(r.clone());
        }
        let (_, label, unit, pats) = SMART.iter().find(|s| s.0 == id)?;
        for p in pats.iter() {
            if let Some(r) = self.readings.get(*p) {
                return Some(Reading { id: id.to_string(), label: label.to_string(), unit: unit_or(&r.unit, unit), ..r.clone() });
            }
            let (g, l) = p.split_once('|').unwrap_or(("", p));
            let hit = self.readings.values().find(|r| {
                let rl = r.label.to_lowercase();
                (g.is_empty() || r.group.to_lowercase().contains(g)) && (rl == l || (l.len() > 6 && rl.starts_with(l)))
            });
            if let Some(r) = hit {
                return Some(Reading { id: id.to_string(), label: label.to_string(), unit: unit_or(&r.unit, unit), ..r.clone() });
            }
        }
        None
    }
}

fn unit_or(actual: &str, fallback: &str) -> String {
    if actual.is_empty() { fallback.to_string() } else { actual.to_string() }
}

pub fn put(map: &mut BTreeMap<String, Reading>, id: &str, label: &str, group: &str, value: f64, unit: &str) {
    map.insert(id.to_string(), Reading { id: id.into(), label: label.into(), group: group.into(), value, unit: unit.into() });
}

/// One poll of every source. Device readings (lianli.*) are merged in by the device manager.
pub fn poll(state: &Sensors, cpu: &mut CpuMeter) {
    let mut map = BTreeMap::new();
    let mut src = Sources::default();
    #[cfg(windows)]
    {
        if let Some(load) = cpu.sample() {
            put(&mut map, "win.cpu.load", "CPU load", "Windows", load, "%");
        }
        if let Some((pct, used_gb)) = win::ram() {
            put(&mut map, "win.ram.load", "RAM used", "Windows", pct, "%");
            put(&mut map, "win.ram.used", "RAM used", "Windows", used_gb, "GB");
        }
        if let Some((game, fps, ft)) = win::rtss() {
            src.rtss = true;
            src.game = game;
            put(&mut map, "rtss.fps", "Framerate", "RTSS", fps, "FPS");
            put(&mut map, "rtss.frametime", "Frame Time", "RTSS", ft, "ms");
        }
        src.nvidia = nvidia::read(&mut map);
        let sm = win::hwinfo_shared_memory(&mut map);
        src.hwinfo_shared_memory = sm;
        if !sm {
            src.hwinfo_gadget = win::hwinfo_gadget(&mut map);
        }
    }
    #[cfg(not(windows))]
    let _ = cpu;
    let mut st = state.write();
    // keep device readings the manager put in
    let keep: Vec<Reading> = st.readings.values().filter(|r| r.id.starts_with("lianli.")).cloned().collect();
    for r in keep {
        map.insert(r.id.clone(), r);
    }
    st.readings = map;
    st.sources = src;
    // case fans do best following whichever of CPU and GPU is hotter right now
    let hot = ["cpu.temp", "gpu.temp"].iter().filter_map(|id| st.get(id)).map(|r| r.value).fold(f64::NAN, f64::max);
    if hot.is_finite() {
        put(&mut st.readings, "vane.hottest", "Hottest of CPU and GPU", "Vane", hot, "°C");
    }
    for (id, ..) in SMART {
        if let Some(r) = st.get(id) {
            let h = st.history.entry(id.to_string()).or_default();
            h.push(r.value);
            if h.len() > 60 {
                h.remove(0);
            }
        }
    }
}

#[derive(Default)]
pub struct CpuMeter {
    last: Option<(u64, u64)>,
}

impl CpuMeter {
    #[cfg(windows)]
    fn sample(&mut self) -> Option<f64> {
        let (idle, total) = win::cpu_times()?;
        let out = self.last.map(|(li, lt)| {
            let dt = total.saturating_sub(lt) as f64;
            if dt <= 0.0 { 0.0 } else { (1.0 - idle.saturating_sub(li) as f64 / dt) * 100.0 }
        });
        self.last = Some((idle, total));
        out
    }
}

#[cfg(windows)]
mod win {
    use super::{put, Reading};
    use std::collections::BTreeMap;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    fn ft(f: &FILETIME) -> u64 {
        ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
    }

    pub fn cpu_times() -> Option<(u64, u64)> {
        let (mut i, mut k, mut u) = (FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 });
        if unsafe { GetSystemTimes(&mut i, &mut k, &mut u) } == 0 {
            return None;
        }
        Some((ft(&i), ft(&k) + ft(&u)))
    }

    pub fn ram() -> Option<(f64, f64)> {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
            return None;
        }
        let used = (m.ullTotalPhys - m.ullAvailPhys) as f64 / 1024f64.powi(3);
        Some((m.dwMemoryLoad as f64, (used * 10.0).round() / 10.0))
    }

    struct View {
        h: HANDLE,
        p: MEMORY_MAPPED_VIEW_ADDRESS,
        len: usize,
    }
    /// A mapped region and its size; every read is checked against the size, because the
    /// writing app can change its layout (or shrink the mapping) between our reads.
    #[derive(Clone, Copy)]
    struct Mem {
        p: *const u8,
        len: usize,
    }
    impl View {
        fn open(name: &str) -> Option<View> {
            let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
            unsafe {
                let h = OpenFileMappingW(FILE_MAP_READ, 0, w.as_ptr());
                if h.is_null() {
                    return None;
                }
                let p = MapViewOfFile(h, FILE_MAP_READ, 0, 0, 0);
                if p.Value.is_null() {
                    CloseHandle(h);
                    return None;
                }
                let mut mbi: windows_sys::Win32::System::Memory::MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                let got = windows_sys::Win32::System::Memory::VirtualQuery(p.Value, &mut mbi, std::mem::size_of_val(&mbi));
                if got == 0 || mbi.RegionSize == 0 {
                    UnmapViewOfFile(p);
                    CloseHandle(h);
                    return None;
                }
                Some(View { h, p, len: mbi.RegionSize })
            }
        }
        fn base(&self) -> Mem {
            Mem { p: self.p.Value as *const u8, len: self.len }
        }
    }
    impl Drop for View {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.p);
                CloseHandle(self.h);
            }
        }
    }
    fn rd<T: Copy + Default>(m: Mem, off: usize) -> T {
        match off.checked_add(std::mem::size_of::<T>()) {
            Some(end) if end <= m.len => unsafe { std::ptr::read_unaligned(m.p.add(off) as *const T) },
            _ => T::default(),
        }
    }
    fn cstr(m: Mem, off: usize, n: usize) -> String {
        if off >= m.len {
            return String::new();
        }
        let n = n.min(m.len - off);
        let s = unsafe { std::slice::from_raw_parts(m.p.add(off), n) };
        let e = s.iter().position(|&b| b == 0).unwrap_or(n);
        // HWiNFO's non-UTF fields are the ANSI code page; the only non-ASCII char that matters is °
        s[..e].iter().map(|&b| if b == 0xB0 { '°' } else { b as char }).collect()
    }

    /// FPS and frame time of the foreground game (or the busiest 3D app) from RivaTuner.
    pub fn rtss() -> Option<(Option<String>, f64, f64)> {
        let v = View::open("RTSSSharedMemoryV2")?;
        let p = v.base();
        let sig: u32 = rd(p, 0);
        if sig != 0x5254_5353 {
            return None; // "RTSS"
        }
        let (size, off, count): (u32, u32, u32) = (rd(p, 8), rd(p, 12), rd(p, 16));
        if size < 284 || count > 1024 {
            return Some((None, 0.0, 0.0)); // RTSS is starting up or rewriting its header
        }
        let mut fg_pid = 0u32;
        unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut fg_pid) };
        let mut best: Option<(bool, f64, f64, String)> = None;
        for i in 0..count as usize {
            let b = off as usize + i * size as usize;
            let pid: u32 = rd(p, b);
            if pid == 0 {
                continue;
            }
            let (t0, t1, frames, ftime): (u32, u32, u32, u32) = (rd(p, b + 268), rd(p, b + 272), rd(p, b + 276), rd(p, b + 280));
            if t1 <= t0 || frames == 0 {
                continue;
            }
            let fps = frames as f64 * 1000.0 / (t1 - t0) as f64;
            let name = cstr(p, b + 4, 260);
            let short = name.rsplit(['\\', '/']).next().unwrap_or(&name).trim_end_matches(".exe").to_string();
            let fg = pid == fg_pid;
            let better = match &best {
                None => true,
                Some((bfg, bfps, ..)) => (fg && !bfg) || (fg == *bfg && fps > *bfps),
            };
            if better {
                best = Some((fg, fps, ftime as f64 / 1000.0, short));
            }
        }
        match best {
            Some((_, fps, ft, name)) => Some((Some(name), (fps * 10.0).round() / 10.0, (ft * 100.0).round() / 100.0)),
            None => Some((None, 0.0, 0.0)),
        }
    }

    fn group_of(sensor: &str) -> String {
        let s = sensor.to_lowercase();
        if s.contains("gpu") { format!("GPU · {sensor}") } else { sensor.to_string() }
    }

    /// Every HWiNFO reading, when its Shared Memory Support is on.
    pub fn hwinfo_shared_memory(map: &mut BTreeMap<String, Reading>) -> bool {
        let Some(v) = View::open("Global\\HWiNFO_SENS_SM2") else { return false };
        let p = v.base();
        let (so, ss, sn): (u32, u32, u32) = (rd(p, 20), rd(p, 24), rd(p, 28));
        let (ro, rs, rn): (u32, u32, u32) = (rd(p, 32), rd(p, 36), rd(p, 40));
        if sn == 0 || rn == 0 || sn > 4096 || rn > 65536 {
            return false;
        }
        let names: Vec<String> = (0..sn as usize)
            .map(|i| {
                let b = so as usize + i * ss as usize;
                let u = cstr(p, b + 8 + 128, 128);
                if u.is_empty() { cstr(p, b + 8, 128) } else { u }
            })
            .collect();
        for i in 0..rn as usize {
            let b = ro as usize + i * rs as usize;
            let si: u32 = rd(p, b + 4);
            let label = cstr(p, b + 140, 128);
            let unit = cstr(p, b + 268, 16);
            let value: f64 = rd(p, b + 284);
            let sensor = names.get(si as usize).cloned().unwrap_or_default();
            let group = group_of(&sensor);
            let id = format!("hw:{sensor}|{label}");
            put(map, &id, &label, &group, value, &unit);
        }
        true
    }

    /// The readings ticked "Report value in Gadget" in HWiNFO (works in the free version without a time limit).
    pub fn hwinfo_gadget(map: &mut BTreeMap<String, Reading>) -> usize {
        use winreg::enums::HKEY_CURRENT_USER;
        let Ok(k) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\HWiNFO64\\VSB") else { return 0 };
        let mut n = 0;
        for i in 0..512 {
            let Ok(sensor) = k.get_value::<String, _>(format!("Sensor{i}")) else { break };
            let label: String = k.get_value(format!("Label{i}")).unwrap_or_default();
            let raw: String = k.get_value(format!("ValueRaw{i}")).unwrap_or_default();
            let shown: String = k.get_value(format!("Value{i}")).unwrap_or_default();
            let Ok(value) = raw.replace(',', "").trim().parse::<f64>() else { continue };
            let unit = shown.trim().rsplit_once(' ').map(|(_, u)| u.to_string()).unwrap_or_default();
            let unit = if unit.chars().any(|c| c.is_ascii_digit()) { String::new() } else { unit.replace('\u{FFFD}', "°") };
            let group = if sensor == "RTSS" { "RTSS".to_string() } else { group_of(&sensor) };
            put(map, &format!("hw:{sensor}|{label}"), &label, &group, value, &unit);
            n += 1;
        }
        n
    }
}


/// NVIDIA's own monitoring library (nvml.dll, installed with the driver). Gives GPU temperature,
/// power, load, clocks, VRAM and fan without HWiNFO. GeForce cards don't report core voltage here.
#[cfg(windows)]
mod nvidia {
    use super::{put, Reading};
    use std::collections::BTreeMap;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    type Dev = *mut std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct Util {
        gpu: u32,
        memory: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct MemInfo {
        total: u64,
        free: u64,
        used: u64,
    }
    struct Nvml {
        dev: usize,
        name: String,
        temp: unsafe extern "C" fn(Dev, u32, *mut u32) -> i32,
        power: unsafe extern "C" fn(Dev, *mut u32) -> i32,
        util: unsafe extern "C" fn(Dev, *mut Util) -> i32,
        clock: unsafe extern "C" fn(Dev, u32, *mut u32) -> i32,
        mem: unsafe extern "C" fn(Dev, *mut MemInfo) -> i32,
        fan: unsafe extern "C" fn(Dev, *mut u32) -> i32,
    }
    unsafe impl Send for Nvml {}
    unsafe impl Sync for Nvml {}

    fn load() -> Option<Nvml> {
        unsafe {
            let w: Vec<u16> = "nvml.dll".encode_utf16().chain([0]).collect();
            let lib = LoadLibraryW(w.as_ptr());
            if lib.is_null() {
                return None;
            }
            macro_rules! sym {
                ($n:literal) => {
                    std::mem::transmute(GetProcAddress(lib, concat!($n, "\0").as_ptr())?)
                };
            }
            let init: unsafe extern "C" fn() -> i32 = sym!("nvmlInit_v2");
            if init() != 0 {
                return None;
            }
            let by_index: unsafe extern "C" fn(u32, *mut Dev) -> i32 = sym!("nvmlDeviceGetHandleByIndex_v2");
            let mut dev: Dev = std::ptr::null_mut();
            if by_index(0, &mut dev) != 0 {
                return None;
            }
            let get_name: unsafe extern "C" fn(Dev, *mut u8, u32) -> i32 = sym!("nvmlDeviceGetName");
            let mut buf = [0u8; 96];
            let name = if get_name(dev, buf.as_mut_ptr(), buf.len() as u32) == 0 {
                let e = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..e]).to_string()
            } else {
                "NVIDIA GPU".into()
            };
            Some(Nvml {
                dev: dev as usize,
                name,
                temp: sym!("nvmlDeviceGetTemperature"),
                power: sym!("nvmlDeviceGetPowerUsage"),
                util: sym!("nvmlDeviceGetUtilizationRates"),
                clock: sym!("nvmlDeviceGetClockInfo"),
                mem: sym!("nvmlDeviceGetMemoryInfo"),
                fan: sym!("nvmlDeviceGetFanSpeed"),
            })
        }
    }

    pub fn read(map: &mut BTreeMap<String, Reading>) -> Option<String> {
        static NV: OnceLock<Option<Nvml>> = OnceLock::new();
        let nv = NV.get_or_init(load).as_ref()?;
        let d = nv.dev as Dev;
        let g = format!("GPU · {} (driver)", nv.name);
        unsafe {
            let mut v = 0u32;
            if (nv.temp)(d, 0, &mut v) == 0 {
                put(map, "nv.temp", "GPU Temperature", &g, v as f64, "°C");
            }
            if (nv.power)(d, &mut v) == 0 {
                put(map, "nv.power", "GPU Power", &g, (v as f64 / 100.0).round() / 10.0, "W");
            }
            let mut u = Util::default();
            if (nv.util)(d, &mut u) == 0 {
                put(map, "nv.load", "GPU Core Load", &g, u.gpu as f64, "%");
            }
            if (nv.clock)(d, 0, &mut v) == 0 {
                put(map, "nv.clock", "GPU Clock", &g, v as f64, "MHz");
            }
            if (nv.clock)(d, 2, &mut v) == 0 {
                put(map, "nv.memclock", "GPU Memory Clock", &g, v as f64, "MHz");
            }
            let mut m = MemInfo::default();
            if (nv.mem)(d, &mut m) == 0 {
                put(map, "nv.vram", "GPU Memory Allocated", &g, (m.used / 1_048_576) as f64, "MB");
            }
            if (nv.fan)(d, &mut v) == 0 {
                put(map, "nv.fan", "GPU Fan", &g, v as f64, "%");
            }
        }
        Some(nv.name.clone())
    }
}

#[cfg(not(windows))]
mod nvidia {
    pub fn read(_: &mut std::collections::BTreeMap<String, super::Reading>) -> Option<String> {
        None
    }
}
