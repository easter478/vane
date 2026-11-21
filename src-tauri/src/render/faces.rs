//! Full-screen designs ("faces"). Each one draws the whole screen itself from the screen's first two
//! readings: slot 0 is the big number, slot 1 the smaller one under it. A screen uses a face when its
//! background is "face:<name>", so "Same look on all LCD fans" shares the face while each fan keeps
//! its own readings.

use super::{arc, circle, glow, hex, line, mix, paint, rect, rgba, rrect, Fonts, ScreenState};
use crate::config::ScreenConfig;
use crate::sensors::{smart_meta, SensorState};
use std::f32::consts::{PI, TAU};
use tiny_skia::Mask as Clip;
use tiny_skia::{BlendMode, Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, RadialGradient, Rect, Shader, SpreadMode, Stroke, Transform};

// font slots loaded by Fonts::load (Windows fonts, so nothing extra ships)
pub const COND: u8 = 10; // Bahnschrift bold condensed
pub const LIGHT: u8 = 11; // Segoe UI Variable light
pub const TEXT: u8 = 12; // Segoe UI Variable regular
pub const SEMI: u8 = 13; // Segoe UI Variable semibold
pub const MONO: u8 = 14; // Cascadia Mono semibold
pub const MONO_R: u8 = 15; // Cascadia Mono regular
pub const BLACK: u8 = 16; // Segoe UI Black
pub const BOLD: u8 = 17; // Segoe UI Variable bold

pub const FACES: &[&str] = &["telemetry", "telemetry-white", "afterglow", "chronograph", "plasma", "porcelain", "instrument", "frost", "poster"];

// ------------------------------------------------------------------ readings

pub(super) struct Rd {
    pub id: String,
    pub name: String,
    pub tag: String,
    pub val: Option<f64>,
    pub text: String,
    pub unit: String,
    pub min: f32,
    pub max: f32,
    pub div: u32,
    pub hist: Vec<f64>,
    /// warning colour and strength (0..1 this frame) from the screen warnings, if any
    pub alert: Option<([u8; 3], f32)>,
}

impl Rd {
    fn frac(&self) -> f32 {
        self.val.map(|v| ((v as f32 - self.min) / (self.max - self.min)).clamp(0.0, 1.0)).unwrap_or(0.0)
    }
    fn temp(&self) -> Option<f32> {
        (self.unit == "°C").then(|| self.val.unwrap_or(0.0) as f32)
    }
    /// The unit drawn beside the big number.
    fn glyph(&self) -> &str {
        match self.unit.as_str() {
            "°C" => "°",
            "%" => "%",
            "W" => "W",
            "ms" => "ms",
            "MHz" => "MHz",
            "RPM" | "rpm" => "rpm",
            _ => "",
        }
    }
    /// The value with its unit, for the smaller reading: 64°C, 248 W, 36%.
    fn short(&self) -> String {
        match self.unit.as_str() {
            "°C" => format!("{}°C", self.text),
            "%" => format!("{}%", self.text),
            "" | "FPS" => self.text.clone(),
            u => format!("{} {u}", self.text),
        }
    }
    /// Colour for this reading's number: the warning colour while it flashes, else `base`.
    fn col(&self, base: [u8; 3]) -> [u8; 3] {
        match self.alert {
            Some((c, k)) => mix(base, c, k),
            None => base,
        }
    }
}

pub(super) fn read(sensors: &SensorState, id: &str) -> Rd {
    let r = sensors.get(id);
    let (label, ufb) = smart_meta(id)
        .map(|(l, u)| (l.to_string(), u.to_string()))
        .unwrap_or_else(|| (r.as_ref().map(|r| r.label.clone()).unwrap_or_else(|| id.trim_start_matches("hw:").to_string()), String::new()));
    let unit = r.as_ref().map(|r| if r.unit.is_empty() { ufb.clone() } else { r.unit.clone() }).unwrap_or(ufb);
    let (name, tag) = match id {
        "fps" => ("FPS", "FPS"),
        "gpu.temp" => ("GPU TEMP", "GPU"),
        "cpu.temp" => ("CPU TEMP", "CPU"),
        "gpu.power" => ("GPU POWER", "GPU"),
        "cpu.power" => ("CPU POWER", "CPU"),
        "gpu.load" => ("GPU LOAD", "GPU"),
        "cpu.load" => ("CPU LOAD", "CPU"),
        "frametime" => ("FRAME TIME", "FRAME"),
        "fps.low1" => ("1% LOW", "LOW"),
        "coolant" => ("COOLANT", "WATER"),
        "pump" => ("PUMP", "PUMP"),
        _ => ("", ""),
    };
    let name = if name.is_empty() { super::short_label(&label) } else { name.to_string() };
    let tag = if tag.is_empty() { name.split_whitespace().next().unwrap_or("").to_string() } else { tag.to_string() };
    let (min, max, div) = match unit.as_str() {
        "FPS" => (0.0, 240.0, 6),
        "°C" if id == "coolant" => (20.0, 50.0, 6),
        "°C" => (30.0, 90.0, 6),
        "%" => (0.0, 100.0, 5),
        "W" if id.starts_with("gpu") => (0.0, 400.0, 4),
        "W" => (0.0, 200.0, 4),
        "ms" => (0.0, 40.0, 4),
        _ => (0.0, super::gauge_max(&unit, id) as f32, 4),
    };
    let val = r.as_ref().map(|r| r.value);
    let text = match val {
        Some(v) if unit == "ms" => format!("{v:.1}"),
        Some(v) => format!("{}", v.round() as i64),
        None => "--".into(),
    };
    Rd { id: id.to_string(), name, tag, val, text, unit, min, max, div, hist: sensors.history.get(id).cloned().unwrap_or_default(), alert: None }
}

