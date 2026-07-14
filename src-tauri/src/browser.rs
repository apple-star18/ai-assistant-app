use std::{
    path::PathBuf,
    sync::mpsc,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{
    webview::{Color, DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    App, AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalSize, Position, Rect,
    Size, State, Url, WebviewUrl, Window, WindowEvent,
};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, GetWindowLongPtrW, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WINDOW_DISPLAY_AFFINITY, WS_EX_LAYERED,
};

use crate::{automation, captions, screenshot::CaptureMask};

const BROWSER_WEBVIEW_LABEL: &str = "chatgpt-browser";
const TRANSPARENCY_OVERLAY_WEBVIEW_LABEL: &str = "transparency-overlay";
const SETTINGS_OVERLAY_WEBVIEW_LABEL: &str = "settings-overlay";
const MAIN_WINDOW_LABEL: &str = "main";
const CHATGPT_HOME_URL: &str = "https://chatgpt.com/";
const TOOLBAR_HEIGHT: f64 = 48.0;
const WINDOW_CONTENT_INSET: f64 = 12.0;
const MIN_TOOLBAR_HEIGHT: f64 = 40.0;
const MAX_TOOLBAR_HEIGHT: f64 = 420.0;
const DEFAULT_WINDOW_OPACITY: f64 = 1.0;
const MIN_WINDOW_OPACITY: f64 = 0.4;
const MAX_WINDOW_OPACITY: f64 = 1.0;
const SCRIPT_RESULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    current_url: String,
    title: String,
    is_loading: bool,
    is_content_protected: bool,
    window_opacity: f64,
    last_download: Option<DownloadSnapshot>,
    last_error: Option<String>,
}

