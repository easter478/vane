//! Draws what each screen shows: an animated background layer, a sensor/clock layout layer on top,
//! then encodes a JPEG for the panel. The same frames feed the app's live previews.

use crate::config::{Alerts, ScreenConfig};
use crate::sensors::{smart_meta, SensorState};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont, VariableFont};
use std::collections::HashMap;
use tiny_skia::*;

mod faces;
pub use faces::{FACES, PIECES};

// ------------------------------------------------------------------ fonts and text

pub struct Fonts {
    faces: Vec<FontVec>, // 0 regular, 1 bold, 2 katakana (optional)
    /// fonts for the full-screen designs, from slot 10 up (see faces::COND and the rest)
    extra: Vec<Option<FontVec>>,
    cache: HashMap<(u8, u16, u16), Mask>,
    rings: HashMap<(u8, u16, u16, u8), Mask>,
    /// dark outline around text drawn with draw() while > 0 (set per screen during the layout pass)
    pub outline: f32,
}

#[derive(Clone)]
struct Mask {
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    cov: Vec<u8>,
}

fn load(path: &str, index: u32) -> Option<FontVec> {
    let bytes = std::fs::read(path).ok()?;
    FontVec::try_from_vec_and_index(bytes, index).ok()
}

impl Fonts {
    pub fn load() -> Self {
        let win = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let f = |n: &str| format!("{win}\\Fonts\\{n}");
        let mut regular = load(&f("bahnschrift.ttf"), 0).or_else(|| load(&f("segoeui.ttf"), 0)).or_else(|| load("/System/Library/Fonts/SFNS.ttf", 0)).expect("no usable font found");
        let mut bold = load(&f("bahnschrift.ttf"), 0).or_else(|| load(&f("segoeuib.ttf"), 0)).or_else(|| load("/System/Library/Fonts/SFNS.ttf", 0)).expect("no usable font found");
        regular.set_variation(b"wght", 350.0);
        bold.set_variation(b"wght", 700.0);
        bold.set_variation(b"wdth", 85.0);
        let mut faces = vec![regular, bold];
        if let Some(k) = load(&f("msgothic.ttc"), 0).or_else(|| load(&f("YuGothM.ttc"), 0)) {
            faces.push(k);
        }
        let var = |names: &[&str], axes: &[(&[u8; 4], f32)]| -> Option<FontVec> {
            names.iter().find_map(|n| load(&f(n), 0)).map(|mut v| {
                for (tag, val) in axes {
                    v.set_variation(tag, *val);
                }
                v
            })
        };
        let extra = vec![
            var(&["bahnschrift.ttf"], &[(b"wght", 700.0), (b"wdth", 75.0)]),
            var(&["SegUIVar.ttf", "segoeuil.ttf"], &[(b"wght", 300.0)]),
            var(&["SegUIVar.ttf", "segoeui.ttf"], &[(b"wght", 450.0)]),
            var(&["SegUIVar.ttf", "seguisb.ttf"], &[(b"wght", 620.0)]),
            var(&["CascadiaMono.ttf", "consolab.ttf"], &[(b"wght", 600.0)]),
            var(&["CascadiaMono.ttf", "consola.ttf"], &[(b"wght", 400.0)]),
            var(&["seguibl.ttf", "ariblk.ttf"], &[]),
            var(&["SegUIVar.ttf", "segoeuib.ttf"], &[(b"wght", 700.0)]),
        ];
        Self { faces, extra, cache: HashMap::new(), rings: HashMap::new(), outline: 0.0 }
    }

    fn face(&self, i: u8) -> &FontVec {
        if i >= 10 {
            return match self.extra.get((i - 10) as usize) {
                Some(Some(f)) => f,
                _ => &self.faces[1],
            };
        }
        self.faces.get(i as usize).unwrap_or(&self.faces[0])
    }

    pub fn has_katakana(&self) -> bool {
        self.faces.len() > 2
    }

    fn mask(&mut self, fi: u8, c: char, px: f32) -> Option<(Mask, f32)> {
        let face = self.face(fi);
        let id = face.glyph_id(c);
        let adv = face.as_scaled(PxScale::from(px)).h_advance(id);
        let key = (fi, id.0, (px * 4.0) as u16);
        if let Some(m) = self.cache.get(&key) {
            return Some((m.clone(), adv));
        }
        let g = id.with_scale_and_position(px, ab_glyph::point(0.0, 0.0));
        let og = face.outline_glyph(g)?;
        let b = og.px_bounds();
        let (w, h) = (b.width().ceil() as u32 + 1, b.height().ceil() as u32 + 1);
        let mut cov = vec![0u8; (w * h) as usize];
        og.draw(|x, y, c| {
            let i = (y * w + x) as usize;
            if i < cov.len() {
                cov[i] = (c.clamp(0.0, 1.0) * 255.0) as u8;
            }
        });
        let m = Mask { w, h, left: b.min.x.floor() as i32, top: b.min.y.floor() as i32, cov };
        if self.cache.len() > 6000 {
            self.cache.clear();
        }
        self.cache.insert(key, m.clone());
        Some((m, adv))
    }

    /// The glyph grown by r pixels in every direction (a round dilation), for text outlines.
    fn ring(&mut self, fi: u8, c: char, px: f32, r: u8) -> Option<Mask> {
        let (m, _) = self.mask(fi, c, px)?;
        let key = (fi, self.face(fi).glyph_id(c).0, (px * 4.0) as u16, r);
        if let Some(o) = self.rings.get(&key) {
            return Some(o.clone());
        }
        let ri = r as i32;
        let (w, h) = (m.w + 2 * r as u32, m.h + 2 * r as u32);
        let mut cov = vec![0u8; (w * h) as usize];
        let disk: Vec<(i32, i32, f32)> = (-ri..=ri)
            .flat_map(|dy| (-ri..=ri).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                // soft edge on the last pixel keeps the outline smooth
                let k = (r as f32 + 0.5 - d).clamp(0.0, 1.0);
                (k > 0.0).then_some((dx, dy, k))
            })
            .collect();
        for y in 0..m.h as i32 {
            for x in 0..m.w as i32 {
                let c = m.cov[(y as u32 * m.w + x as u32) as usize];
                if c == 0 {
                    continue;
                }
                for &(dx, dy, k) in &disk {
                    let i = ((y + ri + dy) as u32 * w + (x + ri + dx) as u32) as usize;
                    let v = (c as f32 * k) as u8;
                    if v > cov[i] {
                        cov[i] = v;
                    }
                }
            }
        }
        let o = Mask { w, h, left: m.left - ri, top: m.top - ri, cov };
        if self.rings.len() > 3000 {
            self.rings.clear();
        }
        self.rings.insert(key, o.clone());
        Some(o)
    }

    pub fn width(&self, fi: u8, text: &str, px: f32) -> f32 {
        let f = self.face(fi).as_scaled(PxScale::from(px));
        let mut w = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                w += f.kern(p, id);
            }
            w += f.h_advance(id);
            prev = Some(id);
        }
        w
    }

    /// Draw text with its baseline at y. align: 0 left, 1 centre, 2 right.
    pub fn draw(&mut self, pm: &mut Pixmap, fi: u8, text: &str, px: f32, x: f32, y: f32, align: u8, col: [u8; 3], alpha: f32) {
        let w = self.width(fi, text, px);
        let mut pen = match align {
            1 => x - w / 2.0,
            2 => x - w,
            _ => x,
        };
        let scaled_kern: Vec<(char, f32)> = {
            let f = self.face(fi).as_scaled(PxScale::from(px));
            let mut prev = None;
            text.chars()
                .map(|c| {
                    let id = f.glyph_id(c);
                    let k = prev.map(|p| f.kern(p, id)).unwrap_or(0.0);
                    prev = Some(id);
                    (c, k)
                })
                .collect()
        };
        if self.outline > 0.0 && alpha > 0.05 {
            // the whole outline first, so no glyph's outline covers its neighbour
            let r = (px * 0.055 * self.outline).round().clamp(1.0, 6.0) as u8;
            let mut p = pen;
            for &(c, k) in &scaled_kern {
                p += k;
                if let Some(o) = self.ring(fi, c, px, r) {
                    blit(pm, &o, (p.round() as i32) + o.left, (y.round() as i32) + o.top, [0, 0, 0], (alpha * 0.85).min(1.0));
                }
                let face = self.face(fi);
                p += face.as_scaled(PxScale::from(px)).h_advance(face.glyph_id(c));
            }
        }
        for (c, k) in scaled_kern {
            pen += k;
            if let Some((m, adv)) = self.mask(fi, c, px) {
                blit(pm, &m, (pen.round() as i32) + m.left, (y.round() as i32) + m.top, col, alpha);
                pen += adv;
            } else {
                // spaces have no outline but still move the pen
                let face = self.face(fi);
                pen += face.as_scaled(PxScale::from(px)).h_advance(face.glyph_id(c));
            }
        }
    }

    pub fn glyph(&mut self, pm: &mut Pixmap, fi: u8, c: char, px: f32, x: f32, y: f32, col: [u8; 3], alpha: f32) {
        if let Some((m, adv)) = self.mask(fi, c, px) {
            blit(pm, &m, (x - adv / 2.0).round() as i32 + m.left, y.round() as i32 + m.top, col, alpha);
        }
    }
}

