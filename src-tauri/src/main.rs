//! Vane: a small replacement for Lian Li L-Connect 3 on this PC. Drives the HydroShift II cooler screen,
//! the three SL V3 LCD fan screens and the wireless fans. Lives in the tray; the window is created only
//! when opened and destroyed when closed, so the background footprint stays small.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod library;
mod lighting;
mod openrgb;
mod openrgb_client;
mod render;
mod sensors;
mod system;
mod tinyuz;
mod usb;
mod video;
mod wireless;

use base64::Engine as _;
use engine::Engine;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

type Eng = Arc<Engine>;

fn open_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Vane")
        .inner_size(1200.0, 800.0)
        .min_inner_size(820.0, 560.0)
        .decorations(false)
        .visible(true)
        .build();
}

// ------------------------------------------------------------------ commands

#[tauri::command]
fn snapshot(e: State<Eng>) -> serde_json::Value {
    let status = e.status.read().clone();
    let cfg = e.cfg.read().clone();
    serde_json::json!({
        "status": status,
        "config": cfg,
        "sensors": e.snapshot_sensors(),
    })
}

#[tauri::command]
fn meta() -> serde_json::Value {
    let themes: Vec<serde_json::Value> = config::THEMES.iter().map(|(id, name, cooler, fans)| serde_json::json!({"id": id, "name": name, "cooler": cooler, "fans": fans, "config": config::theme(id)})).collect();
    let backgrounds: Vec<serde_json::Value> = render::BACKGROUNDS.iter().map(|(id, n)| serde_json::json!({"id": id, "name": n})).collect();
    let layouts: Vec<serde_json::Value> = render::LAYOUTS.iter().map(|(id, n, slots)| serde_json::json!({"id": id, "name": n, "slots": slots})).collect();
    let presets: Vec<serde_json::Value> = ["quiet", "balanced", "perf", "full"].iter().map(|p| serde_json::json!({"id": p, "curve": config::preset(p)})).collect();
    let effects: Vec<serde_json::Value> = lighting::EFFECTS.iter().map(|(id, n, c)| serde_json::json!({"id": id, "name": n, "colors": c})).collect();
    let pieces: Vec<serde_json::Value> = render::PIECES.iter().map(|(face, id, n, x, y, w, h)| serde_json::json!({"face": face, "id": id, "name": n, "x": x, "y": y, "w": w, "h": h})).collect();
    let widgets: Vec<serde_json::Value> = render::WIDGETS.iter().map(|(id, n, w, h)| serde_json::json!({"id": id, "name": n, "w": w, "h": h})).collect();
    serde_json::json!({
        "effects": effects,
        "widgets": widgets,
        "pieces": pieces,
        "lianli_available": library::available(),
        "themes": themes,
        "backgrounds": backgrounds,
        "layouts": layouts,
        "presets": presets,
        "version": env!("CARGO_PKG_VERSION"),
    })
}

#[tauri::command]
fn set_config(e: State<Eng>, config: config::Config) {
    let old_auto = e.cfg.read().start_with_windows;
    if config.start_with_windows != old_auto {
        system::set_autostart(config.start_with_windows);
    }
    *e.cfg.write() = config;
    e.cfg_changed.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn preview(e: State<Eng>, key: String) -> Option<String> {
    let p = e.previews.read().get(&key).cloned()?;
    Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(p.as_slice())))
}

#[tauri::command]
fn identify(e: State<Eng>) {
    e.identify_all();
}

#[tauri::command]
fn lconnect(e: State<Eng>, on: bool) -> Result<(), String> {
    if on {
        e.paused.store(true, Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    let r = system::set_lconnect(on);
    if !on && r.is_ok() {
        e.paused.store(false, Ordering::Relaxed);
    }
    if r.is_err() && on {
        e.paused.store(false, Ordering::Relaxed);
    }
    r
}

#[tauri::command]
fn resume(e: State<Eng>) {
    e.paused.store(false, Ordering::Relaxed);
}

#[tauri::command]
fn pick_image() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Pictures and GIFs", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
        .set_title("Choose a picture or GIF")
        .pick_file()
        .map(|p| p.display().to_string())
}

#[tauri::command]
fn pick_video() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Videos", &["mp4", "avi", "wmv", "mov", "mkv", "m4v"])
        .set_title("Choose a video")
        .pick_file()
        .map(|p| p.display().to_string())
}