/// The two readings a face shows: the screen's first two slots, with a sensible second one if unset.
pub(super) fn pair(cfg: &ScreenConfig, sensors: &SensorState) -> (Rd, Rd) {
    let main = cfg.slots.first().map(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or("fps");
    let sub = cfg.slots.get(1).map(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or(match main {
        "gpu.temp" => "gpu.power",
        "cpu.temp" => "cpu.load",
        _ => "gpu.temp",
    });
    (read(sensors, main), read(sensors, sub))
}

/// The pieces of each full-screen design that can be moved, resized or hidden:
/// (design, id, name, centre x, centre y, width, height) as parts of the screen at size 1.
/// Telemetry white uses Telemetry's pieces.
pub const PIECES: &[(&str, &str, &str, f32, f32, f32, f32)] = &[
    ("afterglow", "dial", "Dial", 0.5, 0.5, 0.86, 0.86),
    ("afterglow", "dial_numbers", "Dial numbers", 0.5, 0.5, 0.7, 0.7),
    ("afterglow", "label", "Big reading label", 0.5, 0.322, 0.22, 0.05),
    ("afterglow", "num", "Big number", 0.5, 0.5, 0.5, 0.25),
    ("afterglow", "sub_label", "Small reading label", 0.5, 0.738, 0.14, 0.04),
    ("afterglow", "sub_value", "Small reading value", 0.5, 0.81, 0.24, 0.08),
    ("afterglow", "sub_bar", "Small reading bar", 0.5, 0.876, 0.18, 0.03),
    ("chronograph", "track", "Outer track", 0.5, 0.5, 0.9, 0.9),
    ("chronograph", "dial_numbers", "Dial numbers", 0.5, 0.5, 0.74, 0.74),
    ("chronograph", "label", "Big reading label", 0.5, 0.328, 0.26, 0.05),
    ("chronograph", "num", "Big number", 0.5, 0.46, 0.45, 0.2),
    ("chronograph", "footer", "Footer text", 0.5, 0.925, 0.4, 0.04),
    ("chronograph", "subdial", "Sub-dial", 0.5, 0.71, 0.26, 0.26),
    ("chronograph", "sub_label", "Small reading label", 0.5, 0.648, 0.1, 0.035),
    ("chronograph", "sub_value", "Small reading value", 0.5, 0.785, 0.14, 0.06),
    ("plasma", "ring", "Outer ring", 0.5, 0.5, 0.9, 0.9),
    ("plasma", "dashes", "Turning ring", 0.5, 0.5, 0.76, 0.76),
    ("plasma", "orb", "Orb", 0.5, 0.5, 0.66, 0.66),
    ("plasma", "label", "Big reading label", 0.5, 0.38, 0.25, 0.045),
    ("plasma", "num", "Big number", 0.5, 0.5, 0.4, 0.17),
    ("plasma", "sub_value", "Small reading", 0.5, 0.646, 0.36, 0.082),
    ("telemetry", "label", "Big reading label", 0.52, 0.237, 0.28, 0.05),
    ("telemetry", "dot", "Live dot", 0.4, 0.237, 0.04, 0.04),
    ("telemetry", "num", "Big number", 0.5, 0.39, 0.55, 0.2),
    ("telemetry", "graph", "Frame graph", 0.5, 0.57, 0.6, 0.11),
    ("telemetry", "captions", "Graph captions", 0.5, 0.66, 0.6, 0.04),
    ("telemetry", "sub_label", "Small reading label", 0.39, 0.76, 0.1, 0.04),
    ("telemetry", "sub_value", "Small reading value", 0.58, 0.752, 0.16, 0.06),
    ("telemetry", "sub_bar", "Small reading bar", 0.5, 0.809, 0.3, 0.03),
    ("porcelain", "ring", "Ring", 0.5, 0.5, 0.8, 0.8),
    ("porcelain", "label", "Big reading label", 0.5, 0.362, 0.26, 0.05),
    ("porcelain", "num", "Big number", 0.5, 0.5, 0.45, 0.2),
    ("porcelain", "sub_value", "Small reading", 0.5, 0.69, 0.4, 0.06),
    ("instrument", "dial", "Dial", 0.5, 0.5, 0.86, 0.86),
    ("instrument", "dial_numbers", "Dial numbers", 0.5, 0.5, 0.68, 0.68),
    ("instrument", "needle", "Needle", 0.5, 0.5, 0.86, 0.86),
    ("instrument", "sub_value", "Small reading window", 0.5, 0.6, 0.33, 0.074),
    ("instrument", "label", "Big reading label", 0.5, 0.695, 0.26, 0.04),
    ("instrument", "num", "Big number", 0.5, 0.785, 0.35, 0.13),
    ("frost", "ring", "Ring", 0.5, 0.5, 0.78, 0.78),
    ("frost", "label", "Big reading label", 0.5, 0.362, 0.26, 0.045),
    ("frost", "num", "Big number", 0.5, 0.5, 0.45, 0.2),
    ("frost", "sub_value", "Small reading", 0.5, 0.669, 0.38, 0.088),
    ("poster", "label", "Big reading label", 0.27, 0.327, 0.16, 0.05),
    ("poster", "unit", "Unit word", 0.72, 0.327, 0.18, 0.05),
    ("poster", "num", "Big number", 0.5, 0.48, 0.62, 0.25),
    ("poster", "sub_bar", "Small reading bar", 0.5, 0.665, 0.6, 0.04),
    ("poster", "sub_label", "Small reading label", 0.25, 0.752, 0.12, 0.045),
    ("poster", "sub_value", "Small reading value", 0.73, 0.752, 0.16, 0.045),
];

pub fn piece_face(face: &str) -> &str {
    if face == "telemetry-white" { "telemetry" } else { face }
}

/// Per-screen tweaks to a full-screen design: number size, label size, highlight colour, and each
/// piece's place, size and visibility.
#[derive(Clone, Copy)]
pub(super) struct O<'a> {
    pub n: f32,
    pub l: f32,
    pub acc: Option<[u8; 3]>,
    face: &'a str,
    parts: &'a std::collections::BTreeMap<String, crate::config::FacePart>,
}

impl<'a> O<'a> {
    pub fn from(cfg: &'a ScreenConfig) -> O<'a> {
        let g = |v: f32| if v <= 0.01 { 1.0 } else { v.clamp(0.5, 2.0) };
        O {
            n: g(cfg.num_size),
            l: g(cfg.label_size),
            acc: (!cfg.face_color.is_empty()).then(|| hex(&cfg.face_color)),
            face: piece_face(cfg.background.strip_prefix("face:").unwrap_or("")),
            parts: &cfg.face_parts,
        }
    }
    /// Where one piece goes this frame: its own move, size and on/off, around its centre.
    fn el(&self, id: &str, s: f32) -> E {
        let (ax, ay) = PIECES.iter().find(|p| p.0 == self.face && p.1 == id).map(|p| (p.3, p.4)).unwrap_or((0.5, 0.5));
        let p = self.parts.get(id);
        let fin = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        E {
            show: !p.map(|p| p.hide).unwrap_or(false),
            ax: ax * s,
            ay: ay * s,
            k: p.map(|p| if p.size <= 0.01 || !p.size.is_finite() { 1.0 } else { p.size.clamp(0.2, 4.0) }).unwrap_or(1.0),
            dx: p.map(|p| fin(p.x)).unwrap_or(0.0) * s,
            dy: p.map(|p| fin(p.y)).unwrap_or(0.0) * s,
        }
    }
}

/// One piece's placement: points are scaled around the piece's centre, then moved.
#[derive(Clone, Copy)]
struct E {
    show: bool,
    ax: f32,
    ay: f32,
    k: f32,
    dx: f32,
    dy: f32,
}

impl E {
    fn x(&self, x: f32) -> f32 {
        self.ax + (x - self.ax) * self.k + self.dx
    }
    fn y(&self, y: f32) -> f32 {
        self.ay + (y - self.ay) * self.k + self.dy
    }
    fn z(&self, v: f32) -> f32 {
        v * self.k
    }
    fn key(&self) -> String {
        format!("{}{:.1}{:.1}{:.3}", self.show, self.dx, self.dy, self.k)
    }
}

// ------------------------------------------------------------------ drawing helpers

const TEMP_DARK: &[(f32, &str)] = &[(35.0, "#4fd1ff"), (55.0, "#4ef0b0"), (72.0, "#ffc23d"), (86.0, "#ff4d5e")];
const TEMP_LIGHT: &[(f32, &str)] = &[(35.0, "#0ea5e9"), (55.0, "#12b886"), (72.0, "#f59f00"), (86.0, "#e03131")];
const ACC: &[(f32, &str)] = &[(0.0, "#2de2c5"), (0.55, "#4f9dff"), (1.0, "#b06bff")];

fn ramp(st: &[(f32, &str)], x: f32) -> [u8; 3] {
    if x <= st[0].0 {
        return hex(st[0].1);
    }
    for w in st.windows(2) {
        if x <= w[1].0 {
            return mix(hex(w[0].1), hex(w[1].1), (x - w[0].0) / (w[1].0 - w[0].0));
        }
    }
    hex(st[st.len() - 1].1)
}

fn stops(s: &[(f32, [u8; 3], f32)]) -> Vec<GradientStop> {
    s.iter().map(|(p, c, a)| GradientStop::new(*p, rgba(*c, *a))).collect()
}
fn radial(focal: (f32, f32), c: (f32, f32), r: f32, st: &[(f32, [u8; 3], f32)]) -> Option<Shader<'static>> {
    RadialGradient::new(Point::from_xy(focal.0, focal.1), Point::from_xy(c.0, c.1), r.max(0.5), stops(st), SpreadMode::Pad, Transform::identity())
}
fn linear(a: (f32, f32), b: (f32, f32), st: &[(f32, [u8; 3], f32)]) -> Option<Shader<'static>> {
    LinearGradient::new(Point::from_xy(a.0, a.1), Point::from_xy(b.0, b.1), stops(st), SpreadMode::Pad, Transform::identity())
}
fn sp(sh: Shader<'static>, blend: BlendMode) -> Paint<'static> {
    Paint { shader: sh, anti_alias: true, blend_mode: blend, ..Default::default() }
}
fn fill_all(pm: &mut Pixmap, sh: Option<Shader<'static>>, blend: BlendMode, clip: Option<&Clip>) {
    if let Some(sh) = sh {
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        pm.fill_rect(Rect::from_xywh(0.0, 0.0, w, h).unwrap(), &sp(sh, blend), Transform::identity(), clip);
    }
}
fn circle_path(x: f32, y: f32, r: f32) -> Path {
    PathBuilder::from_circle(x, y, r.max(0.1)).unwrap()
}
fn fill_circle_sh(pm: &mut Pixmap, x: f32, y: f32, r: f32, sh: Option<Shader<'static>>) {
    if let Some(sh) = sh {
        pm.fill_path(&circle_path(x, y, r), &sp(sh, BlendMode::SourceOver), FillRule::Winding, Transform::identity(), None);
    }
}
fn ring(pm: &mut Pixmap, x: f32, y: f32, r: f32, w: f32, c: Color) {
    let st = Stroke { width: w, ..Default::default() };
    pm.stroke_path(&circle_path(x, y, r), &paint(c), &st, Transform::identity(), None);
}
fn ring_sh(pm: &mut Pixmap, x: f32, y: f32, r: f32, w: f32, sh: Option<Shader<'static>>) {
    if let Some(sh) = sh {
        let st = Stroke { width: w, ..Default::default() };
        pm.stroke_path(&circle_path(x, y, r), &sp(sh, BlendMode::SourceOver), &st, Transform::identity(), None);
    }
}
fn circle_clip(size: u32, x: f32, y: f32, r: f32) -> Option<Clip> {
    let mut m = Clip::new(size, size)?;
    m.fill_path(&circle_path(x, y, r), FillRule::Winding, true, Transform::identity());
    Some(m)
}
fn poly(pm: &mut Pixmap, pts: &[(f32, f32)], c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}
fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}
fn stroke_rrect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: Color) {
    if let Some(p) = rrect_path(x, y, w, h, r) {
        let st = Stroke { width, ..Default::default() };
        pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
    }
}
fn at(cx: f32, cy: f32, a: f32, r: f32) -> (f32, f32) {
    (cx + a.cos() * r, cy + a.sin() * r)
}
fn tick(pm: &mut Pixmap, cx: f32, cy: f32, a: f32, r1: f32, r2: f32, w: f32, c: Color) {
    let (x0, y0) = at(cx, cy, a, r1);
    let (x1, y1) = at(cx, cy, a, r2);
    line(pm, x0, y0, x1, y1, w, c);
}
/// A needle: a thin wedge from the hub (tail behind it) to the tip, rotated to angle a.
fn needle(pm: &mut Pixmap, cx: f32, cy: f32, a: f32, tail: f32, len: f32, w0: f32, w1: f32, c: Color) {
    let (ca, sa) = (a.cos(), a.sin());
    let p = |along: f32, across: f32| (cx + ca * along - sa * across, cy + sa * along + ca * across);
    poly(pm, &[p(-tail, -w0), p(len, -w1), p(len, w1), p(-tail, w0)], c);
}

