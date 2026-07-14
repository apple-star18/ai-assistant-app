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
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
        MOD_SHIFT, MOD_WIN,
    },
    WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE, WM_HOTKEY},
};

use crate::automation;

const MAIN_WINDOW_LABEL: &str = "main";
const HOTKEY_EVENT: &str = "hotkeys://state";
const HOTKEY_MODE_1_ID: i32 = 101;
const HOTKEY_MODE_2_ID: i32 = 102;
const HOTKEY_MODE_3_ID: i32 = 103;
const HOTKEY_MOVE_UP_ID: i32 = 104;
const HOTKEY_MOVE_DOWN_ID: i32 = 105;
const HOTKEY_MOVE_RIGHT_ID: i32 = 106;
const HOTKEY_MOVE_LEFT_ID: i32 = 107;
const HOTKEY_TOGGLE_WINDOW_ID: i32 = 108;
const WINDOW_MOVE_STEP: i32 = 50;
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
    MoveUp,
    MoveDown,
    MoveRight,
    MoveLeft,
    ToggleWindow,
}

#[derive(Clone, Copy)]
enum WindowMoveDirection {
    Up,
    Down,
    Right,
    Left,
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
    match action {
        HotkeyAction::Mode1 | HotkeyAction::Mode2 => {
            let permit = match automation::try_reserve_caption_workflow(&app) {
                Ok(permit) => permit,
                Err(message) => {
                    update_snapshot(&app, |snapshot| {
                        snapshot.last_error = Some(message);
                    });
                    return;
                }
            };

            thread::spawn(move || {
                let result = match action {
                    HotkeyAction::Mode1 => automation::run_mode_1_reserved(&app, permit),
                    HotkeyAction::Mode2 => automation::run_mode_2_reserved(&app, permit),
                    _ => unreachable!(),
                };
                report_hotkey_result(&app, result);
            });
        }
        HotkeyAction::Mode3 => {
            let permit = match automation::start_mode_3_job(&app) {
                Ok(permit) => permit,
                Err(message) => {
                    update_snapshot(&app, |snapshot| {
                        snapshot.last_error = Some(message);
                    });
                    return;
                }
            };

            thread::spawn(move || {
                let result = automation::run_mode_3_reserved(&app, permit);
                report_hotkey_result(&app, result);
            });
        }
        HotkeyAction::MoveUp => {
            report_hotkey_result(&app, move_main_window(&app, WindowMoveDirection::Up))
        }
        HotkeyAction::MoveDown => {
            report_hotkey_result(&app, move_main_window(&app, WindowMoveDirection::Down))
        }
        HotkeyAction::MoveRight => {
            report_hotkey_result(&app, move_main_window(&app, WindowMoveDirection::Right))
        }
        HotkeyAction::MoveLeft => {
            report_hotkey_result(&app, move_main_window(&app, WindowMoveDirection::Left))
        }
        HotkeyAction::ToggleWindow => report_hotkey_result(&app, toggle_main_window(&app)),
    }
}

fn move_main_window(app: &AppHandle, direction: WindowMoveDirection) -> Result<(), String> {
    let window = app
        .get_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is unavailable.".to_string())?;
    let position = window
        .outer_position()
        .map_err(|error| format!("Failed to read the main window position: {error}"))?;
    let size = window
        .outer_size()
        .map_err(|error| format!("Failed to read the main window size: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Failed to find the current monitor: {error}"))?
        .ok_or_else(|| "No monitor contains the main window.".to_string())?;
    let work_area = monitor.work_area();
    let (delta_x, delta_y) = match direction {
        WindowMoveDirection::Up => (0, -WINDOW_MOVE_STEP),
        WindowMoveDirection::Down => (0, WINDOW_MOVE_STEP),
        WindowMoveDirection::Right => (WINDOW_MOVE_STEP, 0),
        WindowMoveDirection::Left => (-WINDOW_MOVE_STEP, 0),
    };
    let next_x = clamped_window_axis(
        position.x,
        delta_x,
        work_area.position.x,
        work_area.size.width,
        size.width,
    );
    let next_y = clamped_window_axis(
        position.y,
        delta_y,
        work_area.position.y,
        work_area.size.height,
        size.height,
    );

    window
        .set_position(PhysicalPosition::new(next_x, next_y))
        .map_err(|error| format!("Failed to move the main window: {error}"))
}