/// Render designs as small pictures (with live readings) for the gallery and pickers.
#[tauri::command]
async fn render_previews(e: State<'_, Eng>, configs: Vec<(String, config::ScreenConfig)>, round: bool, size: Option<u32>) -> Result<std::collections::BTreeMap<String, String>, String> {
    let e = e.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let size = size.unwrap_or(220).clamp(64, 480);
        let mut fonts = render::Fonts::load();
        let mut out = std::collections::BTreeMap::new();
        for (id, cfg) in configs {
            let mut st = render::ScreenState::new(id.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)));
            // run the animation for a moment first so rain, particles and waves have filled in
            let warm = if matches!(cfg.background.as_str(), "carbon" | "solid" | "image") { 1 } else if cfg.background.starts_with("face:") { 3 } else { 40 };
            let mut pm = None;
            for k in 0..warm {
                let s = e.sensors.read();
                pm = Some(render::render(&mut fonts, &mut st, &cfg, &s, size, round, 1.0 + k as f32 * 0.08, None));
            }
            if let Some(pm) = pm {
                out.insert(id, format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(render::jpeg(&pm, 60_000))));
            }
        }
        out
    })
    .await
    .map_err(|e| e.to_string())
}

thread_local! {
    /// fonts and animation state for the design studio's live preview, kept between frames
    static STUDIO: std::cell::RefCell<Option<(render::Fonts, render::ScreenState, String)>> = const { std::cell::RefCell::new(None) };
}

/// One live frame of the design being edited in the design studio.
#[tauri::command]
async fn studio_frame(e: State<'_, Eng>, config: config::ScreenConfig, round: bool, size: Option<u32>, t: f32) -> Result<String, String> {
    let e = e.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let size = size.unwrap_or(360).clamp(64, 480);
        STUDIO.with(|c| {
            let mut c = c.borrow_mut();
            let key = format!("{}|{}|{}", config.background, config.image, round);
            if c.as_ref().map(|x| x.2 != key).unwrap_or(true) {
                let fonts = c.take().map(|x| x.0).unwrap_or_else(render::Fonts::load);
                *c = Some((fonts, render::ScreenState::new(11), key));
            }
            let (fonts, st, _) = c.as_mut().unwrap();
            let alerts = e.cfg.read().alerts.clone();
            let s = e.sensors.read();
            let pm = render::render(fonts, st, &config, &s, size, round, t, Some(&alerts));
            format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(render::jpeg(&pm, 90_000)))
        })
    })
    .await
    .map_err(|e| e.to_string())
}

fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let n = n.trim().to_string();
    if n.is_empty() { "Design".into() } else { n.chars().take(60).collect() }
}

/// Save a design as a .zip: design.json plus its picture or video, if it has one.
#[tauri::command]
async fn export_design(name: String, config: config::ScreenConfig) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = rfd::FileDialog::new().add_filter("Vane design", &["zip"]).set_title("Export design").set_file_name(format!("{}.zip", safe_name(&name))).save_file() else {
            return Ok(None);
        };
        let mut cfg = config.clone();
        let media = std::path::Path::new(&config.image);
        let mut media_name = None;
        if matches!(cfg.background.as_str(), "image" | "video") && !config.image.is_empty() && media.is_file() {
            let n = media.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "media".into());
            cfg.image = format!("media/{n}");
            media_name = Some(n);
        }
        let doc = serde_json::json!({ "format": "vane-design", "version": 1, "name": name, "app": env!("CARGO_PKG_VERSION"), "config": cfg });
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut z = zip::ZipWriter::new(file);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("design.json", opt).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut z, serde_json::to_string_pretty(&doc).unwrap().as_bytes()).map_err(|e| e.to_string())?;
        if let Some(n) = media_name {
            let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
            z.start_file(format!("media/{n}"), stored).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(media).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut z).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a design .zip: returns its name and settings. Its picture or video is unpacked into