/// Text with letter spacing (sp pixels between letters).
fn txt(f: &mut Fonts, pm: &mut Pixmap, fi: u8, s: &str, px: f32, x: f32, y: f32, align: u8, c: [u8; 3], a: f32, sp: f32) {
    if sp <= 0.0 {
        f.draw(pm, fi, s, px, x, y, align, c, a);
        return;
    }
    let ws: Vec<f32> = s.chars().map(|ch| f.width(fi, &ch.to_string(), px)).collect();
    let total = ws.iter().sum::<f32>() + sp * ws.len().saturating_sub(1) as f32;
    let mut pen = match align {
        1 => x - total / 2.0,
        2 => x - total,
        _ => x,
    };
    for (ch, w) in s.chars().zip(ws) {
        f.draw(pm, fi, &ch.to_string(), px, pen, y, 0, c, a);
        pen += w + sp;
    }
}
/// A number with a smaller unit after it, centred as one group on x.
fn num_unit(f: &mut Fonts, pm: &mut Pixmap, fi: u8, n: &str, px: f32, fu: u8, u: &str, pu: f32, x: f32, y: f32, c: [u8; 3], gap: f32) {
    let wn = f.width(fi, n, px);
    let wu = if u.is_empty() { 0.0 } else { f.width(fu, u, pu) + gap };
    let sx = x - (wn + wu) / 2.0;
    f.draw(pm, fi, n, px, sx, y, 0, c, 1.0);
    if !u.is_empty() {
        f.draw(pm, fu, u, pu, sx + wn + gap, y, 0, c, 1.0);
    }
}
/// Paint a static layer once per screen size and reuse it (copied, not redrawn) every frame.
fn cached(st: &mut ScreenState, pm: &mut Pixmap, key: String, make: impl FnOnce(&mut Pixmap)) {
    if st.face_bg.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
        let mut p = Pixmap::new(pm.width(), pm.height()).unwrap();
        make(&mut p);
        st.face_bg = Some((key, p));
    }
    if let Some((_, p)) = &st.face_bg {
        pm.data_mut().copy_from_slice(p.data());
    }
}

// ------------------------------------------------------------------ parts for custom layouts

