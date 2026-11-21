//! Settings, saved as JSON in %APPDATA%\Vane\config.json. Also holds the built-in screen themes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScreenConfig {
    pub enabled: bool,
    pub theme: String,
    pub background: String,
    pub layout: String,
    pub slots: Vec<String>,
    pub accent: String,
    pub text: String,
    pub solid: String,
    pub image: String,
    pub speed: f32,
    pub dim: f32,
    pub plates: bool,
    pub clock_24h: bool,
    pub brightness: u8,
    pub rotation: u16,
    pub fps: u8,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        theme("matrix-fps").unwrap()
    }
}

fn sc(name: &str, bg: &str, layout: &str, slots: &[&str], accent: &str) -> ScreenConfig {
    ScreenConfig {
        enabled: true,
        theme: name.into(),
        background: bg.into(),
        layout: layout.into(),
        slots: slots.iter().map(|s| s.to_string()).collect(),
        accent: accent.into(),
        text: "#eef4f2".into(),
        solid: "#0b1a22".into(),
        image: String::new(),
        speed: 1.0,
        dim: if bg == "matrix" { 0.25 } else { 0.0 },
        plates: true,
        clock_24h: true,
        brightness: 80,
        rotation: 0,
        fps: 20,
    }
}

/// Built-in themes: (id, name, for cooler, for fans).
pub const THEMES: &[(&str, &str, bool, bool)] = &[
    ("matrix-fps", "Matrix FPS", true, true),
    ("matrix-system", "Matrix system", true, true),
    ("matrix-rain", "Matrix rain", true, true),
    ("neon-ring", "Neon ring", true, true),
    ("nebula-gaming", "Nebula gaming", true, false),
    ("synthwave-clock", "Synthwave clock", true, true),
    ("coolant", "Coolant", true, true),
    ("carbon-bars", "Carbon bars", true, false),
    ("radar-fps", "Radar FPS", true, true),
    ("aurora-clock", "Aurora clock", true, true),
    ("pulse-gpu", "Pulse GPU", true, true),
    ("particles", "Particles", true, true),
];

pub fn theme(id: &str) -> Option<ScreenConfig> {
    Some(match id {
        "matrix-fps" => sc(id, "matrix", "gaming", &["fps", "frametime", "fps.low1", "gpu.temp", "gpu.voltage"], "#00ff6a"),
        "matrix-system" => sc(id, "matrix", "system", &["cpu.temp", "cpu.load", "gpu.temp", "gpu.load"], "#00ff6a"),
        "matrix-rain" => sc(id, "matrix", "none", &[], "#00ff6a"),
        "neon-ring" => sc(id, "waves", "ring", &["cpu.temp", "cpu.load"], "#2cc4bc"),
        "nebula-gaming" => sc(id, "nebula", "gaming", &["fps", "frametime", "gpu.temp", "gpu.load", "gpu.voltage"], "#b07cff"),
        "synthwave-clock" => sc(id, "synthwave", "clock_sensors", &["cpu.temp", "gpu.temp", "fps"], "#ff6fa8"),
        "coolant" => sc(id, "pulse", "ring", &["coolant", "pump"], "#2cc4bc"),
        "carbon-bars" => sc(id, "carbon", "bars", &["cpu.temp", "gpu.temp", "gpu.load", "gpu.voltage", "frametime"], "#ffb347"),
        "radar-fps" => sc(id, "radar", "big", &["fps"], "#3dff8f"),
        "aurora-clock" => sc(id, "aurora", "clock", &["gpu.temp"], "#7dffd0"),
        "pulse-gpu" => sc(id, "pulse", "big", &["gpu.temp"], "#4f9dff"),
        "particles" => sc(id, "particles", "ring", &["gpu.load", "gpu.temp"], "#b07cff"),
        _ => return None,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CurvePoint {
    pub t: f32,
    pub p: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FanGroup {
    pub name: String,
    /// "curve", "fixed" or "auto" (leave the fans alone)
    pub mode: String,
    pub follow: String,
    pub fixed: u8,
    pub curve: Vec<CurvePoint>,
    pub pump_mode: String,
    pub pump_fixed: u8,
}

impl Default for FanGroup {
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: "auto".into(),
            follow: "cpu.temp".into(),
            fixed: 50,
            curve: preset("balanced"),
            pump_mode: "fixed".into(),
            pump_fixed: 85,
        }
    }
}

pub fn preset(name: &str) -> Vec<CurvePoint> {
    let pts: &[(f32, f32)] = match name {
        "quiet" => &[(30., 20.), (45., 25.), (60., 40.), (75., 60.), (90., 90.)],
        "perf" => &[(30., 40.), (45., 55.), (60., 75.), (75., 95.), (90., 100.)],
        "full" => &[(30., 100.), (45., 100.), (60., 100.), (75., 100.), (90., 100.)],
        _ => &[(30., 25.), (45., 35.), (60., 55.), (75., 80.), (90., 100.)],
    };
    pts.iter().map(|&(t, p)| CurvePoint { t, p }).collect()
}

impl Default for CurvePoint {
    fn default() -> Self {
        Self { t: 50.0, p: 50.0 }
    }
}

pub fn curve_at(c: &[CurvePoint], t: f32) -> f32 {
    if c.is_empty() {
        return 50.0;
    }
    if t <= c[0].t {
        return c[0].p;
    }
    for w in c.windows(2) {
        if t <= w[1].t {
            return w[0].p + (w[1].p - w[0].p) * (t - w[0].t) / (w[1].t - w[0].t).max(0.01);
        }
    }
    c[c.len() - 1].p
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub app_theme: String,
    pub start_with_windows: bool,
    pub close_to_tray: bool,
    pub screens_on: bool,
    /// keys: "cooler" or a fan screen's USB serial
    pub screens: BTreeMap<String, ScreenConfig>,
    /// display names and order of fan screens, by serial
    pub fan_screen_names: BTreeMap<String, String>,
    /// keys: wireless MAC
    pub fans: BTreeMap<String, FanGroup>,
    pub my_themes: BTreeMap<String, ScreenConfig>,
    pub link_fan_screens: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_theme: "system".into(),
            start_with_windows: true,
            close_to_tray: true,
            screens_on: true,
            screens: BTreeMap::new(),
            fan_screen_names: BTreeMap::new(),
            fans: BTreeMap::new(),
            my_themes: BTreeMap::new(),
            link_fan_screens: false,
        }
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join("Vane");
    let _ = std::fs::create_dir_all(&d);
    d
}

impl Config {
    pub fn load() -> Self {
        // tolerate a byte-order mark (Notepad and PowerShell add one)
        std::fs::read_to_string(dir().join("config.json")).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = dir().join("config.json");
        let tmp = p.with_extension("json.tmp");
        if let Ok(s) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
    pub fn screen(&self, key: &str, fan_default: &str) -> ScreenConfig {
        self.screens.get(key).cloned().unwrap_or_else(|| {
            let mut c = theme(if key == "cooler" { "matrix-fps" } else { fan_default }).unwrap();
            if key != "cooler" {
                c.fps = 15;
            }
            c
        })
    }
}
