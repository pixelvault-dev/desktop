//! Menubar (tray) icon + menu.

use tauri::menu::MenuBuilder;
use tauri::menu::MenuItemBuilder;
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::{state, AppState};

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
    let toggle = MenuItemBuilder::with_id("toggle", "Pause watching").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Open Settings…").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit PixelVault").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&status)
        .item(&counter)
        .separator()
        .item(&toggle)
        .item(&settings)
        .separator()
        .item(&quit)
        .build()?;

    // Stash the dynamic items so the watcher / toggle can update their labels.
    {
        let st = app.state::<AppState>();
        *st.counter_item.lock().unwrap() = Some(counter.clone());
        *st.toggle_item.lock().unwrap() = Some(toggle.clone());
    }

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().cloned().unwrap())
        .tooltip("PixelVault")
        .menu(&menu)
        .on_menu_event(move |app, event| handle_menu(app, event.id().as_ref()))
        .build(app)?;

    *app.state::<AppState>().tray_icon.lock().unwrap() = Some(tray);

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
        _ => {}
    }
}