/// Gauges taken from the full-screen designs, placed as parts in a custom layout.
/// (cx, cy) is the centre, bw × bh the part's box, col its colour, tc the text colour.
pub(super) fn part(pm: &mut Pixmap, f: &mut Fonts, kind: &str, m: &Rd, cx: f32, cy: f32, bw: f32, bh: f32, col: [u8; 3], tc: [u8; 3], label: &str, plate: bool, t: f32) {
    let cap = if label.is_empty() { m.name.clone() } else { label.to_uppercase() };
    let fr = m.frac();
    let shade = Color::from_rgba8(0, 0, 0, 120);
    match kind {
        "ring" => {
            let r = bw * 0.42;
            if plate {
                circle(pm, cx, cy, bw * 0.5, shade);
            }
            ring(pm, cx, cy, r, bw * 0.06, rgba([255, 255, 255], 0.09));
            let a0 = -PI / 2.0;
            let a1 = a0 + TAU * fr.max(0.005);
            let (ex, ey) = at(cx, cy, a1, r);
            glow(pm, ex, ey, bw * 0.14, col, 0.55);
            arc(pm, cx, cy, r, a0, a1, bw * 0.06, rgba(col, 1.0));
            circle(pm, ex, ey, bw * 0.034, rgba(mix(col, [255, 255, 255], 0.5), 1.0));
            txt(f, pm, COND, &cap, bw * 0.075, cx, cy - bw * 0.13, 1, col, 0.95, bw * 0.008);
            num_unit(f, pm, COND, &m.text, bw * 0.3, COND, m.glyph(), bw * 0.1, cx, cy + bw * 0.12, m.col(tc), bw * 0.008);
        }
        "dial" => {
            let r = bw * 0.47;
            if plate {
                circle(pm, cx, cy, r, Color::from_rgba8(0, 0, 0, 150));
            }
            ring(pm, cx, cy, r * 0.98, bw * 0.008, rgba(col, 0.6));
            let (a0, sw) = (PI * 0.75, PI * 1.5);
            let n = m.div.max(1) * 5;
            for i in 0..=n {
                let a = a0 + sw * i as f32 / n as f32;
                let major = i % 5 == 0;
                tick(pm, cx, cy, a, r * if major { 0.78 } else { 0.85 }, r * 0.92, bw * if major { 0.012 } else { 0.005 }, rgba(tc, if major { 0.9 } else { 0.45 }));
                if major {
                    let v = m.min + (m.max - m.min) * i as f32 / n as f32;
                    let (lx, ly) = at(cx, cy, a, r * 0.64);
                    f.draw(pm, TEXT, &format!("{}", v.round() as i64), bw * 0.06, lx, ly + bw * 0.022, 1, tc, 0.85);
                }
            }
            txt(f, pm, SEMI, &cap, bw * 0.048, cx, cy - bw * 0.15, 1, col, 1.0, bw * 0.01);
            num_unit(f, pm, LIGHT, &m.text, bw * 0.15, LIGHT, m.glyph(), bw * 0.06, cx, cy + bw * 0.33, m.col(tc), bw * 0.005);
            needle(pm, cx, cy, a0 + sw * fr, r * 0.14, r * 0.86, bw * 0.012, bw * 0.003, rgba(col, 1.0));
            circle(pm, cx, cy, bw * 0.026, rgba(col, 1.0));
            circle(pm, cx, cy, bw * 0.01, rgba([20, 20, 20], 1.0));
        }
        "rev" => {
            let r = bw * 0.46;
            if plate {
                circle(pm, cx, cy, r, shade);
            }
            let (a0, sw) = (PI * 0.8, PI * 1.4);
            arc(pm, cx, cy, r * 0.9, a0, a0 + sw, bw * 0.035, rgba(tc, 0.14));
            arc(pm, cx, cy, r * 0.9, a0 + sw * 0.8, a0 + sw, bw * 0.035, rgba(col, 0.9));
            arc(pm, cx, cy, r * 0.9, a0, a0 + sw * fr.max(0.003), bw * 0.035, rgba(col, 0.45));
            let n = m.div.max(1) * 2;
            for i in 0..=n {
                let a = a0 + sw * i as f32 / n as f32;
                let major = i % 2 == 0;
                tick(pm, cx, cy, a, r * if major { 0.7 } else { 0.76 }, r * 0.84, bw * if major { 0.016 } else { 0.007 }, rgba(tc, 0.9));
                if major {
                    let v = m.min + (m.max - m.min) * i as f32 / n as f32;
                    let (lx, ly) = at(cx, cy, a, r * 0.56);
                    f.draw(pm, SEMI, &format!("{}", v.round() as i64), bw * 0.062, lx, ly + bw * 0.022, 1, tc, 1.0);
                }
            }
            needle(pm, cx, cy, a0 + sw * fr, r * 0.16, r * 0.84, bw * 0.016, bw * 0.004, rgba(col, 1.0));
            circle(pm, cx, cy, bw * 0.04, rgba([30, 30, 30], 1.0));
            circle(pm, cx, cy, bw * 0.016, rgba(col, 1.0));
            txt(f, pm, TEXT, &cap, bw * 0.045, cx, cy + r * 0.32, 1, tc, 0.8, bw * 0.01);
            num_unit(f, pm, SEMI, &m.text, bw * 0.12, TEXT, m.glyph(), bw * 0.05, cx, cy + r * 0.62, m.col(tc), bw * 0.004);
        }
        "segments" => {
            let (x0, y0) = (cx - bw / 2.0, cy - bh / 2.0);
            if plate {
                rrect(pm, x0, y0, bw, bh, (bh * 0.18).min(bw * 0.05), shade);
            }
            let (ix, iw) = (x0 + bw * 0.04, bw * 0.92);
            txt(f, pm, MONO, &cap, bh * 0.26, ix, cy - bh * 0.06, 0, tc, 0.75, bh * 0.03);
            f.draw(pm, MONO, &m.short(), bh * 0.34, ix + iw, cy - bh * 0.06, 2, m.col(tc), 1.0);
            let n = 20;
            let sw = iw / n as f32;
            let lit = (fr * n as f32).round() as usize;
            for i in 0..n {
                let c = if i < lit { rgba(if i as f32 / n as f32 > 0.8 { mix(col, [255, 70, 60], 0.7) } else { col }, 1.0) } else { rgba(tc, 0.12) };
                rect(pm, ix + i as f32 * sw + sw * 0.1, cy + bh * 0.08, sw * 0.8, bh * 0.28, c);
            }
        }
        "frames" => {
            let (x0, y0) = (cx - bw / 2.0, cy - bh / 2.0);
            if plate {
                rrect(pm, x0, y0, bw, bh, (bh * 0.12).min(bw * 0.05), shade);
            }
            let (ix, iw) = (x0 + bw * 0.04, bw * 0.92);
            txt(f, pm, MONO, &cap, bh * 0.13, ix, y0 + bh * 0.2, 0, tc, 0.75, bh * 0.015);
            f.draw(pm, MONO, &m.short(), bh * 0.17, ix + iw, y0 + bh * 0.22, 2, m.col(tc), 1.0);
            let h = &m.hist;
            let n = 48usize;
            let (gy, gh) = (y0 + bh * 0.32, bh * 0.6);
            line(pm, ix, gy + gh, ix + iw, gy + gh, bh * 0.012, rgba(tc, 0.25));
            if !h.is_empty() {
                let tail = &h[h.len().saturating_sub(n)..];
                let mx = tail.iter().cloned().fold(1.0f64, f64::max);
                let mut sorted = tail.to_vec();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let med = sorted[sorted.len() / 2];
                let bw1 = iw / n as f32;
                let off = n - tail.len();
                for (i, v) in tail.iter().enumerate() {
                    let hh = (*v / mx) as f32 * gh;
                    let low = m.id == "fps" && *v < med * 0.7;
                    let c = if low { [255, 70, 60] } else { col };
                    rect(pm, ix + (off + i) as f32 * bw1 + bw1 * 0.15, gy + gh - hh, bw1 * 0.7, hh.max(1.0), rgba(c, if i + 1 == tail.len() { 1.0 } else { 0.75 }));
                }
            }
        }
        "orb" => {
            let r = bw * 0.36;
            let white = [255, 255, 255];
            fill_circle_sh(pm, cx, cy, r, radial((cx - r * 0.3, cy - r * 0.35), (cx, cy), r * 1.15, &[(0.0, mix(col, white, 0.55), 1.0), (0.6, col, 1.0), (1.0, mix(col, [0, 0, 0], 0.55), 1.0)]));
            if let Some(clip) = circle_clip(pm.width(), cx, cy, r) {
                for i in 0..3 {
                    let a = t * (0.6 + 0.3 * i as f32) + i as f32 * 2.1;
                    let (bx, by) = at(cx, cy, a, r * 0.45);
                    let c = mix(col, if i == 1 { white } else { [255, 120, 200] }, 0.45);
                    fill_all(pm, radial((bx, by), (bx, by), r * 0.6, &[(0.0, c, 0.45), (1.0, c, 0.0)]), BlendMode::Plus, Some(&clip));
                }
            }
            ring(pm, cx, cy, r * 1.2, bw * 0.004, rgba(col, 0.3));
            let a = -PI / 2.0 + t * (0.5 + 2.2 * fr);
            for k in 0..12 {
                let (qx, qy) = at(cx, cy, a - k as f32 * 0.05, r * 1.2);
                circle(pm, qx, qy, bw * (0.014 - k as f32 * 0.001).max(0.003), rgba(mix(col, white, 0.5), 1.0 - k as f32 / 12.0));
            }
            txt(f, pm, BOLD, &cap, bw * 0.055, cx, cy - r * 0.32, 1, white, 0.88, bw * 0.012);
            num_unit(f, pm, BLACK, &m.text, bw * 0.17, BOLD, m.glyph(), bw * 0.065, cx, cy + r * 0.3, m.col(white), bw * 0.005);
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ faces

pub(super) fn draw(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, name: &str, m: &Rd, s2: &Rd, sensors: &SensorState, round: bool, t: f32, o: O<'_>) {
    match name {
        "telemetry" => telemetry(pm, f, st, m, s2, sensors, round, t, false, o),
        "telemetry-white" => telemetry(pm, f, st, m, s2, sensors, round, t, true, o),
        "chronograph" => chronograph(pm, f, st, m, s2, round, o),
        "plasma" => plasma(pm, f, m, s2, round, t, o),
        "porcelain" => porcelain(pm, f, st, m, s2, o),
        "instrument" => instrument(pm, f, st, m, s2, round, o),
        "frost" => frost(pm, f, m, s2, t, o),
        "poster" => poster(pm, f, st, m, s2, o),
        _ => afterglow(pm, f, st, m, s2, o),
    }
}

/// A: rev counter for frames. Ring of ticks fills with the reading, the second reading sits in the gap.
fn afterglow(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, m: &Rd, s2: &Rd, o: O) {
    let s = pm.width() as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    cached(st, pm, format!("afterglow{s}"), |p| {
        fill_all(p, radial((cx, cy * 0.8), (cx, cy * 0.8), s * 0.78, &[(0.0, hex("#112640"), 1.0), (0.5, hex("#071020"), 1.0), (1.0, hex("#020307"), 1.0)]), BlendMode::SourceOver, None);
    });
    let (r, a0, sw, n) = (s * 0.42, PI * 0.75, PI * 1.5, 54usize);
    let lit = m.frac() * n as f32;
    let dial = o.el("dial", s);
    let nums = o.el("dial_numbers", s);
    let (dx, dy, dr) = (dial.x(cx), dial.y(cy), dial.z(r));
    if dial.show {
        arc(pm, dx, dy, dr - dial.z(s * 0.022), a0, a0 + sw * m.frac().max(0.002), dial.z(s * 0.08), rgba([60, 200, 230], 0.09));
    }
    for i in 0..=n {
        let fr = i as f32 / n as f32;
        let a = a0 + sw * fr;
        let maj = i % 9 == 0;
        let on = i as f32 <= lit;
        let col = match o.acc {
            Some(a) => mix(a, [255, 255, 255], fr * 0.35),
            None => ramp(ACC, fr),
        };
        if dial.show {
            if on && i as f32 > lit - 1.0 {
                let (gx, gy) = at(dx, dy, a, dr - dial.z(s * 0.02));
                glow(pm, gx, gy, dial.z(s * 0.07), col, 0.6);
            }
            tick(pm, dx, dy, a, dr - dial.z(if maj { s * 0.055 } else { s * 0.035 }), dr, dial.z(if maj { s * 0.011 } else { s * 0.0075 }), if on { rgba(col, 1.0) } else { rgba(hex("#17222f"), 1.0) });
        }
        if maj && nums.show {
            let (lx, ly) = at(nums.x(cx), nums.y(cy), a, nums.z(r - s * 0.092));
            f.draw(pm, COND, &format!("{}", (m.min + (m.max - m.min) * fr).round() as i64), nums.z(s * 0.034 * o.l), lx, ly + nums.z(s * 0.012), 1, if on { hex("#9fb4cc") } else { hex("#3d4d61") }, 1.0);
        }
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, COND, &m.name, e.z(s * 0.042 * o.l), e.x(cx), e.y(cy - s * 0.165), 1, hex("#8ea2ba"), 1.0, e.z(s * 0.012));
    }
    let e = o.el("num", s);
    if e.show {
        glow(pm, e.x(cx), e.y(cy), e.z(s * 0.24), [70, 210, 235], 0.16);
        num_unit(f, pm, COND, &m.text, e.z(s * 0.35 * o.n), COND, m.glyph(), e.z(s * 0.11 * o.n), e.x(cx), e.y(cy + s * 0.115), m.col(hex("#f2f8ff")), e.z(s * 0.008));
    }
    let tc = s2.col(s2.temp().map(|v| ramp(TEMP_DARK, v)).unwrap_or(hex("#4f9dff")));
    let e = o.el("sub_label", s);
    if e.show {
        txt(f, pm, COND, &s2.tag, e.z(s * 0.036 * o.l), e.x(cx), e.y(cy + s * 0.25), 1, hex("#8297b0"), 1.0, e.z(s * 0.01));
    }
    let e = o.el("sub_value", s);
    if e.show {
        glow(pm, e.x(cx), e.y(cy + s * 0.31), e.z(s * 0.1), tc, 0.18);
        f.draw(pm, COND, &s2.short(), e.z(s * 0.09 * o.l), e.x(cx), e.y(cy + s * 0.34), 1, tc, 1.0);
    }
    let e = o.el("sub_bar", s);
    if e.show {
        let (bw, bh) = (e.z(s * 0.16), e.z(s * 0.012));
        let (bx, by) = (e.x(cx) - bw / 2.0, e.y(cy + s * 0.37));
        rrect(pm, bx, by, bw, bh, bh / 2.0, rgba(hex("#17222f"), 1.0));
        rrect(pm, bx, by, (bw * s2.frac()).max(bh), bh, bh / 2.0, rgba(tc, 1.0));
    }
}

fn screw(pm: &mut Pixmap, x: f32, y: f32, r: f32) {
    fill_circle_sh(pm, x, y, r, radial((x - r * 0.3, y - r * 0.3), (x, y), r, &[(0.0, hex("#5a5e66"), 1.0), (1.0, hex("#1d1f23"), 1.0)]));
    line(pm, x - r * 0.6, y - r * 0.25, x + r * 0.6, y + r * 0.25, r * 0.28, rgba([0, 0, 0], 0.7));
}

/// B: watch dial. FPS marker on the outer track, the second reading on a sub-dial with a needle.
fn chronograph(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, m: &Rd, s2: &Rd, round: bool, o: O) {
    let s = pm.width() as f32;
    let (cx, cy, dr) = (s / 2.0, s / 2.0, s * 0.48);
    let gold = o.acc.unwrap_or(hex("#e6c07a"));
    let ivory = hex("#f3ecdc");
    let sd = o.el("subdial", s);
    let (sx, sy, sr) = (sd.x(cx), sd.y(cy + s * 0.21), sd.z(s * 0.13));
    cached(st, pm, format!("chrono{s}{round}{}", sd.key()), |p| {
        if round {
            p.fill(rgba(hex("#101114"), 1.0));
        } else {
            fill_all(p, linear((0.0, 0.0), (s, s), &[(0.0, hex("#2c2e33"), 1.0), (0.5, hex("#16171a"), 1.0), (1.0, hex("#25272b"), 1.0)]), BlendMode::SourceOver, None);
            let mut y = 0.5;
            while y < s {
                line(p, 0.0, y, s, y, 1.0, rgba([255, 255, 255], 0.022));
                y += 3.0;
            }
            for (x, y) in [(0.075, 0.075), (0.925, 0.075), (0.075, 0.925), (0.925, 0.925)] {
                screw(p, x * s, y * s, s * 0.022);
            }
        }
        fill_circle_sh(p, cx, cy, dr, radial((cx, cy - s * 0.14), (cx, cy), dr, &[(0.0, hex("#3b3f47"), 1.0), (0.6, hex("#22252a"), 1.0), (1.0, hex("#131417"), 1.0)]));
        let mut r = s * 0.03;
        while r < dr {
            ring(p, cx, cy, r, 1.0, rgba([255, 255, 255], 0.02 + 0.014 * (r * 0.7).sin()));
            r += s * 0.0075;
        }
        for i in 0..120 {
            tick(p, cx, cy, i as f32 / 120.0 * TAU, 0.0, dr, 1.0, rgba([255, 255, 255], 0.014));
        }
        ring_sh(p, cx, cy, dr - s * 0.006, s * 0.012, linear((0.0, cy - dr), (0.0, cy + dr), &[(0.0, [255, 255, 255], 0.3), (0.5, [255, 255, 255], 0.04), (1.0, [0, 0, 0], 0.55)]));
        if sd.show {
            // sub-dial face
            fill_circle_sh(p, sx, sy, sr, radial((sx, sy - sr * 0.3), (sx, sy), sr, &[(0.0, hex("#2b2e34"), 1.0), (1.0, hex("#111215"), 1.0)]));
            ring(p, sx, sy, sr, s * 0.004, rgba(hex("#d9b26f"), 0.55));
            let mut r = sr * 0.2;
            while r < sr - s * 0.004 {
                ring(p, sx, sy, r, 1.0, rgba([255, 255, 255], 0.03));
                r += s * 0.005;
            }
            let sa = |fr: f32| PI * 0.75 + fr * PI * 1.5;
            for i in 0..=12 {
                let big = i % 3 == 0;
                let l = sd.z(if big { s * 0.022 } else { s * 0.012 });
                tick(p, sx, sy, sa(i as f32 / 12.0), sr - sd.z(s * 0.01) - l, sr - sd.z(s * 0.01), sd.z(if big { s * 0.005 } else { s * 0.003 }), rgba(hex("#ebe4d4"), if big { 1.0 } else { 0.45 }));
            }
        }
    });
    let ang = |fr: f32| -PI / 2.0 + (-150.0 + 300.0 * fr) * PI / 180.0;
    let (n, ro) = (120usize, s * 0.445);
    let tr = o.el("track", s);
    let nums = o.el("dial_numbers", s);
    let (tx, ty, tro) = (tr.x(cx), tr.y(cy), tr.z(ro));
    for i in 0..=n {
        let fr = i as f32 / n as f32;
        let (big, mid) = (i % 20 == 0, i % 5 == 0);
        let on = fr <= m.frac() + 1e-6;
        if tr.show {
            let c = if on { if big { rgba(hex("#f4e2b8"), 1.0) } else { rgba(gold, 0.95) } } else if big { rgba(hex("#ebe4d4"), 1.0) } else { rgba(hex("#ebe4d4"), 0.33) };
            let len = tr.z(if big { s * 0.045 } else if mid { s * 0.028 } else { s * 0.016 });
            tick(pm, tx, ty, ang(fr), tro - len, tro, tr.z(if big { s * 0.008 } else if mid { s * 0.005 } else { s * 0.003 }), c);
        }
        if big && nums.show {
            let v = m.min + (m.max - m.min) * fr;
            let (lx, ly) = at(nums.x(cx), nums.y(cy), ang(fr), nums.z(ro - s * 0.08));
            f.draw(pm, TEXT, &format!("{}", v.round() as i64), nums.z(s * 0.04 * o.l), lx, ly + nums.z(s * 0.014), 1, hex("#ebe4d4"), 1.0);
        }
    }
    if tr.show {
        // gold marker riding the track
        let av = ang(m.frac());
        let (mx, my) = at(tx, ty, av, tro + tr.z(s * 0.01));
        let (ca, sa2) = ((av + PI).cos(), (av + PI).sin());
        let (px, py) = (-sa2, ca);
        let (a1, a2) = (tr.z(s * 0.034), tr.z(s * 0.017));
        glow(pm, mx, my, tr.z(s * 0.05), gold, 0.35);
        poly(pm, &[(mx + ca * a1, my + sa2 * a1), (mx + px * a2, my + py * a2), (mx - px * a2, my - py * a2)], rgba(m.col(gold), 1.0));
    }
    let e = o.el("footer", s);
    if e.show {
        let foot = if m.id == "fps" { "FRAMES PER SECOND".to_string() } else { m.name.clone() };
        txt(f, pm, TEXT, &foot, e.z(s * 0.025 * o.l), e.x(cx), e.y(cy + s * 0.432), 1, gold, 0.8, e.z(s * 0.008));
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, SEMI, &m.name, e.z(s * 0.034 * o.l), e.x(cx), e.y(cy - s * 0.16), 1, gold, 1.0, e.z(s * 0.018));
    }
    let e = o.el("num", s);
    if e.show {
        num_unit(f, pm, LIGHT, &m.text, e.z(s * 0.24 * o.n), LIGHT, m.glyph(), e.z(s * 0.08 * o.n), e.x(cx), e.y(cy + s * 0.04), m.col(ivory), e.z(s * 0.006));
        rect(pm, e.x(cx - s * 0.05), e.y(cy + s * 0.062), e.z(s * 0.1), e.z(s * 0.003), rgba(gold, 0.7));
    }
    // sub-dial reading
    let (sx0, sy0, sr0) = (cx, cy + s * 0.21, s * 0.13);
    let e = o.el("sub_label", s);
    if e.show {
        txt(f, pm, SEMI, &s2.tag, e.z(s * 0.027 * o.l), e.x(sx0), e.y(sy0 - sr0 * 0.4), 1, gold, 1.0, e.z(s * 0.008));
    }
    let e = o.el("sub_value", s);
    if e.show {
        f.draw(pm, TEXT, &format!("{}{}", s2.text, if s2.unit == "°C" { "°" } else { s2.glyph() }), e.z(s * 0.052 * o.l), e.x(sx0), e.y(sy0 + sr0 * 0.72), 1, s2.col(ivory), 1.0);
    }
    if sd.show {
        let na = PI * 0.75 + s2.frac() * PI * 1.5;
        needle(pm, sx + sd.z(s * 0.002), sy + sd.z(s * 0.005), na, sr * 0.24, sr * 0.84, sd.z(s * 0.0045), sd.z(s * 0.0005), rgba([0, 0, 0], 0.45));
        needle(pm, sx, sy, na, sr * 0.24, sr * 0.84, sd.z(s * 0.0045), sd.z(s * 0.0005), rgba(s2.col(gold), 1.0));
        circle(pm, sx, sy, sd.z(s * 0.012), rgba(gold, 1.0));
        circle(pm, sx, sy, sd.z(s * 0.0045), rgba(hex("#17181b"), 1.0));
    }
    // crystal reflection
    if let Some(clip) = circle_clip(pm.width(), cx, cy, dr) {
        fill_all(pm, linear((cx - dr, cy - dr), (cx + dr * 0.2, cy + dr * 0.2), &[(0.0, [255, 255, 255], 0.075), (0.5, [255, 255, 255], 0.0)]), BlendMode::SourceOver, Some(&clip));
    }
}

const COOL: [&str; 4] = ["#1f4bff", "#00c8ff", "#7b3cff", "#00f0c0"];
const HOT: [&str; 4] = ["#ff2d6f", "#ff7a00", "#ffcf3f", "#ff3dd6"];

/// C: an orb of moving light, with comets on the ring, a turning dashed ring and rising sparks.
/// Everything moves faster as the big reading climbs and turns warmer as the GPU heats up.
fn plasma(pm: &mut Pixmap, f: &mut Fonts, m: &Rd, s2: &Rd, round: bool, t: f32, o: O) {
    let s = pm.width() as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let k = ((s2.temp().unwrap_or(60.0) - 52.0) / 28.0).clamp(0.0, 1.0);
    let mut cols: Vec<[u8; 3]> = (0..4).map(|i| mix(hex(COOL[i]), hex(HOT[i]), k)).collect();
    if let Some(a) = o.acc {
        cols[1] = a;
    }
    let energy = 0.35 + m.frac() * 0.9;
    pm.fill(rgba(hex("#030308"), 1.0));
    // stars
    let mut rng = super::Rng(7);
    for _ in 0..70 {
        let (x, y, rr, a) = (rng.f(), rng.f(), rng.f() * 1.2 + 0.3, rng.f() * 0.5 + 0.15);
        if round && ((x - 0.5).powi(2) + (y - 0.5).powi(2)) > 0.25 {
            continue;
        }
        circle(pm, x * s, y * s, rr * s / 400.0, rgba([255, 255, 255], a * (0.6 + 0.4 * (t * 1.3 + x * 20.0).sin())));
    }
    let ob = o.el("orb", s);
    let (ox, oy, r) = (ob.x(cx), ob.y(cy), ob.z(s * 0.33));
    if ob.show {
        glow(pm, ox, oy, r * 1.6, cols[0], 0.42);
    }
    // ring with the reading, and comets travelling round it
    let rg = o.el("ring", s);
    if rg.show {
        let (rx, ry, rr2) = (rg.x(cx), rg.y(cy), rg.z(s * 0.435));
        ring(pm, rx, ry, rr2, rg.z(s * 0.008), rgba([255, 255, 255], 0.07));
        arc(pm, rx, ry, rr2, -PI / 2.0, -PI / 2.0 + TAU * m.frac().max(0.002), rg.z(s * 0.03), rgba(cols[1], 0.12));
        arc(pm, rx, ry, rr2, -PI / 2.0, -PI / 2.0 + TAU * m.frac().max(0.002), rg.z(s * 0.008), rgba(m.col(cols[1]), 1.0));
        for c in 0..3 {
            let head = t * (0.5 + 2.2 * m.frac()) + c as f32 * TAU / 3.0;
            let col = if c == 1 { cols[3] } else { cols[1] };
            for j in 0..16 {
                let a = head - j as f32 * 0.035;
                let (x, y) = at(rx, ry, a, rr2);
                let fade = 1.0 - j as f32 / 16.0;
                circle(pm, x, y, rg.z(s * (0.004 + 0.007 * fade)), rgba(col, 0.9 * fade * fade));
            }
            let (hx, hy) = at(rx, ry, head, rr2);
            glow(pm, hx, hy, rg.z(s * 0.045), col, 0.7);
            circle(pm, hx, hy, rg.z(s * 0.006), rgba([255, 255, 255], 0.95));
        }
    }
    // dashed ring turning the other way
    let ds = o.el("dashes", s);
    if ds.show {
        let dash_r = ds.z(s * 0.375);
        let spin = -t * 0.25 * energy;
        for i in 0..48 {
            let a = spin + i as f32 / 48.0 * TAU;
            arc(pm, ds.x(cx), ds.y(cy), dash_r, a, a + TAU / 48.0 * 0.45, ds.z(s * 0.004), rgba(cols[2], 0.35));
        }
    }
    // the orb
    if ob.show {
        if let Some(clip) = circle_clip(pm.width(), ox, oy, r) {
            pm.fill_path(&circle_path(ox, oy, r), &paint(rgba(hex("#08061c"), 1.0)), FillRule::Winding, Transform::identity(), None);
            let blobs = [(0.9, 0.3, 0.0, 1.1), (0.7, 0.55, 2.0, 1.7), (1.1, 0.4, 4.0, 0.6), (0.5, 0.8, 1.0, 3.3)];
            for (i, (p0, p1, p2, p3)) in blobs.iter().enumerate() {
                let x = ox + (t * p0 * energy + p2).sin() * r * 0.5;
                let y = oy + (t * p1 * energy + p3).cos() * r * 0.45;
                fill_all(pm, radial((x, y), (x, y), r * 0.95, &[(0.0, cols[i], 0.9), (0.55, cols[i], 0.33), (1.0, cols[i], 0.0)]), BlendMode::Plus, Some(&clip));
            }
            // sparks rising through the orb
            for i in 0..14 {
                let ph = i as f32 * 0.618;
                let rise = ((t * (0.08 + 0.12 * energy) + ph) % 1.0 + 1.0) % 1.0;
                let x = ox + ((i as f32 * 2.399).sin() * 0.7 + (t * 0.7 + ph * 6.0).sin() * 0.08) * r;
                let y = oy + r - rise * r * 2.0;
                let a = (rise * PI).sin() * 0.8;
                circle(pm, x, y, ob.z(s * 0.004), rgba([255, 255, 255], a));
            }
            fill_all(pm, radial((ox, oy), (ox, oy), r, &[(0.55, [0, 0, 0], 0.0), (1.0, [0, 0, 12], 0.6)]), BlendMode::SourceOver, Some(&clip));
            let (hx, hy) = (ox - r * 0.35, oy - r * 0.48);
            fill_all(pm, radial((hx, hy), (hx, hy), r * 0.7, &[(0.0, [255, 255, 255], 0.34), (1.0, [255, 255, 255], 0.0)]), BlendMode::SourceOver, Some(&clip));
        }
        ring(pm, ox, oy, r, ob.z(s * 0.004), rgba([255, 255, 255], 0.2));
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, BOLD, &m.name, e.z(s * 0.034 * o.l), e.x(cx), e.y(cy - s * 0.108), 1, [255, 255, 255], 0.85, e.z(s * 0.02));
    }
    let e = o.el("num", s);
    if e.show {
        glow(pm, e.x(cx), e.y(cy), e.z(s * 0.2), [0, 0, 20], 0.35);
        num_unit(f, pm, BLACK, &m.text, e.z(s * 0.175 * o.n), BOLD, m.glyph(), e.z(s * 0.07 * o.n), e.x(cx), e.y(cy + s * 0.062), m.col([255, 255, 255]), e.z(s * 0.006));
    }
    let e = o.el("sub_value", s);
    if e.show {
        let (pw, ph) = (e.z(s * 0.36), e.z(s * 0.082));
        let (px, py) = (e.x(cx) - pw / 2.0, e.y(cy + s * 0.105));
        rrect(pm, px, py, pw, ph, ph / 2.0, rgba([0, 0, 0], 0.34));
        stroke_rrect(pm, px, py, pw, ph, ph / 2.0, e.z(s * 0.003), rgba([255, 255, 255], 0.24));
        f.draw(pm, BOLD, &format!("{}  {}", s2.tag, s2.short()), e.z(s * 0.035 * o.l), e.x(cx), py + ph * 0.7, 1, s2.col([255, 255, 255]), 1.0);
    }
}

