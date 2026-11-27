use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            GetKeyState, GetKeyboardLayout, ToUnicodeEx, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
            MOD_SHIFT, MOD_WIN, VK_CAPITAL,
        },
        WindowsAndMessaging::{
            CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, PeekMessageW,
            SetWindowsHookExW, ShowWindow, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, PM_REMOVE,
            SW_SHOWNOACTIVATE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use crate::{automation, browser};

const MAIN_WINDOW_LABEL: &str = "main";
const HOTKEY_EVENT: &str = "hotkeys://state";
const HOTKEY_MODE_1_ID: i32 = 101;
const HOTKEY_MODE_2_ID: i32 = 102;
const HOTKEY_MODE_3_ID: i32 = 103;
const HOTKEY_SUBMIT_CAPTURE_ID: i32 = 109;
const HOTKEY_NEW_CHAT_ID: i32 = 110;
const HOTKEY_REFRESH_ID: i32 = 111;
const HOTKEY_BACKWARD_ID: i32 = 112;
const HOTKEY_MOVE_UP_ID: i32 = 104;
const HOTKEY_MOVE_DOWN_ID: i32 = 105;
const HOTKEY_MOVE_RIGHT_ID: i32 = 106;
const HOTKEY_MOVE_LEFT_ID: i32 = 107;
const HOTKEY_TOGGLE_WINDOW_ID: i32 = 108;
const HOTKEY_MOUSE_THROUGH_ID: i32 = 113;
const HOTKEY_TYPE_MESSAGE_ID: i32 = 114;
const WINDOW_MOVE_STEP: i32 = 50;
const HOTKEY_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const HOTKEY_SETTINGS_FILE: &str = "hotkeys.json";

thread_local! {
    static KEYBOARD_HOOK_STATE: RefCell<Option<KeyboardHookState>> = const { RefCell::new(None) };
}

struct KeyboardHookState {
    bindings: Vec<HotkeyBinding>,
    actions: Sender<HotkeyAction>,
    typing: Sender<TypingEvent>,
    typing_active: Arc<AtomicBool>,
    typing_foreground: HWND,
    capture_passthrough_modifiers: Vec<(u32, u32, bool)>,
    held_modifiers: Vec<(u32, u32, bool)>,
    swallowed_keys: Vec<(u32, u32, bool)>,
}

#[derive(Debug, PartialEq)]
enum TypingEvent {
    Begin,
    Character(String),
    Backspace,
    Submit,
    Cancel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeySnapshot {
    is_running: bool,
    is_typing: bool,
    bindings: Vec<HotkeyBindingSnapshot>,
    last_error: Option<String>,
}

impl Default for HotkeySnapshot {
    fn default() -> Self {
        Self {
            is_running: false,
            is_typing: false,
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
    action_generation: AtomicU64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HotkeyAction {
    Mode1,
    Mode2,
    Mode3,
    SubmitCapture,
    NewChat,
    Refresh,
    Backward,
    MoveUp,
    MoveDown,
    MoveRight,
    MoveLeft,
    ToggleWindow,
    MouseThrough,
    TypeMessage,
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

pub(crate) fn mouse_through_recovery_available(app: &AppHandle) -> bool {
    app.state::<HotkeyStore>()
        .snapshot
        .lock()
        .is_ok_and(|snapshot| {
            snapshot.is_running
                && snapshot.bindings.iter().any(|binding| {
                    binding.action == HotkeyAction::MouseThrough.as_str() && binding.registered
                })
        })
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
    let (action_sender, action_receiver) = mpsc::channel();
    let (typing_sender, typing_receiver) = mpsc::channel();
    let typing_active = Arc::new(AtomicBool::new(false));
    let typing_app = app.clone();
    let typing_flag = typing_active.clone();
    if let Err(error) = thread::Builder::new()
        .name("background-chat-typing".to_string())
        .spawn(move || run_typing_thread(typing_app, typing_receiver, typing_flag))
    {
        update_snapshot(&app, |snapshot| {
            snapshot.last_error = Some(format!("Failed to start background typing: {error}"));
        });
        return;
    }
    let action_app = app.clone();
    if let Err(error) = thread::Builder::new()
        .name("global-hotkey-actions".to_string())
        .spawn(move || {
            while let Ok(action) = action_receiver.recv() {
                dispatch_hotkey(action_app.clone(), action);
            }
        })
    {
        update_snapshot(&app, |snapshot| {
            snapshot.is_running = false;
            snapshot.last_error = Some(format!("Failed to start hotkey actions: {error}"));
        });
        return;
    }
    KEYBOARD_HOOK_STATE.with(|state| {
        *state.borrow_mut() = Some(KeyboardHookState {
            bindings: Vec::new(),
            actions: action_sender,
            typing: typing_sender,
            typing_active,
            typing_foreground: HWND::default(),
            capture_passthrough_modifiers: Vec::new(),
            held_modifiers: Vec::new(),
            swallowed_keys: Vec::new(),
        });
    });

    let _hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) } {
        Ok(hook) => hook,
        Err(error) => {
            update_snapshot(&app, |snapshot| {
                snapshot.is_running = false;
                snapshot.last_error = Some(format!("Failed to install keyboard hook: {error}"));
            });
            return;
        }
    };
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
            // The message pump also services WH_KEYBOARD_LL callbacks on this thread.
        }

        while let Ok(command) = receiver.try_recv() {
            match command {
                HotkeyCommand::Apply { bindings, reply } => {
                    let result = apply_hotkey_bindings(&app, &mut registered, bindings);
                    let _ = reply.send(result);
                }
            }
        }

        thread::sleep(Duration::from_millis(10));
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let is_down = match wparam.0 as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return unsafe { CallNextHookEx(None, code, wparam, lparam) },
    };
    let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let consumed = KEYBOARD_HOOK_STATE.with(|state| {
        state
            .borrow_mut()
            .as_mut()
            .is_some_and(|state| state.handle_key(key, is_down))
    });
    if consumed {
        LRESULT(1)
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

impl KeyboardHookState {
    fn handle_key(&mut self, key: &KBDLLHOOKSTRUCT, is_down: bool) -> bool {
        let vk = key.vkCode;
        let physical_key = (vk, key.scanCode, key.flags.contains(LLKHF_EXTENDED));
        if self.typing_active.load(Ordering::Acquire) {
            let foreground = unsafe { GetForegroundWindow() };
            if self.typing_foreground.0.is_null() == false
                && foreground.0 != self.typing_foreground.0
            {
                self.typing_active.store(false, Ordering::Release);
                let _ = self.typing.send(TypingEvent::Cancel);
                if self.swallowed_keys.contains(&physical_key) {
                    if !is_down {
                        self.swallowed_keys.retain(|held| *held != physical_key);
                    }
                    return true;
                }
                return false;
            }
            if modifier_for_key(vk).is_some() {
                if is_down {
                    if !self.held_modifiers.contains(&physical_key) {
                        self.held_modifiers.push(physical_key);
                    }
                } else {
                    self.held_modifiers.retain(|held| *held != physical_key);
                }
                let swallowed = self.swallowed_keys.contains(&physical_key);
                let passthrough =
                    self.capture_passthrough_modifiers.contains(&physical_key) && !swallowed;
                if is_down && !passthrough && !swallowed {
                    self.swallowed_keys.push(physical_key);
                }
                if !is_down {
                    self.capture_passthrough_modifiers
                        .retain(|held| *held != physical_key);
                    self.swallowed_keys.retain(|held| *held != physical_key);
                }
                return !passthrough;
            }
            let already_swallowed = self.swallowed_keys.contains(&physical_key);
            if !is_down {
                if already_swallowed {
                    self.swallowed_keys.retain(|held| *held != physical_key);
                }
                return true;
            }
            if !already_swallowed {
                self.swallowed_keys.push(physical_key);
            }
            let modifiers = self.held_modifiers.iter().fold(0, |mask, (vk, _, _)| {
                mask | modifier_for_key(*vk).unwrap_or(0)
            });
            if !already_swallowed
                && self.bindings.iter().any(|binding| {
                    binding.action == HotkeyAction::TypeMessage
                        && binding.vk == vk
                        && binding.modifiers.0 == modifiers
                })
            {
                self.typing_active.store(false, Ordering::Release);
                let _ = self.typing.send(TypingEvent::Cancel);
            } else if vk == 0x1B && !already_swallowed {
                self.typing_active.store(false, Ordering::Release);
                let _ = self.typing.send(TypingEvent::Cancel);
            } else if vk == 0x0D && modifiers == 0 && !already_swallowed {
                self.typing_active.store(false, Ordering::Release);
                let _ = self.typing.send(TypingEvent::Submit);
            } else if vk == 0x08 && modifiers == 0 {
                let _ = self.typing.send(TypingEvent::Backspace);
            } else if modifiers & (MOD_CONTROL.0 | MOD_ALT.0 | MOD_WIN.0) == 0 {
                if let Some(text) = key_to_text(key, modifiers & MOD_SHIFT.0 != 0) {
                    let _ = self.typing.send(TypingEvent::Character(text));
                }
            }
            return true;
        }
        if let Some(modifier) = modifier_for_key(vk) {
            let swallowed = self.swallowed_keys.contains(&physical_key);
            let opposite_held = self.held_modifiers.iter().any(|(held_vk, _, _)| {
                matches!(
                    (modifier, modifier_for_key(*held_vk)),
                    (mask, Some(other)) if (mask == MOD_CONTROL.0 && other == MOD_SHIFT.0)
                        || (mask == MOD_SHIFT.0 && other == MOD_CONTROL.0)
                )
            });
            if is_down {
                if !self.held_modifiers.contains(&physical_key) {
                    self.held_modifiers.push(physical_key);
                }
            } else {
                self.held_modifiers.retain(|held| *held != physical_key);
            }
            if swallowed {
                if !is_down {
                    self.swallowed_keys.retain(|held| *held != physical_key);
                }
                return true;
            }
            if is_down && opposite_held {
                self.swallowed_keys.push(physical_key);
                return true;
            }
            return false;
        }

        if self.swallowed_keys.contains(&physical_key) {
            if !is_down {
                self.swallowed_keys.retain(|held| *held != physical_key);
            }
            return true;
        }

        if !is_down {
            return false;
        }

        let modifiers = self.held_modifiers.iter().fold(0, |mask, (vk, _, _)| {
            mask | modifier_for_key(*vk).unwrap_or(0)
        });
        if let Some(binding) = self
            .bindings
            .iter()
            .find(|binding| binding.vk == vk && binding.modifiers.0 == modifiers)
        {
            self.swallowed_keys.push(physical_key);
            if binding.action == HotkeyAction::TypeMessage {
                self.capture_passthrough_modifiers = self.held_modifiers.clone();
                self.typing_foreground = unsafe { GetForegroundWindow() };
                self.typing_active.store(true, Ordering::Release);
                let _ = self.typing.send(TypingEvent::Begin);
            } else {
                let _ = self.actions.send(binding.action);
            }
            return true;
        }

        if self
            .swallowed_keys
            .iter()
            .any(|(held_vk, _, _)| matches!(modifier_for_key(*held_vk), Some(mask) if mask == MOD_CONTROL.0 || mask == MOD_SHIFT.0))
        {
            // Do not turn Ctrl+Shift+Tab into Ctrl+Tab in the focused window.
            self.swallowed_keys.push(physical_key);
            return true;
        }

        false
    }
}

fn key_to_text(key: &KBDLLHOOKSTRUCT, shift: bool) -> Option<String> {
    let mut keyboard = [0u8; 256];
    if shift {
        keyboard[0x10] = 0x80;
    }
    if unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0 {
        keyboard[VK_CAPITAL.0 as usize] = 1;
    }
    let foreground = unsafe { GetForegroundWindow() };
    let thread_id = unsafe { GetWindowThreadProcessId(foreground, None) };
    let layout = unsafe { GetKeyboardLayout(thread_id) };
    let mut buffer = [0u16; 8];
    // Bit 2 keeps dead-key state unchanged in the layout translator.
    let count = unsafe {
        ToUnicodeEx(
            key.vkCode,
            key.scanCode,
            &keyboard,
            &mut buffer,
            4,
            Some(layout),
        )
    };
    if count <= 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..count as usize]);
    (text.is_ascii() && text.chars().all(|character| !character.is_ascii_control())).then_some(text)
}

fn run_typing_thread(app: AppHandle, receiver: Receiver<TypingEvent>, active: Arc<AtomicBool>) {
    let mut draft: Option<String> = None;
    let mut original_draft: Option<String> = None;
    while let Ok(event) = receiver.recv() {
        let result = match event {
            TypingEvent::Begin => browser::read_chatgpt_prompt_text_untrimmed(&app).map(|text| {
                original_draft = Some(text.clone());
                draft = Some(text);
                update_snapshot(&app, |snapshot| {
                    snapshot.is_typing = true;
                    snapshot.last_error = None;
                });
            }),
            TypingEvent::Character(text) => {
                if let Some(draft) = draft.as_mut() {
                    draft.push_str(&text);
                    browser::replace_chatgpt_prompt_without_focus(&app, draft)
                } else {
                    Ok(())
                }
            }
            TypingEvent::Backspace => {
                if let Some(draft) = draft.as_mut() {
                    draft.pop();
                    browser::replace_chatgpt_prompt_without_focus(&app, draft)
                } else {
                    Ok(())
                }
            }
            TypingEvent::Submit => {
                update_snapshot(&app, |snapshot| snapshot.is_typing = false);
                let result = if draft.as_ref().is_some_and(|value| !value.trim().is_empty()) {
                    browser::wait_and_submit_chatgpt_input_cancellable(
                        &app,
                        Duration::from_secs(10),
                        || false,
                    )
                } else {
                    Ok(())
                };
                draft = None;
                original_draft = None;
                result
            }
            TypingEvent::Cancel => {
                draft = None;
                update_snapshot(&app, |snapshot| snapshot.is_typing = false);
                let result = original_draft.take().map_or(Ok(()), |text| {
                    browser::replace_chatgpt_prompt_without_focus(&app, &text)
                });
                result
            }
        };
        if let Err(message) = result {
            active.store(false, Ordering::Release);
            draft = None;
            original_draft = None;
            update_snapshot(&app, |snapshot| {
                snapshot.is_typing = false;
                snapshot.last_error = Some(message);
            });
        }
    }
}

fn modifier_for_key(vk: u32) -> Option<u32> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some(MOD_SHIFT.0),
        0x11 | 0xA2 | 0xA3 => Some(MOD_CONTROL.0),
        0x12 | 0xA4 | 0xA5 => Some(MOD_ALT.0),
        0x5B | 0x5C => Some(MOD_WIN.0),
        _ => None,
    }
}
fn dispatch_hotkey(app: AppHandle, action: HotkeyAction) {
    let generation = begin_hotkey_action(&app);
    if matches!(
        action,
        HotkeyAction::Mode1
            | HotkeyAction::Mode2
            | HotkeyAction::Mode3
            | HotkeyAction::SubmitCapture
    ) && browser::new_chat_is_busy(&app)
    {
        report_hotkey_result(
            &app,
            generation,
            Err("A new chat is opening. Try the shortcut again when it is ready.".to_string()),
        );
        return;
    }
    match action {
        HotkeyAction::NewChat => {
            if let Err(message) = browser::reserve_new_chat(&app) {
                report_hotkey_result(&app, generation, Err(message));
                return;
            }
            thread::spawn(move || {
                report_hotkey_result(
                    &app,
                    generation,
                    browser::open_new_chat_with_instruction(&app),
                );
            });
        }
        HotkeyAction::Refresh => {
            thread::spawn(move || {
                report_hotkey_result(&app, generation, browser::reload_for_hotkey(&app));
            });
        }
        HotkeyAction::Backward => {
            report_hotkey_result(&app, generation, browser::go_back_for_hotkey(&app));
        }
        HotkeyAction::Mode1 | HotkeyAction::Mode2 => {
            let permit = match automation::try_reserve_caption_workflow(&app) {
                Ok(permit) => permit,
                Err(message) => {
                    report_hotkey_result(&app, generation, Err(message));
                    return;
                }
            };

            thread::spawn(move || {
                let result = match action {
                    HotkeyAction::Mode1 => automation::run_mode_1_reserved(&app, permit),
                    HotkeyAction::Mode2 => automation::run_mode_2_reserved(&app, permit),
                    _ => unreachable!(),
                };
                report_hotkey_result(&app, generation, result);
            });
        }
        HotkeyAction::Mode3 => {
            let permit = match automation::start_mode_3_job(&app) {
                Ok(permit) => permit,
                Err(message) => {
                    report_hotkey_result(&app, generation, Err(message));
                    return;
                }
            };

            thread::spawn(move || {
                let result = automation::run_mode_3_reserved(&app, permit);
                report_hotkey_result(&app, generation, result);
            });
        }
        HotkeyAction::SubmitCapture => {
            thread::spawn(move || {
                report_hotkey_result(&app, generation, automation::submit_after_upload(&app));
            });
        }
        HotkeyAction::MoveUp => report_hotkey_result(
            &app,
            generation,
            move_main_window(&app, WindowMoveDirection::Up),
        ),
        HotkeyAction::MoveDown => report_hotkey_result(
            &app,
            generation,
            move_main_window(&app, WindowMoveDirection::Down),
        ),
        HotkeyAction::MoveRight => report_hotkey_result(
            &app,
            generation,
            move_main_window(&app, WindowMoveDirection::Right),
        ),
        HotkeyAction::MoveLeft => report_hotkey_result(
            &app,
            generation,
            move_main_window(&app, WindowMoveDirection::Left),
        ),
        HotkeyAction::ToggleWindow => {
            report_hotkey_result(&app, generation, toggle_main_window(&app))
        }
        HotkeyAction::MouseThrough => {
            report_hotkey_result(&app, generation, browser::toggle_mouse_through(&app))
        }
        HotkeyAction::TypeMessage => {}
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
        let hwnd = window
            .hwnd()
            .map_err(|error| format!("Failed to get the main window handle: {error}"))?;
        // Restore and show without taking keyboard focus from the current app.
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        Ok(())
    }
}

