//! Vane: a small replacement for Lian Li L-Connect 3 on this PC. Drives the HydroShift II cooler screen,
//! the three SL V3 LCD fan screens and the wireless fans. Lives in the tray; the window is created only
//! when opened and destroyed when closed, so the background footprint stays small.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod engine;
mod render;
mod sensors;
mod system;
mod usb;
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
    serde_json::json!({
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
        .invoke_handler(tauri::generate_handler![snapshot, meta, set_config, preview, identify, lconnect, resume, pick_image, quit])
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