/// D: telemetry. Big fixed-width number, a frame-time graph that marks stutters, the 1% low, and the
/// second reading as a segmented bar. `white` is the light version.
fn telemetry(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, m: &Rd, s2: &Rd, sensors: &SensorState, round: bool, t: f32, white: bool, o: O) {
    let s = pm.width() as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let (bg, dot, acc, fg, muted, light, off, spike, temps): ([u8; 3], ([u8; 3], f32), [u8; 3], [u8; 3], [u8; 3], [u8; 3], [u8; 3], [u8; 3], &[(f32, &str)]) = if white {
        (hex("#fbfbfc"), ([20, 25, 35], 0.09), hex("#f07800"), hex("#11151b"), hex("#5f6670"), hex("#262b33"), hex("#e4e7eb"), hex("#e03131"), TEMP_LIGHT)
    } else {
        (hex("#08090c"), ([255, 255, 255], 0.07), hex("#ffb020"), hex("#f3f5f7"), hex("#7b8494"), hex("#c9ced6"), hex("#1c2027"), hex("#ff4d5e"), TEMP_DARK)
    };
    cached(st, pm, format!("tele{white}{s}"), |p| {
        p.fill(rgba(bg, 1.0));
        let step = s * 0.04;
        let mut y = step / 2.0;
        while y < s {
            let mut x = step / 2.0;
            while x < s {
                rect(p, x - s * 0.002, y - s * 0.002, s * 0.004, s * 0.004, rgba(dot.0, dot.1));
                x += step;
            }
            y += step;
        }
    });
    let _ = round;
    let acc = o.acc.unwrap_or(acc);
    // label with a live dot
    let top = cy - s * 0.25;
    let lw = f.width(MONO, &m.name, s * 0.038 * o.l) + s * 0.012 * (m.name.chars().count().saturating_sub(1)) as f32;
    let e = o.el("dot", s);
    if e.show {
        circle(pm, e.x(cx - lw / 2.0 - s * 0.012), e.y(top - s * 0.013), e.z(s * 0.01), rgba(acc, 0.55 + 0.45 * (t * 4.0).sin()));
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, MONO, &m.name, e.z(s * 0.038 * o.l), e.x(cx + s * 0.012), e.y(top), 1, acc, 1.0, e.z(s * 0.012));
    }
    let e = o.el("num", s);
    if e.show {
        num_unit(f, pm, MONO, &m.text, e.z(s * 0.24 * o.n), MONO, m.glyph(), e.z(s * 0.08 * o.n), e.x(cx), e.y(cy - s * 0.025), m.col(fg), e.z(s * 0.006));
    }
    // graph: frame time when the big number is FPS, else the big reading's last minute
    let fps_mode = m.id == "fps";
    let mut h: Vec<f64> = if fps_mode {
        let ft = sensors.history.get("frametime").cloned().unwrap_or_default();
        if ft.is_empty() { m.hist.iter().filter(|v| **v > 1.0).map(|v| 1000.0 / v).collect() } else { ft }
    } else {
        m.hist.clone()
    };
    if h.len() > 60 {
        h = h.split_off(h.len() - 60);
    }
    let (cap_l, cap_r) = if fps_mode {
        let low = sensors.get("fps.low1").map(|r| r.value).or_else(|| {
            let mut v: Vec<f64> = h.iter().copied().filter(|x| *x > 0.0).collect();
            v.sort_by(|a, b| b.partial_cmp(a).unwrap());
            v.get(v.len() / 100).map(|ft| 1000.0 / ft)
        });
        ("FRAME TIME".to_string(), format!("1% LOW {}", low.map(|v| (v.round() as i64).to_string()).unwrap_or("--".into())))
    } else {
        let peak = h.iter().copied().fold(f64::MIN, f64::max);
        ("LAST MINUTE".to_string(), format!("PEAK {}", if peak > f64::MIN { format!("{}", peak.round() as i64) } else { "--".into() }))
    };
    let e = o.el("graph", s);
    if e.show {
        let (gx, gw, gy, gh) = (e.x(cx - s * 0.3), e.z(s * 0.6), e.y(cy + s * 0.015), e.z(s * 0.11));
        line(pm, gx, gy + gh, gx + gw, gy + gh, e.z(s * 0.002), rgba(fg, 0.14));
        if !h.is_empty() {
            let mut srt = h.clone();
            srt.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = srt[srt.len() / 2];
            let (lo, hi) = if fps_mode { (0.0, (med * 2.2).max(*srt.last().unwrap()).max(1.0)) } else { (srt[0] - (srt.last().unwrap() - srt[0]).max(1.0) * 0.3, srt.last().unwrap() + 0.5) };
            let bw = gw / 60.0;
            let off_x = gx + gw - bw * h.len() as f32;
            for (i, v) in h.iter().enumerate() {
                let hh = (gh * ((v - lo) / (hi - lo)) as f32).clamp(e.z(s * 0.006), gh);
                let pos = (60 - h.len() + i) as f32 / 60.0;
                let c = if fps_mode && *v > med * 1.4 { rgba(spike, 1.0) } else { rgba(acc, 0.3 + 0.7 * pos) };
                rect(pm, off_x + i as f32 * bw + bw * 0.18, gy + gh - hh, bw * 0.64, hh, c);
            }
        }
    }
    let e = o.el("captions", s);
    if e.show {
        let y = e.y(cy + s * 0.015 + s * 0.11 + s * 0.046);
        txt(f, pm, MONO_R, &cap_l, e.z(s * 0.03 * o.l), e.x(cx - s * 0.3), y, 0, muted, 1.0, e.z(s * 0.003));
        txt(f, pm, MONO_R, &cap_r, e.z(s * 0.03 * o.l), e.x(cx + s * 0.3), y, 2, light, 1.0, e.z(s * 0.003));
    }
    // second reading as a segmented bar
    let y2 = cy + s * 0.272;
    let tc = s2.col(s2.temp().map(|v| ramp(temps, v)).unwrap_or(fg));
    let e = o.el("sub_label", s);
    if e.show {
        txt(f, pm, MONO, &s2.tag, e.z(s * 0.036 * o.l), e.x(cx - s * 0.15), e.y(y2), 0, muted, 1.0, e.z(s * 0.008));
    }
    let e = o.el("sub_value", s);
    if e.show {
        f.draw(pm, MONO, &s2.short(), e.z(s * 0.06 * o.l), e.x(cx + s * 0.15), e.y(y2), 2, tc, 1.0);
    }
    let e = o.el("sub_bar", s);
    if e.show {
        let n = 12;
        let sw = s * 0.3 / n as f32;
        let lit = (s2.frac() * n as f32).round() as usize;
        for i in 0..n {
            let c = if i < lit {
                match s2.temp() {
                    Some(_) => ramp(temps, s2.min + (s2.max - s2.min) * (i as f32 + 0.5) / n as f32),
                    None => acc,
                }
            } else {
                off
            };
            rect(pm, e.x(cx - s * 0.15 + i as f32 * sw + s * 0.002), e.y(y2 + s * 0.024), e.z(sw - s * 0.006), e.z(s * 0.026), rgba(c, 1.0));
        }
    }
}