fn begin_hotkey_action(app: &AppHandle) -> u64 {
    let generation = app
        .state::<HotkeyStore>()
        .action_generation
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    update_snapshot(app, |snapshot| snapshot.last_error = None);
    automation::clear_last_error(app);
    generation
}

fn report_hotkey_result(app: &AppHandle, generation: u64, result: Result<(), String>) {
    if result
        .as_ref()
        .err()
        .is_some_and(|message| automation::is_cancelled_error(message))
    {
        return;
    }
    let store = app.state::<HotkeyStore>();
    if store.action_generation.load(Ordering::Acquire) != generation {
        return;
    }
    update_snapshot(app, |snapshot| {
        apply_hotkey_result(
            snapshot,
            store.action_generation.load(Ordering::Acquire),
            generation,
            result.err(),
        );
    });
}

fn apply_hotkey_result(
    snapshot: &mut HotkeySnapshot,
    current_generation: u64,
    result_generation: u64,
    error: Option<String>,
) {
    if current_generation == result_generation {
        snapshot.last_error = error;
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
    KEYBOARD_HOOK_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut().ok_or("Keyboard hook is unavailable.")?;
        state.bindings = bindings.clone();
        Ok::<(), &str>(())
    })?;
    *registered = bindings;
    update_snapshot(app, |snapshot| {
        snapshot.is_running = true;
        snapshot.bindings = registered
            .iter()
            .map(|binding| binding.snapshot(true, None))
            .collect();
        snapshot.last_error = None;
    });
    Ok(())
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
            id: HOTKEY_SUBMIT_CAPTURE_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_SHIFT.0),
            vk: 0x0D,
            action: HotkeyAction::SubmitCapture,
            accelerator: "Shift+Enter".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_NEW_CHAT_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: b'N' as u32,
            action: HotkeyAction::NewChat,
            accelerator: "Ctrl+Shift+N".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_REFRESH_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: b'R' as u32,
            action: HotkeyAction::Refresh,
            accelerator: "Ctrl+Shift+R".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_BACKWARD_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: b'B' as u32,
            action: HotkeyAction::Backward,
            accelerator: "Ctrl+Shift+B".to_string(),
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
        HotkeyBinding {
            id: HOTKEY_MOUSE_THROUGH_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: b'M' as u32,
            action: HotkeyAction::MouseThrough,
            accelerator: "Ctrl+Shift+M".to_string(),
        },
        HotkeyBinding {
            id: HOTKEY_TYPE_MESSAGE_ID,
            modifiers: HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0),
            vk: b'T' as u32,
            action: HotkeyAction::TypeMessage,
            accelerator: "Ctrl+Shift+T".to_string(),
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
                HotkeyAction::SubmitCapture => "shortcutSubmitCapture",
                HotkeyAction::NewChat => "shortcutNewChat",
                HotkeyAction::Refresh => "shortcutRefresh",
                HotkeyAction::Backward => "shortcutBackward",
                HotkeyAction::MoveUp => "shortcutMoveUp",
                HotkeyAction::MoveDown => "shortcutMoveDown",
                HotkeyAction::MoveRight => "shortcutMoveRight",
                HotkeyAction::MoveLeft => "shortcutMoveLeft",
                HotkeyAction::ToggleWindow => "shortcutToggleWindow",
                HotkeyAction::MouseThrough => "shortcutMouseThrough",
                HotkeyAction::TypeMessage => "shortcutTypeMessage",
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
            Self::SubmitCapture => "shortcutSubmitCapture",
            Self::NewChat => "shortcutNewChat",
            Self::Refresh => "shortcutRefresh",
            Self::Backward => "shortcutBackward",
            Self::MoveUp => "shortcutMoveUp",
            Self::MoveDown => "shortcutMoveDown",
            Self::MoveRight => "shortcutMoveRight",
            Self::MoveLeft => "shortcutMoveLeft",
            Self::ToggleWindow => "shortcutToggleWindow",
            Self::MouseThrough => "shortcutMouseThrough",
            Self::TypeMessage => "shortcutTypeMessage",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "shortcutMode1" => Some(Self::Mode1),
            "shortcutMode2" => Some(Self::Mode2),
            "shortcutMode3" => Some(Self::Mode3),
            "shortcutSubmitCapture" => Some(Self::SubmitCapture),
            "shortcutNewChat" => Some(Self::NewChat),
            "shortcutRefresh" => Some(Self::Refresh),
            "shortcutBackward" => Some(Self::Backward),
            "shortcutMoveUp" => Some(Self::MoveUp),
            "shortcutMoveDown" => Some(Self::MoveDown),
            "shortcutMoveRight" => Some(Self::MoveRight),
            "shortcutMoveLeft" => Some(Self::MoveLeft),
            "shortcutToggleWindow" => Some(Self::ToggleWindow),
            "shortcutMouseThrough" => Some(Self::MouseThrough),
            "shortcutTypeMessage" => Some(Self::TypeMessage),
            _ => None,
        }
    }

    fn hotkey_id(self) -> i32 {
        match self {
            Self::Mode1 => HOTKEY_MODE_1_ID,
            Self::Mode2 => HOTKEY_MODE_2_ID,
            Self::Mode3 => HOTKEY_MODE_3_ID,
            Self::SubmitCapture => HOTKEY_SUBMIT_CAPTURE_ID,
            Self::NewChat => HOTKEY_NEW_CHAT_ID,
            Self::Refresh => HOTKEY_REFRESH_ID,
            Self::Backward => HOTKEY_BACKWARD_ID,
            Self::MoveUp => HOTKEY_MOVE_UP_ID,
            Self::MoveDown => HOTKEY_MOVE_DOWN_ID,
            Self::MoveRight => HOTKEY_MOVE_RIGHT_ID,
            Self::MoveLeft => HOTKEY_MOVE_LEFT_ID,
            Self::ToggleWindow => HOTKEY_TOGGLE_WINDOW_ID,
            Self::MouseThrough => HOTKEY_MOUSE_THROUGH_ID,
            Self::TypeMessage => HOTKEY_TYPE_MESSAGE_ID,
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
    fn latest_shortcut_result_replaces_old_error_without_late_result_overwrite() {
        let mut snapshot = HotkeySnapshot::default();
        snapshot.last_error = Some("previous failure".to_string());

        apply_hotkey_result(&mut snapshot, 2, 1, Some("late failure".to_string()));
        assert_eq!(snapshot.last_error.as_deref(), Some("previous failure"));

        apply_hotkey_result(&mut snapshot, 2, 2, None);
        assert_eq!(snapshot.last_error, None);

        apply_hotkey_result(&mut snapshot, 3, 2, Some("late failure".to_string()));
        assert_eq!(snapshot.last_error, None);

        apply_hotkey_result(&mut snapshot, 3, 3, Some("current failure".to_string()));
        assert_eq!(snapshot.last_error.as_deref(), Some("current failure"));
    }

    fn hook_state(bindings: Vec<HotkeyBinding>) -> (KeyboardHookState, Receiver<HotkeyAction>) {
        let (actions, received) = mpsc::channel();
        let (typing, _typing_received) = mpsc::channel();
        (
            KeyboardHookState {
                bindings,
                actions,
                typing,
                typing_active: Arc::new(AtomicBool::new(false)),
                typing_foreground: HWND::default(),
                capture_passthrough_modifiers: Vec::new(),
                held_modifiers: Vec::new(),
                swallowed_keys: Vec::new(),
            },
            received,
        )
    }

    fn physical_key(vk: u32) -> KBDLLHOOKSTRUCT {
        KBDLLHOOKSTRUCT {
            vkCode: vk,
            ..Default::default()
        }
    }

    fn delivered_keys(state: &mut KeyboardHookState, events: &[(u32, bool)]) -> Vec<(u32, bool)> {
        events
            .iter()
            .copied()
            .filter(|(vk, down)| !state.handle_key(&physical_key(*vk), *down))
            .collect()
    }

    #[test]
    fn every_default_hotkey_hides_its_final_key_and_control_shift_pair() {
        for binding in default_hotkeys()
            .into_iter()
            .filter(|binding| binding.action != HotkeyAction::TypeMessage)
        {
            let (mut state, received) = hook_state(vec![binding.clone()]);
            let mut modifiers = Vec::new();
            if binding.modifiers.0 & MOD_CONTROL.0 != 0 {
                modifiers.push(0xA2);
            }
            if binding.modifiers.0 & MOD_SHIFT.0 != 0 {
                modifiers.push(0xA0);
            }
            let mut events: Vec<_> = modifiers.iter().map(|vk| (*vk, true)).collect();
            events.extend([(binding.vk, true), (binding.vk, false)]);
            events.extend(modifiers.iter().rev().map(|vk| (*vk, false)));
            let visible_modifiers = if modifiers.len() > 1 {
                &modifiers[..1]
            } else {
                &modifiers[..]
            };
            let expected: Vec<_> = visible_modifiers
                .iter()
                .map(|vk| (*vk, true))
                .chain(visible_modifiers.iter().rev().map(|vk| (*vk, false)))
                .collect();

            assert_eq!(
                delivered_keys(&mut state, &events),
                expected,
                "{} leaked its final key",
                binding.accelerator
            );
            assert_eq!(received.try_recv().unwrap(), binding.action);
            assert!(received.try_recv().is_err());
        }
    }

    #[test]
    fn typing_mode_consumes_letters_and_enter_without_retargeting_the_foreground_window() {
        let (mut state, received) = hook_state(default_hotkeys());
        let events = [
            (0xA2, true),
            (0xA0, true),
            (b'T' as u32, true),
            (b'T' as u32, false),
            (0xA0, false),
            (0xA2, false),
            (b'H' as u32, true),
            (b'H' as u32, false),
            (0x0D, true),
            (0x0D, false),
            (b'A' as u32, true),
            (b'A' as u32, false),
        ];
        assert_eq!(
            delivered_keys(&mut state, &events),
            vec![events[0], events[5], events[10], events[11]]
        );
        assert!(!state.typing_active.load(Ordering::Acquire));
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn typing_mode_queues_repeated_backspace_and_stops_on_escape() {
        let (actions, _received_actions) = mpsc::channel();
        let (typing, received_typing) = mpsc::channel();
        let mut state = KeyboardHookState {
            bindings: default_hotkeys(),
            actions,
            typing,
            typing_active: Arc::new(AtomicBool::new(true)),
            typing_foreground: HWND::default(),
            capture_passthrough_modifiers: Vec::new(),
            held_modifiers: Vec::new(),
            swallowed_keys: Vec::new(),
        };
        for (vk, down) in [
            (0x08, true),
            (0x08, true),
            (0x08, false),
            (0x1B, true),
            (0x1B, false),
        ] {
            assert!(state.handle_key(&physical_key(vk), down));
        }
        assert_eq!(received_typing.try_recv().unwrap(), TypingEvent::Backspace);
        assert_eq!(received_typing.try_recv().unwrap(), TypingEvent::Backspace);
        assert_eq!(received_typing.try_recv().unwrap(), TypingEvent::Cancel);
        assert!(received_typing.try_recv().is_err());
        assert!(!state.typing_active.load(Ordering::Acquire));
    }

    #[test]
    fn control_shift_pair_is_hidden_in_either_press_order() {
        for (first, second) in [(0xA2, 0xA0), (0xA0, 0xA2)] {
            let (mut state, received) = hook_state(default_hotkeys());
            let events = [
                (first, true),
                (second, true),
                (second, false),
                (first, false),
            ];
            assert_eq!(
                delivered_keys(&mut state, &events),
                vec![(first, true), (first, false)]
            );
            assert!(received.try_recv().is_err());
        }
    }

    #[test]
    fn hidden_control_shift_does_not_turn_tab_into_another_shortcut() {
        let (mut state, received) = hook_state(default_hotkeys());
        let events = [
            (0xA2, true),
            (0xA0, true),
            (0x09, true),
            (0x09, false),
            (0xA2, false),
            (b'A' as u32, true),
            (b'A' as u32, false),
            (0xA0, false),
            (b'B' as u32, true),
            (b'B' as u32, false),
        ];
        assert_eq!(
            delivered_keys(&mut state, &events),
            vec![events[0], events[4], events[8], events[9]]
        );
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn ordinary_key_sequences_pass_through_unchanged() {
        let (mut state, received) = hook_state(default_hotkeys());
        let cases: &[&[(u32, bool)]] = &[
            &[(b'A' as u32, true), (b'A' as u32, false)],
            &[
                (0xA2, true),
                (b'C' as u32, true),
                (b'C' as u32, false),
                (0xA2, false),
            ],
            &[
                (0xA0, true),
                (b'A' as u32, true),
                (b'A' as u32, false),
                (0xA0, false),
            ],
            &[
                (0xA4, true),
                (0xA0, true),
                (b'Z' as u32, true),
                (b'Z' as u32, false),
                (0xA0, false),
                (0xA4, false),
            ],
            &[(0xA2, true), (0xA2, false)],
            &[(0x12, true), (0x09, true), (0x09, false), (0x12, false)],
        ];
        for events in cases {
            assert_eq!(delivered_keys(&mut state, events), *events);
        }
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn shift_enter_sends_capture_without_consuming_plain_enter_or_other_shift_keys() {
        let (mut state, received) = hook_state(default_hotkeys());
        let events = [
            (0x0D, true),
            (0x0D, false),
            (0xA0, true),
            (b'A' as u32, true),
            (b'A' as u32, false),
            (0x0D, true),
            (0x0D, false),
            (0xA0, false),
        ];
        assert_eq!(
            delivered_keys(&mut state, &events),
            vec![events[0], events[1], events[2], events[3], events[4], events[7]]
        );
        assert_eq!(received.try_recv().unwrap(), HotkeyAction::SubmitCapture);
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn send_capture_shortcut_can_be_changed_in_settings() {
        let mut request = HotkeySettingsRequest {
            bindings: default_hotkeys()
                .into_iter()
                .map(|binding| HotkeyBindingRequest {
                    action: binding.action.as_str().to_string(),
                    accelerator: binding.accelerator,
                })
                .collect(),
        };
        request
            .bindings
            .iter_mut()
            .find(|binding| binding.action == "shortcutSubmitCapture")
            .unwrap()
            .accelerator = "Alt+F8".to_string();
        let bindings = hotkey_bindings_from_request(&request).unwrap();
        let (mut state, received) = hook_state(bindings);
        let events = [(0xA4, true), (0x77, true), (0x77, false), (0xA4, false)];
        assert_eq!(
            delivered_keys(&mut state, &events),
            vec![events[0], events[3]]
        );
        assert_eq!(received.try_recv().unwrap(), HotkeyAction::SubmitCapture);
    }

    #[test]
    fn ordinary_input_before_and_after_a_hotkey_is_not_replayed() {
        let (mut state, received) = hook_state(default_hotkeys());
        let events = [
            (0xA2, true),
            (b'C' as u32, true),
            (b'C' as u32, false),
            (0x0D, true),
            (0x0D, false),
            (b'V' as u32, true),
            (b'V' as u32, false),
            (0xA2, false),
        ];
        assert_eq!(
            delivered_keys(&mut state, &events),
            vec![events[0], events[1], events[2], events[5], events[6], events[7]]
        );
        assert_eq!(received.try_recv().unwrap(), HotkeyAction::Mode1);
    }

    #[test]
    fn focus_change_does_not_queue_or_reroute_ordinary_events() {
        let (mut state, received) = hook_state(default_hotkeys());
        assert!(!state.handle_key(&physical_key(0xA2), true));
        // The foreground window can change between two physical key events.
        assert!(!state.handle_key(&physical_key(b'C' as u32), true));
        assert!(!state.handle_key(&physical_key(b'C' as u32), false));
        assert!(!state.handle_key(&physical_key(0xA2), false));
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn releasing_one_control_key_keeps_the_other_control_key_active() {
        let (mut state, received) = hook_state(default_hotkeys());
        let mut left = physical_key(0x11);
        left.scanCode = 0x1D;
        let mut right = left;
        right.flags = LLKHF_EXTENDED;

        assert!(!state.handle_key(&left, true));
        assert!(!state.handle_key(&right, true));
        assert!(!state.handle_key(&left, false));
        assert!(state.handle_key(&physical_key(0x0D), true));
        assert!(state.handle_key(&physical_key(0x0D), false));
        assert!(!state.handle_key(&right, false));
        assert_eq!(received.try_recv().unwrap(), HotkeyAction::Mode1);
    }

    #[test]
    fn a_second_enter_key_is_not_swallowed_after_the_modifier_is_released() {
        let (mut state, received) = hook_state(default_hotkeys());
        let mut main_enter = physical_key(0x0D);
        main_enter.scanCode = 0x1C;
        let mut numpad_enter = main_enter;
        numpad_enter.flags = LLKHF_EXTENDED;

        assert!(!state.handle_key(&physical_key(0xA2), true));
        assert!(state.handle_key(&main_enter, true));
        assert!(!state.handle_key(&physical_key(0xA2), false));
        assert!(!state.handle_key(&numpad_enter, true));
        assert!(!state.handle_key(&numpad_enter, false));
        assert!(state.handle_key(&main_enter, false));
        assert_eq!(received.try_recv().unwrap(), HotkeyAction::Mode1);
    }

    #[test]
    fn balanced_non_hotkey_sequences_keep_all_key_events() {
        fn check_sequences(held: u8, used: u8, events: &mut Vec<(u32, bool)>, checked: &mut usize) {
            const KEYS: [u32; 4] = [0xA2, 0xA4, b'A' as u32, b'C' as u32];
            if used == 0b1111 && held == 0 {
                let (mut state, received) = hook_state(default_hotkeys());
                assert_eq!(delivered_keys(&mut state, events), *events);
                assert!(received.try_recv().is_err());
                *checked += 1;
                return;
            }
            for (index, vk) in KEYS.iter().enumerate() {
                let bit = 1 << index;
                if used & bit == 0 {
                    events.push((*vk, true));
                    check_sequences(held | bit, used | bit, events, checked);
                    events.pop();
                } else if held & bit != 0 {
                    events.push((*vk, false));
                    check_sequences(held & !bit, used, events, checked);
                    events.pop();
                }
            }
        }

        let mut checked = 0;
        check_sequences(0, 0, &mut Vec::new(), &mut checked);
        assert!(checked > 1000);
    }
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

        assert_eq!(bindings.len(), default_hotkeys().len());
        assert_eq!(bindings[0].accelerator, "Alt+1");
        assert_eq!(bindings[3].accelerator, "Ctrl+Up");
        assert_eq!(bindings[7].accelerator, "Ctrl+\\");
        assert_eq!(bindings[8].accelerator, "Shift+Enter");
        assert_eq!(bindings[9].accelerator, "Ctrl+Shift+N");
        assert_eq!(bindings[10].accelerator, "Ctrl+Shift+R");
        assert_eq!(bindings[11].accelerator, "Ctrl+Shift+B");
        assert_eq!(bindings[12].accelerator, "Ctrl+Shift+M");
    }

    #[test]
    fn existing_shortcut_settings_gain_mouse_through_recovery_binding() {
        let settings = StoredHotkeySettings {
            bindings: default_hotkeys()
                .into_iter()
                .filter(|binding| binding.action != HotkeyAction::MouseThrough)
                .map(|binding| StoredHotkeyBinding {
                    action: binding.action.as_str().to_string(),
                    accelerator: binding.accelerator,
                })
                .collect(),
        };

        let request = merge_saved_hotkey_settings(settings).unwrap();
        let bindings = hotkey_bindings_from_request(&request).unwrap();
        assert!(bindings.iter().any(|binding| {
            binding.action == HotkeyAction::MouseThrough && binding.accelerator == "Ctrl+Shift+M"
        }));
    }

    #[test]
    fn existing_shortcut_settings_gain_type_message_binding() {
        let settings = StoredHotkeySettings {
            bindings: default_hotkeys()
                .into_iter()
                .filter(|binding| binding.action != HotkeyAction::TypeMessage)
                .map(|binding| StoredHotkeyBinding {
                    action: binding.action.as_str().to_string(),
                    accelerator: binding.accelerator,
                })
                .collect(),
        };
        let request = merge_saved_hotkey_settings(settings).unwrap();
        let bindings = hotkey_bindings_from_request(&request).unwrap();
        assert!(bindings.iter().any(|binding| {
            binding.action == HotkeyAction::TypeMessage && binding.accelerator == "Ctrl+Shift+T"
        }));
    }

    #[test]
    fn new_chat_shortcut_is_consumed_before_reaching_the_focused_window() {
        let (mut state, received) = hook_state(default_hotkeys());
        let events = [
            (0xA2, true),
            (0xA0, true),
            (b'N' as u32, true),
            (b'N' as u32, false),
            (0xA0, false),
            (0xA2, false),
        ];
        assert_eq!(
            delivered_keys(&mut state, &events),
            vec![events[0], events[5]]
        );
        assert_eq!(received.try_recv().unwrap(), HotkeyAction::NewChat);
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn clamps_window_movement_to_monitor_work_area() {
        assert_eq!(clamped_window_axis(100, -50, 100, 1000, 400), 100);
        assert_eq!(clamped_window_axis(650, 50, 100, 1000, 400), 700);
        assert_eq!(clamped_window_axis(700, 50, 100, 1000, 400), 700);
        assert_eq!(clamped_window_axis(250, 50, 100, 200, 400), 100);
    }
}