impl Default for BrowserSnapshot {
    fn default() -> Self {
        Self {
            current_url: CHATGPT_HOME_URL.to_string(),
            title: "ChatGPT".to_string(),
            is_loading: true,
            is_content_protected: false,
            window_opacity: DEFAULT_WINDOW_OPACITY,
            last_download: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSnapshot {
    url: String,
    destination: Option<String>,
    success: Option<bool>,
}

#[derive(Debug, Default)]
pub struct BrowserStore {
    snapshot: Mutex<BrowserSnapshot>,
    layout: Mutex<BrowserLayout>,
}

#[derive(Debug)]
struct BrowserLayout {
    toolbar_height: f64,
}

impl Default for BrowserLayout {
    fn default() -> Self {
        Self {
            toolbar_height: TOOLBAR_HEIGHT,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateRequest {
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeRequest {
    toolbar_height: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetContentProtectionRequest {
    is_content_protected: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetWindowOpacityRequest {
    opacity: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTransparencyOverlayRequest {
    is_open: bool,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    opacity_percent: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSettingsOverlayRequest {
    is_open: bool,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    indicator_left: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCommandError {
    code: &'static str,
    message: String,
}

type CommandResult<T> = Result<T, BrowserCommandError>;

pub fn setup(app: &mut App) -> tauri::Result<()> {
    let main_window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .expect("main window is configured");
    main_window.set_content_protected(false)?;
    if let Ok(hwnd) = main_window.hwnd() {
        let _ = unsafe { SetWindowDisplayAffinity(hwnd, WDA_NONE) };
    }
    let app_handle = app.handle().clone();
    let resize_app_handle = app.handle().clone();
    let profile_dir = browser_profile_dir(app)?;

    let browser = WebviewBuilder::new(
        BROWSER_WEBVIEW_LABEL,
        WebviewUrl::External(CHATGPT_HOME_URL.parse().expect("valid ChatGPT URL")),
    )
    .data_directory(profile_dir)
    .accept_first_mouse(true)
    .on_navigation(|url| is_allowed_navigation_url(url))
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .on_document_title_changed({
        let app_handle = app_handle.clone();
        move |_, title| {
            update_snapshot(&app_handle, |snapshot| {
                snapshot.title = sanitize_title(&title);
            });
        }
    })
    .on_page_load({
        let app_handle = app_handle.clone();
        let resize_app_handle = resize_app_handle.clone();
        move |_, payload| {
            let url = payload.url().to_string();
            let is_loading = matches!(payload.event(), PageLoadEvent::Started);
            update_snapshot(&app_handle, |snapshot| {
                snapshot.current_url = url;
                snapshot.is_loading = is_loading;
                snapshot.last_error = None;
            });
            if matches!(payload.event(), PageLoadEvent::Finished) {
                automation::restore_retained_prompt_after_page_load(&app_handle);
            }
            resize_browser_to_window(&resize_app_handle);
        }
    })
    .on_download({
        let app_handle = app_handle.clone();
        move |_, event| handle_download_event(&app_handle, event)
    });

    let bounds = browser_bounds(&main_window.as_ref().window())?;
    main_window
        .as_ref()
        .window()
        .add_child(browser, bounds.position, bounds.size)?;

    let transparency_overlay = WebviewBuilder::new(
        TRANSPARENCY_OVERLAY_WEBVIEW_LABEL,
        WebviewUrl::App("transparency-overlay.html".into()),
    )
    .accept_first_mouse(true)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .focused(false);

    let transparency_overlay = main_window.as_ref().window().add_child(
        transparency_overlay,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(1.0, 1.0),
    )?;
    transparency_overlay.hide()?;

    let settings_overlay = WebviewBuilder::new(
        SETTINGS_OVERLAY_WEBVIEW_LABEL,
        WebviewUrl::App("settings-overlay.html".into()),
    )
    .accept_first_mouse(true)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    .focused(false);

    let settings_overlay = main_window.as_ref().window().add_child(
        settings_overlay,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(1.0, 1.0),
    )?;
    settings_overlay.hide()?;

    main_window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            resize_browser_to_window_size(&app_handle, BrowserWindowSize::Physical(*size));
        }
        WindowEvent::ScaleFactorChanged { .. } => {
            resize_browser_to_window(&app_handle);
        }
        _ => {}
    });

    Ok(())
}

#[tauri::command]
pub fn browser_get_state(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
    if let Some(main_window) = app.get_window(MAIN_WINDOW_LABEL) {
        if let Ok(is_content_protected) = read_window_content_protection(&main_window) {
            update_snapshot(&app, |snapshot| {
                snapshot.is_content_protected = is_content_protected;
            });
        }
    }

    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_open_home(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
    automation::reset_for_home(&app).map_err(BrowserCommandError::automation)?;
    captions::reset_for_home(&app).map_err(BrowserCommandError::automation)?;
    let _ = clear_chatgpt_composer(&app, Duration::from_secs(3));
    navigate_to(&app, &state, CHATGPT_HOME_URL)
}

#[tauri::command]
pub fn browser_navigate(
    app: AppHandle,
    state: State<'_, BrowserStore>,
    request: NavigateRequest,
) -> CommandResult<BrowserSnapshot> {
    navigate_to(&app, &state, &request.url)
}

#[tauri::command]
pub fn browser_reload(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
    automation::prepare_for_refresh(&app).map_err(BrowserCommandError::automation)?;
    let browser = app.browser_webview()?;
    browser.reload().map_err(BrowserCommandError::from_tauri)?;

    update_snapshot(&app, |snapshot| {
        snapshot.is_loading = true;
        snapshot.last_error = None;
    });

    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_go_back(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
    run_fixed_browser_script(&app, "history.back();")?;
    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_go_forward(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
    run_fixed_browser_script(&app, "history.forward();")?;
    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_focus(app: AppHandle) -> CommandResult<()> {
    app.browser_webview()?
        .set_focus()
        .map_err(BrowserCommandError::from_tauri)
}

#[tauri::command]
pub fn browser_clear_session(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
    let browser = app.browser_webview()?;
    browser
        .clear_all_browsing_data()
        .map_err(BrowserCommandError::from_tauri)?;
    navigate_to(&app, &state, CHATGPT_HOME_URL)
}

#[tauri::command]
pub fn browser_resize(
    app: AppHandle,
    state: State<'_, BrowserStore>,
    request: ResizeRequest,
) -> CommandResult<BrowserSnapshot> {
    let toolbar_height = validate_toolbar_height(request.toolbar_height)?;
    {
        let mut layout = state.layout()?;
        layout.toolbar_height = toolbar_height;
    }

    resize_browser_to_window(&app);
    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_set_content_protected(
    app: AppHandle,
    state: State<'_, BrowserStore>,
    request: SetContentProtectionRequest,
) -> CommandResult<BrowserSnapshot> {
    let main_window = match app.get_window(MAIN_WINDOW_LABEL) {
        Some(window) => window,
        None => {
            return Err(BrowserCommandError {
                code: "window_unavailable",
                message: "Main window is not available.".to_string(),
            })
        }
    };

    let is_content_protected =
        apply_window_content_protection(&main_window, request.is_content_protected)?;

    update_snapshot(&app, |snapshot| {
        snapshot.is_content_protected = is_content_protected;
        snapshot.last_error = None;
    });

    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_set_window_opacity(
    app: AppHandle,
    state: State<'_, BrowserStore>,
    request: SetWindowOpacityRequest,
) -> CommandResult<BrowserSnapshot> {
    let opacity = validate_window_opacity(request.opacity)?;
    let main_window = match app.get_window(MAIN_WINDOW_LABEL) {
        Some(window) => window,
        None => {
            return Err(BrowserCommandError {
                code: "window_unavailable",
                message: "Main window is not available.".to_string(),
            })
        }
    };

    apply_window_opacity(&main_window, opacity)?;

    update_snapshot(&app, |snapshot| {
        snapshot.window_opacity = opacity;
        snapshot.last_error = None;
    });

    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn browser_set_transparency_overlay(
    app: AppHandle,
    request: SetTransparencyOverlayRequest,
) -> CommandResult<()> {
    let overlay = app
        .get_webview(TRANSPARENCY_OVERLAY_WEBVIEW_LABEL)
        .ok_or_else(|| BrowserCommandError {
            code: "webview_unavailable",
            message: "Transparency overlay WebView is not available.".to_string(),
        })?;

    if !request.is_open {
        overlay.hide().map_err(BrowserCommandError::from_tauri)?;
        return Ok(());
    }

    validate_overlay_bounds(&request)?;
    let opacity_percent = request.opacity_percent.clamp(40, 100);
    let position = Position::Logical(LogicalPosition::new(
        request.left.round(),
        request.top.round(),
    ));
    let size = Size::Logical(LogicalSize::new(
        request.width.round().max(1.0),
        request.height.round().max(1.0),
    ));

    overlay
        .set_auto_resize(false)
        .map_err(BrowserCommandError::from_tauri)?;
    overlay
        .set_bounds(Rect { position, size })
        .map_err(BrowserCommandError::from_tauri)?;
    overlay.show().map_err(BrowserCommandError::from_tauri)?;
    overlay
        .eval(format!(
            "window.setOpacityPercent && window.setOpacityPercent({opacity_percent});"
        ))
        .map_err(BrowserCommandError::from_tauri)?;

    Ok(())
}

#[tauri::command]
pub fn browser_set_settings_overlay(
    app: AppHandle,
    request: SetSettingsOverlayRequest,
) -> CommandResult<()> {
    let overlay = app
        .get_webview(SETTINGS_OVERLAY_WEBVIEW_LABEL)
        .ok_or_else(|| BrowserCommandError {
            code: "webview_unavailable",
            message: "Settings overlay WebView is not available.".to_string(),
        })?;

    if !request.is_open {
        overlay.hide().map_err(BrowserCommandError::from_tauri)?;
        let _ = app.emit("settings-overlay://closed", ());
        return Ok(());
    }

    validate_overlay_rect(
        request.left,
        request.top,
        request.width,
        request.height,
        "Settings overlay",
    )?;
    if !request.indicator_left.is_finite() {
        return Err(BrowserCommandError::validation(
            "Settings overlay indicator position must be a finite number.",
        ));
    }

    let indicator_left = request
        .indicator_left
        .round()
        .clamp(14.0, (request.width - 14.0).max(14.0));
    let position = Position::Logical(LogicalPosition::new(
        request.left.round(),
        request.top.round(),
    ));
    let size = Size::Logical(LogicalSize::new(
        request.width.round().max(1.0),
        request.height.round().max(1.0),
    ));

    overlay
        .set_auto_resize(false)
        .map_err(BrowserCommandError::from_tauri)?;
    overlay
        .set_bounds(Rect { position, size })
        .map_err(BrowserCommandError::from_tauri)?;
    overlay.show().map_err(BrowserCommandError::from_tauri)?;
    overlay
        .eval(format!(
            "window.setSettingsIndicatorLeft && window.setSettingsIndicatorLeft({indicator_left}); window.refreshSettings && window.refreshSettings();"
        ))
        .map_err(BrowserCommandError::from_tauri)?;
    overlay
        .set_focus()
        .map_err(BrowserCommandError::from_tauri)?;

    Ok(())
}

fn apply_window_content_protection(
    window: &Window,
    is_content_protected: bool,
) -> CommandResult<bool> {
    window
        .set_content_protected(is_content_protected)
        .map_err(BrowserCommandError::from_tauri)?;

    let affinity = if is_content_protected {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };

    let hwnd = window.hwnd().map_err(BrowserCommandError::from_tauri)?;
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) }.map_err(|error| BrowserCommandError {
        code: "native_error",
        message: format!(
            "Failed to {} protected content mode: {}",
            if is_content_protected {
                "enable"
            } else {
                "disable"
            },
            error
        ),
    })?;

    Ok(is_content_protected)
}

fn apply_window_opacity(window: &Window, opacity: f64) -> CommandResult<()> {
    let hwnd = window.hwnd().map_err(BrowserCommandError::from_tauri)?;
    let current_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let layered_style = current_style | WS_EX_LAYERED.0 as isize;
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, layered_style);
    }

    let alpha = (opacity * u8::MAX as f64).round() as u8;
    unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA) }.map_err(|error| {
        BrowserCommandError {
            code: "native_error",
            message: format!("Failed to update window transparency: {error}"),
        }
    })
}

fn read_window_content_protection(window: &Window) -> CommandResult<bool> {
    Ok(read_window_display_affinity(window)? != WDA_NONE)
}

fn read_window_display_affinity(window: &Window) -> CommandResult<WINDOW_DISPLAY_AFFINITY> {
    let hwnd = window.hwnd().map_err(BrowserCommandError::from_tauri)?;
    let mut affinity = WDA_NONE.0;

    unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.map_err(|error| {
        BrowserCommandError {
            code: "native_error",
            message: format!("Failed to read protected content mode: {error}"),
        }
    })?;

    Ok(WINDOW_DISPLAY_AFFINITY(affinity))
}

pub fn protected_content_capture_mask(app: &AppHandle) -> Option<CaptureMask> {
    let state = app.state::<BrowserStore>();
    let is_content_protected = state
        .snapshot
        .lock()
        .ok()
        .map(|snapshot| snapshot.is_content_protected)?;

    if !is_content_protected {
        return None;
    }

    let toolbar_height = state
        .layout
        .lock()
        .map(|layout| layout.toolbar_height)
        .unwrap_or(TOOLBAR_HEIGHT);
    let main_window = app.get_window(MAIN_WINDOW_LABEL)?;
    let scale_factor = main_window.scale_factor().ok()?;
    let inner_position = main_window.inner_position().ok()?;
    let inner_size = main_window.inner_size().ok()?;
    let toolbar_height = (toolbar_height * scale_factor).round() as i32;
    let content_inset = (WINDOW_CONTENT_INSET * scale_factor).round() as i32;
    let width = inner_size.width as i32 - content_inset * 2;
    let height = inner_size.height as i32 - toolbar_height - content_inset * 2;

    if width <= 0 || height <= 0 {
        return None;
    }

    let mask = CaptureMask {
        x: inner_position.x + content_inset,
        y: inner_position.y + toolbar_height + content_inset,
        width,
        height,
    };

    Some(mask)
}

fn navigate_to(
    app: &AppHandle,
    state: &State<'_, BrowserStore>,
    raw_url: &str,
) -> CommandResult<BrowserSnapshot> {
    let url = normalize_user_url(raw_url)?;
    let url_string = url.to_string();

    app.browser_webview()?
        .navigate(url)
        .map_err(BrowserCommandError::from_tauri)?;

    update_snapshot(app, |snapshot| {
        snapshot.current_url = url_string;
        snapshot.is_loading = true;
        snapshot.last_error = None;
    });

    Ok(state.snapshot()?.clone())
}

fn run_fixed_browser_script(app: &AppHandle, script: &'static str) -> CommandResult<()> {
    app.browser_webview()?
        .eval(script)
        .map_err(BrowserCommandError::from_tauri)
}

fn handle_download_event(app: &AppHandle, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => {
            if !is_allowed_navigation_url(&url) {
                update_snapshot(app, |snapshot| {
                    snapshot.last_error =
                        Some("Blocked download from an unsupported URL.".to_string());
                });
                return false;
            }

            update_snapshot(app, |snapshot| {
                snapshot.last_download = Some(DownloadSnapshot {
                    url: url.to_string(),
                    destination: Some(destination.display().to_string()),
                    success: None,
                });
            });

            true
        }
        DownloadEvent::Finished { url, path, success } => {
            update_snapshot(app, |snapshot| {
                snapshot.last_download = Some(DownloadSnapshot {
                    url: url.to_string(),
                    destination: path.map(|path| path.display().to_string()),
                    success: Some(success),
                });
            });

            true
        }
        _ => true,
    }
}

fn update_snapshot(app: &AppHandle, update: impl FnOnce(&mut BrowserSnapshot)) {
    let state = app.state::<BrowserStore>();
    let next_snapshot = match state.snapshot.lock() {
        Ok(mut snapshot) => {
            update(&mut snapshot);
            snapshot.clone()
        }
        Err(_) => return,
    };

    let _ = app.emit_to(MAIN_WINDOW_LABEL, "browser://state", next_snapshot);
}

fn normalize_user_url(raw_url: &str) -> CommandResult<Url> {
    let trimmed = raw_url.trim();

    if trimmed.is_empty() {
        return Err(BrowserCommandError::validation("URL is required."));
    }

    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    let url = Url::parse(&candidate)
        .map_err(|_| BrowserCommandError::validation("Enter a valid HTTPS URL."))?;

    if !is_allowed_navigation_url(&url) {
        return Err(BrowserCommandError::validation(
            "Only HTTPS URLs are allowed. Local HTTP is allowed for development.",
        ));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(BrowserCommandError::validation(
            "URLs with embedded credentials are not allowed.",
        ));
    }

    Ok(url)
}

fn is_allowed_navigation_url(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1")),
        _ => false,
    }
}

fn resize_browser_to_window(app: &AppHandle) {
    resize_browser_to_window_size(app, BrowserWindowSize::Current);
}

fn resize_browser_to_window_size(app: &AppHandle, window_size: BrowserWindowSize) {
    let Some(main_window) = app.get_window(MAIN_WINDOW_LABEL) else {
        return;
    };

    let Some(browser) = app.get_webview(BROWSER_WEBVIEW_LABEL) else {
        return;
    };

    let toolbar_height = app
        .state::<BrowserStore>()
        .layout
        .lock()
        .map(|layout| layout.toolbar_height)
        .unwrap_or(TOOLBAR_HEIGHT);
    let bounds = match browser_fit_bounds(&main_window, toolbar_height, window_size) {
        Ok(bounds) => bounds,
        Err(error) => {
            update_snapshot(app, |snapshot| {
                snapshot.last_error = Some(format!("Failed to read browser window size: {error}"));
            });
            return;
        }
    };

    if let Err(error) = browser.set_auto_resize(false) {
        update_snapshot(app, |snapshot| {
            snapshot.last_error = Some(format!("Failed to disable browser auto-resize: {error}"));
        });
        return;
    }

    if let Err(error) = browser.set_bounds(Rect {
        position: bounds.position,
        size: bounds.size,
    }) {
        update_snapshot(app, |snapshot| {
            snapshot.last_error = Some(format!("Failed to set browser WebView bounds: {error}"));
        });
        return;
    }
}

fn browser_bounds(window: &Window) -> tauri::Result<Rect> {
    let bounds = browser_fit_bounds(window, TOOLBAR_HEIGHT, BrowserWindowSize::Current)?;

    Ok(Rect {
        position: bounds.position,
        size: bounds.size,
    })
}

#[derive(Debug)]
struct BrowserFitBounds {
    position: Position,
    size: Size,
}

#[derive(Debug, Clone, Copy)]
enum BrowserWindowSize {
    Current,
    Physical(PhysicalSize<u32>),
}

fn browser_fit_bounds(
    window: &Window,
    toolbar_height: f64,
    window_size: BrowserWindowSize,
) -> tauri::Result<BrowserFitBounds> {
    let scale_factor = window.scale_factor()?;
    let logical_size = match window_size {
        BrowserWindowSize::Current => {
            let physical_size = window.inner_size()?;
            physical_size.to_logical::<f64>(scale_factor)
        }
        BrowserWindowSize::Physical(physical_size) => physical_size.to_logical::<f64>(scale_factor),
    };
    let logical_toolbar_height = toolbar_height.round().max(1.0);
    let logical_browser_width = (logical_size.width - WINDOW_CONTENT_INSET * 2.0).max(1.0);
    let logical_browser_height =
        (logical_size.height - logical_toolbar_height - WINDOW_CONTENT_INSET * 2.0).max(1.0);

    Ok(BrowserFitBounds {
        position: Position::Logical(LogicalPosition::new(
            WINDOW_CONTENT_INSET,
            logical_toolbar_height + WINDOW_CONTENT_INSET,
        )),
        size: Size::Logical(LogicalSize::new(
            logical_browser_width,
            logical_browser_height,
        )),
    })
}

fn validate_toolbar_height(toolbar_height: f64) -> CommandResult<f64> {
    if !toolbar_height.is_finite() {
        return Err(BrowserCommandError::validation(
            "Toolbar height must be a finite number.",
        ));
    }

    if !(MIN_TOOLBAR_HEIGHT..=MAX_TOOLBAR_HEIGHT).contains(&toolbar_height) {
        return Err(BrowserCommandError::validation(
            "Toolbar height is outside the allowed resize range.",
        ));
    }

    Ok(toolbar_height)
}

fn validate_window_opacity(opacity: f64) -> CommandResult<f64> {
    if !opacity.is_finite() {
        return Err(BrowserCommandError::validation(
            "Window opacity must be a finite number.",
        ));
    }

    if !(MIN_WINDOW_OPACITY..=MAX_WINDOW_OPACITY).contains(&opacity) {
        return Err(BrowserCommandError::validation(
            "Window opacity is outside the allowed range.",
        ));
    }

    Ok(opacity)
}

fn validate_overlay_bounds(request: &SetTransparencyOverlayRequest) -> CommandResult<()> {
    validate_overlay_rect(
        request.left,
        request.top,
        request.width,
        request.height,
        "Transparency overlay",
    )
}

fn validate_overlay_rect(
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    label: &str,
) -> CommandResult<()> {
    if !left.is_finite() || !top.is_finite() || !width.is_finite() || !height.is_finite() {
        return Err(BrowserCommandError::validation(&format!(
            "{label} bounds must be finite numbers."
        )));
    }

    if width <= 0.0 || height <= 0.0 {
        return Err(BrowserCommandError::validation(&format!(
            "{label} size must be positive."
        )));
    }

    Ok(())
}

fn browser_profile_dir(app: &App) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("browser-profile"))
}

fn sanitize_title(title: &str) -> String {
    let title = title.trim();

    if title.is_empty() {
        "ChatGPT".to_string()
    } else {
        title.chars().take(120).collect()
    }
}

trait BrowserStoreExt {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, BrowserSnapshot>>;
    fn layout(&self) -> CommandResult<MutexGuard<'_, BrowserLayout>>;
}

impl BrowserStoreExt for BrowserStore {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, BrowserSnapshot>> {
        self.snapshot.lock().map_err(|_| BrowserCommandError {
            code: "state_unavailable",
            message: "Browser state is unavailable.".to_string(),
        })
    }

    fn layout(&self) -> CommandResult<MutexGuard<'_, BrowserLayout>> {
        self.layout.lock().map_err(|_| BrowserCommandError {
            code: "state_unavailable",
            message: "Browser layout state is unavailable.".to_string(),
        })
    }
}

trait BrowserAppExt {
    fn browser_webview(&self) -> CommandResult<tauri::Webview>;
}

impl BrowserAppExt for AppHandle {
    fn browser_webview(&self) -> CommandResult<tauri::Webview> {
        self.get_webview(BROWSER_WEBVIEW_LABEL)
            .ok_or_else(|| BrowserCommandError {
                code: "browser_unavailable",
                message: "Browser WebView is not available.".to_string(),
            })
    }
}

impl BrowserCommandError {
    fn validation(message: &str) -> Self {
        Self {
            code: "validation_error",
            message: message.to_string(),
        }
    }

    fn from_tauri(error: tauri::Error) -> Self {
        Self {
            code: "native_error",
            message: error.to_string(),
        }
    }

    fn automation(message: String) -> Self {
        Self {
            code: "automation_reset_failed",
            message,
        }
    }
}

pub fn copy_text_to_chatgpt_input(app: &AppHandle, text: &str) -> Result<(), String> {
    insert_text_to_chatgpt_input(app, text)
}

pub fn read_chatgpt_prompt_text(app: &AppHandle) -> Result<String, String> {
    let result = eval_json(
        app,
        r#"
(() => {
  const input = document.querySelector('#prompt-textarea, textarea[data-testid="prompt-textarea"], div[contenteditable="true"][data-testid="prompt-textarea"]');

  if (!input) {
    return { ok: false, reason: 'input_not_found' };
  }

  const text = input instanceof HTMLTextAreaElement ? input.value : (input.innerText || input.textContent || '');
  return { ok: true, text };
})();
"#,
    )?;

    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(result
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string())
    } else {
        Err(format!(
            "ChatGPT prompt was not available: {}",
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ))
    }
}

pub fn upload_screenshot_to_chatgpt_input(
    app: &AppHandle,
    file_name: &str,
    png_bytes: &[u8],
) -> Result<(), String> {
    let encoded_name = serde_json::to_string(file_name)
        .map_err(|error| format!("Failed to encode screenshot file name: {error}"))?;
    let encoded_png = serde_json::to_string(&BASE64_STANDARD.encode(png_bytes))
        .map_err(|error| format!("Failed to encode screenshot bytes: {error}"))?;
    let script = format!(
        r#"
(() => {{
  const fileName = {encoded_name};
  const base64 = {encoded_png};
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);

  for (let index = 0; index < binary.length; index += 1) {{
    bytes[index] = binary.charCodeAt(index);
  }}

  const file = new File([bytes], fileName, {{ type: 'image/png' }});
  const inputs = Array.from(document.querySelectorAll('input[type="file"]'));
  const input = inputs.find((candidate) => {{
    const accept = (candidate.getAttribute('accept') || '').toLowerCase();
    return !accept || accept.includes('image') || accept.includes('png') || accept.includes('*');
  }}) || inputs[0];

  if (!input) {{
    return {{ ok: false, reason: 'file_input_not_found' }};
  }}

  const transfer = new DataTransfer();
  transfer.items.add(file);
  input.files = transfer.files;
  input.dispatchEvent(new Event('input', {{ bubbles: true }}));
  input.dispatchEvent(new Event('change', {{ bubbles: true }}));
  window.__aiAssistantUploadStartedAt = Date.now();

  return {{ ok: true }};
}})();
"#
    );
    let result = eval_json(app, script)?;

    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "ChatGPT file input was not available: {}",
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ))
    }
}