/// E: white ceramic with a pressed-in ring that fills with the reading.
fn porcelain(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, m: &Rd, s2: &Rd, o: O) {
    let s = pm.width() as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let rg = o.el("ring", s);
    let (rx, ry, r, w) = (rg.x(cx), rg.y(cy), rg.z(s * 0.37), rg.z(s * 0.056));
    cached(st, pm, format!("porcelain{s}{}", rg.key()), |p| {
        fill_all(p, radial((cx, cy - s * 0.08), (cx, cy), s * 0.72, &[(0.0, [255, 255, 255], 1.0), (0.7, hex("#f3f5f9"), 1.0), (1.0, hex("#e1e6ed"), 1.0)]), BlendMode::SourceOver, None);
        if rg.show {
            ring_sh(p, rx, ry, r, w, linear((0.0, ry - r), (0.0, ry + r), &[(0.0, hex("#d6dce5"), 1.0), (1.0, hex("#eef1f5"), 1.0)]));
            ring(p, rx, ry, r + w / 2.0 + rg.z(s * 0.003), rg.z(s * 0.004), rgba([255, 255, 255], 1.0));
            ring(p, rx, ry, r - w / 2.0 - rg.z(s * 0.002), rg.z(s * 0.004), rgba([20, 30, 60], 0.07));
        }
    });
    let (ca, cb) = match m.temp() {
        Some(v) => (ramp(TEMP_LIGHT, v), ramp(TEMP_LIGHT, v)),
        None => match o.acc {
            Some(a) => (a, mix(a, [255, 255, 255], 0.25)),
            None => (hex("#4c7dff"), hex("#8b5cf6")),
        },
    };
    let (ca, cb) = (m.col(ca), m.col(cb));
    if rg.show {
        let (a0, a1) = (-PI / 2.0, -PI / 2.0 + TAU * m.frac().max(0.003));
        // soft shadow under the fill, then the fill blending from one colour to the other
        arc(pm, rx, ry + rg.z(s * 0.008), r, a0, a1, w * 0.62 + rg.z(s * 0.012), rgba(ca, 0.12));
        let steps = 24;
        for i in 0..steps {
            let fa = a0 + (a1 - a0) * i as f32 / steps as f32;
            let fb = a0 + (a1 - a0) * (i + 1) as f32 / steps as f32;
            arc(pm, rx, ry, r, fa, fb, w * 0.62, rgba(mix(ca, cb, i as f32 / steps as f32), 1.0));
        }
        let (kx, ky) = at(rx, ry, a1, r);
        glow(pm, kx, ky + rg.z(s * 0.006), rg.z(s * 0.05), [20, 30, 60], 0.18);
        circle(pm, kx, ky, rg.z(s * 0.03), rgba([255, 255, 255], 1.0));
        circle(pm, kx, ky, rg.z(s * 0.011), rgba(cb, 1.0));
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, SEMI, &m.name, e.z(s * 0.04 * o.l), e.x(cx), e.y(cy - s * 0.125), 1, hex("#6b7486"), 1.0, e.z(s * 0.014));
    }
    let e = o.el("num", s);
    if e.show {
        num_unit(f, pm, BOLD, &m.text, e.z(s * 0.24 * o.n), SEMI, m.glyph(), e.z(s * 0.09 * o.n), e.x(cx), e.y(cy + s * 0.085), m.col(hex("#151b27")), e.z(s * 0.006));
    }
    let e = o.el("sub_value", s);
    if e.show {
        let line_txt = format!("{} {}", s2.tag, s2.short());
        let px = e.z(s * 0.046 * o.l);
        let wd = f.width(SEMI, &line_txt, px);
        let dc = s2.col(s2.temp().map(|v| ramp(TEMP_LIGHT, v)).unwrap_or(cb));
        circle(pm, e.x(cx) - wd / 2.0 - e.z(s * 0.006), e.y(cy + s * 0.19), e.z(s * 0.011), rgba(dc, 1.0));
        f.draw(pm, SEMI, &line_txt, px, e.x(cx + s * 0.012), e.y(cy + s * 0.205), 1, s2.col(hex("#3f4757")), 1.0);
    }
}

