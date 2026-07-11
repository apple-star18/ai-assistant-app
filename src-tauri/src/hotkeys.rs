use std::{
    sync::{Mutex, MutexGuard},
    thread,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
    },
    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
};

use crate::automation;

const MAIN_WINDOW_LABEL: &str = "main";
const HOTKEY_EVENT: &str = "hotkeys://state";
const HOTKEY_MODIFIERS: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0);
const HOTKEY_MODE_1_ID: i32 = 101;
const HOTKEY_MODE_2_ID: i32 = 102;
const HOTKEY_MODE_3_ID: i32 = 103;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeySnapshot {
    is_running: bool,
    bindings: Vec<HotkeyBindingSnapshot>,
    last_error: Option<String>,
}

impl Default for HotkeySnapshot {
    fn default() -> Self {
        Self {
            is_running: false,
            bindings: default_hotkeys()
                .iter()
                .map(|binding| binding.snapshot(false, None))
                .collect(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindingSnapshot {
    action: &'static str,
    accelerator: &'static str,
    registered: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyCommandError {
    code: &'static str,
    message: String,
}

#[derive(Debug, Default)]
pub struct HotkeyStore {
    snapshot: Mutex<HotkeySnapshot>,
}

#[derive(Clone, Copy)]
struct HotkeyBinding {
    id: i32,
    vk: u32,
    action: HotkeyAction,
    accelerator: &'static str,
}

#[derive(Clone, Copy)]
enum HotkeyAction {
    Mode1,
    Mode2,
    Mode3,
}

type CommandResult<T> = Result<T, HotkeyCommandError>;

pub fn setup(app: &AppHandle) {
    let app_handle = app.clone();

    if let Err(error) = thread::Builder::new()
        .name("global-hotkey-listener".to_string())
        .spawn(move || run_hotkey_thread(app_handle))
    {
        update_snapshot(app, |snapshot| {
            snapshot.is_running = false;
            snapshot.last_error = Some(format!("Failed to start global hotkey listener: {error}"));
        });
    }
}

#[tauri::command]
pub fn hotkeys_get_state(state: State<'_, HotkeyStore>) -> CommandResult<HotkeySnapshot> {
    Ok(state.snapshot()?.clone())
}

fn run_hotkey_thread(app: AppHandle) {
    let mut registered = Vec::new();

    update_snapshot(&app, |snapshot| {
        snapshot.is_running = true;
        snapshot.last_error = None;
        snapshot.bindings.clear();
    });

    for binding in default_hotkeys() {
        match unsafe { RegisterHotKey(None, binding.id, HOTKEY_MODIFIERS, binding.vk) } {
            Ok(()) => {
                registered.push(binding);
                push_binding_snapshot(&app, binding.snapshot(true, None));
            }
            Err(error) => {
                let message = format!(
                    "{} could not be registered. The shortcut may already be in use: {error}",
                    binding.accelerator
                );
                push_binding_snapshot(&app, binding.snapshot(false, Some(message.clone())));
                update_snapshot(&app, |snapshot| {
                    snapshot.last_error = Some(message);
                });
            }
        }
    }

    let mut message = MSG::default();

    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };

        if status.0 <= 0 {
            break;
        }

        if message.message == WM_HOTKEY {
            let id = message.wParam.0 as i32;

            if let Some(binding) = registered.iter().find(|binding| binding.id == id).copied() {
                dispatch_hotkey(app.clone(), binding.action);
            }
        }
    }

    for binding in registered {
        let _ = unsafe { UnregisterHotKey(None, binding.id) };
    }

    update_snapshot(&app, |snapshot| {
        snapshot.is_running = false;
    });
}

fn dispatch_hotkey(app: AppHandle, action: HotkeyAction) {
    thread::spawn(move || {
        let result = match action {
            HotkeyAction::Mode1 => automation::run_mode_1(&app),
            HotkeyAction::Mode2 => automation::run_mode_2(&app),
            HotkeyAction::Mode3 => automation::run_mode_3(&app),
        };

        if let Err(message) = result {
            update_snapshot(&app, |snapshot| {
                snapshot.last_error = Some(message);
            });
        }
    });
}

fn push_binding_snapshot(app: &AppHandle, binding: HotkeyBindingSnapshot) {
    update_snapshot(app, |snapshot| {
        snapshot.bindings.push(binding);
    });
}

fn update_snapshot(app: &AppHandle, update: impl FnOnce(&mut HotkeySnapshot)) {
    let state = app.state::<HotkeyStore>();
    let next_snapshot = match state.snapshot.lock() {
        Ok(mut snapshot) => {
            update(&mut snapshot);
            snapshot.clone()
        }
        Err(_) => return,
    };

    let _ = app.emit_to(MAIN_WINDOW_LABEL, HOTKEY_EVENT, next_snapshot);
}

fn default_hotkeys() -> [HotkeyBinding; 3] {
    [
        HotkeyBinding {
            id: HOTKEY_MODE_1_ID,
            vk: b'1' as u32,
            action: HotkeyAction::Mode1,
            accelerator: "Ctrl+Alt+1",
        },
        HotkeyBinding {
            id: HOTKEY_MODE_2_ID,
            vk: b'2' as u32,
            action: HotkeyAction::Mode2,
            accelerator: "Ctrl+Alt+2",
        },
        HotkeyBinding {
            id: HOTKEY_MODE_3_ID,
            vk: b'3' as u32,
            action: HotkeyAction::Mode3,
            accelerator: "Ctrl+Alt+3",
        },
    ]
}

impl HotkeyBinding {
    fn snapshot(self, registered: bool, error: Option<String>) -> HotkeyBindingSnapshot {
        HotkeyBindingSnapshot {
            action: match self.action {
                HotkeyAction::Mode1 => "shortcutMode1",
                HotkeyAction::Mode2 => "shortcutMode2",
                HotkeyAction::Mode3 => "shortcutMode3",
            },
            accelerator: self.accelerator,
            registered,
            error,
        }
    }
}

trait HotkeyStoreExt {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, HotkeySnapshot>>;
}

impl HotkeyStoreExt for HotkeyStore {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, HotkeySnapshot>> {
        self.snapshot.lock().map_err(|_| HotkeyCommandError {
            code: "state_unavailable",
            message: "Hotkey state is unavailable.".to_string(),
        })
    }
}
