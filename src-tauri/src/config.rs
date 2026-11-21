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
    /// dark outline around numbers and words so they stand out from busy backgrounds
    pub outline: bool,
    /// background blur, 0 (sharp) to 1 (very soft)
    pub blur: f32,
    /// colour of the whole picture, for panels that look washed out: 1.0 leaves it as drawn
    pub contrast: f32,
    pub saturation: f32,
    /// above 1 darkens the mid-tones (deeper picture), below 1 lifts them
    pub gamma: f32,
    /// 0 to 0.3: tones darker than this become pure black, the darkest an LCD backlight allows
    pub black: f32,
    pub clock_24h: bool,
    pub brightness: u8,
    pub rotation: u16,
    pub fps: u8,
    /// widgets for the "custom" layout, placed freely on the screen
    pub widgets: Vec<Widget>,
    /// full-screen designs: size of the big number and of the labels (1 = as designed)
    pub num_size: f32,
    pub label_size: f32,
    /// full-screen designs: replaces the design's own highlight colour when set (#hex)
    pub face_color: String,
    /// square screens: corner rounding, 0 (square) to 0.5 (a circle)
    pub corner: f32,
    /// full-screen designs: each piece's move, size and visibility, by piece id (see faces::PIECES)
    pub face_parts: BTreeMap<String, FacePart>,
}

/// One piece of a full-screen design, moved, resized or hidden by the user.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct FacePart {
    /// moved by this much (parts of the screen)
    pub x: f32,
    pub y: f32,
    /// 1 = as designed (0 counts as 1)
    pub size: f32,
    pub hide: bool,
}

/// One item on a custom layout. x and y are the centre (0..1 of the screen); size scales its box.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Widget {
    /// value, gauge, bar, clock, date, text, graph
    pub kind: String,
    pub sensor: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    /// "accent", "text", "heat" (accent that warms up with temperature) or a #hex colour
    pub color: String,
    /// caption override, or the words for a text widget
    pub label: String,
    pub plate: bool,
}

impl Default for Widget {
    fn default() -> Self {
        Self { kind: "value".into(), sensor: "cpu.load".into(), x: 0.5, y: 0.5, size: 1.0, color: "heat".into(), label: String::new(), plate: false }
    }
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
        plates: false,
        outline: true,
        blur: 0.0,
        contrast: VIVID.0,
        saturation: VIVID.1,
        gamma: VIVID.2,
        black: 0.06,
        clock_24h: true,
        brightness: 80,
        rotation: 0,
        fps: 20,
        widgets: Vec::new(),
        num_size: 1.0,
        label_size: 1.0,
        face_color: String::new(),
        corner: 0.0, face_parts: BTreeMap::new(),
    }
}

/// One warning: `medium` turns the reading yellow, `high` flashes it red. With `below` the warning is
/// for a drop (FPS), otherwise for a climb (power, temperature).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertRule {
    pub id: String,
    pub on: bool,
    pub below: bool,
    pub medium: f64,
    pub high: f64,
}

impl Default for AlertRule {
    fn default() -> Self {
        Self { id: String::new(), on: false, below: false, medium: 80.0, high: 90.0 }
    }
}