fn blit(pm: &mut Pixmap, m: &Mask, ox: i32, oy: i32, col: [u8; 3], alpha: f32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for my in 0..m.h as i32 {
        let y = oy + my;
        if y < 0 || y >= ph {
            continue;
        }
        for mx in 0..m.w as i32 {
            let x = ox + mx;
            if x < 0 || x >= pw {
                continue;
            }
            let c = m.cov[(my as u32 * m.w + mx as u32) as usize];
            if c == 0 {
                continue;
            }
            let a = (c as f32 / 255.0) * alpha;
            let i = ((y * pw + x) * 4) as usize;
            for k in 0..3 {
                data[i + k] = (col[k] as f32 * a + data[i + k] as f32 * (1.0 - a)) as u8;
            }
            data[i + 3] = (255.0 * a + data[i + 3] as f32 * (1.0 - a)) as u8;
        }
    }
}

// ------------------------------------------------------------------ helpers

fn rgba(c: [u8; 3], a: f32) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0) as u8)
}
fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}
fn circle(pm: &mut Pixmap, x: f32, y: f32, r: f32, c: Color) {
    if let Some(p) = PathBuilder::from_circle(x, y, r.max(0.1)) {
        pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}
fn rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w.max(0.1), h.max(0.1)) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}
fn rrect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
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
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}
fn arc(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, width: f32, c: Color) {
    if (a1 - a0).abs() < 0.001 {
        return;
    }
    let steps = ((a1 - a0).abs() * r / 4.0).clamp(8.0, 240.0) as usize;
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32;
        let (x, y) = (cx + a.cos() * r, cy + a.sin() * r);
        if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    if let Some(p) = pb.finish() {
        let st = Stroke { width, line_cap: LineCap::Round, ..Default::default() };
        pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
    }
}
fn line(pm: &mut Pixmap, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);
    pb.line_to(x1, y1);
    if let Some(p) = pb.finish() {
        let st = Stroke { width: w, line_cap: LineCap::Round, ..Default::default() };
        pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
    }
}
fn polyline(pm: &mut Pixmap, pts: &[(f32, f32)], w: f32, c: Color) {
    if pts.len() < 2 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    if let Some(p) = pb.finish() {
        let st = Stroke { width: w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
    }
}
fn vgrad(pm: &mut Pixmap, stops: &[(f32, [u8; 3])]) {
    let s = pm.height() as f32;
    let st: Vec<GradientStop> = stops.iter().map(|(p, c)| GradientStop::new(*p, rgba(*c, 1.0))).collect();
    if let Some(sh) = LinearGradient::new(Point::from_xy(0.0, 0.0), Point::from_xy(0.0, s), st, SpreadMode::Pad, Transform::identity()) {
        let p = Paint { shader: sh, anti_alias: true, ..Default::default() };
        pm.fill_rect(Rect::from_xywh(0.0, 0.0, pm.width() as f32, s).unwrap(), &p, Transform::identity(), None);
    }
}
fn glow(pm: &mut Pixmap, x: f32, y: f32, r: f32, c: [u8; 3], a: f32) {
    let st = vec![GradientStop::new(0.0, rgba(c, a)), GradientStop::new(1.0, rgba(c, 0.0))];
    if let Some(sh) = RadialGradient::new(Point::from_xy(x, y), Point::from_xy(x, y), r, st, SpreadMode::Pad, Transform::identity()) {
        let p = Paint { shader: sh, anti_alias: true, ..Default::default() };
        if let Some(rc) = Rect::from_xywh(x - r, y - r, r * 2.0, r * 2.0) {
            pm.fill_rect(rc, &p, Transform::identity(), None);
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}

pub fn hex(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).unwrap_or(0x2cc4bc);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

// ------------------------------------------------------------------ per-screen state

pub struct ScreenState {
    rng: Rng,
    matrix: Vec<(f32, f32, usize, Vec<char>)>, // y, speed, length, glyphs
    matrix_size: u32,
    image: Option<(String, Vec<Pixmap>, Vec<u32>)>, // path, frames, delays (ms)
    video: Option<crate::video::Video>,
    pub identify: Option<(u8, std::time::Instant)>,
    /// static layer of the current full-screen design, keyed by design and size
    face_bg: Option<(String, Pixmap)>,
}

impl ScreenState {
    pub fn new(seed: u64) -> Self {
        Self { rng: Rng(seed | 1), matrix: Vec::new(), matrix_size: 0, image: None, video: None, identify: None, face_bg: None }
    }
}

// ------------------------------------------------------------------ backgrounds

pub const BACKGROUNDS: &[(&str, &str)] = &[
    ("matrix", "Matrix rain"),
    ("aurora", "Aurora"),
    ("nebula", "Nebula"),
    ("synthwave", "Synthwave"),
    ("waves", "Neon waves"),
    ("pulse", "Pulse rings"),
    ("radar", "Radar"),
    ("particles", "Particles"),
    ("carbon", "Carbon"),
    ("solid", "Solid colour"),
    ("image", "Picture or GIF"),
    ("video", "Video"),
];

const KATAKANA: &str = "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜｦﾝ0123456789";
const LATIN: &str = "0123456789ABCDEFZ:=*+<>";

fn background(pm: &mut Pixmap, f: &mut Fonts, st: &mut ScreenState, cfg: &ScreenConfig, t: f32) {
    let s = pm.width() as f32;
    let acc = hex(&cfg.accent);
    let sp = cfg.speed.clamp(0.1, 3.0);
    let t = t * sp;
    match cfg.background.as_str() {
        "matrix" => {
            pm.fill(Color::from_rgba8(0, 4, 2, 255));
            let col_w = s / 24.0;
            let cols = (s / col_w) as usize;
            if st.matrix.len() != cols || st.matrix_size != pm.width() {
                st.matrix_size = pm.width();
                let glyphs: Vec<char> = if f.has_katakana() { KATAKANA.chars().collect() } else { LATIN.chars().collect() };
                st.matrix = (0..cols)
                    .map(|_| {
                        let len = 8 + (st.rng.next() % 18) as usize;
                        let g = (0..40).map(|_| glyphs[(st.rng.next() as usize) % glyphs.len()]).collect();
                        (-(st.rng.f() * s), 0.6 + st.rng.f() * 1.4, len, g)
                    })
                    .collect();
            }
            let fi = if f.has_katakana() { 2 } else { 1 };
            let glyphs: Vec<char> = if f.has_katakana() { KATAKANA.chars().collect() } else { LATIN.chars().collect() };
            let head = mix(acc, [255, 255, 255], 0.75);
            for (ci, col) in st.matrix.iter_mut().enumerate() {
                col.0 += col.1 * col_w * 0.35 * sp;
                let len = col.2;
                if col.0 - len as f32 * col_w > s {
                    col.0 = -(st.rng.f() * s * 0.5);
                    col.1 = 0.6 + st.rng.f() * 1.4;
                }
                if st.rng.f() < 0.08 {
                    let k = (st.rng.next() as usize) % col.3.len();
                    col.3[k] = glyphs[(st.rng.next() as usize) % glyphs.len()];
                }
                let x = ci as f32 * col_w + col_w / 2.0;
                for k in 0..len {
                    let y = col.0 - k as f32 * col_w;
                    if y < -col_w || y > s + col_w {
                        continue;
                    }
                    let fade = 1.0 - k as f32 / len as f32;
                    let (c, a) = if k == 0 { (head, 1.0) } else { (acc, fade * 0.85) };
                    let ch = col.3[(k + (col.0 / col_w) as usize) % col.3.len()];
                    f.glyph(pm, fi, ch, col_w * 0.95, x, y, c, a);
                }
            }
        }
        "aurora" => {
            vgrad(pm, &[(0.0, [3, 10, 24]), (0.6, [6, 30, 44]), (1.0, [10, 6, 30])]);
            for i in 0..4 {
                let c = [mix(acc, [120, 255, 200], 0.3), [90, 120, 255], [180, 90, 255], acc][i];
                let mut pts = Vec::new();
                for k in 0..=48 {
                    let x = s * k as f32 / 48.0;
                    let y = s * (0.3 + i as f32 * 0.1) + (x / s * 5.0 + t * 0.5 + i as f32 * 1.3).sin() * s * 0.06 + (x / s * 11.0 - t * 0.3).sin() * s * 0.02;
                    pts.push((x, y));
                }
                polyline(pm, &pts, s * 0.07, rgba(c, 0.10));
                polyline(pm, &pts, s * 0.025, rgba(c, 0.25));
                polyline(pm, &pts, s * 0.006, rgba(c, 0.7));
            }
        }
        "nebula" => {
            pm.fill(Color::from_rgba8(4, 3, 14, 255));
            let blobs = [([140, 60, 255], 0.0), (acc, 2.1), ([255, 70, 150], 4.2)];
            for (c, ph) in blobs {
                let x = s * (0.5 + 0.28 * (t * 0.13 + ph).cos());
                let y = s * (0.5 + 0.28 * (t * 0.11 + ph * 1.3).sin());
                glow(pm, x, y, s * 0.55, c, 0.45);
            }
            let mut r = Rng(7);
            for _ in 0..90 {
                let (x, y) = (r.f() * s, r.f() * s);
                let tw = 0.4 + 0.6 * ((t * 1.5 + r.f() * 9.0).sin() * 0.5 + 0.5);
                circle(pm, x, y, s * 0.0035 * (0.5 + r.f()), rgba([255, 255, 255], tw));
            }
        }
        "synthwave" => {
            vgrad(pm, &[(0.0, [20, 6, 40]), (0.45, [120, 30, 110]), (0.56, [255, 120, 90]), (0.561, [12, 4, 28]), (1.0, [12, 4, 28])]);
            let hz = s * 0.56;
            let sun = [255, 190, 90];
            circle(pm, s / 2.0, hz - s * 0.02, s * 0.2, rgba(sun, 0.95));
            for k in 0..6 {
                let y = hz - s * 0.03 - k as f32 * s * 0.03;
                rect(pm, s * 0.25, y, s * 0.5, s * 0.008 + k as f32 * 0.6, rgba([120, 30, 110], 1.0));
            }
            rect(pm, 0.0, hz, s, s - hz, Color::from_rgba8(12, 4, 28, 255));
            let gc = rgba(mix(acc, [255, 60, 200], 0.5), 0.8);
            for i in -10..=10 {
                let x = s / 2.0 + i as f32 * s * 0.12;
                line(pm, s / 2.0 + (x - s / 2.0) * 0.08, hz, x + (x - s / 2.0) * 1.6, s, s * 0.004, gc);
            }
            for k in 0..10 {
                let p = ((k as f32 + (t * 0.6) % 1.0) / 10.0).powf(2.0);
                let y = hz + p * (s - hz);
                line(pm, 0.0, y, s, y, s * 0.004, gc);
            }
        }
        "waves" => {
            pm.fill(Color::from_rgba8(3, 6, 10, 255));
            for i in 0..6 {
                let c = mix(acc, [160, 90, 255], i as f32 / 5.0);
                let mut pts = Vec::new();
                for k in 0..=60 {
                    let x = s * k as f32 / 60.0;
                    let y = s / 2.0 + (x / s * 8.0 + t * 1.6 + i as f32 * 0.7).sin() * s * (0.06 + i as f32 * 0.03);
                    pts.push((x, y));
                }
                polyline(pm, &pts, s * 0.018, rgba(c, 0.15));
                polyline(pm, &pts, s * 0.006, rgba(c, 0.8));
            }
        }
        "pulse" => {
            pm.fill(Color::from_rgba8(2, 8, 10, 255));
            glow(pm, s / 2.0, s / 2.0, s * 0.45, acc, 0.25);
            for k in 0..7 {
                let r = ((k as f32 / 7.0 + t * 0.12) % 1.0) * s * 0.7;
                let a = (1.0 - r / (s * 0.7)) * 0.8;
                arc(pm, s / 2.0, s / 2.0, r, 0.0, std::f32::consts::TAU, s * 0.006, rgba(acc, a));
            }
        }
        "radar" => {
            pm.fill(Color::from_rgba8(1, 10, 4, 255));
            let c = acc;
            for k in 1..=4 {
                arc(pm, s / 2.0, s / 2.0, s * 0.12 * k as f32, 0.0, std::f32::consts::TAU, s * 0.003, rgba(c, 0.35));
            }
            line(pm, s / 2.0, s * 0.02, s / 2.0, s * 0.98, s * 0.002, rgba(c, 0.3));
            line(pm, s * 0.02, s / 2.0, s * 0.98, s / 2.0, s * 0.002, rgba(c, 0.3));
            let a = t * 1.2;
            for k in 0..30 {
                let aa = a - k as f32 * 0.025;
                line(pm, s / 2.0, s / 2.0, s / 2.0 + aa.cos() * s * 0.48, s / 2.0 + aa.sin() * s * 0.48, s * 0.012, rgba(c, 0.35 * (1.0 - k as f32 / 30.0)));
            }
            let mut r = Rng(11);
            for _ in 0..6 {
                let (ang, rad) = (r.f() * 6.283, s * (0.1 + r.f() * 0.35));
                let since = ((a - ang).rem_euclid(6.283)) / 6.283;
                circle(pm, s / 2.0 + ang.cos() * rad, s / 2.0 + ang.sin() * rad, s * 0.012, rgba(c, (1.0 - since).powf(2.0)));
            }
        }
        "particles" => {
            pm.fill(Color::from_rgba8(3, 4, 10, 255));
            for i in 0..90 {
                let a = i as f32 * 2.39996 + t * 0.3;
                let r = ((i as f32 * 7.0 + t * 40.0) % 100.0) / 100.0 * s * 0.55;
                let c = mix(acc, [170, 110, 255], (i % 10) as f32 / 10.0);
                circle(pm, s / 2.0 + a.cos() * r, s / 2.0 + a.sin() * r, s * 0.008, rgba(c, 1.0 - r / (s * 0.55)));
            }
        }
        "carbon" => {
            pm.fill(Color::from_rgba8(18, 20, 23, 255));
            let step = s / 24.0;
            for yi in 0..25 {
                for xi in 0..25 {
                    let a = if (xi + yi) % 2 == 0 { 0.05 } else { 0.0 };
                    rect(pm, xi as f32 * step, yi as f32 * step, step, step, rgba([255, 255, 255], a));
                }
            }
            glow(pm, s * 0.2, s * 0.1, s * 0.8, acc, 0.10);
        }
        "image" => {
            pm.fill(Color::BLACK);
            draw_image(pm, st, &cfg.image, t / sp);
        }
        "video" => {
            pm.fill(Color::BLACK);
            let size = pm.width();
            if st.video.as_ref().is_none_or(|v| v.path != cfg.image || v.size() != size) {
                st.video = (!cfg.image.is_empty()).then(|| crate::video::Video::new(&cfg.image, size));
            }
            if let Some(frame) = st.video.as_mut().and_then(|v| v.frame(cfg.speed)) {
                pm.draw_pixmap(0, 0, frame.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
            }
        }
        _ => {
            let c = hex(&cfg.solid);
            pm.fill(rgba(c, 1.0));
        }
    }
    if cfg.dim > 0.0 {
        rect(pm, 0.0, 0.0, s, s, Color::from_rgba8(0, 0, 0, (cfg.dim.clamp(0.0, 0.9) * 255.0) as u8));
    }
}

fn draw_image(pm: &mut Pixmap, st: &mut ScreenState, path: &str, t: f32) {
    if path.is_empty() {
        return;
    }
    let size = pm.width();
    if st.image.as_ref().is_none_or(|i| i.0 != path || i.1.first().is_none_or(|p| p.width() != size)) {
        st.image = Some((path.to_string(), load_frames(path, size), Vec::new()));
        if let Some(img) = st.image.as_mut() {
            img.2 = gif_delays(path);
        }
    }
    let Some((_, frames, delays)) = st.image.as_ref() else { return };
    if frames.is_empty() {
        return;
    }
    let idx = if frames.len() > 1 {
        let total: u32 = delays.iter().sum::<u32>().max(1);
        let mut ms = ((t * 1000.0) as u32) % total;
        let mut i = 0;
        for (k, d) in delays.iter().enumerate() {
            if ms < *d {
                i = k;
                break;
            }
            ms -= d;
        }
        i.min(frames.len() - 1)
    } else {
        0
    };
    pm.draw_pixmap(0, 0, frames[idx].as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn cover(img: image::RgbaImage, size: u32) -> Pixmap {
    let (w, h) = img.dimensions();
    let scale = size as f32 / w.min(h) as f32;
    let (nw, nh) = ((w as f32 * scale).ceil() as u32, (h as f32 * scale).ceil() as u32);
    let r = image::imageops::resize(&img, nw.max(size), nh.max(size), image::imageops::FilterType::Triangle);
    let (x0, y0) = ((r.width() - size) / 2, (r.height() - size) / 2);
    let crop = image::imageops::crop_imm(&r, x0, y0, size, size).to_image();
    let mut pm = Pixmap::new(size, size).unwrap();
    for (i, p) in crop.pixels().enumerate() {
        let a = p[3] as f32 / 255.0;
        let d = &mut pm.data_mut()[i * 4..i * 4 + 4];
        d[0] = (p[0] as f32 * a) as u8;
        d[1] = (p[1] as f32 * a) as u8;
        d[2] = (p[2] as f32 * a) as u8;
        d[3] = p[3];
    }
    pm
}

fn load_frames(path: &str, size: u32) -> Vec<Pixmap> {
    use image::AnimationDecoder;
    if path.to_lowercase().ends_with(".gif") {
        if let Ok(file) = std::fs::File::open(path) {
            if let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)) {
                let frames: Vec<Pixmap> = dec.into_frames().take(300).filter_map(|f| f.ok()).map(|f| cover(f.into_buffer(), size)).collect();
                if !frames.is_empty() {
                    return frames;
                }
            }
        }
    }
    image::open(path).map(|i| vec![cover(i.to_rgba8(), size)]).unwrap_or_default()
}

fn gif_delays(path: &str) -> Vec<u32> {
    use image::AnimationDecoder;
    if !path.to_lowercase().ends_with(".gif") {
        return vec![1000];
    }
    let Ok(file) = std::fs::File::open(path) else { return vec![1000] };
    let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)) else { return vec![1000] };
    dec.into_frames()
        .take(300)
        .filter_map(|f| f.ok())
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            (n / d.max(1)).max(20)
        })
        .collect()
}

