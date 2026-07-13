use std::{
    fs,
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, MutexGuard,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
        MOD_WIN,
    },
    WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE, WM_HOTKEY},
};

use crate::automation;

const MAIN_WINDOW_LABEL: &str = "main";
const HOTKEY_EVENT: &str = "hotkeys://state";
const HOTKEY_MODE_1_ID: i32 = 101;
const HOTKEY_MODE_2_ID: i32 = 102;
const HOTKEY_MODE_3_ID: i32 = 103;
const HOTKEY_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const HOTKEY_SETTINGS_FILE: &str = "hotkeys.json";

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
    accelerator: String,
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
    controller: Mutex<Option<Sender<HotkeyCommand>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeySettingsRequest {
    bindings: Vec<HotkeyBindingRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindingRequest {
    action: String,
    accelerator: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredHotkeySettings {
    bindings: Vec<StoredHotkeyBinding>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredHotkeyBinding {
    action: String,
    accelerator: String,
}

#[derive(Clone)]
struct HotkeyBinding {
    id: i32,
    modifiers: HOT_KEY_MODIFIERS,
    vk: u32,
    action: HotkeyAction,
    accelerator: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HotkeyAction {
    Mode1,
    Mode2,
    Mode3,
}

enum HotkeyCommand {
    Apply {
        bindings: Vec<HotkeyBinding>,
        reply: Sender<Result<(), String>>,
    },
}

type CommandResult<T> = Result<T, HotkeyCommandError>;

pub fn setup(app: &AppHandle) {
    let app_handle = app.clone();
    let (sender, receiver) = mpsc::channel();
    let (initial_bindings, initial_error) = match load_hotkey_settings(app) {
        Ok(Some(bindings)) => (bindings, None),
        Ok(None) => (default_hotkeys(), None),
        Err(message) => (default_hotkeys(), Some(message)),
    };

    if let Ok(mut controller) = app.state::<HotkeyStore>().controller.lock() {
        *controller = Some(sender);
    }

    if let Err(error) = thread::Builder::new()
        .name("global-hotkey-listener".to_string())
        .spawn(move || run_hotkey_thread(app_handle, receiver, initial_bindings, initial_error))
    {
        if let Ok(mut controller) = app.state::<HotkeyStore>().controller.lock() {
            *controller = None;
        }

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

#[tauri::command]
pub fn hotkeys_apply_settings(
    app: AppHandle,
    state: State<'_, HotkeyStore>,
    request: HotkeySettingsRequest,
) -> CommandResult<HotkeySnapshot> {
    let bindings =
        hotkey_bindings_from_request(&request).map_err(|message| HotkeyCommandError {
            code: "invalid_hotkeys",
            message,
        })?;
    let stored_settings = stored_settings_from_bindings(&bindings);

    let sender = state.controller()?;
    let (reply_sender, reply_receiver) = mpsc::channel();

    sender
        .send(HotkeyCommand::Apply {
            bindings,
            reply: reply_sender,
        })
        .map_err(|_| HotkeyCommandError {
            code: "hotkey_listener_unavailable",
            message: "Global hotkey listener is not available.".to_string(),
        })?;

    match reply_receiver.recv_timeout(HOTKEY_COMMAND_TIMEOUT) {
        Ok(Ok(())) => {
            save_hotkey_settings(&app, &stored_settings).map_err(|message| HotkeyCommandError {
                code: "hotkey_save_failed",
                message,
            })?;
            Ok(state.snapshot()?.clone())
        }
        Ok(Err(message)) => Err(HotkeyCommandError {
            code: "hotkey_apply_failed",
            message,
        }),
        Err(_) => Err(HotkeyCommandError {
            code: "hotkey_apply_timeout",
            message: "Timed out while applying shortcut settings.".to_string(),
        }),
    }
}

fn run_hotkey_thread(
    app: AppHandle,
    receiver: Receiver<HotkeyCommand>,
    initial_bindings: Vec<HotkeyBinding>,
    initial_error: Option<String>,
) {
    let mut registered = Vec::new();

    update_snapshot(&app, |snapshot| {
        snapshot.is_running = true;
        snapshot.last_error = None;
        snapshot.bindings.clear();
    });

    let initial_apply_result = apply_hotkey_bindings(&app, &mut registered, initial_bindings);
    if let Some(message) = initial_error {
        update_snapshot(&app, |snapshot| {
            snapshot.last_error = Some(message);
        });
    } else if let Err(message) = initial_apply_result {
        update_snapshot(&app, |snapshot| {
            snapshot.last_error = Some(message);
        });
    }

    let mut message = MSG::default();

    loop {
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
            if message.message == WM_HOTKEY {
                let id = message.wParam.0 as i32;

                if let Some(binding) = registered.iter().find(|binding| binding.id == id).cloned() {
                    dispatch_hotkey(app.clone(), binding.action);
                }
            }
        }

        while let Ok(command) = receiver.try_recv() {
            match command {
                HotkeyCommand::Apply { bindings, reply } => {
                    let result = apply_hotkey_bindings(&app, &mut registered, bindings);
                    let _ = reply.send(result);
                }
            }
        }

        thread::sleep(Duration::from_millis(50));
    }
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

fn apply_hotkey_bindings(
    app: &AppHandle,
    registered: &mut Vec<HotkeyBinding>,
    bindings: Vec<HotkeyBinding>,
) -> Result<(), String> {
    for binding in registered.drain(..) {
        let _ = unsafe { UnregisterHotKey(None, binding.id) };
    }

    update_snapshot(app, |snapshot| {
        snapshot.is_running = true;
        snapshot.last_error = None;
        snapshot.bindings.clear();
    });

    let mut errors = Vec::new();

    for binding in bindings {
        match unsafe { RegisterHotKey(None, binding.id, binding.modifiers, binding.vk) } {
            Ok(()) => {
                registered.push(binding.clone());
                push_binding_snapshot(app, binding.snapshot(true, None));
            }
            Err(error) => {
                let message = format!(
                    "{} could not be registered. The shortcut may already be in use: {error}",
                    binding.accelerator
                );
                errors.push(message.clone());
                push_binding_snapshot(app, binding.snapshot(false, Some(message)));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        let message = errors.join(" ");
        update_snapshot(app, |snapshot| {
            snapshot.last_error = Some(message.clone());
        });
        Err(message)
    }
}

fn default_hotkeys() -> Vec<HotkeyBinding> {
    vec![
        HotkeyBinding {
            id: HOTKEY_MODE_1_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0),
            vk: b'1' as u32,
            action: HotkeyAction::Mode1,
            accelerator: "Ctrl+Alt+1".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MODE_2_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0),
            vk: b'2' as u32,
            action: HotkeyAction::Mode2,
            accelerator: "Ctrl+Alt+2".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MODE_3_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0),
            vk: b'3' as u32,
            action: HotkeyAction::Mode3,
            accelerator: "Ctrl+Alt+3".to_string(),
        },
    ]
}

fn hotkey_bindings_from_request(
    request: &HotkeySettingsRequest,
) -> Result<Vec<HotkeyBinding>, String> {
    let mut bindings = Vec::new();

    if request.bindings.len() != 3 {
        return Err(
            "Shortcut settings must include exactly three automation shortcuts.".to_string(),
        );
    }

    for binding in &request.bindings {
        let action = HotkeyAction::from_str(&binding.action)
            .ok_or_else(|| format!("Unknown shortcut action `{}`.", binding.action))?;
        let (modifiers, vk, accelerator) = parse_accelerator(&binding.accelerator)?;
        let id = action.hotkey_id();

        if bindings
            .iter()
            .any(|existing: &HotkeyBinding| existing.action == action)
        {
            return Err(format!("Duplicate shortcut action `{}`.", binding.action));
        }

        if bindings
            .iter()
            .any(|existing| existing.modifiers.0 == modifiers.0 && existing.vk == vk)
        {
            return Err(format!("Duplicate shortcut `{accelerator}`."));
        }

        bindings.push(HotkeyBinding {
            id,
            modifiers,
            vk,
            action,
            accelerator,
        });
    }

    bindings.sort_by_key(|binding| binding.id);
    Ok(bindings)
}

fn load_hotkey_settings(app: &AppHandle) -> Result<Option<Vec<HotkeyBinding>>, String> {
    let path = hotkey_settings_path(app)?;

    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Failed to read saved shortcut settings from {}: {error}",
            path.display()
        )
    })?;
    let settings: StoredHotkeySettings = serde_json::from_str(&contents).map_err(|error| {
        format!(
            "Saved shortcut settings in {} are invalid: {error}",
            path.display()
        )
    })?;
    let request = HotkeySettingsRequest {
        bindings: settings
            .bindings
            .into_iter()
            .map(|binding| HotkeyBindingRequest {
                action: binding.action,
                accelerator: binding.accelerator,
            })
            .collect(),
    };

    hotkey_bindings_from_request(&request)
        .map(Some)
        .map_err(|message| format!("Saved shortcut settings are invalid: {message}"))
}

fn save_hotkey_settings(
    app: &AppHandle,
    settings: &StoredHotkeySettings,
) -> Result<(), String> {
    let path = hotkey_settings_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Shortcut settings path has no parent directory.".to_string())?;

    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Failed to create shortcut settings directory {}: {error}",
            parent.display()
        )
    })?;

    let contents = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("Failed to serialize shortcut settings: {error}"))?;

    fs::write(&path, contents).map_err(|error| {
        format!(
            "Failed to save shortcut settings to {}: {error}",
            path.display()
        )
    })
}

