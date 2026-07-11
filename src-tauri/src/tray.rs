//! Menubar (tray) icon + menu.

use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::{state, AppState};

/// Transparent PixelVault mark, rendered as a template so macOS tints it to
/// match the menu bar (light/dark).
const TRAY_ICON_PNG: &[u8] = include_bytes!("../icons/tray.png");

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let remaining = app.state::<AppState>().trial.remaining();

    let status = MenuItemBuilder::with_id("status", "PixelVault — watching clipboard")
        .enabled(false)
        .build(app)?;
    let counter = MenuItemBuilder::with_id(
        "counter",
        format!("Free uploads left: {}/{}", remaining, state::FREE_UPLOAD_LIMIT),
    )
    .enabled(false)
    .build(app)?;
    let recent_header = MenuItemBuilder::with_id("recent-header", "Recent uploads (click to copy)")
        .enabled(false)
        .build(app)?;

    // Pre-create fixed slots; `crate::refresh_recent` fills/enables them.
    let mut recent_items = Vec::with_capacity(state::RECENT_LIMIT);
    for i in 0..state::RECENT_LIMIT {
        let item = MenuItemBuilder::with_id(format!("recent-{i}"), "—")
            .enabled(false)
            .build(app)?;
        recent_items.push(item);
    }

    let toggle = MenuItemBuilder::with_id("toggle", "Pause watching").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Open Settings…").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit PixelVault").build(app)?;

    let mut builder = MenuBuilder::new(app)
        .item(&status)
        .item(&counter)
        .separator()
        .item(&recent_header);
    for item in &recent_items {
        builder = builder.item(item);
    }
    let menu = builder
        .separator()
        .item(&toggle)
        .item(&settings)
        .separator()
        .item(&quit)
        .build()?;

    // Stash dynamic items so the watcher / toggle / recent list can update them.
    {
        let st = app.state::<AppState>();
        *st.counter_item.lock().unwrap() = Some(counter.clone());
        *st.toggle_item.lock().unwrap() = Some(toggle.clone());
        *st.recent_items.lock().unwrap() = recent_items;
    }

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(Image::from_bytes(TRAY_ICON_PNG)?)
        .icon_as_template(true)
        .tooltip("PixelVault")
        .menu(&menu)
        .on_menu_event(move |app, event| handle_menu(app, event.id().as_ref()))
        .build(app)?;

    *app.state::<AppState>().tray_icon.lock().unwrap() = Some(tray);

    // Reflect any persisted recent uploads into the freshly built menu.
    crate::refresh_recent(app);

    Ok(())
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "toggle" => crate::toggle_watching(app),
        "settings" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        "quit" => app.exit(0),
        _ if id.starts_with("recent-") && id != "recent-header" => {
            if let Ok(idx) = id["recent-".len()..].parse::<usize>() {
                copy_recent(app, idx);
            }
        }
        _ => {}
    }
}

fn copy_recent(app: &AppHandle, idx: usize) {
    let recent = app.state::<AppState>().trial.recent();
    if let Some(url) = recent.get(idx) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            if clipboard.set_text(url.clone()).is_ok() {
                crate::notify(app, "URL copied", url);
            }
        }
    }
}