fn clamped_window_axis(
    position: i32,
    delta: i32,
    work_origin: i32,
    work_size: u32,
    window_size: u32,
) -> i32 {
    let minimum = i64::from(work_origin);
    let maximum = (minimum + i64::from(work_size) - i64::from(window_size)).max(minimum);

    (i64::from(position) + i64::from(delta)).clamp(minimum, maximum) as i32
}

fn toggle_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is unavailable.".to_string())?;
    let is_visible = window
        .is_visible()
        .map_err(|error| format!("Failed to read main window visibility: {error}"))?;

    if is_visible {
        window
            .hide()
            .map_err(|error| format!("Failed to hide the main window: {error}"))
    } else {
        window
            .show()
            .map_err(|error| format!("Failed to show the main window: {error}"))?;
        window
            .unminimize()
            .map_err(|error| format!("Failed to restore the main window: {error}"))?;
        window
            .set_focus()
            .map_err(|error| format!("Failed to focus the main window: {error}"))
    }
}

fn report_hotkey_result(app: &AppHandle, result: Result<(), String>) {
    if let Err(message) = result {
        if automation::is_cancelled_error(&message) {
            return;
        }
        update_snapshot(app, |snapshot| {
            snapshot.last_error = Some(message);
        });
    }
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
    let previous_bindings = registered.clone();
    unregister_hotkey_bindings(registered);
    let (mut next_registered, attempted_snapshots, errors) = register_hotkey_bindings(&bindings);

    if errors.is_empty() {
        *registered = next_registered;
        update_snapshot(app, |snapshot| {
            snapshot.is_running = true;
            snapshot.bindings = attempted_snapshots;
            snapshot.last_error = None;
        });
        return Ok(());
    }

    let mut message = errors.join(" ");
    let snapshots = if previous_bindings.is_empty() {
        *registered = next_registered;
        attempted_snapshots
    } else {
        unregister_hotkey_bindings(&mut next_registered);
        let (restored, restored_snapshots, restore_errors) =
            register_hotkey_bindings(&previous_bindings);
        *registered = restored;

        if !restore_errors.is_empty() {
            message.push_str(" The previous shortcuts could not be fully restored: ");
            message.push_str(&restore_errors.join(" "));
        }

        restored_snapshots
    };

    update_snapshot(app, |snapshot| {
        snapshot.is_running = true;
        snapshot.bindings = snapshots;
        snapshot.last_error = Some(message.clone());
    });
    Err(message)
}

fn unregister_hotkey_bindings(bindings: &mut Vec<HotkeyBinding>) {
    for binding in bindings.drain(..) {
        let _ = unsafe { UnregisterHotKey(None, binding.id) };
    }
}

fn register_hotkey_bindings(
    bindings: &[HotkeyBinding],
) -> (Vec<HotkeyBinding>, Vec<HotkeyBindingSnapshot>, Vec<String>) {
    let mut registered = Vec::with_capacity(bindings.len());
    let mut snapshots = Vec::with_capacity(bindings.len());
    let mut errors = Vec::new();

    for binding in bindings {
        let modifiers = HOT_KEY_MODIFIERS(binding.modifiers.0 | MOD_NOREPEAT.0);

        match unsafe { RegisterHotKey(None, binding.id, modifiers, binding.vk) } {
            Ok(()) => {
                registered.push(binding.clone());
                snapshots.push(binding.snapshot(true, None));
            }
            Err(error) => {
                let message = format!(
                    "{} could not be registered. The shortcut may already be in use: {error}",
                    binding.accelerator
                );
                errors.push(message.clone());
                snapshots.push(binding.snapshot(false, Some(message)));
            }
        }
    }

    (registered, snapshots, errors)
}

