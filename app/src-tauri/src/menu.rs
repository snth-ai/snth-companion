//! The macOS application menu. Tauri's default menu quits the process
//! straight from Cmd+Q; ours routes Quit through the same clean shutdown
//! as the tray's Quit (stop the link and our helper daemon, write what is
//! batched). The Edit menu is what makes Cmd+C / Cmd+V work in text
//! fields on macOS.

use std::sync::Arc;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Manager};

use crate::actions;
use crate::state::Ctx;

const QUIT_ID: &str = "app_quit";

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let quit = MenuItemBuilder::with_id(QUIT_ID, "Quit SNTH Connect")
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;
    let app_menu = SubmenuBuilder::new(app, "SNTH Connect")
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .close_window()
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &edit, &window])
        .build()?;
    app.set_menu(menu)?;

    app.on_menu_event(|app, event| {
        if event.id().as_ref() != QUIT_ID {
            return;
        }
        if let Some(ctx) = app.try_state::<Arc<Ctx>>() {
            let ctx = ctx.inner().clone();
            tauri::async_runtime::spawn(async move { actions::quit(&ctx).await });
        }
    });
    Ok(())
}
