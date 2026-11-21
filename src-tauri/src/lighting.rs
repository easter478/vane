//! Lighting for the wireless fans and pump, with effects like L-Connect's. An effect is rendered here as a
//! short looping animation and uploaded once; the fans then play it by themselves, so it costs nothing
//! while it runs. Colours from OpenRGB are uploaded as a still frame.

use crate::config::Lighting;
use crate::wireless::WirelessDevice;

/// (id, name, uses colours)
pub const EFFECTS: &[(&str, &str, bool)] = &[
    ("auto", "Leave as it is", false),
    ("static", "Static", true),
    ("breathing", "Breathing", true),
    ("rainbow", "Rainbow", false),
    ("wave", "Rainbow wave", false),
    ("cycle", "Colour cycle", true),
    ("meteor", "Meteor", true),
    ("runway", "Runway", true),
    ("twinkle", "Twinkle", true),
    ("temperature", "Temperature", false),
    ("openrgb", "OpenRGB", false),
    ("off", "Off", false),
];

pub struct Anim {
    pub frames: Vec<Vec<[u8; 3]>>,
    pub interval_ms: u32,
}

pub fn hex(s: &str) -> [u8; 3] {
    let h = s.trim_start_matches('#');
    if h.len() < 6 {
        return [255, 255, 255];
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(255);
    [p(0), p(2), p(4)]
}

fn hsv(h: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn f(c: [u8; 3]) -> [f32; 3] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}

/// Colour for a temperature: blue when cool, through teal, green and amber, to red when hot.
pub fn heat(t: f64) -> [f32; 3] {
    let stops: [(f64, [f32; 3]); 5] = [(35.0, [0.0, 0.35, 1.0]), (48.0, [0.0, 0.8, 0.75]), (60.0, [0.1, 1.0, 0.2]), (72.0, [1.0, 0.65, 0.0]), (82.0, [1.0, 0.0, 0.0])];
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        if t <= w[1].0 {
            return lerp(w[0].1, w[1].1, ((t - w[0].0) / (w[1].0 - w[0].0)) as f32);
        }
    }
    stops[4].1
}

/// Render the effect for this device. `div` thins the animation (half the frames, twice as long each)
/// when the full one is too big for the fans' memory.
pub fn build(dev: &WirelessDevice, l: &Lighting, temp: Option<f64>, div: usize) -> Option<Anim> {
    let zones = dev.led_zones();
    if zones.is_empty() {
        return None;
    }
    let colors: Vec<[f32; 3]> = if l.colors.is_empty() { vec![[1.0, 1.0, 1.0]] } else { l.colors.iter().map(|c| f(hex(c))).collect() };
    let nz = zones.len();
    let per = *zones.last().unwrap();
    // speed 5 is the base; each step is about 1.45x faster or slower
    let pace = 1.45f32.powf(5.0 - l.speed.clamp(1, 10) as f32);
    let bright = l.brightness.min(100) as f32 / 100.0;
    let dir = if l.reverse { -1.0 } else { 1.0 };
    let (n, base_ms): (usize, f32) = match l.effect.as_str() {
        "static" | "off" | "temperature" => (1, 1000.0),
        "breathing" => (64 * colors.len().min(4), 40.0),
        "cycle" => (96, 50.0),
        "rainbow" | "meteor" | "runway" => (per, 2000.0 / per as f32),
        "wave" => (90, 40.0),
        "twinkle" => (96, 50.0),
        _ => return None,
    };
    let n = if n > 1 { (n / div.max(1)).max(2) } else { 1 };
    let interval_ms = if n > 1 { (base_ms * pace * div.max(1) as f32).round().clamp(10.0, 40_000.0) as u32 } else { 1000 };
    let mut seed = 0x2545_f491u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 10_000) as f32 / 10_000.0
    };
    // twinkle: each LED's sparkle start frame (or none)
    let total: usize = zones.iter().sum();
    let sparks: Vec<Option<usize>> = (0..total).map(|_| if rnd() < 0.35 { Some((rnd() * n as f32) as usize) } else { None }).collect();
    let mut frames = Vec::with_capacity(n);
    for k in 0..n {
        let t = k as f32 / n as f32;
        let mut frame = Vec::with_capacity(total);
        let mut led = 0usize;
        for (z, &len) in zones.iter().enumerate() {
            for i in 0..len {
                let pos = i as f32 / len as f32;
                let c: [f32; 3] = match l.effect.as_str() {
                    "off" => [0.0; 3],
                    "static" => colors[z % colors.len()],
                    "temperature" => heat(temp.unwrap_or(40.0)),
                    "breathing" => {
                        let per_col = (n / colors.len().min(4)).max(1);
                        let ci = (k / per_col) % colors.len();
                        let ph = (k % per_col) as f32 / per_col as f32;
                        let v = 0.5 - 0.5 * (ph * std::f32::consts::TAU).cos();
                        let c = colors[ci];
                        [c[0] * (0.03 + 0.97 * v), c[1] * (0.03 + 0.97 * v), c[2] * (0.03 + 0.97 * v)]
                    }
                    "cycle" => {
                        if colors.len() < 2 {
                            hsv(t * dir)
                        } else {
                            let x = t * colors.len() as f32;
                            let a = x as usize % colors.len();
                            lerp(colors[a], colors[(a + 1) % colors.len()], x.fract())
                        }
                    }
                    "rainbow" => hsv(pos - t * dir),
                    "wave" => hsv(z as f32 / nz as f32 * 0.8 + pos * 0.12 - t * dir),
                    "meteor" => {
                        let head = t * len as f32;
                        let ii = if l.reverse { len - 1 - i } else { i };
                        let d = (head - ii as f32).rem_euclid(len as f32);
                        let v = if d < 1.0 { 1.0 } else { (1.0 - d / (len as f32 * 0.4)).max(0.0).powf(1.6) };
                        let c = colors[z % colors.len()];
                        [c[0] * v, c[1] * v, c[2] * v]
                    }
                    "runway" => {
                        let seg = (len / 4).max(1);
                        let shift = (k as f32 * len as f32 / n as f32) as isize * if l.reverse { -1 } else { 1 };
                        let idx = ((i as isize + shift).rem_euclid(len as isize) as usize / seg) % 2;
                        let other = if colors.len() > 1 { colors[1] } else { [0.0; 3] };
                        if idx == 0 { colors[0] } else { other }
                    }
                    "twinkle" => {
                        let c = colors[led % colors.len()];
                        let base = 0.06;
                        let v = match sparks[led] {
                            Some(s) => {
                                let age = (k + n - s) % n;
                                if age < 14 { base + (1.0 - base) * (1.0 - age as f32 / 14.0) } else { base }
                            }
                            None => base,
                        };
                        [c[0] * v, c[1] * v, c[2] * v]
                    }
                    _ => [0.0; 3],
                };
                frame.push([(c[0] * bright * 255.0).round().clamp(0.0, 255.0) as u8, (c[1] * bright * 255.0).round().clamp(0.0, 255.0) as u8, (c[2] * bright * 255.0).round().clamp(0.0, 255.0) as u8]);
                led += 1;
            }
        }
        frames.push(frame);
    }
    Some(Anim { frames, interval_ms })
}