// ------------------------------------------------------------------ layouts

/// Widget kinds for custom layouts: (id, name, box width, box height) at size 1, as parts of the screen.
pub const WIDGETS: &[(&str, &str, f32, f32)] = &[
    ("value", "Number", 0.42, 0.22),
    ("gauge", "Gauge", 0.34, 0.34),
    ("bar", "Bar", 0.7, 0.13),
    ("clock", "Clock", 0.7, 0.26),
    ("date", "Date", 0.5, 0.08),
    ("text", "Text", 0.6, 0.09),
    ("graph", "Graph", 0.6, 0.2),
    ("ring", "Glow ring", 0.46, 0.46),
    ("dial", "Watch dial", 0.6, 0.6),
    ("rev", "Rev counter", 0.6, 0.6),
    ("segments", "Segment bar", 0.56, 0.12),
    ("frames", "Frame graph", 0.62, 0.2),
    ("orb", "Plasma orb", 0.6, 0.6),
];

pub fn widget_box(kind: &str) -> (f32, f32) {
    WIDGETS.iter().find(|w| w.0 == kind).map(|w| (w.2, w.3)).unwrap_or((0.4, 0.2))
}

/// The widgets that reproduce a preset layout, so any design can be opened in the layout editor.
pub fn preset_widgets(cfg: &ScreenConfig, round: bool) -> Vec<crate::config::Widget> {
    use crate::config::Widget;
    let slot = |i: usize| cfg.slots.get(i).cloned().unwrap_or_default();
    let w = |kind: &str, sensor: String, x: f32, y: f32, size: f32, color: &str| Widget { kind: kind.into(), sensor, x, y, size, color: color.into(), label: String::new(), plate: cfg.plates };
    let mut out = Vec::new();
    if cfg.background.starts_with("face:") {
        return if cfg.layout == "custom" { cfg.widgets.clone() } else { out }; // the design draws itself; parts go on top
    }
    match cfg.layout.as_str() {
        "gaming" => {
            out.push(w("value", slot(0), 0.5, if round { 0.35 } else { 0.27 }, 1.6, "accent"));
            let cells: [(f32, f32); 4] = if round { [(0.31, 0.6), (0.69, 0.6), (0.31, 0.78), (0.69, 0.78)] } else { [(0.27, 0.6), (0.73, 0.6), (0.27, 0.82), (0.73, 0.82)] };
            for (k, (x, y)) in cells.iter().enumerate() {
                out.push(w("value", slot(k + 1), *x, *y, if round { 0.8 } else { 0.95 }, "heat"));
            }
        }
        "system" => {
            let cells: [(f32, f32); 4] = if round { [(0.33, 0.35), (0.67, 0.35), (0.33, 0.66), (0.67, 0.66)] } else { [(0.27, 0.27), (0.73, 0.27), (0.27, 0.73), (0.73, 0.73)] };
            for (k, (x, y)) in cells.iter().enumerate() {
                out.push(w("gauge", slot(k), *x, *y, if round { 0.85 } else { 1.15 }, "heat"));
            }
        }
        "ring" => {
            out.push(w("gauge", slot(0), 0.5, 0.48, if round { 2.0 } else { 2.3 }, "heat"));
            if !slot(1).is_empty() {
                out.push(w("value", slot(1), 0.5, 0.84, 0.6, "accent"));
            }
        }
        "big" => out.push(w("value", slot(0), 0.5, 0.5, 2.0, "heat")),
        "dual" => {
            out.push(w("value", slot(0), 0.5, 0.33, 1.4, "heat"));
            out.push(w("value", slot(1), 0.5, 0.67, 1.4, "text"));
        }
        "bars" => {
            let n = cfg.slots.len().clamp(1, 5);
            let top = if round { 0.22 } else { 0.1 };
            let step = (1.0 - 2.0 * top) / n as f32;
            for k in 0..n {
                out.push(w("bar", slot(k), 0.5, top + (k as f32 + 0.5) * step, if round { 0.85 } else { 1.1 }, "heat"));
            }
        }
        "clock" => {
            out.push(w("clock", String::new(), 0.5, 0.44, 1.0, "accent"));
            out.push(w("date", String::new(), 0.5, 0.6, 1.0, "text"));
            if !slot(0).is_empty() {
                out.push(w("value", slot(0), 0.5, 0.78, 0.6, "heat"));
            }
        }
        "clock_sensors" => {
            out.push(w("clock", String::new(), 0.5, 0.33, 1.0, "accent"));
            out.push(w("date", String::new(), 0.5, 0.48, 1.0, "text"));
            let xs: [f32; 3] = if round { [0.3, 0.5, 0.7] } else { [0.2, 0.5, 0.8] };
            for (k, x) in xs.iter().enumerate() {
                out.push(w("value", slot(k), *x, 0.72, 0.6, "heat"));
            }
        }
        "custom" => return cfg.widgets.clone(),
        _ => {}
    }
    out.retain(|x| !(matches!(x.kind.as_str(), "value" | "gauge" | "bar" | "graph") && x.sensor.is_empty()));
    out
}