pub fn wait_for_chatgpt_upload(app: &AppHandle, timeout: Duration) -> Result<(), String> {
    wait_for_chatgpt_upload_cancellable(app, timeout, || false)
}

pub fn wait_for_chatgpt_upload_cancellable(
    app: &AppHandle,
    timeout: Duration,
    is_cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    let started_at = std::time::Instant::now();

    while started_at.elapsed() < timeout {
        if is_cancelled() {
            return Err(automation::cancelled_error());
        }
        let state = chatgpt_upload_state(app)?;
        let is_uploading = state
            .get("isUploading")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_attachment = state
            .get("hasAttachment")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if has_attachment && !is_uploading {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    Err("Timed out waiting for ChatGPT upload to finish.".to_string())
}

pub fn discard_chatgpt_attachments(app: &AppHandle, timeout: Duration) -> Result<(), String> {
    let started_at = std::time::Instant::now();

    while started_at.elapsed() < timeout {
        let result = eval_json(
            app,
            r#"
(() => {
  const prompt = document.querySelector('#prompt-textarea, textarea[data-testid="prompt-textarea"], div[contenteditable="true"][data-testid="prompt-textarea"]');
  const composer = prompt?.closest('form') || document;
  const attachmentSelectors = [
    '[data-testid="composer-file-preview"]',
    '[data-testid*="attachment"]',
    '[data-testid*="file-preview"]',
    '[data-testid*="image-preview"]',
    'button[aria-label*="remove file" i]',
    'button[aria-label*="remove attachment" i]',
    'button[aria-label*="remove image" i]',
    'img[src^="blob:"]'
  ];
  const removeSelectors = [
    'button[aria-label*="remove file" i]',
    'button[aria-label*="remove attachment" i]',
    'button[aria-label*="remove image" i]',
    'button[data-testid*="remove"][data-testid*="attachment"]',
    'button[data-testid*="remove"][data-testid*="file"]',
    'button[data-testid*="remove"][data-testid*="image"]'
  ];
  const attachments = attachmentSelectors.flatMap((selector) => Array.from(composer.querySelectorAll(selector)));
  const removeButtons = [...new Set(removeSelectors.flatMap((selector) => Array.from(composer.querySelectorAll(selector))))];

  removeButtons.forEach((button) => button.click());

  return { attachmentCount: attachments.length, removeButtonCount: removeButtons.length };
})();
"#,
        )?;
        let attachment_count = result
            .get("attachmentCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        if attachment_count == 0 {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    Err("Timed out removing the failed ChatGPT image attachment.".to_string())
}

pub fn clear_chatgpt_composer(app: &AppHandle, timeout: Duration) -> Result<(), String> {
    let started_at = std::time::Instant::now();

    while started_at.elapsed() < timeout {
        let result = eval_json(
            app,
            r#"
(() => {
  const prompt = document.querySelector('#prompt-textarea, textarea[data-testid="prompt-textarea"], div[contenteditable="true"][data-testid="prompt-textarea"]');

  if (!prompt) {
    return { ok: false, reason: 'input_not_found' };
  }

  const composer = prompt.closest('form') || document;
  const removeSelectors = [
    'button[aria-label*="remove file" i]',
    'button[aria-label*="remove attachment" i]',
    'button[aria-label*="remove image" i]',
    'button[data-testid*="remove"][data-testid*="attachment"]',
    'button[data-testid*="remove"][data-testid*="file"]',
    'button[data-testid*="remove"][data-testid*="image"]'
  ];
  const attachmentSelectors = [
    '[data-testid="composer-file-preview"]',
    '[data-testid*="attachment"]',
    '[data-testid*="file-preview"]',
    '[data-testid*="image-preview"]',
    'img[src^="blob:"]'
  ];
  const removeButtons = [...new Set(removeSelectors.flatMap((selector) => Array.from(composer.querySelectorAll(selector))))];

  removeButtons.forEach((button) => button.click());
  prompt.focus();

  if (prompt instanceof HTMLTextAreaElement) {
    prompt.value = '';
    prompt.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward', data: null }));
    prompt.dispatchEvent(new Event('change', { bubbles: true }));
  } else if (prompt.isContentEditable) {
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(prompt);
    selection.removeAllRanges();
    selection.addRange(range);
    document.execCommand('delete', false);
    prompt.textContent = '';
    prompt.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward', data: null }));
  }

  const attachments = [...new Set(attachmentSelectors.flatMap((selector) => Array.from(composer.querySelectorAll(selector))))];
  const promptText = prompt instanceof HTMLTextAreaElement ? prompt.value : (prompt.textContent || '');

  return {
    ok: true,
    attachmentCount: attachments.length,
    hasText: promptText.trim().length > 0
  };
})();
"#,
        )?;

        if result.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(format!(
                "ChatGPT composer was not available: {}",
                result
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }

        let attachment_count = result
            .get("attachmentCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let has_text = result
            .get("hasText")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if attachment_count == 0 && !has_text {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    Err("Timed out clearing the ChatGPT composer.".to_string())
}

pub fn submit_chatgpt_when_upload_ready(app: &AppHandle) -> Result<(), String> {
    wait_for_chatgpt_upload(app, Duration::from_secs(45))?;
    submit_chatgpt_input(app)
}

pub fn submit_chatgpt_input(app: &AppHandle) -> Result<(), String> {
    let script = r#"
(() => {
  const selectors = [
    'button[data-testid="send-button"]',
    'button[aria-label="Send prompt"]',
    'button[aria-label="Send message"]',
    'form button[type="submit"]'
  ];
  const button = selectors.map((selector) => document.querySelector(selector)).find(Boolean);

  if (button && !button.disabled && button.getAttribute('aria-disabled') !== 'true') {
    button.click();
    return { ok: true, method: 'button' };
  }

  const input = document.querySelector('#prompt-textarea, textarea[data-testid="prompt-textarea"], div[contenteditable="true"][data-testid="prompt-textarea"], div[contenteditable="true"], textarea');

  if (!input) {
    return { ok: false, reason: 'input_not_found' };
  }

  input.focus();
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true }));
  input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true }));

  return { ok: true, method: 'keyboard' };
})();
"#;
    let result = eval_json(app, script)?;

    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "ChatGPT submit control was not available: {}",
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ))
    }
}

