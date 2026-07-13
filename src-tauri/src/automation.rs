use std::{
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{browser, captions, screenshot};

const MAIN_WINDOW_LABEL: &str = "main";
const AUTOMATION_EVENT: &str = "automation://state";

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
}

type CommandResult<T> = Result<T, AutomationCommandError>;

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
    let automation = app.state::<AutomationStore>();
    let captions = app.state::<captions::CaptionStore>();

    run_workflow(&app, &automation, AutomationMode::CaptionSubmit, || {
        let caption_text = captions::caption_text_for_submission(&captions)?;
        browser::insert_text_and_submit(&app, &caption_text)?;
        captions::mark_caption_submitted(&app, caption_text);
        Ok(UploadState::Idle)
    })
    .map(|_| ())
    .map_err(|error| error.message)
}

pub fn run_mode_2(app: &AppHandle) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();
    let captions = app.state::<captions::CaptionStore>();

    run_workflow(
        &app,
        &automation,
        AutomationMode::ScreenshotCaptionSubmit,
        || {
            let caption_text = captions::caption_text_for_submission(&captions)?;
            let masks = browser::protected_content_capture_mask(&app)
                .into_iter()
                .collect::<Vec<_>>();
            let screenshot = screenshot::capture_primary_display_png(&masks)?;
            browser::upload_screenshot_to_chatgpt_input(
                &app,
                &screenshot.file_name,
                &screenshot.bytes,
            )?;
            update_snapshot(&app, |snapshot| {
                snapshot.upload_state = UploadState::Uploading;
            });
            browser::wait_for_chatgpt_upload(&app, Duration::from_secs(45))?;
            update_snapshot(&app, |snapshot| {
                snapshot.upload_state = UploadState::Ready;
            });
            browser::insert_text_and_submit(&app, &caption_text)?;
            captions::mark_caption_submitted(&app, caption_text);
            Ok(UploadState::Ready)
        },
    )
    .map(|_| ())
    .map_err(|error| error.message)
}

pub fn run_mode_3(app: &AppHandle) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(&app, &automation, AutomationMode::ScreenshotOnly, || {
        let masks = browser::protected_content_capture_mask(&app)
            .into_iter()
            .collect::<Vec<_>>();
        let screenshot = screenshot::capture_primary_display_png(&masks)?;
        browser::upload_screenshot_to_chatgpt_input(
            &app,
            &screenshot.file_name,
            &screenshot.bytes,
        )?;
        Ok(UploadState::Uploading)
    })
    .map(|_| ())
    .map_err(|error| error.message)
}

pub fn submit_after_upload(app: &AppHandle) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(&app, &automation, AutomationMode::ScreenshotOnly, || {
        browser::submit_chatgpt_when_upload_ready(&app)?;
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
    update_snapshot(app, |snapshot| {
        snapshot.is_running = true;
        snapshot.last_mode = Some(mode);
        snapshot.last_error = None;
    });

    match workflow() {
        Ok(upload_state) => {
            update_snapshot(app, |snapshot| {
                snapshot.is_running = false;
                snapshot.upload_state = upload_state;
            });
        }
        Err(message) => {
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
}