pub const LAYOUTS: &[(&str, &str, usize)] = &[
    ("gaming", "Gaming: FPS + 4 readings", 5),
    ("system", "System: 4 gauges", 4),
    ("ring", "Ring gauge", 2),
    ("big", "Big number", 1),
    ("dual", "Two readings", 2),
    ("bars", "Bars", 5),
    ("clock", "Clock", 1),
    ("clock_sensors", "Clock + 3 readings", 3),
    ("none", "Background only", 0),
    ("custom", "My layout", 0),
];

struct V {
    label: String,
    text: String,
    unit: String,
    frac: f32,
    hot: f32,
    history: Vec<f64>,
    /// warning colour and how strongly it shows this frame
    alert: Option<([u8; 3], f32)>,
}

fn fmt_value(v: f64, unit: &str) -> String {
    match unit {
        "V" => format!("{v:.3}"),
        "ms" => format!("{v:.1}"),
        "GB" => format!("{v:.1}"),
        "°C" if v < 100.0 && v.fract().abs() > 0.05 && v < 40.0 => format!("{v:.1}"),
        _ => format!("{}", v.round() as i64),
    }
}

fn gauge_max(unit: &str, id: &str) -> f64 {
    match unit {
        "°C" => if id == "coolant" { 60.0 } else { 100.0 },
        "%" => 100.0,
        "FPS" => 240.0,
        "ms" => 40.0,
        "V" => 1.25,
        "MHz" => if id.starts_with("gpu") { 3200.0 } else { 6000.0 },
        "W" => if id.starts_with("gpu") { 400.0 } else { 200.0 },
        "RPM" | "rpm" => 3500.0,
        "MB" => 16384.0,
        "GB" => 64.0,
        _ => 100.0,
    }
}

