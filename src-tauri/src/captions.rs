use std::{
    collections::HashSet,
    env,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use windows::{
    core::Result as WindowsResult,
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        },
        UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
            TreeScope_Children, TreeScope_Descendants, UIA_DocumentControlTypeId,
            UIA_EditControlTypeId, UIA_TextControlTypeId, UIA_TextPatternId,
        },
    },
};

use crate::browser;

const MAIN_WINDOW_LABEL: &str = "main";
const CAPTION_EVENT: &str = "captions://state";
const POLL_INTERVAL: Duration = Duration::from_millis(450);
const WINDOW_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_BUFFER_LINES: usize = 80;
const MAX_DESCENDANTS_TO_SCAN: i32 = 600;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSnapshot {
    is_monitoring: bool,
    window_found: bool,
    text_element_found: bool,
    launch_attempted: bool,
    current_caption_text: String,
    last_submitted_caption_text: String,
    pending_caption_text: String,
    latest_caption: String,
    caption_buffer: Vec<String>,
    last_error: Option<String>,
}

impl Default for CaptionSnapshot {
    fn default() -> Self {
        Self {
            is_monitoring: false,
            window_found: false,
            text_element_found: false,
            launch_attempted: false,
            current_caption_text: String::new(),
            last_submitted_caption_text: String::new(),
            pending_caption_text: String::new(),
            latest_caption: String::new(),
            caption_buffer: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionCommandError {
    code: &'static str,
    message: String,
}

#[derive(Debug, Default)]
pub struct CaptionStore {
    snapshot: Mutex<CaptionSnapshot>,
    monitor: Mutex<Option<CaptionMonitor>>,
}

#[derive(Debug)]
struct CaptionMonitor {
    stop_requested: Arc<AtomicBool>,
}

type CommandResult<T> = Result<T, CaptionCommandError>;

#[tauri::command]
pub fn captions_get_state(state: State<'_, CaptionStore>) -> CommandResult<CaptionSnapshot> {
    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn captions_start(
    app: AppHandle,
    state: State<'_, CaptionStore>,
) -> CommandResult<CaptionSnapshot> {
    {
        let monitor = state.monitor.lock().map_err(|_| CaptionCommandError {
            code: "state_unavailable",
            message: "Caption monitor state is unavailable.".to_string(),
        })?;

        if monitor.is_some() {
            return Ok(state.snapshot()?.clone());
        }
    }

    launch_live_captions().map_err(|message| CaptionCommandError {
        code: "launch_failed",
        message,
    })?;

    update_snapshot(&app, |snapshot| {
        snapshot.is_monitoring = true;
        snapshot.launch_attempted = true;
        snapshot.last_error = None;
    });

    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_requested);
    let worker_app = app.clone();

    thread::Builder::new()
        .name("live-captions-uia-monitor".to_string())
        .spawn(move || monitor_live_captions(worker_app, worker_stop))
        .map_err(|error| CaptionCommandError {
            code: "monitor_start_failed",
            message: error.to_string(),
        })?;

    let mut monitor = state.monitor.lock().map_err(|_| CaptionCommandError {
        code: "state_unavailable",
        message: "Caption monitor state is unavailable.".to_string(),
    })?;
    *monitor = Some(CaptionMonitor { stop_requested });

    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn captions_stop(
    app: AppHandle,
    state: State<'_, CaptionStore>,
) -> CommandResult<CaptionSnapshot> {
    let monitor = {
        let mut monitor = state.monitor.lock().map_err(|_| CaptionCommandError {
            code: "state_unavailable",
            message: "Caption monitor state is unavailable.".to_string(),
        })?;
        monitor.take()
    };

    if let Some(monitor) = monitor {
        monitor.stop_requested.store(true, Ordering::Relaxed);
    }

    update_snapshot(&app, |snapshot| {
        snapshot.is_monitoring = false;
    });

    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn captions_submit_to_chatgpt(
    app: AppHandle,
    state: State<'_, CaptionStore>,
) -> CommandResult<CaptionSnapshot> {
    let caption_text =
        caption_text_for_submission(&state).map_err(|message| CaptionCommandError {
            code: "empty_caption",
            message,
        })?;

    browser::copy_text_to_chatgpt_input(&app, &caption_text).map_err(|message| {
        CaptionCommandError {
            code: "browser_copy_failed",
            message,
        }
    })?;

    mark_caption_submitted(&app, caption_text);

    Ok(state.snapshot()?.clone())
}

pub fn caption_text_for_submission(state: &State<'_, CaptionStore>) -> Result<String, String> {
    let snapshot = state
        .snapshot
        .lock()
        .map_err(|_| "Caption state is unavailable.".to_string())?;
    let source = if snapshot.pending_caption_text.trim().is_empty() {
        &snapshot.current_caption_text
    } else {
        &snapshot.pending_caption_text
    };
    let caption_text = clean_caption_text(source);

    if caption_text.is_empty() {
        Err("No caption text is ready to submit.".to_string())
    } else {
        Ok(caption_text)
    }
}

pub fn mark_caption_submitted(app: &AppHandle, caption_text: String) {
    update_snapshot(app, |snapshot| {
        snapshot.last_submitted_caption_text = caption_text;
        snapshot.current_caption_text.clear();
        snapshot.pending_caption_text.clear();
        snapshot.latest_caption.clear();
        snapshot.caption_buffer.clear();
        snapshot.last_error = None;
    });
}

fn monitor_live_captions(app: AppHandle, stop_requested: Arc<AtomicBool>) {
    let result = run_uia_monitor(&app, &stop_requested);

    if let Err(error) = result {
        update_snapshot(&app, |snapshot| {
            snapshot.is_monitoring = false;
            snapshot.last_error = Some(error);
        });
    }
}

fn run_uia_monitor(app: &AppHandle, stop_requested: &AtomicBool) -> Result<(), String> {
    let _com = ComApartment::initialize()?;
    let automation =
        create_automation().map_err(|error| format!("Failed to create UI Automation: {error}"))?;
    let started_at = Instant::now();

    loop {
        if stop_requested.load(Ordering::Relaxed) {
            return Ok(());
        }

        match capture_caption_text(&automation) {
            Ok(Some(capture)) => {
                update_snapshot(app, |snapshot| {
                    snapshot.window_found = true;
                    snapshot.text_element_found = true;
                    snapshot.last_error = None;
                    push_caption(snapshot, capture.text);
                });
            }
            Ok(None) => {
                let timed_out = started_at.elapsed() > WINDOW_DISCOVERY_TIMEOUT;
                update_snapshot(app, |snapshot| {
                    snapshot.window_found = false;
                    snapshot.text_element_found = false;
                    if timed_out {
                        snapshot.last_error = Some(
                            "Live Captions window was not found through UI Automation.".to_string(),
                        );
                    }
                });
            }
            Err(error) => {
                update_snapshot(app, |snapshot| {
                    snapshot.last_error = Some(error);
                });
            }
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn capture_caption_text(automation: &IUIAutomation) -> Result<Option<CaptionCapture>, String> {
    let window = unsafe { find_live_captions_window(automation) }
        .map_err(|error| format!("UI Automation window search failed: {error}"))?;
    let Some(window) = window else {
        return Ok(None);
    };

    let text = unsafe { find_caption_text(&window) }
        .map_err(|error| format!("UI Automation text search failed: {error}"))?;

    Ok(text.map(|text| CaptionCapture { text }))
}

struct CaptionCapture {
    text: String,
}

unsafe fn find_live_captions_window(
    automation: &IUIAutomation,
) -> WindowsResult<Option<IUIAutomationElement>> {
    let root = unsafe { automation.GetRootElement()? };
    let condition = unsafe { automation.CreateTrueCondition()? };
    let windows = unsafe { root.FindAll(TreeScope_Children, &condition)? };
    let count = unsafe { windows.Length()? };

    for index in 0..count {
        let element = unsafe { windows.GetElement(index)? };
        let name = current_name(&element);
        let class_name = current_class_name(&element);
        let searchable = format!("{name} {class_name}").to_ascii_lowercase();

        if searchable.contains("live captions")
            || searchable.contains("livecaptions")
            || searchable.contains("windows caption")
        {
            return Ok(Some(element));
        }
    }

    Ok(None)
}

unsafe fn find_caption_text(window: &IUIAutomationElement) -> WindowsResult<Option<String>> {
    let condition = unsafe { create_true_condition_from_element(window)? };
    let descendants = unsafe { window.FindAll(TreeScope_Descendants, &condition)? };
    let count = unsafe { descendants.Length()?.min(MAX_DESCENDANTS_TO_SCAN) };
    let mut best_text = String::new();

    for index in 0..count {
        let element = unsafe { descendants.GetElement(index)? };
        let control_type = unsafe { element.CurrentControlType().ok() };

        if !is_text_like_control(control_type) {
            continue;
        }

        let text = extract_element_text(&element);

        if is_probable_caption(&text) && text.len() > best_text.len() {
            best_text = text;
        }
    }

    if best_text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(best_text))
    }
}

unsafe fn create_true_condition_from_element(
    element: &IUIAutomationElement,
) -> WindowsResult<windows::Win32::UI::Accessibility::IUIAutomationCondition> {
    let automation = create_automation()?;
    let _ = element;
    unsafe { automation.CreateTrueCondition() }
}

fn is_text_like_control(
    control_type: Option<windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID>,
) -> bool {
    control_type.is_some_and(|value| {
        value == UIA_TextControlTypeId
            || value == UIA_EditControlTypeId
            || value == UIA_DocumentControlTypeId
    })
}

fn extract_element_text(element: &IUIAutomationElement) -> String {
    if let Ok(pattern) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
    {
        if let Ok(range) = unsafe { pattern.DocumentRange() } {
            if let Ok(text) = unsafe { range.GetText(4096) } {
                return sanitize_caption_text(&text.to_string());
            }
        }
    }

    sanitize_caption_text(&current_name(element))
}

fn current_name(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentName() }
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn current_class_name(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentClassName() }
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn is_probable_caption(text: &str) -> bool {
    if text.len() < 2 {
        return false;
    }

    let normalized = text.to_ascii_lowercase();
    let blocked = [
        "live captions",
        "settings",
        "close",
        "minimize",
        "maximize",
        "restore",
        "language",
        "caption language",
        "ready to caption",
        "start listening",
    ];

    !blocked.iter().any(|value| normalized == *value)
}

fn sanitize_caption_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_caption(snapshot: &mut CaptionSnapshot, caption: String) {
    if caption.is_empty() || snapshot.latest_caption == caption {
        return;
    }

    snapshot.latest_caption = caption.clone();
    snapshot.caption_buffer.push(caption);

    if snapshot.caption_buffer.len() > MAX_BUFFER_LINES {
        let drain_count = snapshot.caption_buffer.len() - MAX_BUFFER_LINES;
        snapshot.caption_buffer.drain(0..drain_count);
    }

    snapshot.current_caption_text = clean_caption_lines(&snapshot.caption_buffer);
    snapshot.pending_caption_text = pending_caption_text(
        &snapshot.current_caption_text,
        &snapshot.last_submitted_caption_text,
    );
}

fn pending_caption_text(current: &str, last_submitted: &str) -> String {
    if last_submitted.trim().is_empty() {
        return current.to_string();
    }

    current
        .strip_prefix(last_submitted)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(current)
        .to_string()
}

fn clean_caption_lines(lines: &[String]) -> String {
    clean_caption_text(&lines.join("\n"))
}

fn clean_caption_text(text: &str) -> String {
    let mut cleaned_lines = Vec::new();
    let mut seen_lines = HashSet::new();

    for raw_line in text.lines() {
        let line = clean_caption_line(raw_line);
        let normalized = normalize_for_dedupe(&line);

        if line.is_empty() || normalized.is_empty() || seen_lines.contains(&normalized) {
            continue;
        }

        seen_lines.insert(normalized);
        cleaned_lines.push(line);
    }

    join_caption_sentences(&cleaned_lines)
}

fn clean_caption_line(line: &str) -> String {
    let mut cleaned = line
        .replace('\u{266a}', " ")
        .replace('\u{266b}', " ")
        .replace("[Music]", " ")
        .replace("(Music)", " ")
        .replace("[Applause]", " ")
        .replace("(Applause)", " ")
        .replace("[Laughter]", " ")
        .replace("(Laughter)", " ")
        .replace("[Inaudible]", " ")
        .replace("(Inaudible)", " ")
        .replace("[Silence]", " ")
        .replace("(Silence)", " ");

    cleaned = cleaned
        .trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '-' | '>' | ':' | '|')
        })
        .to_string();

    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_for_dedupe(line: &str) -> String {
    line.trim()
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_ascii_lowercase()
}

fn join_caption_sentences(lines: &[String]) -> String {
    let mut result = String::new();

    for line in lines {
        if result.is_empty() {
            result.push_str(line);
            continue;
        }

        if result.ends_with(['.', '!', '?', ':', ';']) {
            result.push(' ');
            result.push_str(line);
        } else if starts_with_sentence_continuation(line) {
            result.push(' ');
            result.push_str(line);
        } else {
            result.push(' ');
            result.push_str(line);
        }
    }

    result.trim().to_string()
}

fn starts_with_sentence_continuation(line: &str) -> bool {
    line.chars().next().is_some_and(|character| {
        character.is_lowercase() || matches!(character, ',' | '.' | '?' | '!')
    })
}

fn launch_live_captions() -> Result<(), String> {
    if let Some(path) = live_captions_exe_path() {
        Command::new(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Failed to launch Live Captions executable: {error}"))
    } else {
        launch_shell_target(
            "shell:AppsFolder\\MicrosoftWindows.Client.CBS_cw5n1h2txyewy!LiveCaptions",
        )
        .or_else(|_| launch_shell_target("ms-settings:easeofaccess-captions"))
    }
}

fn launch_shell_target(target: &str) -> Result<(), String> {
    Command::new("explorer.exe")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Failed to launch `{target}`: {error}"))
}

fn live_captions_exe_path() -> Option<PathBuf> {
    let windows_dir = env::var_os("WINDIR").map(PathBuf::from)?;
    let candidates = [
        windows_dir
            .join("SystemApps")
            .join("MicrosoftWindows.Client.CBS_cw5n1h2txyewy")
            .join("LiveCaptions.exe"),
        windows_dir.join("System32").join("LiveCaptions.exe"),
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn create_automation() -> WindowsResult<IUIAutomation> {
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .map(|| Self)
            .map_err(|error| format!("Failed to initialize COM for UI Automation: {error}"))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn update_snapshot(app: &AppHandle, update: impl FnOnce(&mut CaptionSnapshot)) {
    let state = app.state::<CaptionStore>();
    let next_snapshot = match state.snapshot.lock() {
        Ok(mut snapshot) => {
            update(&mut snapshot);
            snapshot.clone()
        }
        Err(_) => return,
    };

    let _ = app.emit_to(MAIN_WINDOW_LABEL, CAPTION_EVENT, next_snapshot);
}

trait CaptionStoreExt {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, CaptionSnapshot>>;
}

impl CaptionStoreExt for CaptionStore {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, CaptionSnapshot>> {
        self.snapshot.lock().map_err(|_| CaptionCommandError {
            code: "state_unavailable",
            message: "Caption state is unavailable.".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_caption_text, pending_caption_text};

    #[test]
    fn clean_caption_text_removes_duplicate_lines_and_artifacts() {
        let input = "\n  Hello   there.\n[Music]\nHello there.\nthis is still useful\n";

        assert_eq!(
            clean_caption_text(input),
            "Hello there. this is still useful"
        );
    }

    #[test]
    fn clean_caption_text_preserves_punctuation() {
        let input = "Can you hear me?\nYes, I can!\nLet's continue.";

        assert_eq!(
            clean_caption_text(input),
            "Can you hear me? Yes, I can! Let's continue."
        );
    }

    #[test]
    fn pending_caption_text_prefers_unsubmitted_suffix() {
        assert_eq!(
            pending_caption_text("First sentence. Second sentence.", "First sentence."),
            "Second sentence."
        );
    }
}