/// %APPDATA%\Vane\designs\<name>\ (file names only, nothing outside that folder).
#[tauri::command]
async fn import_design() -> Result<Option<(String, config::ScreenConfig)>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = rfd::FileDialog::new().add_filter("Vane design", &["zip"]).set_title("Import design").pick_file() else {
            return Ok(None);
        };
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(file).map_err(|_| "This file is not a zip.".to_string())?;
        let doc: serde_json::Value = {
            let mut f = z.by_name("design.json").map_err(|_| "This zip has no design.json, so it is not a Vane design.".to_string())?;
            if f.size() > 2_000_000 {
                return Err("design.json is too big.".into());
            }
            let mut s = String::new();
            std::io::Read::read_to_string(&mut f, &mut s).map_err(|e| e.to_string())?;
            serde_json::from_str(&s).map_err(|e| format!("design.json could not be read: {e}"))?
        };
        if doc.get("format").and_then(|v| v.as_str()) != Some("vane-design") {
            return Err("This zip is not a Vane design.".into());
        }
        let name = doc.get("name").and_then(|v| v.as_str()).unwrap_or("Imported design").to_string();
        let mut cfg: config::ScreenConfig = serde_json::from_value(doc.get("config").cloned().unwrap_or_default()).map_err(|e| format!("The design's settings could not be read: {e}"))?;
        if let Some(rel) = cfg.image.strip_prefix("media/").map(|s| s.to_string()) {
            let fname = std::path::Path::new(&rel).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let entry = format!("media/{rel}");
            cfg.image = String::new();
            if !fname.is_empty() {
                if let Ok(mut f) = z.by_name(&entry) {
                    let dir = config::dir().join("designs").join(safe_name(&name));
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let out = dir.join(&fname);
                    let mut o = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                    std::io::copy(&mut f, &mut o).map_err(|e| e.to_string())?;
                    cfg.image = out.display().to_string();
                }
            }
        }
        Ok(Some((name, cfg)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Turn a design's ready-made layout into movable widgets, so it can be edited.
#[tauri::command]
fn preset_widgets(config: config::ScreenConfig, round: bool) -> Vec<config::Widget> {
    render::preset_widgets(&config, round)
}

#[tauri::command]
async fn import_lianli() -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(library::import).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn lianli_library() -> Result<Vec<library::Item>, String> {
    tauri::async_runtime::spawn_blocking(library::list).await.map_err(|e| e.to_string())
}

/// `--page=<name>[:edit]` opens the window on that page (used for screenshots).
#[tauri::command]
fn start_page() -> Option<String> {
    std::env::args().find_map(|a| a.strip_prefix("--page=").map(|s| s.to_string()))
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn main() {
    #[cfg(windows)]
    if !system::claim_single_instance() {
        return;
    }
    let engine = Engine::start();
    let start_hidden = std::env::args().any(|a| a == "--tray");
    {
        let auto = engine.cfg.read().start_with_windows;
        system::set_autostart(auto);
    }

    let app = tauri::Builder::default()
        .manage(engine.clone())
        .invoke_handler(tauri::generate_handler![snapshot, meta, set_config, preview, identify, lconnect, resume, pick_image, pick_video, render_previews, studio_frame, export_design, import_design, preset_widgets, import_lianli, lianli_library, start_page, quit])
        .setup(move |app| {
            let open = MenuItemBuilder::with_id("open", "Open Vane").build(app)?;
            let screens = CheckMenuItemBuilder::with_id("screens", "Screens on").checked(true).build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit Vane").build(app)?;
            let menu = MenuBuilder::new(app).item(&open).item(&screens).separator().item(&quit).build()?;
            let eng = app.state::<Eng>().inner().clone();
            let screens_item = screens.clone();
            TrayIconBuilder::with_id("vane")
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .tooltip("Vane")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, ev| match ev.id().as_ref() {
                    "open" => open_window(app),
                    "screens" => {
                        let mut c = eng.cfg.write();
                        c.screens_on = !c.screens_on;
                        let _ = screens_item.set_checked(c.screens_on);
                        eng.cfg_changed.store(true, Ordering::Relaxed);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        open_window(tray.app_handle());
                    }
                })
                .build(app)?;
            if !start_hidden {
                open_window(app.handle());
            }
            #[cfg(windows)]
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    system::wait_show_request();
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || open_window(&h));
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("starting Vane");

    app.run(|_app, ev| {
        // closing the window keeps Vane running in the tray; only Quit exits
        if let RunEvent::ExitRequested { api, code, .. } = ev {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}