fn hotkey_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(HOTKEY_SETTINGS_FILE))
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))
}

fn stored_settings_from_bindings(bindings: &[HotkeyBinding]) -> StoredHotkeySettings {
    StoredHotkeySettings {
        bindings: bindings
            .iter()
            .map(|binding| StoredHotkeyBinding {
                action: binding.action.as_str().to_string(),
                accelerator: binding.accelerator.clone(),
            })
            .collect(),
    }
}

fn parse_accelerator(value: &str) -> Result<(HOT_KEY_MODIFIERS, u32, String), String> {
    let mut modifiers = 0;
    let mut key = None;
    let mut normalized_parts = Vec::new();

    for raw_part in value.split('+') {
        let part = raw_part.trim();

        if part.is_empty() {
            continue;
        }

        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                modifiers |= MOD_CONTROL.0;
                normalized_parts.push("Ctrl".to_string());
            }
            "alt" => {
                modifiers |= MOD_ALT.0;
                normalized_parts.push("Alt".to_string());
            }
            "shift" => {
                modifiers |= MOD_SHIFT.0;
                normalized_parts.push("Shift".to_string());
            }
            "win" | "windows" | "meta" => {
                modifiers |= MOD_WIN.0;
                normalized_parts.push("Win".to_string());
            }
            _ => {
                if key.is_some() {
                    return Err(format!("Shortcut `{value}` has more than one key."));
                }

                let (vk, normalized_key) = parse_virtual_key(part)
                    .ok_or_else(|| format!("Shortcut key `{part}` is not supported."))?;
                key = Some(vk);
                normalized_parts.push(normalized_key);
            }
        }
    }

    let Some(key) = key else {
        return Err(format!("Shortcut `{value}` needs a key."));
    };

    if modifiers == 0 {
        return Err(format!("Shortcut `{value}` needs at least one modifier."));
    }

    Ok((
        HOT_KEY_MODIFIERS(modifiers),
        key,
        normalized_parts.join("+"),
    ))
}