/// F: white gauge with black ticks and an orange needle; the number sits in the open space at the bottom.
fn instrument(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, m: &Rd, s2: &Rd, round: bool, o: O) {
    let s = pm.width() as f32;
    let (cx, cy, r) = (s / 2.0, s / 2.0, s * 0.43);
    let orange = o.acc.unwrap_or(hex("#ff5a1f"));
    let a = |fr: f32| PI * 0.75 + fr * PI * 1.5;
    let n = (m.div * 8) as usize;
    let hot_from = if m.unit == "°C" { Some(((80.0 - m.min) / (m.max - m.min)).clamp(0.0, 1.0)) } else { None };
    let dl = o.el("dial", s);
    let (dx, dy, dr) = (dl.x(cx), dl.y(cy), dl.z(r));
    cached(st, pm, format!("instr{s}{round}{}{}{}{orange:?}{}", m.min, m.max, m.div, dl.key()), |p| {
        fill_all(p, radial((cx, cy), (cx, cy), s * 0.72, &[(0.0, [255, 255, 255], 1.0), (0.28, [255, 255, 255], 1.0), (1.0, hex("#ebe9e4"), 1.0)]), BlendMode::SourceOver, None);
        if !round {
            ring(p, cx, cy, s * 0.475, s * 0.004, rgba(hex("#dad8d2"), 1.0));
        }
        if dl.show {
            if let Some(h) = hot_from {
                arc(p, dx, dy, dr - dl.z(s * 0.006), a(h), a(1.0), dl.z(s * 0.012), rgba(orange, 1.0));
            }
            for i in 0..=n {
                let maj = i % 8 == 0;
                tick(p, dx, dy, a(i as f32 / n as f32), dr - dl.z(if maj { s * 0.055 } else { s * 0.026 }), dr, dl.z(if maj { s * 0.009 } else { s * 0.0045 }), rgba(hex("#111111"), if maj { 1.0 } else { 0.42 }));
            }
        }
    });
    let e = o.el("dial_numbers", s);
    if e.show {
        for i in (0..=n).step_by(8) {
            let v = m.min + (m.max - m.min) * i as f32 / n as f32;
            let (lx, ly) = at(e.x(cx), e.y(cy), a(i as f32 / n as f32), e.z(r - s * 0.1));
            f.draw(pm, TEXT, &format!("{}", v.round() as i64), e.z(s * 0.044 * o.l), lx, ly + e.z(s * 0.015), 1, hex("#111111"), 1.0);
        }
    }
    let e = o.el("sub_value", s);
    if e.show {
        let (ww, wh) = (e.z(s * 0.33), e.z(s * 0.074));
        let (wx, wy) = (e.x(cx) - ww / 2.0, e.y(cy + s * 0.062));
        rrect(pm, wx, wy, ww, wh, e.z(s * 0.012), rgba(hex("#f1f0ec"), 1.0));
        stroke_rrect(pm, wx, wy, ww, wh, e.z(s * 0.012), e.z(s * 0.003), rgba(hex("#dcdad3"), 1.0));
        f.draw(pm, TEXT, &format!("{} {}", s2.tag, s2.short()), e.z(s * 0.038 * o.l), e.x(cx), wy + wh * 0.7, 1, s2.col(hex("#2a2a2a")), 1.0);
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, TEXT, &m.name, e.z(s * 0.032 * o.l), e.x(cx), e.y(cy + s * 0.205), 1, hex("#6f6e69"), 1.0, e.z(s * 0.018));
    }
    let e = o.el("num", s);
    if e.show {
        num_unit(f, pm, SEMI, &m.text, e.z(s * 0.14 * o.n), TEXT, m.glyph(), e.z(s * 0.06 * o.n), e.x(cx), e.y(cy + s * 0.325), m.col(hex("#111111")), e.z(s * 0.004));
    }
    let e = o.el("needle", s);
    if e.show {
        let (nx, ny) = (e.x(cx), e.y(cy));
        let na = a(m.frac());
        needle(pm, nx + e.z(s * 0.006), ny + e.z(s * 0.01), na, e.z(s * 0.045), e.z(r - s * 0.012), e.z(s * 0.009), e.z(s * 0.0025), rgba([0, 0, 0], 0.16));
        needle(pm, nx, ny, na, e.z(s * 0.045), e.z(r - s * 0.012), e.z(s * 0.009), e.z(s * 0.0025), rgba(m.col(orange), 1.0));
        glow(pm, nx, ny + e.z(s * 0.004), e.z(s * 0.05), [0, 0, 0], 0.2);
        circle(pm, nx, ny, e.z(s * 0.032), rgba(hex("#111111"), 1.0));
        circle(pm, nx, ny, e.z(s * 0.01), rgba(orange, 1.0));
    }
}