fn value_of(sensors: &SensorState, id: &str) -> V {
    let r = sensors.get(id);
    let (label, unit_fallback) = smart_meta(id).map(|(l, u)| (l.to_string(), u.to_string())).unwrap_or_else(|| {
        (r.as_ref().map(|r| r.label.clone()).unwrap_or_else(|| id.trim_start_matches("hw:").to_string()), String::new())
    });
    match r {
        Some(r) => {
            let unit = if r.unit.is_empty() { unit_fallback } else { r.unit.clone() };
            let max = gauge_max(&unit, id);
            let hot = if unit == "°C" { ((r.value - 65.0) / 20.0).clamp(0.0, 1.0) as f32 } else { 0.0 };
            V { label, text: fmt_value(r.value, &unit), unit, frac: (r.value / max).clamp(0.0, 1.0) as f32, hot, history: sensors.history.get(id).cloned().unwrap_or_default(), alert: text_alert(sensors, id) }
        }
        None => V { label, text: "--".into(), unit: unit_fallback, frac: 0.0, hot: 0.0, history: Vec::new(), alert: None },
    }
}

fn short_label(l: &str) -> String {
    l.replace("temperature", "temp").replace("Temperature", "temp").to_uppercase()
}

/// Caption under a value: the label, plus the unit unless the label already says it (FPS).
fn caption(v: &V) -> String {
    let l = short_label(&v.label);
    let u = v.unit.trim();
    if u.is_empty() || l.split_whitespace().any(|w| w.eq_ignore_ascii_case(u)) { l } else { format!("{l}  {}", u.to_uppercase()) }
}