fn parse_virtual_key(value: &str) -> Option<(u32, String)> {
    let upper = value.trim().to_ascii_uppercase();

    if upper.len() == 1 {
        let byte = upper.as_bytes()[0];

        if byte.is_ascii_alphanumeric() {
            return Some((byte as u32, upper));
        }
    }

    if let Some(number) = upper
        .strip_prefix('F')
        .and_then(|value| value.parse::<u32>().ok())
    {
        if (1..=12).contains(&number) {
            return Some((0x70 + number - 1, format!("F{number}")));
        }
    }

    match upper.as_str() {
        "ENTER" => Some((0x0D, "Enter".to_string())),
        "SPACE" => Some((0x20, "Space".to_string())),
        "TAB" => Some((0x09, "Tab".to_string())),
        "ESC" | "ESCAPE" => Some((0x1B, "Esc".to_string())),
        _ => None,
    }
}

impl HotkeyBinding {
    fn snapshot(&self, registered: bool, error: Option<String>) -> HotkeyBindingSnapshot {
        HotkeyBindingSnapshot {
            action: match self.action {
                HotkeyAction::Mode1 => "shortcutMode1",
                HotkeyAction::Mode2 => "shortcutMode2",
                HotkeyAction::Mode3 => "shortcutMode3",
            },
            accelerator: self.accelerator.clone(),
            registered,
            error,
        }
    }
}

impl HotkeyAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Mode1 => "shortcutMode1",
            Self::Mode2 => "shortcutMode2",
            Self::Mode3 => "shortcutMode3",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "shortcutMode1" => Some(Self::Mode1),
            "shortcutMode2" => Some(Self::Mode2),
            "shortcutMode3" => Some(Self::Mode3),
            _ => None,
        }
    }

    fn hotkey_id(self) -> i32 {
        match self {
            Self::Mode1 => HOTKEY_MODE_1_ID,
            Self::Mode2 => HOTKEY_MODE_2_ID,
            Self::Mode3 => HOTKEY_MODE_3_ID,
        }
    }
}

trait HotkeyStoreExt {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, HotkeySnapshot>>;
    fn controller(&self) -> CommandResult<Sender<HotkeyCommand>>;
}

impl HotkeyStoreExt for HotkeyStore {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, HotkeySnapshot>> {
        self.snapshot.lock().map_err(|_| HotkeyCommandError {
            code: "state_unavailable",
            message: "Hotkey state is unavailable.".to_string(),
        })
    }

    fn controller(&self) -> CommandResult<Sender<HotkeyCommand>> {
        self.controller
            .lock()
            .map_err(|_| HotkeyCommandError {
                code: "state_unavailable",
                message: "Hotkey controller state is unavailable.".to_string(),
            })?
            .clone()
            .ok_or_else(|| HotkeyCommandError {
                code: "hotkey_listener_unavailable",
                message: "Global hotkey listener is not available.".to_string(),
            })
    }
}