/// G: pastel light drifting slowly behind frosted glass, deep navy numbers.
fn frost(pm: &mut Pixmap, f: &mut Fonts, m: &Rd, s2: &Rd, t: f32, o: O) {
    let s = pm.width() as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let navy = o.acc.unwrap_or(hex("#1a2140"));
    pm.fill(rgba(hex("#f6f8fc"), 1.0));
    for (i, h) in ["#9fd3ff", "#ffb8d9", "#b4f0d4", "#cbb8ff"].iter().enumerate() {
        let x = cx + (t * 0.23 + i as f32 * 1.7).sin() * s * 0.27;
        let y = cy + (t * 0.19 + i as f32 * 2.3).cos() * s * 0.25;
        glow(pm, x, y, s * 0.44, hex(h), 0.85);
    }
    rect(pm, 0.0, 0.0, s, s, rgba([255, 255, 255], 0.26));
    let rg = o.el("ring", s);
    if rg.show {
        let (rx, ry, r) = (rg.x(cx), rg.y(cy), rg.z(s * 0.39));
        glow(pm, rx, ry + rg.z(s * 0.02), r * 1.15, [40, 50, 90], 0.08);
        circle(pm, rx, ry, r, rgba([255, 255, 255], 0.45));
        ring(pm, rx, ry, r, rg.z(s * 0.006), rgba([255, 255, 255], 0.95));
        ring(pm, rx, ry, r - rg.z(s * 0.036), rg.z(s * 0.012), rgba(navy, 0.09));
        arc(pm, rx, ry, r - rg.z(s * 0.036), -PI / 2.0, -PI / 2.0 + TAU * m.frac().max(0.003), rg.z(s * 0.012), rgba(m.col(navy), 1.0));
    }
    let e = o.el("label", s);
    if e.show {
        txt(f, pm, TEXT, &m.name, e.z(s * 0.036 * o.l), e.x(cx), e.y(cy - s * 0.125), 1, hex("#4a5480"), 1.0, e.z(s * 0.014));
    }
    let e = o.el("num", s);
    if e.show {
        num_unit(f, pm, SEMI, &m.text, e.z(s * 0.225 * o.n), TEXT, m.glyph(), e.z(s * 0.08 * o.n), e.x(cx), e.y(cy + s * 0.08), m.col(navy), e.z(s * 0.006));
    }
    let e = o.el("sub_value", s);
    if e.show {
        let (pw, ph) = (e.z(s * 0.38), e.z(s * 0.088));
        let (px, py) = (e.x(cx) - pw / 2.0, e.y(cy + s * 0.125));
        rrect(pm, px, py, pw, ph, ph / 2.0, rgba([255, 255, 255], 0.78));
        stroke_rrect(pm, px, py, pw, ph, ph / 2.0, e.z(s * 0.003), rgba(navy, 0.08));
        let line_txt = format!("{} {}", s2.tag, s2.short());
        let fz = e.z(s * 0.04 * o.l);
        let wd = f.width(SEMI, &line_txt, fz);
        let dc = s2.col(s2.temp().map(|v| ramp(TEMP_LIGHT, v)).unwrap_or(hex("#6d5cf6")));
        circle(pm, e.x(cx) - wd / 2.0 - e.z(s * 0.002), py + ph / 2.0, e.z(s * 0.01), rgba(dc, 1.0));
        f.draw(pm, SEMI, &line_txt, fz, e.x(cx) + e.z(s * 0.014), py + ph * 0.68, 1, s2.col(navy), 1.0);
    }
}

/// H: Swiss poster. Huge black number, a red bar for the second reading, hairline grid.
fn poster(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, m: &Rd, s2: &Rd, o: O) {
    let s = pm.width() as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let (l, rt) = (cx - s * 0.3, cx + s * 0.3);
    let red = o.acc.unwrap_or(hex("#e3262e"));
    let ink = hex("#0a0a0a");
    cached(st, pm, format!("poster{s}"), |p| {
        p.fill(rgba([255, 255, 255], 1.0));
        let c = rgba(hex("#ececec"), 1.0);
        line(p, l, 0.0, l, s, s * 0.003, c);
        line(p, rt, 0.0, rt, s, s * 0.003, c);
        line(p, 0.0, cy - s * 0.215, s, cy - s * 0.215, s * 0.003, c);
        line(p, 0.0, cy + s * 0.31, s, cy + s * 0.31, s * 0.003, c);
    });
    let word = match m.unit.as_str() {
        "FPS" | "" => "frames / s",
        "°C" => "degrees C",
        "W" => "watts",
        "%" => "percent",
        u => u,
    };
    let e = o.el("label", s);
    if e.show {
        f.draw(pm, BOLD, &m.name, e.z(s * 0.042 * o.l), e.x(l), e.y(cy - s * 0.16), 0, red, 1.0);
    }
    let e = o.el("unit", s);
    if e.show {
        f.draw(pm, TEXT, word, e.z(s * 0.036 * o.l), e.x(rt), e.y(cy - s * 0.16), 2, ink, 1.0);
    }
    let e = o.el("num", s);
    if e.show {
        let mut px = s * 0.33 * o.n;
        let w = f.width(BLACK, &m.text, px);
        if w > s * 0.6 {
            px *= s * 0.6 / w;
        }
        f.draw(pm, BLACK, &m.text, e.z(px), e.x(l - px * 0.02), e.y(cy + s * 0.1), 0, m.col(ink), 1.0);
    }
    let e = o.el("sub_bar", s);
    if e.show {
        rect(pm, e.x(l), e.y(cy + s * 0.145), e.z(s * 0.6), e.z(s * 0.04), rgba(hex("#f0f0f0"), 1.0));
        rect(pm, e.x(l), e.y(cy + s * 0.145), e.z((s * 0.6 * s2.frac()).max(0.5)), e.z(s * 0.04), rgba(s2.col(red), 1.0));
    }
    let e = o.el("sub_label", s);
    if e.show {
        f.draw(pm, BOLD, &s2.tag, e.z(s * 0.042 * o.l), e.x(l), e.y(cy + s * 0.265), 0, ink, 1.0);
    }
    let e = o.el("sub_value", s);
    if e.show {
        f.draw(pm, BOLD, &s2.short(), e.z(s * 0.042 * o.l), e.x(rt), e.y(cy + s * 0.265), 2, s2.col(ink), 1.0);
    }
}