fn chatgpt_upload_state(app: &AppHandle) -> Result<Value, String> {
    eval_json(
        app,
        r#"
(() => {
  const prompt = document.querySelector('#prompt-textarea, textarea[data-testid="prompt-textarea"], div[contenteditable="true"][data-testid="prompt-textarea"]');
  const composer = prompt?.closest('form') || document;
  const text = composer.innerText.toLowerCase();
  const busy = Array.from(composer.querySelectorAll('[aria-busy="true"], [role="progressbar"], progress')).length > 0;
  const textSuggestsUploading = text.includes('uploading') || text.includes('attaching') || text.includes('processing image');
  const attachmentSelectors = [
    '[data-testid="composer-file-preview"]',
    '[data-testid*="attachment"]',
    '[data-testid*="file-preview"]',
    '[data-testid*="image-preview"]',
    'button[aria-label*="remove file" i]',
    'button[aria-label*="remove attachment" i]',
    'button[aria-label*="remove image" i]',
    'img[src^="blob:"]'
  ];
  const hasAttachment = attachmentSelectors.some((selector) => composer.querySelector(selector));

  return {
    isUploading: Boolean(busy || textSuggestsUploading),
    hasAttachment: Boolean(hasAttachment)
  };
})();
"#,
    )
}

fn insert_text_to_chatgpt_input(app: &AppHandle, text: &str) -> Result<(), String> {
    if app.get_webview(BROWSER_WEBVIEW_LABEL).is_none() {
        return Err(
            "Browser WebView is not available. Open ChatGPT before submitting captions."
                .to_string(),
        );
    }
    let encoded_text = serde_json::to_string(text)
        .map_err(|error| format!("Failed to encode caption text: {error}"))?;
    let script = format!(
        r#"
(() => {{
  const text = {encoded_text};
  const selectors = [
    '#prompt-textarea',
    'textarea[data-testid="prompt-textarea"]',
    'div[contenteditable="true"][data-testid="prompt-textarea"]',
    'div[contenteditable="true"]',
    'textarea'
  ];
  const input = selectors.map((selector) => document.querySelector(selector)).find(Boolean);

  if (!input) {{
    return {{ ok: false, reason: 'input_not_found' }};
  }}

  input.focus();

  if (input instanceof HTMLTextAreaElement) {{
    input.value = text;
    input.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: text }}));
    input.dispatchEvent(new Event('change', {{ bubbles: true }}));
    return {{ ok: true }};
  }}

  if (input.isContentEditable) {{
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(input);
    selection.removeAllRanges();
    selection.addRange(range);

    if (document.execCommand('insertText', false, text)) {{
      input.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: text }}));
      return {{ ok: true }};
    }}

    input.textContent = text;
    input.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: text }}));
    return {{ ok: true }};
  }}

  return {{ ok: false, reason: 'input_not_editable' }};
}})();
"#
    );
    let result = eval_json(app, script)?;

    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "ChatGPT prompt was not available: {}",
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ))
    }
}

