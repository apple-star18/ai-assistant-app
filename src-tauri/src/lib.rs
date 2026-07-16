mod automation;
mod browser;
mod captions;
mod commands;
mod config;
mod hotkeys;
mod profiles;
mod screenshot;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(browser::BrowserStore::default())
        .manage(captions::CaptionStore::default())
        .manage(automation::AutomationStore::default())
        .manage(hotkeys::HotkeyStore::default())
        .manage(profiles::ProfileStore::default())
        .setup(|app| {
            // Profile WebViews request their state as soon as they load, so restore the
            // persisted snapshot before creating any of the browser child WebViews.
            profiles::setup(app.handle());
            browser::setup(app)?;
            hotkeys::setup(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::health::get_app_health,
            browser::browser_get_state,
            browser::browser_open_home,
            browser::browser_navigate,
            browser::browser_reload,
            browser::browser_go_back,
            browser::browser_go_forward,
            browser::browser_focus,
            browser::browser_clear_session,
            browser::browser_resize,
            browser::browser_set_content_protected,
            browser::browser_set_settings_overlay,
            browser::browser_set_profile_overlay,
            browser::browser_set_transparency_overlay,
            browser::browser_set_scale_overlay,
            browser::browser_set_window_opacity,
            browser::browser_set_scale,
            captions::captions_get_state,
            captions::captions_start,
            captions::captions_stop,
            captions::captions_clear,
            captions::captions_submit_to_chatgpt,
            automation::automation_get_state,
            automation::automation_shortcut_mode_1,
            automation::automation_shortcut_mode_2,
            automation::automation_shortcut_mode_3,
            automation::automation_submit_after_upload,
            hotkeys::hotkeys_get_state,
            hotkeys::hotkeys_apply_settings,
            profiles::profiles_get_state,
            profiles::profiles_add,
            profiles::profiles_save,
            profiles::profiles_delete,
            profiles::profiles_activate
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri application");
}
