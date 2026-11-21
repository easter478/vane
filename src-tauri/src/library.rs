//! Videos imported from Lian Li L-Connect 3 (asked for by the user): the cooler screen's theme videos and
//! the SL V3 fan screens' animations are copied into %APPDATA%\Vane\lianli so they keep working after
//! L-Connect is removed. Each gets a small JPEG thumbnail next to it for the background picker.

use base64::Engine as _;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
pub struct Item {
    pub path: String,
    pub name: String,
    /// "cooler" or "fan"
    pub kind: String,
    pub thumb: Option<String>,
}

fn root() -> PathBuf {
    crate::config::dir().join("lianli")
}

fn assets() -> PathBuf {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    Path::new(&pf).join("Lian-Li").join("L-Connect 3").join("Assets")
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut list: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    list.sort();
    for p in list {
        if p.is_dir() {
            if !p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("thumbnail")) {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
}

/// True if L-Connect's assets are on this PC.
pub fn available() -> bool {
    assets().is_dir()
}

/// Copy L-Connect's videos in. Returns how many were imported (already-imported ones count too).
pub fn import() -> Result<usize, String> {
    let a = assets();
    if !a.is_dir() {
        return Err("L-Connect 3 isn't installed on this PC, so there's nothing to import.".into());
    }
    let mut cooler = Vec::new();
    files(&a.join("hydroshift-ii-lcd-s").join("video"), "mp4", &mut cooler);
    let mut fan = Vec::new();
    files(&a.join("SLV3").join("animation"), "avi", &mut fan);
    let mut n = 0;
    for (kind, list) in [("cooler", cooler), ("fan", fan)] {
        let dst = root().join(kind);
        std::fs::create_dir_all(&dst).map_err(|e| format!("Couldn't create {}: {e}", dst.display()))?;
        for (i, src) in list.iter().enumerate() {
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_lowercase();
            let target = dst.join(format!("{kind}-{:02}.{ext}", i + 1));
            let thumb_path = target.with_extension("jpg");
            if target.exists() && thumb_path.exists() {
                n += 1;
                continue;
            }
            // only keep videos Windows can play
            let Some(pm) = crate::video::thumbnail(&src.to_string_lossy(), 200) else { continue };
            if std::fs::copy(src, &target).is_err() {
                continue;
            }
            let _ = std::fs::write(&thumb_path, crate::render::jpeg(&pm, 30_000));
            n += 1;
        }
    }
    if n == 0 {
        return Err("No L-Connect videos could be read on this PC.".into());
    }
    Ok(n)
}

pub fn list() -> Vec<Item> {
    let mut out = Vec::new();
    for kind in ["cooler", "fan"] {
        let mut v = Vec::new();
        let dir = root().join(kind);
        files(&dir, "mp4", &mut v);
        files(&dir, "avi", &mut v);
        v.sort();
        for (i, p) in v.iter().enumerate() {
            let thumb = std::fs::read(p.with_extension("jpg")).ok().map(|b| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)));
            out.push(Item {
                path: p.display().to_string(),
                name: format!("{} {}", if kind == "cooler" { "Cooler video" } else { "Fan video" }, i + 1),
                kind: kind.into(),
                thumb,
            });
        }
    }
    out
}