fn layout(pm: &mut Pixmap, f: &mut Fonts, cfg: &ScreenConfig, sensors: &SensorState, round: bool, t: f32) {
    let s = pm.width() as f32;
    let acc = hex(&cfg.accent);
    let txt = hex(&cfg.text);
    let warm = [255, 170, 60];
    let hotc = [255, 80, 70];
    let heat = |v: &V| vc(v, if v.hot <= 0.0 { acc } else if v.hot < 0.6 { mix(acc, warm, v.hot / 0.6) } else { mix(warm, hotc, (v.hot - 0.6) / 0.4) });
    let slots: Vec<V> = cfg.slots.iter().map(|id| value_of(sensors, id)).collect();
    let get = |i: usize| slots.get(i);
    let plate = |pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32| {
        if cfg.plates {
            rrect(pm, x, y, w, h, s * 0.03, Color::from_rgba8(0, 0, 0, 120));
        }
    };
    let mid = s / 2.0;
    match cfg.layout.as_str() {
        "gaming" => {
            if let Some(v) = get(0) {
                let (py, ph) = if round { (s * 0.17, s * 0.36) } else { (s * 0.06, s * 0.42) };
                plate(pm, s * 0.14, py, s * 0.72, ph);
                f.draw(pm, 1, &v.text, s * 0.25, mid, py + ph * 0.66, 1, vc(v, acc), 1.0);
                f.draw(pm, 0, &caption(v), s * 0.05, mid, py + ph * 0.9, 1, txt, 0.75);
                if !v.history.is_empty() && !round {
                    spark(pm, &v.history, s * 0.18, py + ph * 0.04, s * 0.64, ph * 0.13, acc);
                }
            }
            let cells: [(f32, f32); 4] = if round {
                [(0.31, 0.6), (0.69, 0.6), (0.31, 0.78), (0.69, 0.78)]
            } else {
                [(0.27, 0.6), (0.73, 0.6), (0.27, 0.82), (0.73, 0.82)]
            };
            for (k, (cx, cy)) in cells.iter().enumerate() {
                if let Some(v) = get(k + 1) {
                    let (w, h) = if round { (s * 0.34, s * 0.16) } else { (s * 0.42, s * 0.19) };
                    plate(pm, cx * s - w / 2.0, cy * s - h * 0.5, w, h);
                    f.draw(pm, 1, &val_text(v), s * if round { 0.075 } else { 0.085 }, cx * s, cy * s + h * 0.12, 1, heat(v), 1.0);
                    f.draw(pm, 0, &short_label(&v.label), s * 0.036, cx * s, cy * s + h * 0.38, 1, txt, 0.7);
                }
            }
        }
        "system" => {
            let cells: [(f32, f32); 4] = if round { [(0.33, 0.35), (0.67, 0.35), (0.33, 0.66), (0.67, 0.66)] } else { [(0.27, 0.27), (0.73, 0.27), (0.27, 0.73), (0.73, 0.73)] };
            let r = s * if round { 0.12 } else { 0.16 };
            for (k, (cx, cy)) in cells.iter().enumerate() {
                if let Some(v) = get(k) {
                    let (x, y) = (cx * s, cy * s);
                    if cfg.plates {
                        circle(pm, x, y, r * 1.25, Color::from_rgba8(0, 0, 0, 110));
                    }
                    let a0 = std::f32::consts::PI * 0.75;
                    arc(pm, x, y, r, a0, a0 + std::f32::consts::PI * 1.5, s * 0.02, rgba([255, 255, 255], 0.12));
                    arc(pm, x, y, r, a0, a0 + std::f32::consts::PI * 1.5 * v.frac.max(0.01), s * 0.02, rgba(heat(v), 1.0));
                    f.draw(pm, 1, &v.text, r * 0.62, x, y + r * 0.2, 1, vc(v, txt), 1.0);
                    f.draw(pm, 0, &caption(v), s * 0.032, x, y + r * 0.72, 1, txt, 0.7);
                }
            }
        }
        "ring" => {
            if let Some(v) = get(0) {
                let r = s * if round { 0.34 } else { 0.38 };
                if cfg.plates {
                    circle(pm, mid, mid, r * 1.12, Color::from_rgba8(0, 0, 0, 110));
                }
                let a0 = std::f32::consts::PI * 0.75;
                arc(pm, mid, mid, r, a0, a0 + std::f32::consts::PI * 1.5, s * 0.055, rgba([255, 255, 255], 0.1));
                arc(pm, mid, mid, r, a0, a0 + std::f32::consts::PI * 1.5 * v.frac.max(0.01), s * 0.055, rgba(heat(v), 1.0));
                f.draw(pm, 1, &v.text, s * 0.22, mid, mid + s * 0.07, 1, vc(v, txt), 1.0);
                f.draw(pm, 0, &caption(v), s * 0.055, mid, mid + s * 0.17, 1, txt, 0.65);
                if let Some(v2) = get(1) {
                    f.draw(pm, 1, &format!("{}{}", v2.text, unit_suffix(&v2.unit)), s * 0.06, mid, mid + s * 0.32, 1, vc(v2, acc), 0.95);
                }
            }
        }
        "big" => {
            if let Some(v) = get(0) {
                plate(pm, s * 0.15, s * 0.26, s * 0.7, s * 0.48);
                f.draw(pm, 1, &v.text, s * 0.32, mid, mid + s * 0.08, 1, heat(v), 1.0);
                f.draw(pm, 0, &caption(v), s * 0.06, mid, mid + s * 0.2, 1, txt, 0.75);
            }
        }
        "dual" => {
            for k in 0..2 {
                if let Some(v) = get(k) {
                    let y = s * if k == 0 { 0.36 } else { 0.7 };
                    plate(pm, s * 0.18, y - s * 0.2, s * 0.64, s * 0.27);
                    f.draw(pm, 0, &short_label(&v.label), s * 0.05, mid, y - s * 0.12, 1, txt, 0.65);
                    f.draw(pm, 1, &val_text(v), s * 0.16, mid, y + s * 0.03, 1, if k == 0 { heat(v) } else { vc(v, txt) }, 1.0);
                }
            }
        }
        "bars" => {
            let n = slots.len().clamp(1, 5);
            let (x0, w) = if round { (s * 0.2, s * 0.6) } else { (s * 0.1, s * 0.8) };
            let top = if round { s * 0.22 } else { s * 0.1 };
            let step = (s - 2.0 * top) / n as f32;
            for (k, v) in slots.iter().take(5).enumerate() {
                let y = top + k as f32 * step + step * 0.45;
                plate(pm, x0 - s * 0.03, y - step * 0.4, w + s * 0.06, step * 0.85);
                f.draw(pm, 0, &short_label(&v.label), s * 0.042, x0, y - step * 0.08, 0, txt, 0.75);
                f.draw(pm, 1, &format!("{} {}", v.text, v.unit), s * 0.055, x0 + w, y - step * 0.06, 2, heat(v), 1.0);
                rrect(pm, x0, y + step * 0.06, w, step * 0.14, step * 0.07, rgba([255, 255, 255], 0.12));
                rrect(pm, x0, y + step * 0.06, (w * v.frac).max(step * 0.14), step * 0.14, step * 0.07, rgba(heat(v), 1.0));
            }
        }
        "custom" => {
            for w in &cfg.widgets {
                draw_widget(pm, f, w, cfg, sensors, s, &heat, t);
            }
        }
        "clock" | "clock_sensors" => {
            let now = chrono::Local::now();
            let time = if cfg.clock_24h { now.format("%H:%M").to_string() } else { now.format("%-I:%M").to_string() };
            let date = now.format("%a %-d %b").to_string().to_uppercase();
            let ty = if cfg.layout == "clock" { mid + s * 0.06 } else { s * 0.42 };
            plate(pm, s * 0.12, ty - s * 0.24, s * 0.76, s * 0.36);
            f.draw(pm, 1, &time, s * 0.26, mid, ty, 1, acc, 1.0);
            f.draw(pm, 0, &date, s * 0.06, mid, ty + s * 0.09, 1, txt, 0.75);
            if cfg.layout == "clock" {
                if let Some(v) = get(0) {
                    f.draw(pm, 1, &format!("{}{}", v.text, unit_suffix(&v.unit)), s * 0.07, mid, ty + s * 0.22, 1, heat(v), 0.95);
                }
            } else {
                let xs: &[f32] = if round { &[0.3, 0.5, 0.7] } else { &[0.2, 0.5, 0.8] };
                for (k, x) in xs.iter().enumerate() {
                    if let Some(v) = get(k) {
                        let y = s * if round { 0.68 } else { 0.72 };
                        plate(pm, x * s - s * 0.13, y - s * 0.07, s * 0.26, s * 0.16);
                        f.draw(pm, 1, &format!("{}{}", v.text, unit_suffix(&v.unit)), s * 0.07, x * s, y + s * 0.03, 1, heat(v), 1.0);
                        f.draw(pm, 0, &short_label(&v.label), s * 0.032, x * s, y + s * 0.07, 1, txt, 0.7);
                    }
                }
            }
        }
        _ => {}
    }
}