pub fn wait_and_submit_chatgpt_input_cancellable(
    app: &AppHandle,
    timeout: Duration,
    is_cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    if app.get_webview(BROWSER_WEBVIEW_LABEL).is_none() {
        return Err("Browser WebView is not available.".to_string());
    }

    let started_at = std::time::Instant::now();
    let mut last_reason = "send_button_not_found".to_string();

    while started_at.elapsed() < timeout {
        if is_cancelled() {
            return Err(automation::cancelled_error());
        }
        let result = eval_json(
            app,
            r#"
(() => {
  const selectors = [
    'button[data-testid="send-button"]',
    'button[aria-label="Send prompt"]',
    'button[aria-label="Send message"]',
    'form button[type="submit"]'
  ];
  const button = selectors.map((selector) => document.querySelector(selector)).find(Boolean);

  if (!button) {
    return { submitted: false, reason: 'send_button_not_found' };
  }

  if (button.disabled || button.getAttribute('aria-disabled') === 'true') {
    return { submitted: false, reason: 'send_button_disabled' };
  }

  button.click();
  return { submitted: true };
})();
"#,
        )?;

        if result.get("submitted").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }

        last_reason = result
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        std::thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Timed out waiting for the ChatGPT send button to become enabled: {last_reason}"
    ))
}