fn default_hotkeys() -> Vec<HotkeyBinding> {
    vec![
        HotkeyBinding {
            id: HOTKEY_MODE_1_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0),
            vk: 0x0D,
            action: HotkeyAction::Mode1,
            accelerator: "Ctrl+Enter".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MODE_2_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: 0x0D,
            action: HotkeyAction::Mode2,
            accelerator: "Ctrl+Shift+Enter".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MODE_3_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: b'S' as u32,
            action: HotkeyAction::Mode3,
            accelerator: "Ctrl+Shift+S".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MOVE_UP_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0),
            vk: 0x26,
            action: HotkeyAction::MoveUp,
            accelerator: "Ctrl+Up".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MOVE_DOWN_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0),
            vk: 0x28,
            action: HotkeyAction::MoveDown,
            accelerator: "Ctrl+Down".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MOVE_RIGHT_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0),
            vk: 0x27,
            action: HotkeyAction::MoveRight,
            accelerator: "Ctrl+Right".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_MOVE_LEFT_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0),
            vk: 0x25,
            action: HotkeyAction::MoveLeft,
            accelerator: "Ctrl+Left".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_TOGGLE_WINDOW_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0),
            vk: 0xDC,
            action: HotkeyAction::ToggleWindow,
            accelerator: "Ctrl+\\".to_string(),
        },
    ]
}

