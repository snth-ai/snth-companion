//! The tray icon and its menu: a status line, Pause / Resume, Open, Quit.

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::state::{Ctx, TrayItems};
use crate::{actions, i18n, windows};

const TRAY_ID: &str = "main";
const ICON: &[u8] = include_bytes!("../icons/tray.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/tray-paused.png");

pub fn build(app: &AppHandle, ctx: &Arc<Ctx>) -> tauri::Result<()> {
    let lang = ctx.lang();
    let status = MenuItem::with_id(app, "status", "SNTH Connect", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", i18n::tray_pause(lang), true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", i18n::tray_open(lang), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", i18n::tray_quit(lang), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICON)?)
        .icon_as_template(true)
        .tooltip("SNTH Connect")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let Some(ctx) = app.try_state::<Arc<Ctx>>() else {
                return;
            };
            let ctx = ctx.inner().clone();
            match event.id().as_ref() {
                "toggle" => {
                    tauri::async_runtime::spawn(async move {
                        if ctx.core.link_brief().paused {
                            actions::resume(&ctx);
                        } else {
                            actions::pause(&ctx).await;
                        }
                    });
                }
                "open" => windows::show_main(app),
                "quit" => {
                    tauri::async_runtime::spawn(async move { actions::quit(&ctx).await });
                }
                _ => {}
            }
        })
        .build(app)?;

    *ctx.tray.lock().unwrap() = Some(TrayItems { status, toggle, open, quit });
    refresh(ctx);
    Ok(())
}

/// Put the current link state and language into the menu and the icon.
pub fn refresh(ctx: &Ctx) {
    let lang = ctx.lang();
    let brief = ctx.core.link_brief();
    let synth = brief
        .synth_name
        .clone()
        .or(brief.synth_id.clone())
        .unwrap_or_default();
    let status = i18n::tray_status(lang, brief.link_state, brief.links.len(), brief.paused, &synth);
    if let Some(items) = ctx.tray.lock().unwrap().as_ref() {
        let _ = items.status.set_text(&status);
        let _ = items.toggle.set_text(if brief.paused {
            i18n::tray_resume(lang)
        } else {
            i18n::tray_pause(lang)
        });
        let onboarded = ctx.prefs.lock().unwrap().onboarded;
        let _ = items.toggle.set_enabled(brief.paired && onboarded);
        let _ = items.open.set_text(i18n::tray_open(lang));
        let _ = items.quit.set_text(i18n::tray_quit(lang));
    }
    if let Some(tray) = ctx.app.tray_by_id(TRAY_ID) {
        let bytes = if brief.paused { ICON_PAUSED } else { ICON };
        if let Ok(img) = Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
            let _ = tray.set_icon_as_template(true);
        }
        let _ = tray.set_tooltip(Some(format!("SNTH Connect: {status}")));
    }
}