fn draw_widget(pm: &mut Pixmap, f: &mut Fonts, w: &crate::config::Widget, cfg: &ScreenConfig, sensors: &SensorState, s: f32, heat: &dyn Fn(&V) -> [u8; 3], t: f32) {
    let (bw0, bh0) = widget_box(&w.kind);
    let size = w.size.clamp(0.2, 4.0);
    let (bw, bh) = (bw0 * size * s, bh0 * size * s);
    let (cx, cy) = (w.x * s, w.y * s);
    let (x0, y0) = (cx - bw / 2.0, cy - bh / 2.0);
    let acc = hex(&cfg.accent);
    let txt = hex(&cfg.text);
    let v = value_of(sensors, &w.sensor);
    let col = vc(&v, match w.color.as_str() {
        "accent" => acc,
        "text" => txt,
        "heat" | "" => heat(&v),
        c => hex(c),
    });
    let cap = if w.label.is_empty() { caption(&v) } else { w.label.to_uppercase() };
    let plate = |pm: &mut Pixmap, x: f32, y: f32, ww: f32, hh: f32| {
        if w.plate {
            rrect(pm, x, y, ww, hh, (hh * 0.18).min(s * 0.03), Color::from_rgba8(0, 0, 0, 120));
        }
    };
    match w.kind.as_str() {
        "value" => {
            plate(pm, x0, y0, bw, bh);
            f.draw(pm, 1, &val_text(&v), bh * 0.52, cx, cy + bh * 0.12, 1, col, 1.0);
            f.draw(pm, 0, &cap, bh * 0.15, cx, cy + bh * 0.38, 1, txt, 0.72);
        }
        "gauge" => {
            let r = bw * 0.4;
            if w.plate {
                circle(pm, cx, cy, bw * 0.5, Color::from_rgba8(0, 0, 0, 110));
            }
            let a0 = std::f32::consts::PI * 0.75;
            arc(pm, cx, cy, r, a0, a0 + std::f32::consts::PI * 1.5, bw * 0.055, rgba([255, 255, 255], 0.12));
            arc(pm, cx, cy, r, a0, a0 + std::f32::consts::PI * 1.5 * v.frac.max(0.01), bw * 0.055, rgba(col, 1.0));
            f.draw(pm, 1, &v.text, r * 0.62, cx, cy + r * 0.2, 1, vc(&v, txt), 1.0);
            f.draw(pm, 0, &cap, r * 0.2, cx, cy + r * 0.66, 1, txt, 0.7);
        }
        "bar" => {
            plate(pm, x0, y0, bw, bh);
            let (ix, iw) = (x0 + bw * 0.05, bw * 0.9);
            f.draw(pm, 0, &cap, bh * 0.28, ix, cy - bh * 0.02, 0, txt, 0.75);
            f.draw(pm, 1, &val_text(&v), bh * 0.36, ix + iw, cy - bh * 0.02, 2, col, 1.0);
            rrect(pm, ix, cy + bh * 0.12, iw, bh * 0.18, bh * 0.09, rgba([255, 255, 255], 0.12));
            rrect(pm, ix, cy + bh * 0.12, (iw * v.frac).max(bh * 0.18), bh * 0.18, bh * 0.09, rgba(col, 1.0));
        }
        "clock" => {
            let now = chrono::Local::now();
            let time = if cfg.clock_24h { now.format("%H:%M").to_string() } else { now.format("%-I:%M").to_string() };
            plate(pm, x0, y0, bw, bh);
            f.draw(pm, 1, &time, bh * 0.78, cx, cy + bh * 0.28, 1, col, 1.0);
        }
        "date" => {
            let date = chrono::Local::now().format("%a %-d %b").to_string().to_uppercase();
            f.draw(pm, 0, &date, bh * 0.7, cx, cy + bh * 0.25, 1, col, 0.85);
        }
        "text" => {
            f.draw(pm, 1, &w.label, bh * 0.7, cx, cy + bh * 0.25, 1, col, 1.0);
        }
        "graph" => {
            plate(pm, x0, y0, bw, bh);
            f.draw(pm, 0, &cap, bh * 0.14, x0 + bw * 0.05, y0 + bh * 0.2, 0, txt, 0.7);
            f.draw(pm, 1, &val_text(&v), bh * 0.2, x0 + bw * 0.95, y0 + bh * 0.24, 2, col, 1.0);
            spark(pm, &v.history, x0 + bw * 0.05, y0 + bh * 0.32, bw * 0.9, bh * 0.58, col);
        }
        "ring" | "dial" | "rev" | "segments" | "frames" | "orb" => {
            let mut rd = faces::read(sensors, &w.sensor);
            rd.alert = text_alert(sensors, &w.sensor);
            let f0 = f.outline;
            f.outline = 0.0;
            faces::part(pm, f, &w.kind, &rd, cx, cy, bw, bh, col, txt, &w.label, w.plate, t);
            f.outline = f0;
        }
        _ => {}
    }
}

/// Number with its unit, unless the label already names the unit (FPS).
fn val_text(v: &V) -> String {
    let u = v.unit.trim();
    if !u.is_empty() && v.label.split_whitespace().any(|w| w.eq_ignore_ascii_case(u)) { v.text.clone() } else { format!("{}{}", v.text, unit_suffix(&v.unit)) }
}

fn unit_suffix(u: &str) -> String {
    match u {
        "°C" => "°".into(),
        "%" => "%".into(),
        "" => String::new(),
        other => format!(" {other}"),
    }
}

fn spark(pm: &mut Pixmap, h: &[f64], x: f32, y: f32, w: f32, hh: f32, c: [u8; 3]) {
    if h.len() < 2 {
        return;
    }
    let (mn, mx) = h.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let span = (mx - mn).max(1.0);
    let pts: Vec<(f32, f32)> = h.iter().enumerate().map(|(i, v)| (x + w * i as f32 / (h.len() - 1) as f32, y + hh - ((v - mn) / span) as f32 * hh)).collect();
    polyline(pm, &pts, 2.0, rgba(c, 0.6));
}

// ------------------------------------------------------------------ warnings

thread_local! {
    /// warnings for the screen being drawn on this thread, while numbers flash (not the whole screen)
    static ALERT: std::cell::RefCell<Option<(Alerts, f32)>> = const { std::cell::RefCell::new(None) };
}

const ALERT_GREEN: [u8; 3] = [52, 199, 89];
const ALERT_YELLOW: [u8; 3] = [255, 204, 0];
const ALERT_RED: [u8; 3] = [255, 59, 48];

/// Warning colour and strength for one reading this frame: red flashes fast, yellow pulses,
/// green (only if asked for) stays on.
fn alert_level(a: &Alerts, sensors: &SensorState, id: &str, t: f32) -> Option<([u8; 3], f32, u8)> {
    let rule = a.rules.iter().find(|r| r.on && r.id == id)?;
    let v = sensors.get(id)?.value;
    if id == "fps" && v <= 1.0 {
        return None; // no game in front
    }
    match rule.level(v) {
        2 => Some((ALERT_RED, if (t * 3.0).fract() < 0.55 { 1.0 } else { 0.12 }, 2)),
        1 => Some((ALERT_YELLOW, 0.35 + 0.65 * (0.5 + 0.5 * (t * std::f32::consts::TAU * 1.2).sin()), 1)),
        _ if a.green => Some((ALERT_GREEN, 1.0, 0)),
        _ => None,
    }
}

fn text_alert(sensors: &SensorState, id: &str) -> Option<([u8; 3], f32)> {
    ALERT.with(|c| c.borrow().as_ref().and_then(|(a, t)| alert_level(a, sensors, id, *t)).map(|(c, k, _)| (c, k)))
}

/// A value's colour, blended towards its warning colour while it warns.
fn vc(v: &V, base: [u8; 3]) -> [u8; 3] {
    match v.alert {
        Some((c, k)) => mix(base, c, k),
        None => base,
    }
}

/// Whole-screen warning: the worst warning among this screen's readings tints and rims the screen.
fn screen_alert(pm: &mut Pixmap, a: &Alerts, cfg: &ScreenConfig, sensors: &SensorState, round: bool, t: f32) {
    let mut ids: Vec<String> = cfg.slots.iter().filter(|s| !s.is_empty()).cloned().collect();
    if cfg.layout == "custom" && !cfg.background.starts_with("face:") {
        ids = cfg.widgets.iter().map(|w| w.sensor.clone()).filter(|s| !s.is_empty()).collect();
    }
    if cfg.background.starts_with("face:") {
        let (m, s2) = faces::pair(cfg, sensors);
        let mut v = vec![m.id, s2.id];
        if cfg.layout == "custom" {
            v.extend(cfg.widgets.iter().map(|w| w.sensor.clone()).filter(|s| !s.is_empty()));
        }
        ids = v;
    }
    let worst = ids.iter().filter_map(|id| alert_level(a, sensors, id, t)).max_by_key(|x| x.2);
    let Some((c, k, lvl)) = worst else { return };
    let s = pm.width() as f32;
    if lvl > 0 {
        rect(pm, 0.0, 0.0, s, s, rgba(c, 0.30 * k));
    }
    let edge = if lvl == 0 { 0.5 } else { 0.9 * k };
    if round {
        for i in 0..6 {
            let r = s * (0.497 - i as f32 * 0.008);
            arc(pm, s / 2.0, s / 2.0, r, 0.0, std::f32::consts::TAU, s * 0.01, rgba(c, edge * (1.0 - i as f32 / 6.0)));
        }
    } else {
        for i in 0..6 {
            let d = s * (0.004 + i as f32 * 0.008);
            let st = Stroke { width: s * 0.01, ..Default::default() };
            if let Some(r) = Rect::from_xywh(d, d, s - 2.0 * d, s - 2.0 * d) {
                let p = PathBuilder::from_rect(r);
                pm.stroke_path(&p, &paint(rgba(c, edge * (1.0 - i as f32 / 6.0))), &st, Transform::identity(), None);
            }
        }
    }
}

// ------------------------------------------------------------------ frame