fn hotkey_bindings_from_request(
    request: &HotkeySettingsRequest,
) -> Result<Vec<HotkeyBinding>, String> {
    let mut bindings = Vec::new();

    if request.bindings.len() != default_hotkeys().len() {
        return Err(format!(
            "Shortcut settings must include exactly {} shortcuts.",
            default_hotkeys().len()
        ));
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
    let request = merge_saved_hotkey_settings(settings)?;

    hotkey_bindings_from_request(&request)
        .map(Some)
        .map_err(|message| format!("Saved shortcut settings are invalid: {message}"))
}

fn merge_saved_hotkey_settings(
    settings: StoredHotkeySettings,
) -> Result<HotkeySettingsRequest, String> {
    let mut bindings: Vec<HotkeyBindingRequest> = default_hotkeys()
        .into_iter()
        .map(|binding| HotkeyBindingRequest {
            action: binding.action.as_str().to_string(),
            accelerator: binding.accelerator,
        })
        .collect();
    let mut seen_actions = Vec::new();

    for saved_binding in settings.bindings {
        let action = HotkeyAction::from_str(&saved_binding.action)
            .ok_or_else(|| format!("Unknown shortcut action `{}`.", saved_binding.action))?;

        if seen_actions.contains(&action) {
            return Err(format!(
                "Duplicate shortcut action `{}`.",
                saved_binding.action
            ));
        }
        seen_actions.push(action);

        let binding = bindings
            .iter_mut()
            .find(|binding| binding.action == action.as_str())
            .expect("every hotkey action has a default binding");
        binding.accelerator = saved_binding.accelerator;
    }

    Ok(HotkeySettingsRequest { bindings })
}

fn save_hotkey_settings(app: &AppHandle, settings: &StoredHotkeySettings) -> Result<(), String> {
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
        "UP" | "ARROWUP" => Some((0x26, "Up".to_string())),
        "DOWN" | "ARROWDOWN" => Some((0x28, "Down".to_string())),
        "RIGHT" | "ARROWRIGHT" => Some((0x27, "Right".to_string())),
        "LEFT" | "ARROWLEFT" => Some((0x25, "Left".to_string())),
        "\\" | "BACKSLASH" => Some((0xDC, "\\".to_string())),
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
                HotkeyAction::MoveUp => "shortcutMoveUp",
                HotkeyAction::MoveDown => "shortcutMoveDown",
                HotkeyAction::MoveRight => "shortcutMoveRight",
                HotkeyAction::MoveLeft => "shortcutMoveLeft",
                HotkeyAction::ToggleWindow => "shortcutToggleWindow",
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
            Self::MoveUp => "shortcutMoveUp",
            Self::MoveDown => "shortcutMoveDown",
            Self::MoveRight => "shortcutMoveRight",
            Self::MoveLeft => "shortcutMoveLeft",
            Self::ToggleWindow => "shortcutToggleWindow",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "shortcutMode1" => Some(Self::Mode1),
            "shortcutMode2" => Some(Self::Mode2),
            "shortcutMode3" => Some(Self::Mode3),
            "shortcutMoveUp" => Some(Self::MoveUp),
            "shortcutMoveDown" => Some(Self::MoveDown),
            "shortcutMoveRight" => Some(Self::MoveRight),
            "shortcutMoveLeft" => Some(Self::MoveLeft),
            "shortcutToggleWindow" => Some(Self::ToggleWindow),
            _ => None,
        }
    }

    fn hotkey_id(self) -> i32 {
        match self {
            Self::Mode1 => HOTKEY_MODE_1_ID,
            Self::Mode2 => HOTKEY_MODE_2_ID,
            Self::Mode3 => HOTKEY_MODE_3_ID,
            Self::MoveUp => HOTKEY_MOVE_UP_ID,
            Self::MoveDown => HOTKEY_MOVE_DOWN_ID,
            Self::MoveRight => HOTKEY_MOVE_RIGHT_ID,
            Self::MoveLeft => HOTKEY_MOVE_LEFT_ID,
            Self::ToggleWindow => HOTKEY_TOGGLE_WINDOW_ID,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_window_shortcut_keys() {
        assert_eq!(parse_accelerator("Ctrl+Up").unwrap().1, 0x26);
        assert_eq!(parse_accelerator("Ctrl+Down").unwrap().1, 0x28);
        assert_eq!(parse_accelerator("Ctrl+Right").unwrap().1, 0x27);
        assert_eq!(parse_accelerator("Ctrl+Left").unwrap().1, 0x25);
        assert_eq!(parse_accelerator("Ctrl+\\").unwrap().1, 0xDC);
    }

    #[test]
    fn adds_new_defaults_to_legacy_three_shortcut_settings() {
        let settings = StoredHotkeySettings {
            bindings: vec![
                StoredHotkeyBinding {
                    action: "shortcutMode1".to_string(),
                    accelerator: "Alt+1".to_string(),
                },
                StoredHotkeyBinding {
                    action: "shortcutMode2".to_string(),
                    accelerator: "Alt+2".to_string(),
                },
                StoredHotkeyBinding {
                    action: "shortcutMode3".to_string(),
                    accelerator: "Alt+3".to_string(),
                },
            ],
        };

        let request = merge_saved_hotkey_settings(settings).unwrap();
        let bindings = hotkey_bindings_from_request(&request).unwrap();

        assert_eq!(bindings.len(), 8);
        assert_eq!(bindings[0].accelerator, "Alt+1");
        assert_eq!(bindings[3].accelerator, "Ctrl+Up");
        assert_eq!(bindings[7].accelerator, "Ctrl+\\");
    }

    #[test]
    fn clamps_window_movement_to_monitor_work_area() {
        assert_eq!(clamped_window_axis(100, -50, 100, 1000, 400), 100);
        assert_eq!(clamped_window_axis(650, 50, 100, 1000, 400), 700);
        assert_eq!(clamped_window_axis(700, 50, 100, 1000, 400), 700);
        assert_eq!(clamped_window_axis(250, 50, 100, 200, 400), 100);
    }
}