impl AlertRule {
    /// 0 fine, 1 medium (yellow), 2 high (red)
    pub fn level(&self, v: f64) -> u8 {
        if self.below {
            if v < self.high { 2 } else if v < self.medium { 1 } else { 0 }
        } else if v >= self.high {
            2
        } else if v >= self.medium {
            1
        } else {
            0
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Alerts {
    /// "text": only the number flashes; "screen": the whole screen flashes
    pub style: String,
    /// show readings in green while they're fine
    pub green: bool,
    pub rules: Vec<AlertRule>,
}

impl Default for Alerts {
    fn default() -> Self {
        let r = |id: &str, on: bool, below: bool, medium: f64, high: f64| AlertRule { id: id.into(), on, below, medium, high };
        Self {
            style: "text".into(),
            green: false,
            rules: vec![
                r("fps", true, true, 90.0, 60.0),
                r("gpu.power", true, false, 320.0, 350.0),
                r("cpu.temp", true, false, 80.0, 90.0),
                r("gpu.temp", false, false, 75.0, 83.0),
            ],
        }
    }
}

/// Built-in themes: (id, name, for cooler, for fans).
pub const THEMES: &[(&str, &str, bool, bool)] = &[
    ("telemetry", "Telemetry", true, true),
    ("telemetry-white", "Telemetry white", true, true),
    ("afterglow", "Afterglow", true, true),
    ("chronograph", "Chronograph", true, true),
    ("plasma", "Plasma", true, true),
    ("porcelain", "Porcelain", true, true),
    ("instrument", "Instrument", true, true),
    ("frost", "Frost", true, true),
    ("poster", "Poster", true, true),
    ("matrix-fps", "Matrix FPS", true, true),
    ("matrix-system", "Matrix system", true, true),
    ("matrix-rain", "Matrix rain", true, true),
    ("fps-watts", "FPS and GPU power", true, true),
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
    if let Some(face) = crate::render::FACES.iter().find(|f| **f == id) {
        let mut c = sc(id, &format!("face:{face}"), "dual", &["fps", "gpu.temp"], "#ffb020");
        c.outline = false;
        return Some(c);
    }
    Some(match id {
        "matrix-fps" => sc(id, "matrix", "gaming", &["fps", "frametime", "fps.low1", "gpu.temp", "gpu.power"], "#00ff6a"),
        "matrix-system" => sc(id, "matrix", "system", &["cpu.temp", "cpu.load", "gpu.temp", "gpu.load"], "#00ff6a"),
        "matrix-rain" => sc(id, "matrix", "none", &[], "#00ff6a"),
        "fps-watts" => sc(id, "pulse", "dual", &["fps", "gpu.power"], "#4f9dff"),
        "neon-ring" => sc(id, "waves", "ring", &["cpu.temp", "cpu.load"], "#2cc4bc"),
        "nebula-gaming" => sc(id, "nebula", "gaming", &["fps", "frametime", "gpu.temp", "gpu.load", "gpu.power"], "#b07cff"),
        "synthwave-clock" => sc(id, "synthwave", "clock_sensors", &["cpu.temp", "gpu.temp", "fps"], "#ff6fa8"),
        "coolant" => sc(id, "pulse", "ring", &["coolant", "pump"], "#2cc4bc"),
        "carbon-bars" => sc(id, "carbon", "bars", &["cpu.temp", "gpu.temp", "gpu.load", "gpu.power", "frametime"], "#ffb347"),
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
    /// keys: wireless MAC
    pub lighting: BTreeMap<String, Lighting>,
    /// lets OpenRGB see and colour the wireless fans (Vane acts as an OpenRGB SDK server on this PC only)
    pub openrgb_server: bool,
    pub openrgb_port: u16,
    /// connect to OpenRGB (port 6742) to light the motherboard, memory, graphics card and other RGB it finds
    pub openrgb_client: bool,
    /// start OpenRGB in the background (with its server on) when Vane starts, if it's installed
    pub openrgb_launch: bool,
    /// start MSI Afterburner and RivaTuner (through their own startup tasks) when they aren't running
    pub start_afterburner: bool,
    /// warnings that flash on every screen when a reading crosses a step
    pub alerts: Alerts,
    /// OpenRGB device names Vane leaves alone (motherboard, memory, graphics card… are on by default)
    pub openrgb_skip: Vec<String>,
    /// other OpenRGB devices (keyboards, Stream Deck…) the user switched on
    pub openrgb_include: Vec<String>,
    #[serde(default)]
    pub version: u32,
}

/// Lighting for one wireless group (fans, and the pump ring on a HydroShift).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Hash)]
#[serde(default)]
pub struct Lighting {
    /// see lighting::EFFECTS; "auto" leaves the device's own lighting alone
    pub effect: String,
    pub colors: Vec<String>,
    /// 1 (slow) to 10 (fast)
    pub speed: u8,
    /// 0 to 100
    pub brightness: u8,
    pub reverse: bool,
}

impl Default for Lighting {
    fn default() -> Self {
        Self { effect: "auto".into(), colors: vec!["#2cc4bc".into(), "#b07cff".into()], speed: 5, brightness: 80, reverse: false }
    }
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
            lighting: BTreeMap::new(),
            openrgb_server: true,
            openrgb_port: 6743,
            openrgb_client: true,
            openrgb_launch: true,
            openrgb_skip: Vec::new(),
            openrgb_include: Vec::new(),
            start_afterburner: true,
            alerts: Alerts::default(),
            version: 3,
        }
    }
}

/// Appends a timestamped line to a diagnostics log in Vane's folder (kept under 200 KB).
pub fn log_to(name: &str, msg: &str) {
    use std::io::Write;
    let p = dir().join(name);
    if std::fs::metadata(&p).map(|m| m.len() > 200_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Colour (contrast, saturation, gamma) new screens start with: the LCD panels look washed out
/// with the picture as drawn, so it starts a little deeper and more colourful.
pub const VIVID: (f32, f32, f32) = (1.12, 1.3, 1.15);

pub fn dir() -> PathBuf {
    let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join("Vane");
    let _ = std::fs::create_dir_all(&d);
    d
}

impl Config {
    pub fn load() -> Self {
        // tolerate a byte-order mark (Notepad and PowerShell add one)
        let mut c: Config = std::fs::read_to_string(dir().join("config.json")).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or_default();
        if c.version < 2 {
            // 0.2: readings get a dark outline instead of grey glass boxes
            for sc in c.screens.values_mut().chain(c.my_themes.values_mut()) {
                sc.plates = false;
                sc.outline = true;
                sc.widgets.iter_mut().for_each(|w| w.plate = false);
            }
            c.version = 2;
        }
        if c.version < 3 {
            // 0.2.1: screens start with the vivid colour setting
            for sc in c.screens.values_mut().chain(c.my_themes.values_mut()) {
                (sc.contrast, sc.saturation, sc.gamma) = VIVID;
                sc.black = 0.06;
            }
            c.version = 3;
        }
        c
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