/// `alerts`: the warnings to flash on this screen (None for gallery pictures).
pub fn render(f: &mut Fonts, st: &mut ScreenState, cfg: &ScreenConfig, sensors: &SensorState, size: u32, round: bool, t: f32, alerts: Option<&Alerts>) -> Pixmap {
    let mut pm = Pixmap::new(size, size).unwrap();
    let text_style = alerts.map(|a| a.style != "screen").unwrap_or(false);
    ALERT.with(|c| *c.borrow_mut() = if text_style { alerts.map(|a| (a.clone(), t)) } else { None });
    if let Some(name) = cfg.background.strip_prefix("face:") {
        let (mut m, mut s2) = faces::pair(cfg, sensors);
        m.alert = text_alert(sensors, &m.id);
        s2.alert = text_alert(sensors, &s2.id);
        let tf = t * cfg.speed.clamp(0.1, 4.0);
        faces::draw(&mut pm, f, st, name, &m, &s2, sensors, round, tf, faces::O::from(cfg));
        // parts added on top of a design in the design studio
        if cfg.layout == "custom" && !cfg.widgets.is_empty() {
            let s = size as f32;
            let acc = hex(&cfg.accent);
            let heat = |v: &V| vc(v, acc);
            for w in &cfg.widgets {
                draw_widget(&mut pm, f, w, cfg, sensors, s, &heat, tf);
            }
        }
    } else {
        background(&mut pm, f, st, cfg, t);
        if cfg.blur > 0.01 {
            blur(&mut pm, (cfg.blur.clamp(0.0, 1.0) * size as f32 * 0.025).round() as usize);
        }
        f.outline = if cfg.outline { 1.0 } else { 0.0 };
        layout(&mut pm, f, cfg, sensors, round, t);
        f.outline = 0.0;
    }
    ALERT.with(|c| *c.borrow_mut() = None);
    if let Some(a) = alerts.filter(|a| a.style == "screen") {
        screen_alert(&mut pm, a, cfg, sensors, round, t);
    }
    if let Some((n, at)) = st.identify {
        if at.elapsed().as_secs_f32() < 4.0 {
            let s = size as f32;
            rect(&mut pm, 0.0, 0.0, s, s, Color::from_rgba8(0, 0, 0, 200));
            f.draw(&mut pm, 1, &n.to_string(), s * 0.6, s / 2.0, s * 0.7, 1, hex(&cfg.accent), 1.0);
        } else {
            st.identify = None;
        }
    }
    grade(&mut pm, cfg.contrast, cfg.saturation, cfg.gamma, cfg.black);
    if !round && cfg.corner > 0.005 {
        round_corners(&mut pm, cfg.corner);
    }
    match cfg.rotation {
        90 | 180 | 270 => rotate(&pm, cfg.rotation),
        _ => pm,
    }
}

/// Square screens: black out the corners outside a rounded rectangle (r = 0.5 makes a circle).
fn round_corners(pm: &mut Pixmap, r: f32) {
    let s = pm.width() as f32;
    let rad = r.clamp(0.0, 0.5) * s;
    let w = pm.width() as usize;
    let data = pm.data_mut();
    for y in 0..w {
        let fy = y as f32 + 0.5;
        let dy = if fy < rad { rad - fy } else if fy > s - rad { fy - (s - rad) } else { continue };
        for x in 0..w {
            let fx = x as f32 + 0.5;
            let dx = if fx < rad { rad - fx } else if fx > s - rad { fx - (s - rad) } else { continue };
            let d = (dx * dx + dy * dy).sqrt() - rad;
            if d > -1.0 {
                let k = (1.0 - (d + 1.0) / 2.0).clamp(0.0, 1.0); // 1 inside, 0 outside, soft over 2 px
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    data[i + c] = (data[i + c] as f32 * k) as u8;
                }
                data[i + 3] = 255;
            }
        }
    }
}

/// Contrast, saturation and gamma over the finished picture. 0 (an old settings file) counts as 1.
fn grade(pm: &mut Pixmap, contrast: f32, saturation: f32, gamma: f32, black: f32) {
    let f = |v: f32, lo: f32, hi: f32| if v <= 0.0 { 1.0 } else { v.clamp(lo, hi) };
    let (c, s, g) = (f(contrast, 0.5, 2.0), f(saturation, 0.0, 2.5), f(gamma, 0.5, 2.0));
    let bp = black.clamp(0.0, 0.3);
    if (c - 1.0).abs() < 0.01 && (s - 1.0).abs() < 0.01 && (g - 1.0).abs() < 0.01 && bp < 0.005 {
        return;
    }
    // black point, gamma and contrast per channel through a table (pure black always stays black);
    // saturation mixes each pixel with its own grey
    let lut: Vec<f32> = (0..256)
        .map(|i| {
            let v = ((i as f32 / 255.0 - bp) / (1.0 - bp)).max(0.0);
            if v == 0.0 {
                return 0.0;
            }
            ((v.powf(g) - 0.5) * c + 0.5).clamp(0.0, 1.0) * 255.0
        })
        .collect();
    for px in pm.data_mut().chunks_exact_mut(4) {
        let (r, gg, b) = (lut[px[0] as usize], lut[px[1] as usize], lut[px[2] as usize]);
        let y = 0.299 * r + 0.587 * gg + 0.114 * b;
        let a = px[3] as f32;
        px[0] = (y + (r - y) * s).clamp(0.0, a) as u8;
        px[1] = (y + (gg - y) * s).clamp(0.0, a) as u8;
        px[2] = (y + (b - y) * s).clamp(0.0, a) as u8;
    }
}

/// Soft blur: three box-blur passes each way, which comes out close to a Gaussian.
fn blur(pm: &mut Pixmap, r: usize) {
    if r == 0 {
        return;
    }
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let d = pm.data_mut();
    let mut line = vec![0u8; w.max(h) * 4];
    let pass = |d: &mut [u8], line: &mut [u8], n: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        for j in 0..count {
            for i in 0..n {
                let p = at(j, i);
                line[i * 4..i * 4 + 4].copy_from_slice(&d[p..p + 4]);
            }
            let mut sum = [0u32; 4];
            let get = |line: &[u8], i: isize, k: usize| line[(i.clamp(0, n as isize - 1) as usize) * 4 + k] as u32;
            for k in 0..4 {
                for i in -(r as isize)..=(r as isize) {
                    sum[k] += get(line, i, k);
                }
            }
            let div = (2 * r + 1) as u32;
            for i in 0..n {
                let p = at(j, i);
                for k in 0..4 {
                    d[p + k] = (sum[k] / div) as u8;
                    sum[k] += get(line, i as isize + r as isize + 1, k);
                    sum[k] -= get(line, i as isize - r as isize, k);
                }
            }
        }
    };
    for _ in 0..3 {
        pass(d, &mut line, w, h, &|y, x| (y * w + x) * 4);
        pass(d, &mut line, h, w, &|x, y| (y * w + x) * 4);
    }
}

fn rotate(src: &Pixmap, deg: u16) -> Pixmap {
    let n = src.width();
    let mut out = Pixmap::new(n, n).unwrap();
    let (s, d) = (src.data(), out.data_mut());
    for y in 0..n {
        for x in 0..n {
            let (sx, sy) = match deg {
                90 => (y, n - 1 - x),
                180 => (n - 1 - x, n - 1 - y),
                _ => (n - 1 - y, x),
            };
            let si = ((sy * n + sx) * 4) as usize;
            let di = ((y * n + x) * 4) as usize;
            d[di..di + 4].copy_from_slice(&s[si..si + 4]);
        }
    }
    out
}

/// JPEG for the panel, shrinking quality until it fits the panel's payload limit.
pub fn jpeg(pm: &Pixmap, max_bytes: usize) -> Vec<u8> {
    let (w, h) = (pm.width(), pm.height());
    let rgb: Vec<u8> = pm.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut q = 88u8;
    loop {
        let mut out = Vec::with_capacity(96 * 1024);
        let _ = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q).encode(&rgb, w, h, image::ExtendedColorType::Rgb8);
        if out.len() <= max_bytes || q <= 40 {
            return out;
        }
        q -= 12;
    }
}
