use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{browser, captions, diagnostics, profiles, screenshot};

const MAIN_WINDOW_LABEL: &str = "main";
const AUTOMATION_EVENT: &str = "automation://state";
const CAPTION_INPUT_ATTEMPTS: usize = 3;
const CAPTION_RETRY_DELAY: Duration = Duration::from_millis(250);
const CHATGPT_UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);
const CHATGPT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(30);
const ATTACHMENT_DISCARD_TIMEOUT: Duration = Duration::from_secs(3);
const MODE_3_COORDINATOR_POLL_INTERVAL: Duration = Duration::from_millis(100);
const AUTOMATION_RESET_TIMEOUT: Duration = Duration::from_secs(7);
const AUTOMATION_SETTINGS_FILE: &str = "automation-settings.json";
const AUTOMATION_CANCELLED: &str = "Automation was cancelled by browser navigation.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationSnapshot {
    is_running: bool,
    last_mode: Option<AutomationMode>,
    upload_state: UploadState,
    last_error: Option<String>,
}

impl Default for AutomationSnapshot {
    fn default() -> Self {
        Self {
            is_running: false,
            last_mode: None,
            upload_state: UploadState::Idle,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationMode {
    CaptionSubmit,
    ScreenshotCaptionSubmit,
    ScreenshotOnly,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UploadState {
    Idle,
    Uploading,
    Ready,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationCommandError {
    code: &'static str,
    message: String,
}

#[derive(Debug, Default)]
pub struct AutomationStore {
    snapshot: Mutex<AutomationSnapshot>,
    caption_workflow_reserved: Arc<AtomicBool>,
    caption_prompt_gate: Mutex<()>,
    mode_3_coordinator: Arc<Mode3Coordinator>,
    cancellation_epoch: Arc<AtomicU64>,
    resetting: Arc<AtomicBool>,
    prepared_prompt: Mutex<String>,
    refresh_prompt: Mutex<String>,
    refresh_restore_pending: AtomicBool,
    refresh_prompt_restored: AtomicBool,
    previous_submitted_prompt: Mutex<String>,
    preferences: Mutex<AutomationPreferences>,
}

pub(crate) struct CaptionWorkflowPermit {
    reserved: Arc<AtomicBool>,
    token: CancellationToken,
}

#[derive(Debug, Clone)]
struct CancellationToken {
    epoch: Arc<AtomicU64>,
    expected: u64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPreferences {
    keep_existing_prompt: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPreferencesRequest {
    keep_existing_prompt: bool,
}

#[derive(Debug, Default)]
struct Mode3Coordinator {
    state: Mutex<Mode3CoordinatorState>,
}

#[derive(Debug, Default)]
struct Mode3CoordinatorState {
    active_jobs: usize,
    generation: u64,
    finalizing: bool,
    successful_injections: usize,
    upload_errors: Vec<String>,
}

pub(crate) struct Mode3JobPermit {
    coordinator: Arc<Mode3Coordinator>,
    token: CancellationToken,
}

impl Drop for CaptionWorkflowPermit {
    fn drop(&mut self) {
        self.reserved.store(false, Ordering::Release);
    }
}

impl CancellationToken {
    fn is_cancelled(&self) -> bool {
        self.epoch.load(Ordering::Acquire) != self.expected
    }
}

type CommandResult<T> = Result<T, AutomationCommandError>;

pub fn setup(app: &AppHandle) {
    if let Ok(Some(preferences)) = load_automation_preferences(app) {
        if let Ok(mut stored) = app.state::<AutomationStore>().preferences.lock() {
            *stored = preferences;
        }
    }
}

pub fn cancelled_error() -> String {
    AUTOMATION_CANCELLED.to_string()
}

pub fn is_cancelled_error(message: &str) -> bool {
    message == AUTOMATION_CANCELLED
}

#[tauri::command]
pub fn automation_get_state(
    state: State<'_, AutomationStore>,
) -> CommandResult<AutomationSnapshot> {
    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn automation_shortcut_mode_1(
    app: AppHandle,
    _automation: State<'_, AutomationStore>,
    _captions: State<'_, captions::CaptionStore>,
) -> CommandResult<AutomationSnapshot> {
    run_mode_1(&app).map_err(AutomationCommandError::automation_failed)?;
    Ok(app.state::<AutomationStore>().snapshot()?.clone())
}

#[tauri::command]
pub fn automation_shortcut_mode_2(
    app: AppHandle,
    _automation: State<'_, AutomationStore>,
    _captions: State<'_, captions::CaptionStore>,
) -> CommandResult<AutomationSnapshot> {
    run_mode_2(&app).map_err(AutomationCommandError::automation_failed)?;
    Ok(app.state::<AutomationStore>().snapshot()?.clone())
}

#[tauri::command]
pub fn automation_shortcut_mode_3(
    app: AppHandle,
    _automation: State<'_, AutomationStore>,
) -> CommandResult<AutomationSnapshot> {
    run_mode_3(&app).map_err(AutomationCommandError::automation_failed)?;
    Ok(app.state::<AutomationStore>().snapshot()?.clone())
}

#[tauri::command]
pub fn automation_submit_after_upload(
    app: AppHandle,
    _automation: State<'_, AutomationStore>,
) -> CommandResult<AutomationSnapshot> {
    submit_after_upload(&app).map_err(AutomationCommandError::automation_failed)?;
    Ok(app.state::<AutomationStore>().snapshot()?.clone())
}

pub fn run_mode_1(app: &AppHandle) -> Result<(), String> {
    let permit = try_reserve_caption_workflow(app)?;
    run_mode_1_reserved(app, permit)
}

#[tauri::command]
pub fn automation_get_preferences(
    state: State<'_, AutomationStore>,
) -> CommandResult<AutomationPreferences> {
    state
        .preferences
        .lock()
        .map(|preferences| *preferences)
        .map_err(|_| AutomationCommandError::state_unavailable())
}

#[tauri::command]
pub fn automation_apply_preferences(
    app: AppHandle,
    state: State<'_, AutomationStore>,
    request: AutomationPreferencesRequest,
) -> CommandResult<AutomationPreferences> {
    let preferences = AutomationPreferences {
        keep_existing_prompt: request.keep_existing_prompt,
    };
    save_automation_preferences(&app, &preferences)
        .map_err(AutomationCommandError::automation_failed)?;
    *state
        .preferences
        .lock()
        .map_err(|_| AutomationCommandError::state_unavailable())? = preferences;
    Ok(preferences)
}

pub(crate) fn run_mode_1_reserved(
    app: &AppHandle,
    permit: CaptionWorkflowPermit,
) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(app, &automation, AutomationMode::CaptionSubmit, || {
        let prompt_text = take_and_prepare_caption_prompt(app, &permit.token, false)?;

        let result = (|| {
            submit_caption_when_ready(
                &prompt_text,
                |text| {
                    ensure_not_cancelled(&permit.token)?;
                    browser::copy_text_to_chatgpt_input(app, text)
                },
                || {
                    browser::wait_and_submit_chatgpt_input_cancellable(
                        app,
                        CHATGPT_SUBMIT_TIMEOUT,
                        || permit.token.is_cancelled(),
                    )
                },
                CAPTION_RETRY_DELAY,
            )?;
            Ok(UploadState::Idle)
        })();

        finish_prepared_prompt(
            app,
            &prompt_text,
            result.is_ok(),
            permit.token.is_cancelled(),
        );
        result
    })
    .map(|_| ())
    .map_err(|error| error.message)
}

fn submit_caption_when_ready(
    caption_text: &str,
    mut insert: impl FnMut(&str) -> Result<(), String>,
    mut wait_and_submit: impl FnMut() -> Result<(), String>,
    retry_delay: Duration,
) -> Result<(), String> {
    let mut input_errors = Vec::with_capacity(CAPTION_INPUT_ATTEMPTS);
    let mut inserted = false;

    for attempt in 1..=CAPTION_INPUT_ATTEMPTS {
        match insert(caption_text) {
            Ok(()) => {
                inserted = true;
                break;
            }
            Err(error) => input_errors.push(format!("attempt {attempt}: {error}")),
        }

        if attempt < CAPTION_INPUT_ATTEMPTS {
            thread::sleep(retry_delay);
        }
    }

    if !inserted {
        return Err(format!(
            "The caption text could not be inserted after {CAPTION_INPUT_ATTEMPTS} attempts. {}",
            input_errors.join("; ")
        ));
    }

    match wait_and_submit() {
        Ok(()) => Ok(()),
        Err(submit_error) => match insert(caption_text) {
            Ok(()) => Err(format!(
                "Caption submission failed while waiting for ChatGPT: {submit_error}. The text was left in the ChatGPT prompt."
            )),
            Err(fallback_error) => Err(format!(
                "Caption submission failed while waiting for ChatGPT: {submit_error}. The text could not be restored in the ChatGPT prompt: {fallback_error}."
            )),
        },
    }
}

pub fn run_mode_2(app: &AppHandle) -> Result<(), String> {
    let permit = try_reserve_caption_workflow(app)?;
    run_mode_2_reserved(app, permit)
}

pub(crate) fn run_mode_2_reserved(
    app: &AppHandle,
    permit: CaptionWorkflowPermit,
) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(
        app,
        &automation,
        AutomationMode::ScreenshotCaptionSubmit,
        || {
            let prompt_text = take_and_prepare_caption_prompt(app, &permit.token, true)?;

            let result = run_mode_2_prompt_workflow(app, &permit.token, &prompt_text);
            finish_prepared_prompt(
                app,
                &prompt_text,
                result.is_ok(),
                permit.token.is_cancelled(),
            );
            result
        },
    )
    .map(|_| ())
    .map_err(|error| error.message)
}

fn run_mode_2_prompt_workflow(
    app: &AppHandle,
    token: &CancellationToken,
    prompt_text: &str,
) -> Result<UploadState, String> {
    let mut upload_was_injected = false;
    let upload_result = (|| {
        ensure_not_cancelled(token)?;
        let screenshot = screenshot::capture_primary_display_png()?;
        ensure_not_cancelled(token)?;
        browser::upload_screenshot_to_chatgpt_input(app, &screenshot.file_name, &screenshot.bytes)?;
        drop(screenshot);
        upload_was_injected = true;
        update_snapshot(app, |snapshot| {
            snapshot.upload_state = UploadState::Uploading;
        });

        ensure_not_cancelled(token)?;
        let _ = browser::copy_text_to_chatgpt_input(app, prompt_text);
        browser::wait_for_chatgpt_upload_cancellable(app, CHATGPT_UPLOAD_TIMEOUT, || {
            token.is_cancelled()
        })?;
        update_snapshot(app, |snapshot| {
            snapshot.upload_state = UploadState::Ready;
        });
        Ok::<(), String>(())
    })();

    match upload_result {
        Ok(()) => {
            submit_caption_when_ready(
                prompt_text,
                |text| {
                    ensure_not_cancelled(token)?;
                    browser::copy_text_to_chatgpt_input(app, text)
                },
                || {
                    browser::wait_and_submit_chatgpt_input_cancellable(
                        app,
                        CHATGPT_SUBMIT_TIMEOUT,
                        || token.is_cancelled(),
                    )
                },
                CAPTION_RETRY_DELAY,
            )?;
            Ok(UploadState::Ready)
        }
        Err(upload_error) if token.is_cancelled() => Err(upload_error),
        Err(upload_error) => {
            ensure_not_cancelled(token)?;
            update_snapshot(app, |snapshot| {
                snapshot.upload_state = UploadState::Failed;
            });

            if upload_was_injected {
                if let Err(discard_error) =
                    browser::discard_chatgpt_attachments(app, ATTACHMENT_DISCARD_TIMEOUT)
                {
                    let prompt_result = browser::copy_text_to_chatgpt_input(app, prompt_text);
                    return Err(match prompt_result {
                        Ok(()) => format!(
                            "Image upload failed: {upload_error}. The image attachment could not be removed: {discard_error}. The caption text was left in the ChatGPT prompt and was not submitted."
                        ),
                        Err(prompt_error) => format!(
                            "Image upload failed: {upload_error}. The image attachment could not be removed: {discard_error}. The caption text could not be left in the ChatGPT prompt: {prompt_error}."
                        ),
                    });
                }
            }

            if prompt_text.trim().is_empty() {
                return Err(format!(
                    "Image upload failed and no Live Captions text was available to submit: {upload_error}"
                ));
            }

            submit_caption_when_ready(
                prompt_text,
                |text| {
                    ensure_not_cancelled(token)?;
                    browser::copy_text_to_chatgpt_input(app, text)
                },
                || {
                    browser::wait_and_submit_chatgpt_input_cancellable(
                        app,
                        CHATGPT_SUBMIT_TIMEOUT,
                        || token.is_cancelled(),
                    )
                },
                CAPTION_RETRY_DELAY,
            )
            .map_err(|submit_error| {
                format!("Image upload failed: {upload_error}. {submit_error}")
            })?;

            update_snapshot(app, |snapshot| {
                snapshot.last_error = Some(format!(
                    "Image upload failed, so only the caption text was submitted: {upload_error}"
                ));
            });
            Ok(UploadState::Failed)
        }
    }
}

fn ensure_not_cancelled(token: &CancellationToken) -> Result<(), String> {
    if token.is_cancelled() {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

fn take_and_prepare_caption_prompt(
    app: &AppHandle,
    token: &CancellationToken,
    allow_empty_caption: bool,
) -> Result<String, String> {
    let automation = app.state::<AutomationStore>();
    let has_refresh_prompt = !automation
        .refresh_prompt
        .lock()
        .map_err(|_| "Refresh prompt memory is unavailable.".to_string())?
        .trim()
        .is_empty();
    let live_refresh_prompt = has_refresh_prompt
        .then(|| browser::read_chatgpt_prompt_text(app).ok())
        .flatten();

    let _gate = automation
        .caption_prompt_gate
        .lock()
        .map_err(|_| "Caption prompt preparation is unavailable.".to_string())?;

    // Refresh takes the same gate after changing the cancellation epoch. This makes
    // detaching captions and remembering their prompt atomic from Refresh's point of view.
    ensure_not_cancelled(token)?;
    let new_caption_text = if allow_empty_caption {
        captions::take_caption_batch_for_hotkey_or_empty(app)?
    } else {
        captions::take_caption_batch_for_hotkey(app)?
    };
    let prompt_text =
        prepare_caption_prompt(app, &new_caption_text, live_refresh_prompt.as_deref())?;
    remember_prepared_prompt(app, &prompt_text)?;
    Ok(prompt_text)
}

fn prepare_caption_prompt(
    app: &AppHandle,
    new_caption_text: &str,
    live_refresh_prompt: Option<&str>,
) -> Result<String, String> {
    let prompt = prepare_caption_prompt_body(app, new_caption_text, live_refresh_prompt)?;
    let profile_prompt = profiles::active_prompt(app)?.unwrap_or_default();
    Ok(merge_profile_prompt(&profile_prompt, &prompt))
}

fn prepare_caption_prompt_body(
    app: &AppHandle,
    new_caption_text: &str,
    live_refresh_prompt: Option<&str>,
) -> Result<String, String> {
    let automation = app.state::<AutomationStore>();
    let refresh_prompt = automation
        .refresh_prompt
        .lock()
        .map_err(|_| "Refresh prompt memory is unavailable.".to_string())?
        .clone();

    // A draft restored by Refresh is a one-time continuation for the next Mode 1/2 run.
    if !refresh_prompt.trim().is_empty() {
        if let Some(live_prompt) = live_refresh_prompt {
            if !live_prompt.trim().is_empty() {
                return Ok(merge_prompt_text(live_prompt, new_caption_text));
            }

            if automation.refresh_prompt_restored.load(Ordering::Acquire) {
                // The user submitted or cleared the restored draft manually.
                automation
                    .refresh_prompt
                    .lock()
                    .map_err(|_| "Refresh prompt memory is unavailable.".to_string())?
                    .clear();
            } else {
                // Mode 1/2 can be pressed before the post-reload restore worker finds
                // ChatGPT's composer. Keep the stored draft in that case.
                return Ok(merge_prompt_text(&refresh_prompt, new_caption_text));
            }
        } else {
            return Ok(merge_prompt_text(&refresh_prompt, new_caption_text));
        }
    }

    let keep_existing = automation
        .preferences
        .lock()
        .map_err(|_| "Automation preferences are unavailable.".to_string())?
        .keep_existing_prompt;

    if !keep_existing {
        return Ok(new_caption_text.to_string());
    }

    let existing = automation
        .previous_submitted_prompt
        .lock()
        .map_err(|_| "Previous submitted prompt memory is unavailable.".to_string())?
        .clone();

    Ok(merge_prompt_text(&existing, new_caption_text))
}

fn merge_profile_prompt(profile_prompt: &str, prompt: &str) -> String {
    let profile_prompt = profile_prompt.trim();
    let prompt = prompt.trim();
    if profile_prompt.is_empty() {
        return prompt.to_string();
    }
    if prompt == profile_prompt
        || prompt
            .strip_prefix(profile_prompt)
            .is_some_and(|remainder| remainder.starts_with('\n'))
    {
        return prompt.to_string();
    }
    merge_prompt_text(profile_prompt, prompt)
}

fn merge_prompt_text(existing: &str, new_text: &str) -> String {
    let existing = existing.trim();
    let new_text = new_text.trim();
    if existing.is_empty() {
        return new_text.to_string();
    }
    if new_text.is_empty() || existing == new_text || existing.ends_with(new_text) {
        return existing.to_string();
    }
    format!("{existing}\n{new_text}")
}

fn remember_prepared_prompt(app: &AppHandle, prompt_text: &str) -> Result<(), String> {
    *app.state::<AutomationStore>()
        .prepared_prompt
        .lock()
        .map_err(|_| "Prepared prompt memory is unavailable.".to_string())? =
        prompt_text.to_string();
    Ok(())
}

fn finish_prepared_prompt(app: &AppHandle, prompt_text: &str, submitted: bool, cancelled: bool) {
    let automation = app.state::<AutomationStore>();
    if submitted || !cancelled {
        if let Ok(mut prompt) = automation.prepared_prompt.lock() {
            prompt.clear();
            prompt.shrink_to_fit();
        }
    }
    if submitted {
        if let Ok(mut prompt) = automation.previous_submitted_prompt.lock() {
            *prompt = prompt_text.to_string();
        }
        if let Ok(mut prompt) = automation.refresh_prompt.lock() {
            prompt.clear();
            prompt.shrink_to_fit();
        }
        automation
            .refresh_restore_pending
            .store(false, Ordering::Release);
        automation
            .refresh_prompt_restored
            .store(false, Ordering::Release);
    }
}

pub(crate) struct NavigationResetGuard {
    resetting: Arc<AtomicBool>,
}

impl Drop for NavigationResetGuard {
    fn drop(&mut self) {
        self.resetting.store(false, Ordering::Release);
    }
}

fn begin_navigation_reset(app: &AppHandle) -> Result<NavigationResetGuard, String> {
    let automation = app.state::<AutomationStore>();
    automation
        .resetting
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "Automation reset is already in progress.".to_string())?;
    automation.cancellation_epoch.fetch_add(1, Ordering::AcqRel);
    Ok(NavigationResetGuard {
        resetting: Arc::clone(&automation.resetting),
    })
}

fn wait_for_automation_workers(app: &AppHandle) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();
    let started_at = Instant::now();

    while started_at.elapsed() < AUTOMATION_RESET_TIMEOUT {
        let caption_running = automation.caption_workflow_reserved.load(Ordering::Acquire);
        let mode_3_running = automation
            .mode_3_coordinator
            .state
            .lock()
            .map(|state| state.active_jobs > 0 || state.finalizing)
            .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;

        if !caption_running && !mode_3_running {
            return Ok(());
        }
        thread::sleep(MODE_3_COORDINATOR_POLL_INTERVAL);
    }

    Err("Timed out waiting for automation workers to exit.".to_string())
}

pub(crate) fn prepare_for_refresh(app: &AppHandle) -> Result<NavigationResetGuard, String> {
    let guard = begin_navigation_reset(app)?;
    let automation = app.state::<AutomationStore>();
    let prepared_before_read = {
        let _gate = automation
            .caption_prompt_gate
            .lock()
            .map_err(|_| "Caption prompt preparation is unavailable.".to_string())?;
        automation
            .prepared_prompt
            .lock()
            .map_err(|_| "Prepared prompt memory is unavailable.".to_string())?
            .clone()
    };

    // Prefer an in-flight Mode 1/2 prompt. Reading the page is only necessary for an
    // ordinary unsent draft, which keeps active-workflow refreshes close to native reload speed.
    let prompt_from_page = if prepared_before_read.trim().is_empty() {
        browser::read_chatgpt_prompt_text(app).unwrap_or_default()
    } else {
        String::new()
    };

    let prompt_to_restore = if prepared_before_read.trim().is_empty() {
        prompt_from_page
    } else {
        prepared_before_read
    };
    let has_prompt_to_restore = !prompt_to_restore.trim().is_empty();
    *automation
        .refresh_prompt
        .lock()
        .map_err(|_| "Refresh prompt memory is unavailable.".to_string())? = prompt_to_restore;
    automation
        .refresh_restore_pending
        .store(has_prompt_to_restore, Ordering::Release);
    automation
        .refresh_prompt_restored
        .store(false, Ordering::Release);
    automation
        .prepared_prompt
        .lock()
        .map_err(|_| "Prepared prompt memory is unavailable.".to_string())?
        .clear();

    reset_automation_runtime(app)?;
    Ok(guard)
}

pub fn reset_for_home(app: &AppHandle) -> Result<(), String> {
    let _guard = begin_navigation_reset(app)?;
    wait_for_automation_workers(app)?;
    let automation = app.state::<AutomationStore>();
    for memory in [
        &automation.prepared_prompt,
        &automation.refresh_prompt,
        &automation.previous_submitted_prompt,
    ] {
        let mut text = memory
            .lock()
            .map_err(|_| "Prompt memory is unavailable.".to_string())?;
        text.clear();
        text.shrink_to_fit();
    }
    automation
        .refresh_restore_pending
        .store(false, Ordering::Release);
    automation
        .refresh_prompt_restored
        .store(false, Ordering::Release);
    reset_automation_runtime(app)
}

fn reset_automation_runtime(app: &AppHandle) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();
    {
        let mut state = automation
            .mode_3_coordinator
            .state
            .lock()
            .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;
        state.active_jobs = 0;
        reset_mode_3_state(&mut state);
    }
    update_snapshot(app, |snapshot| *snapshot = AutomationSnapshot::default());
    Ok(())
}

pub fn restore_refresh_prompt_after_page_load(app: &AppHandle) {
    let automation = app.state::<AutomationStore>();
    if !automation
        .refresh_restore_pending
        .swap(false, Ordering::AcqRel)
    {
        return;
    }

    let worker_app = app.clone();
    let _ = thread::Builder::new()
        .name("restore-chatgpt-prompt".to_string())
        .spawn(move || {
            for _ in 0..20 {
                let automation = worker_app.state::<AutomationStore>();
                if automation.resetting.load(Ordering::Acquire) {
                    return;
                }
                let prompt = automation
                    .refresh_prompt
                    .lock()
                    .map(|prompt| prompt.clone())
                    .unwrap_or_default();
                if prompt.trim().is_empty() {
                    return;
                }
                if browser::copy_text_to_chatgpt_input(&worker_app, &prompt).is_ok() {
                    automation
                        .refresh_prompt_restored
                        .store(true, Ordering::Release);
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
}

fn automation_preferences_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(AUTOMATION_SETTINGS_FILE))
        .map_err(|error| format!("Failed to resolve automation settings directory: {error}"))
}

fn load_automation_preferences(app: &AppHandle) -> Result<Option<AutomationPreferences>, String> {
    let path = automation_preferences_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(|error| format!("Automation settings are invalid: {error}"))
}

fn save_automation_preferences(
    app: &AppHandle,
    preferences: &AutomationPreferences,
) -> Result<(), String> {
    let path = automation_preferences_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Automation settings path has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    let contents = serde_json::to_string_pretty(preferences)
        .map_err(|error| format!("Failed to serialize automation settings: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("Failed to save {}: {error}", path.display()))
}

pub(crate) fn try_reserve_caption_workflow(
    app: &AppHandle,
) -> Result<CaptionWorkflowPermit, String> {
    let state = app.state::<AutomationStore>();
    if state.resetting.load(Ordering::Acquire) {
        return Err(
            "Automation is resetting. Try the shortcut again after navigation finishes."
                .to_string(),
        );
    }

    let permit = try_reserve_caption_workflow_flag(
        &state.caption_workflow_reserved,
        &state.cancellation_epoch,
    )
    .ok_or_else(|| {
        "Mode 1 or Mode 2 automation is already running. The new request was ignored.".to_string()
    })?;
    if state.resetting.load(Ordering::Acquire) || permit.token.is_cancelled() {
        drop(permit);
        Err(
            "Automation is resetting. Try the shortcut again after navigation finishes."
                .to_string(),
        )
    } else {
        Ok(permit)
    }
}

fn try_reserve_caption_workflow_flag(
    reserved: &Arc<AtomicBool>,
    cancellation_epoch: &Arc<AtomicU64>,
) -> Option<CaptionWorkflowPermit> {
    reserved
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| CaptionWorkflowPermit {
            reserved: Arc::clone(reserved),
            token: CancellationToken {
                epoch: Arc::clone(cancellation_epoch),
                expected: cancellation_epoch.load(Ordering::Acquire),
            },
        })
}

pub fn run_mode_3(app: &AppHandle) -> Result<(), String> {
    let permit = start_mode_3_job(app)?;
    run_mode_3_reserved(app, permit)
}

pub(crate) fn start_mode_3_job(app: &AppHandle) -> Result<Mode3JobPermit, String> {
    let automation = app.state::<AutomationStore>();
    if automation.resetting.load(Ordering::Acquire) {
        return Err(
            "Automation is resetting. Try the shortcut again after navigation finishes."
                .to_string(),
        );
    }
    let coordinator = Arc::clone(&automation.mode_3_coordinator);
    let token = CancellationToken {
        epoch: Arc::clone(&automation.cancellation_epoch),
        expected: automation.cancellation_epoch.load(Ordering::Acquire),
    };
    let mut state = coordinator
        .state
        .lock()
        .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;

    if automation.resetting.load(Ordering::Acquire) || token.is_cancelled() {
        return Err(
            "Automation is resetting. Try the shortcut again after navigation finishes."
                .to_string(),
        );
    }

    state.active_jobs += 1;
    state.generation = state.generation.wrapping_add(1);
    drop(state);

    update_snapshot(app, |snapshot| {
        snapshot.is_running = true;
        snapshot.last_mode = Some(AutomationMode::ScreenshotOnly);
        snapshot.upload_state = UploadState::Uploading;
        snapshot.last_error = None;
    });

    Ok(Mode3JobPermit { coordinator, token })
}

pub(crate) fn run_mode_3_reserved(app: &AppHandle, permit: Mode3JobPermit) -> Result<(), String> {
    let upload_result = (|| {
        ensure_not_cancelled(&permit.token)?;
        let screenshot = screenshot::capture_primary_display_png()?;
        ensure_not_cancelled(&permit.token)?;
        browser::upload_screenshot_to_chatgpt_input(app, &screenshot.file_name, &screenshot.bytes)?;
        drop(screenshot);
        Ok::<(), String>(())
    })();

    let cancelled = permit.token.is_cancelled();
    let should_finalize = finish_mode_3_job(&permit, upload_result, !cancelled);

    if should_finalize? {
        finalize_mode_3_batch(app, &permit.coordinator, &permit.token)
    } else if cancelled {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

fn finish_mode_3_job(
    permit: &Mode3JobPermit,
    upload_result: Result<(), String>,
    allow_finalize: bool,
) -> Result<bool, String> {
    // Refresh resets the shared coordinator immediately. A cancelled job must not
    // decrement or add errors to a newer batch that may begin after reload.
    if !allow_finalize {
        return Ok(false);
    }

    let mut state = permit
        .coordinator
        .state
        .lock()
        .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;

    state.active_jobs = state.active_jobs.saturating_sub(1);
    match upload_result {
        Ok(()) => state.successful_injections += 1,
        Err(error) => state.upload_errors.push(error),
    }

    if allow_finalize && state.active_jobs == 0 && !state.finalizing {
        state.finalizing = true;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn finalize_mode_3_batch(
    app: &AppHandle,
    coordinator: &Arc<Mode3Coordinator>,
    token: &CancellationToken,
) -> Result<(), String> {
    loop {
        if token.is_cancelled() {
            cancel_mode_3_finalizer(coordinator)?;
            return Err(cancelled_error());
        }
        let (generation, successful_injections) = loop {
            let state = coordinator
                .state
                .lock()
                .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;

            if state.active_jobs == 0 {
                break (state.generation, state.successful_injections);
            }

            drop(state);
            if token.is_cancelled() {
                cancel_mode_3_finalizer(coordinator)?;
                return Err(cancelled_error());
            }
            thread::sleep(MODE_3_COORDINATOR_POLL_INTERVAL);
        };

        if successful_injections == 0 {
            return fail_mode_3_batch_without_submission(
                app,
                coordinator,
                "Mode 3 could not add any screenshots to the ChatGPT composer.".to_string(),
            );
        }

        // This readiness check covers every screenshot currently in the shared composer.
        // A timeout is intentionally non-fatal: ChatGPT may have discarded only the failed
        // files while leaving successful attachments ready for submission.
        let upload_readiness =
            browser::wait_for_chatgpt_upload_cancellable(app, CHATGPT_UPLOAD_TIMEOUT, || {
                token.is_cancelled()
            });

        if token.is_cancelled() {
            cancel_mode_3_finalizer(coordinator)?;
            return Err(cancelled_error());
        }

        let readiness_warning = upload_readiness.err();
        if complete_mode_3_upload_batch(app, coordinator, generation, readiness_warning)? {
            return Ok(());
        }
    }
}

fn complete_mode_3_upload_batch(
    app: &AppHandle,
    coordinator: &Arc<Mode3Coordinator>,
    generation: u64,
    readiness_warning: Option<String>,
) -> Result<bool, String> {
    let mut state = coordinator
        .state
        .lock()
        .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;

    if state.active_jobs != 0 || state.generation != generation {
        return Ok(false);
    }

    let mut warnings = state.upload_errors.clone();
    if let Some(warning) = readiness_warning {
        warnings.push(warning);
    }
    let warnings = format_mode_3_warnings(warnings);
    reset_mode_3_state(&mut state);

    // Publish while holding the coordinator lock. A new Mode 3 job can only set the
    // snapshot back to running after this completed batch is marked ready.
    update_snapshot(app, |snapshot| {
        snapshot.is_running = false;
        snapshot.upload_state = UploadState::Ready;
        snapshot.last_error = warnings;
    });
    drop(state);
    Ok(true)
}

fn cancel_mode_3_finalizer(coordinator: &Arc<Mode3Coordinator>) -> Result<(), String> {
    let mut state = coordinator
        .state
        .lock()
        .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;
    reset_mode_3_state(&mut state);
    Ok(())
}

fn format_mode_3_warnings(warnings: Vec<String>) -> Option<String> {
    (!warnings.is_empty()).then(|| {
        format!(
            "Mode 3 uploaded the available screenshots and ignored upload failures: {}",
            warnings.join("; ")
        )
    })
}

fn fail_mode_3_batch_without_submission(
    app: &AppHandle,
    coordinator: &Arc<Mode3Coordinator>,
    message: String,
) -> Result<(), String> {
    let mut state = coordinator
        .state
        .lock()
        .map_err(|_| "Mode 3 coordinator state is unavailable.".to_string())?;
    reset_mode_3_state(&mut state);

    update_snapshot(app, |snapshot| {
        snapshot.is_running = false;
        snapshot.upload_state = UploadState::Failed;
        snapshot.last_error = Some(message.clone());
    });
    drop(state);
    Err(message)
}

fn reset_mode_3_state(state: &mut Mode3CoordinatorState) {
    state.finalizing = false;
    state.successful_injections = 0;
    state.upload_errors.clear();
    state.upload_errors.shrink_to_fit();
}

pub fn submit_after_upload(app: &AppHandle) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(app, &automation, AutomationMode::ScreenshotOnly, || {
        browser::submit_chatgpt_when_upload_ready(app)?;
        Ok(UploadState::Ready)
    })
    .map(|_| ())
    .map_err(|error| error.message)
}

fn run_workflow(
    app: &AppHandle,
    state: &State<'_, AutomationStore>,
    mode: AutomationMode,
    workflow: impl FnOnce() -> Result<UploadState, String>,
) -> CommandResult<AutomationSnapshot> {
    diagnostics::record(app, "INFO", "automation", &format!("started mode={mode:?}"));
    update_snapshot(app, |snapshot| {
        snapshot.is_running = true;
        snapshot.last_mode = Some(mode);
        snapshot.last_error = None;
    });

    match workflow() {
        Ok(upload_state) => {
            diagnostics::record(
                app,
                "INFO",
                "automation",
                &format!("completed mode={mode:?} upload_state={upload_state:?}"),
            );
            update_snapshot(app, |snapshot| {
                snapshot.is_running = false;
                snapshot.upload_state = upload_state;
            });
        }
        Err(message) if is_cancelled_error(&message) => {
            diagnostics::record(
                app,
                "WARN",
                "automation",
                &format!("cancelled mode={mode:?}: {message}"),
            );
            return Err(AutomationCommandError {
                code: "automation_cancelled",
                message,
            });
        }
        Err(message) => {
            diagnostics::record(
                app,
                "ERROR",
                "automation",
                &format!("failed mode={mode:?}: {message}"),
            );
            update_snapshot(app, |snapshot| {
                snapshot.is_running = false;
                snapshot.upload_state = UploadState::Failed;
                snapshot.last_error = Some(message.clone());
            });

            return Err(AutomationCommandError {
                code: "automation_failed",
                message,
            });
        }
    }

    Ok(state.snapshot()?.clone())
}

fn update_snapshot(app: &AppHandle, update: impl FnOnce(&mut AutomationSnapshot)) {
    let state = app.state::<AutomationStore>();
    let next_snapshot = match state.snapshot.lock() {
        Ok(mut snapshot) => {
            update(&mut snapshot);
            snapshot.clone()
        }
        Err(_) => return,
    };

    let _ = app.emit_to(MAIN_WINDOW_LABEL, AUTOMATION_EVENT, next_snapshot);
}

trait AutomationStoreExt {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, AutomationSnapshot>>;
}

impl AutomationStoreExt for AutomationStore {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, AutomationSnapshot>> {
        self.snapshot.lock().map_err(|_| AutomationCommandError {
            code: "state_unavailable",
            message: "Automation state is unavailable.".to_string(),
        })
    }
}

impl AutomationCommandError {
    fn automation_failed(message: String) -> Self {
        Self {
            code: "automation_failed",
            message,
        }
    }

    fn state_unavailable() -> Self {
        Self {
            code: "state_unavailable",
            message: "Automation state is unavailable.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{
            atomic::{AtomicBool, AtomicU64},
            Arc,
        },
        time::Duration,
    };

    use super::{
        finish_mode_3_job, merge_profile_prompt, merge_prompt_text, submit_caption_when_ready,
        try_reserve_caption_workflow_flag, CancellationToken, Mode3Coordinator, Mode3JobPermit,
    };

    #[test]
    fn caption_input_retries_twice_then_waits_for_submit() {
        let input_calls = Cell::new(0);
        let submit_calls = Cell::new(0);

        let result = submit_caption_when_ready(
            "caption",
            |_| {
                let call = input_calls.get() + 1;
                input_calls.set(call);
                if call < 3 {
                    Err("not ready".to_string())
                } else {
                    Ok(())
                }
            },
            || {
                submit_calls.set(submit_calls.get() + 1);
                Ok(())
            },
            Duration::ZERO,
        );

        assert!(result.is_ok());
        assert_eq!(input_calls.get(), 3);
        assert_eq!(submit_calls.get(), 1);
    }

    #[test]
    fn caption_text_is_restored_when_waiting_for_submit_fails() {
        let input_calls = Cell::new(0);

        let error = submit_caption_when_ready(
            "caption",
            |text| {
                assert_eq!(text, "caption");
                input_calls.set(input_calls.get() + 1);
                Ok(())
            },
            || Err("send remained disabled".to_string()),
            Duration::ZERO,
        )
        .expect_err("submission should fail");

        assert_eq!(input_calls.get(), 2);
        assert!(error.contains("text was left in the ChatGPT prompt"));
    }

    #[test]
    fn only_one_caption_workflow_permit_can_exist() {
        let reserved = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(0));
        let first = try_reserve_caption_workflow_flag(&reserved, &epoch).expect("first permit");

        assert!(try_reserve_caption_workflow_flag(&reserved, &epoch).is_none());
        drop(first);
        assert!(try_reserve_caption_workflow_flag(&reserved, &epoch).is_some());
    }

    #[test]
    fn last_mode_3_job_becomes_the_only_batch_finalizer() {
        let coordinator = Arc::new(Mode3Coordinator::default());
        let epoch = Arc::new(AtomicU64::new(0));
        {
            let mut state = coordinator.state.lock().expect("coordinator state");
            state.active_jobs = 2;
            state.generation = 2;
        }
        let first = Mode3JobPermit {
            coordinator: Arc::clone(&coordinator),
            token: super::CancellationToken {
                epoch: Arc::clone(&epoch),
                expected: 0,
            },
        };
        let second = Mode3JobPermit {
            coordinator: Arc::clone(&coordinator),
            token: super::CancellationToken { epoch, expected: 0 },
        };

        assert!(!finish_mode_3_job(&first, Err("capture failed".to_string()), true).unwrap());
        assert!(finish_mode_3_job(&second, Ok(()), true).unwrap());

        let state = coordinator.state.lock().expect("coordinator state");
        assert_eq!(state.active_jobs, 0);
        assert_eq!(state.successful_injections, 1);
        assert_eq!(state.upload_errors, vec!["capture failed"]);
        assert!(state.finalizing);
    }

    #[test]
    fn cancelled_mode_3_job_does_not_mutate_a_reset_batch() {
        let coordinator = Arc::new(Mode3Coordinator::default());
        let epoch = Arc::new(AtomicU64::new(1));
        {
            let mut state = coordinator.state.lock().expect("coordinator state");
            state.active_jobs = 1;
            state.generation = 8;
        }
        let cancelled_job = Mode3JobPermit {
            coordinator: Arc::clone(&coordinator),
            token: super::CancellationToken { epoch, expected: 0 },
        };

        assert!(!finish_mode_3_job(
            &cancelled_job,
            Err("navigation interrupted upload".to_string()),
            false,
        )
        .unwrap());

        let state = coordinator.state.lock().expect("coordinator state");
        assert_eq!(state.active_jobs, 1);
        assert_eq!(state.generation, 8);
        assert!(state.upload_errors.is_empty());
        assert!(!state.finalizing);
    }

    #[test]
    fn existing_prompt_is_combined_with_only_the_new_caption_batch() {
        assert_eq!(
            merge_prompt_text("Restored prompt", "New live caption"),
            "Restored prompt\nNew live caption"
        );
        assert_eq!(
            merge_prompt_text("Restored prompt\nNew live caption", "New live caption"),
            "Restored prompt\nNew live caption"
        );
    }

    #[test]
    fn active_profile_prompt_is_prefixed_without_duplication() {
        assert_eq!(
            merge_profile_prompt("Answer briefly", "Live caption"),
            "Answer briefly\nLive caption"
        );
        assert_eq!(
            merge_profile_prompt("Answer briefly", "Answer briefly\nLive caption"),
            "Answer briefly\nLive caption"
        );
    }

    #[test]
    fn changing_the_epoch_cancels_an_existing_worker_token() {
        let epoch = Arc::new(AtomicU64::new(4));
        let token = CancellationToken {
            epoch: Arc::clone(&epoch),
            expected: 4,
        };

        assert!(!token.is_cancelled());
        epoch.store(5, std::sync::atomic::Ordering::Release);
        assert!(token.is_cancelled());
    }
}
