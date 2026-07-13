use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{browser, captions, screenshot};

const MAIN_WINDOW_LABEL: &str = "main";
const AUTOMATION_EVENT: &str = "automation://state";
const CAPTION_INPUT_ATTEMPTS: usize = 3;
const CAPTION_RETRY_DELAY: Duration = Duration::from_millis(250);
const CHATGPT_UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);
const CHATGPT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(30);
const ATTACHMENT_DISCARD_TIMEOUT: Duration = Duration::from_secs(3);

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
}

pub(crate) struct CaptionWorkflowPermit {
    reserved: Arc<AtomicBool>,
}

impl Drop for CaptionWorkflowPermit {
    fn drop(&mut self) {
        self.reserved.store(false, Ordering::Release);
    }
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
    let permit = try_reserve_caption_workflow(app)?;
    run_mode_1_reserved(app, permit)
}

pub(crate) fn run_mode_1_reserved(
    app: &AppHandle,
    _permit: CaptionWorkflowPermit,
) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(&app, &automation, AutomationMode::CaptionSubmit, || {
        let caption_text = captions::take_caption_batch_for_hotkey(&app)?;
        submit_caption_when_ready(
            &caption_text,
            |text| browser::copy_text_to_chatgpt_input(&app, text),
            || browser::wait_and_submit_chatgpt_input(&app, CHATGPT_SUBMIT_TIMEOUT),
            CAPTION_RETRY_DELAY,
        )?;
        Ok(UploadState::Idle)
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
    _permit: CaptionWorkflowPermit,
) -> Result<(), String> {
    let automation = app.state::<AutomationStore>();

    run_workflow(
        &app,
        &automation,
        AutomationMode::ScreenshotCaptionSubmit,
        || {
            let caption_text = captions::take_caption_batch_for_hotkey(&app)?;
            let mut upload_was_injected = false;
            let upload_result = (|| {
                let masks = browser::protected_content_capture_mask(&app)
                    .into_iter()
                    .collect::<Vec<_>>();
                let screenshot = screenshot::capture_primary_display_png(&masks)?;
                browser::upload_screenshot_to_chatgpt_input(
                    &app,
                    &screenshot.file_name,
                    &screenshot.bytes,
                )?;
                drop(screenshot);
                upload_was_injected = true;
                update_snapshot(&app, |snapshot| {
                    snapshot.upload_state = UploadState::Uploading;
                });

                // Put the caption in the composer while the image uploads. A failed early insert
                // is harmless because the submit attempts below insert the same batch again.
                let _ = browser::copy_text_to_chatgpt_input(&app, &caption_text);
                browser::wait_for_chatgpt_upload(&app, CHATGPT_UPLOAD_TIMEOUT)?;
                update_snapshot(&app, |snapshot| {
                    snapshot.upload_state = UploadState::Ready;
                });
                Ok::<(), String>(())
            })();

            match upload_result {
                Ok(()) => {
                    submit_caption_when_ready(
                        &caption_text,
                        |text| browser::copy_text_to_chatgpt_input(&app, text),
                        || {
                            browser::wait_and_submit_chatgpt_input(
                                &app,
                                CHATGPT_SUBMIT_TIMEOUT,
                            )
                        },
                        CAPTION_RETRY_DELAY,
                    )?;
                    Ok(UploadState::Ready)
                }
                Err(upload_error) => {
                    update_snapshot(&app, |snapshot| {
                        snapshot.upload_state = UploadState::Failed;
                    });

                    if upload_was_injected {
                        if let Err(discard_error) = browser::discard_chatgpt_attachments(
                            &app,
                            ATTACHMENT_DISCARD_TIMEOUT,
                        ) {
                            let prompt_result = browser::copy_text_to_chatgpt_input(
                                &app,
                                &caption_text,
                            );
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

                    submit_caption_when_ready(
                        &caption_text,
                        |text| browser::copy_text_to_chatgpt_input(&app, text),
                        || {
                            browser::wait_and_submit_chatgpt_input(
                                &app,
                                CHATGPT_SUBMIT_TIMEOUT,
                            )
                        },
                        CAPTION_RETRY_DELAY,
                    )
                    .map_err(|submit_error| {
                        format!("Image upload failed: {upload_error}. {submit_error}")
                    })?;

                    update_snapshot(&app, |snapshot| {
                        snapshot.last_error = Some(format!(
                            "Image upload failed, so only the caption text was submitted: {upload_error}"
                        ));
                    });
                    Ok(UploadState::Failed)
                }
            }
        },
    )
    .map(|_| ())
    .map_err(|error| error.message)
}

pub(crate) fn try_reserve_caption_workflow(
    app: &AppHandle,
) -> Result<CaptionWorkflowPermit, String> {
    let state = app.state::<AutomationStore>();
    try_reserve_caption_workflow_flag(&state.caption_workflow_reserved).ok_or_else(|| {
        "Mode 1 or Mode 2 automation is already running. The new request was ignored.".to_string()
    })
}

fn try_reserve_caption_workflow_flag(reserved: &Arc<AtomicBool>) -> Option<CaptionWorkflowPermit> {
    reserved
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| CaptionWorkflowPermit {
            reserved: Arc::clone(reserved),
        })
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

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{atomic::AtomicBool, Arc},
        time::Duration,
    };

    use super::{submit_caption_when_ready, try_reserve_caption_workflow_flag};

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
        let first = try_reserve_caption_workflow_flag(&reserved).expect("first permit");

        assert!(try_reserve_caption_workflow_flag(&reserved).is_none());
        drop(first);
        assert!(try_reserve_caption_workflow_flag(&reserved).is_some());
    }
}