pub fn submit_chatgpt_input_if_ready(app: &AppHandle) -> Result<bool, String> {
    if app.get_webview(BROWSER_WEBVIEW_LABEL).is_none() {
        return Err("Browser WebView is not available.".to_string());
    }

    let result = eval_json(
        app,
        r#"
(() => {
  const selectors = [
    'button[data-testid="send-button"]',
    'button[aria-label="Send prompt"]',
    'button[aria-label="Send message"]',
    'form button[type="submit"]'
  ];
  const button = selectors.map((selector) => document.querySelector(selector)).find(Boolean);

  if (!button || button.disabled || button.getAttribute('aria-disabled') === 'true') {
    return { submitted: false };
  }

  button.click();
  return { submitted: true };
})();
"#,
    )?;

    Ok(result.get("submitted").and_then(Value::as_bool) == Some(true))
}

fn eval_json(app: &AppHandle, script: impl Into<String>) -> Result<Value, String> {
    let browser = app
        .get_webview(BROWSER_WEBVIEW_LABEL)
        .ok_or_else(|| "Browser WebView is not available.".to_string())?;
    let (tx, rx) = mpsc::channel();

    browser
        .eval_with_callback(script, move |result| {
            let _ = tx.send(result);
        })
        .map_err(|error| format!("Failed to evaluate ChatGPT automation script: {error}"))?;

    let result = rx
        .recv_timeout(SCRIPT_RESULT_TIMEOUT)
        .map_err(|_| "Timed out waiting for ChatGPT automation script result.".to_string())?;

    match serde_json::from_str(&result) {
        Ok(value) => Ok(value),
        Err(_) => Ok(Value::String(result)),
    }
}
