//! Native menus follow the same preference as the bundled React interface.
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    menu::{Menu, MenuBuilder, MenuItem, SubmenuBuilder},
    Manager,
};

#[derive(Default)]
pub struct Language(AtomicBool);

pub fn is_japanese(app: &tauri::AppHandle) -> bool {
    app.state::<Language>().0.load(Ordering::Relaxed)
}

pub fn apply(app: &tauri::AppHandle, locale: &str) -> Result<(), String> {
    if !matches!(locale, "en" | "ja") {
        return Err("Unsupported UI language".into());
    }
    let ja = locale == "ja";
    let text = |en, jp| if ja { jp } else { en };
    let update = || -> tauri::Result<()> {
        let show = MenuItem::with_id(
            app,
            "show",
            text("Open Dashboard", "ダッシュボードを開く"),
            true,
            None::<&str>,
        )?;
        let settings = MenuItem::with_id(
            app,
            "settings",
            text("App Settings…", "アプリの設定…"),
            true,
            Some("CmdOrCtrl+,"),
        )?;
        let menu = MenuBuilder::new(app)
            .item(
                &SubmenuBuilder::new(app, "kikimimi")
                    .item(&settings)
                    .separator()
                    .quit_with_text(text("Quit kikimimi", "kikimimiを終了"))
                    .build()?,
            )
            .item(
                &SubmenuBuilder::new(app, text("Edit", "編集"))
                    .undo_with_text(text("Undo", "取り消す"))
                    .redo_with_text(text("Redo", "やり直す"))
                    .separator()
                    .cut_with_text(text("Cut", "切り取り"))
                    .copy_with_text(text("Copy", "コピー"))
                    .paste_with_text(text("Paste", "貼り付け"))
                    .select_all_with_text(text("Select All", "すべて選択"))
                    .build()?,
            )
            .build()?;
        app.set_menu(menu)?;
        if let Some(tray) = app.tray_by_id("kikimimi") {
            let quit = MenuItem::with_id(
                app,
                "quit",
                text(
                    "Quit app (collection continues)",
                    "アプリを終了（収集は継続）",
                ),
                true,
                None::<&str>,
            )?;
            tray.set_menu(Some(Menu::with_items(app, &[&show, &settings, &quit])?))?;
        }
        Ok(())
    };
    update().map_err(|error| error.to_string())?;
    app.state::<Language>().0.store(ja, Ordering::Relaxed);
    Ok(())
}
