use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    sync::mpsc,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    App, AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalSize, Position, Rect,
    Size, State, Url, WebviewUrl, WebviewWindow, Window, WindowEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    WINDOW_DISPLAY_AFFINITY,
};

use crate::screenshot::CaptureMask;

const BROWSER_WEBVIEW_LABEL: &str = "chatgpt-browser";
const MAIN_WINDOW_LABEL: &str = "main";
const CHATGPT_HOME_URL: &str = "https://chatgpt.com/";
const TOOLBAR_HEIGHT: f64 = 64.0;
const MIN_TOOLBAR_HEIGHT: f64 = 40.0;
const MAX_TOOLBAR_HEIGHT: f64 = 420.0;
const SCRIPT_RESULT_TIMEOUT: Duration = Duration::from_secs(5);
const CONTENT_PROTECTION_LOG_TARGET: &str = "content-protection";
static CONTENT_PROTECTION_LOG_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    current_url: String,
    title: String,
    is_loading: bool,
    is_content_protected: bool,
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
    .auto_resize()
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
            resize_browser_to_window(&resize_app_handle);
        }
    })
    .on_download({
        let app_handle = app_handle.clone();
        move |_, event| handle_download_event(&app_handle, event)
    });

    let bounds = browser_bounds(&main_window)?;
    main_window
        .as_ref()
        .window()
        .add_child(browser, bounds.position, bounds.size)?;

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
    let trace_id = next_content_protection_trace_id();
    log_content_protection(
        trace_id,
        format_args!("browser_get_state start; syncing native affinity"),
    );

    if let Some(main_window) = app.get_window(MAIN_WINDOW_LABEL) {
        log_content_protection(
            trace_id,
            format_args!("browser_get_state main window found"),
        );
        match read_window_content_protection(trace_id, &main_window, MAIN_WINDOW_LABEL) {
            Ok(is_content_protected) => {
                log_content_protection(
                    trace_id,
                    format_args!(
                        "browser_get_state native affinity protected={is_content_protected}"
                    ),
                );
                update_snapshot(&app, |snapshot| {
                    snapshot.is_content_protected = is_content_protected;
                });
            }
            Err(error) => log_content_protection(
                trace_id,
                format_args!(
                    "browser_get_state native affinity read failed: {}",
                    error.message
                ),
            ),
        }
    } else {
        log_content_protection(
            trace_id,
            format_args!("browser_get_state main window missing"),
        );
    }

    let snapshot = state.snapshot()?.clone();
    log_content_protection(
        trace_id,
        format_args!(
            "browser_get_state return snapshot protected={} loading={} url={}",
            snapshot.is_content_protected, snapshot.is_loading, snapshot.current_url
        ),
    );
    Ok(snapshot)
}

#[tauri::command]
pub fn browser_open_home(
    app: AppHandle,
    state: State<'_, BrowserStore>,
) -> CommandResult<BrowserSnapshot> {
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
    let trace_id = next_content_protection_trace_id();
    let current_snapshot = state.snapshot()?.clone();
    log_content_protection(
        trace_id,
        format_args!(
            "command start requested={} current_snapshot_protected={} loading={} url={}",
            request.is_content_protected,
            current_snapshot.is_content_protected,
            current_snapshot.is_loading,
            current_snapshot.current_url
        ),
    );

    log_content_protection(
        trace_id,
        format_args!("looking up main window label={MAIN_WINDOW_LABEL}"),
    );
    let main_window = match app.get_window(MAIN_WINDOW_LABEL) {
        Some(window) => {
            log_content_protection(trace_id, format_args!("main window found"));
            window
        }
        None => {
            log_content_protection(trace_id, format_args!("main window missing"));
            return Err(BrowserCommandError {
                code: "window_unavailable",
                message: "Main window is not available.".to_string(),
            });
        }
    };

    log_content_protection(
        trace_id,
        format_args!(
            "browser webview exists={}",
            app.get_webview(BROWSER_WEBVIEW_LABEL).is_some()
        ),
    );

    log_window_geometry(trace_id, &main_window, MAIN_WINDOW_LABEL);

    match read_window_display_affinity(trace_id, &main_window, MAIN_WINDOW_LABEL) {
        Ok(affinity) => log_content_protection(
            trace_id,
            format_args!(
                "before apply hwnd_affinity={} protected={}",
                affinity.0,
                affinity != WDA_NONE
            ),
        ),
        Err(error) => log_content_protection(
            trace_id,
            format_args!("before apply affinity read failed: {}", error.message),
        ),
    }

    let is_content_protected = apply_window_content_protection(
        trace_id,
        &main_window,
        MAIN_WINDOW_LABEL,
        request.is_content_protected,
    )?;

    log_content_protection(
        trace_id,
        format_args!(
            "command completed requested={} actual={}",
            request.is_content_protected, is_content_protected
        ),
    );

    log_content_protection(trace_id, format_args!("updating snapshot"));
    update_snapshot(&app, |snapshot| {
        snapshot.is_content_protected = is_content_protected;
        snapshot.last_error = None;
    });
    let snapshot = state.snapshot()?.clone();
    log_content_protection(
        trace_id,
        format_args!(
            "return snapshot protected={} loading={} url={} last_error={:?}",
            snapshot.is_content_protected,
            snapshot.is_loading,
            snapshot.current_url,
            snapshot.last_error
        ),
    );

    Ok(snapshot)
}

fn apply_window_content_protection(
    trace_id: u64,
    window: &Window,
    label: &str,
    is_content_protected: bool,
) -> CommandResult<bool> {
    log_content_protection(
        trace_id,
        format_args!(
            "{label}: tauri set_content_protected({})",
            is_content_protected
        ),
    );
    if let Err(error) = window.set_content_protected(is_content_protected) {
        log_content_protection(
            trace_id,
            format_args!("{label}: tauri set_content_protected failed: {error}"),
        );
        return Err(BrowserCommandError::from_tauri(error));
    }

    let affinity = if is_content_protected {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };

    let hwnd = match window.hwnd() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            log_content_protection(trace_id, format_args!("{label}: hwnd read failed: {error}"));
            return Err(BrowserCommandError::from_tauri(error));
        }
    };
    log_content_protection(
        trace_id,
        format_args!(
            "{label}: win32 SetWindowDisplayAffinity hwnd={:?} affinity={}",
            hwnd, affinity.0
        ),
    );
    if let Err(error) = unsafe { SetWindowDisplayAffinity(hwnd, affinity) } {
        log_content_protection(
            trace_id,
            format_args!("{label}: win32 SetWindowDisplayAffinity failed: {error}"),
        );
        return Err(BrowserCommandError {
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
        });
    }

    match read_window_display_affinity(trace_id, window, label) {
        Ok(readback_affinity) => log_content_protection(
            trace_id,
            format_args!(
                "{label}: readback hwnd_affinity={} protected={}",
                readback_affinity.0,
                readback_affinity != WDA_NONE
            ),
        ),
        Err(error) => log_content_protection(
            trace_id,
            format_args!("{label}: readback affinity failed: {}", error.message),
        ),
    }

    Ok(is_content_protected)
}

fn read_window_content_protection(
    trace_id: u64,
    window: &Window,
    label: &str,
) -> CommandResult<bool> {
    Ok(read_window_display_affinity(trace_id, window, label)? != WDA_NONE)
}

fn read_window_display_affinity(
    trace_id: u64,
    window: &Window,
    label: &str,
) -> CommandResult<WINDOW_DISPLAY_AFFINITY> {
    log_content_protection(trace_id, format_args!("{label}: reading hwnd"));
    let hwnd = match window.hwnd() {
        Ok(hwnd) => {
            log_content_protection(trace_id, format_args!("{label}: hwnd={hwnd:?}"));
            hwnd
        }
        Err(error) => {
            log_content_protection(trace_id, format_args!("{label}: hwnd read failed: {error}"));
            return Err(BrowserCommandError::from_tauri(error));
        }
    };
    let mut affinity = WDA_NONE.0;

    log_content_protection(
        trace_id,
        format_args!("{label}: calling GetWindowDisplayAffinity"),
    );
    if let Err(error) = unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) } {
        log_content_protection(
            trace_id,
            format_args!("{label}: GetWindowDisplayAffinity failed: {error}"),
        );
        return Err(BrowserCommandError {
            code: "native_error",
            message: format!("Failed to read protected content mode: {error}"),
        });
    }
    log_content_protection(
        trace_id,
        format_args!("{label}: GetWindowDisplayAffinity returned affinity={affinity}"),
    );

    Ok(WINDOW_DISPLAY_AFFINITY(affinity))
}

fn next_content_protection_trace_id() -> u64 {
    CONTENT_PROTECTION_LOG_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn log_content_protection(trace_id: u64, args: std::fmt::Arguments<'_>) {
    eprintln!("[{CONTENT_PROTECTION_LOG_TARGET} #{trace_id}] {args}");
}

fn log_window_geometry(trace_id: u64, window: &Window, label: &str) {
    match window.scale_factor() {
        Ok(scale_factor) => log_content_protection(
            trace_id,
            format_args!("{label}: scale_factor={scale_factor}"),
        ),
        Err(error) => log_content_protection(
            trace_id,
            format_args!("{label}: scale_factor failed: {error}"),
        ),
    }

    match window.inner_position() {
        Ok(position) => log_content_protection(
            trace_id,
            format_args!("{label}: inner_position=({}, {})", position.x, position.y),
        ),
        Err(error) => log_content_protection(
            trace_id,
            format_args!("{label}: inner_position failed: {error}"),
        ),
    }

    match window.inner_size() {
        Ok(size) => log_content_protection(
            trace_id,
            format_args!("{label}: inner_size=({}, {})", size.width, size.height),
        ),
        Err(error) => log_content_protection(
            trace_id,
            format_args!("{label}: inner_size failed: {error}"),
        ),
    }
}

pub fn protected_content_capture_mask(app: &AppHandle) -> Option<CaptureMask> {
    let trace_id = next_content_protection_trace_id();
    log_content_protection(trace_id, format_args!("capture mask lookup start"));
    let state = app.state::<BrowserStore>();
    let is_content_protected = state
        .snapshot
        .lock()
        .ok()
        .map(|snapshot| snapshot.is_content_protected)?;

    if !is_content_protected {
        log_content_protection(
            trace_id,
            format_args!("capture mask disabled: snapshot protected=false"),
        );
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
    let height = inner_size.height as i32 - toolbar_height;

    if height <= 0 {
        log_content_protection(
            trace_id,
            format_args!("capture mask unavailable: browser height is not positive"),
        );
        return None;
    }

    let mask = CaptureMask {
        x: inner_position.x,
        y: inner_position.y + toolbar_height,
        width: inner_size.width as i32,
        height,
    };

    log_content_protection(trace_id, format_args!("capture mask active: {mask:?}"));
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
    let Some(main_window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
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

    if let Err(error) = browser.set_bounds(Rect {
        position: bounds.position,
        size: bounds.size,
    }) {
        update_snapshot(app, |snapshot| {
            snapshot.last_error = Some(format!("Failed to set browser WebView bounds: {error}"));
        });
        return;
    }

    if let Err(error) = browser.set_auto_resize(true) {
        update_snapshot(app, |snapshot| {
            snapshot.last_error = Some(format!("Failed to enable browser auto-resize: {error}"));
        });
    }
}

fn browser_bounds(window: &WebviewWindow) -> tauri::Result<Rect> {
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
    window: &WebviewWindow,
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
    let logical_browser_height = (logical_size.height - logical_toolbar_height).max(1.0);

    Ok(BrowserFitBounds {
        position: Position::Logical(LogicalPosition::new(0.0, logical_toolbar_height)),
        size: Size::Logical(LogicalSize::new(logical_size.width, logical_browser_height)),
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
}

pub fn copy_text_to_chatgpt_input(app: &AppHandle, text: &str) -> Result<(), String> {
    insert_text_to_chatgpt_input(app, text)
}

pub fn insert_text_and_submit(app: &AppHandle, text: &str) -> Result<(), String> {
    insert_text_to_chatgpt_input(app, text)?;
    submit_chatgpt_input(app)
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
    let started_at = std::time::Instant::now();

    while started_at.elapsed() < timeout {
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
  const text = document.body.innerText.toLowerCase();
  const busy = Array.from(document.querySelectorAll('[aria-busy="true"], [role="progressbar"], progress')).length > 0;
  const textSuggestsUploading = text.includes('uploading') || text.includes('attaching') || text.includes('processing image');
  const attachmentSelectors = [
    '[data-testid*="attachment"]',
    '[data-testid*="file"]',
    '[aria-label*="attachment" i]',
    '[aria-label*="image" i]',
    'img[src^="blob:"]'
  ];
  const hasAttachment = attachmentSelectors.some((selector) => document.querySelector(selector));

  return {
    isUploading: Boolean(busy || textSuggestsUploading),
    hasAttachment: Boolean(hasAttachment)
  };
})();
"#,
    )
}

fn insert_text_to_chatgpt_input(app: &AppHandle, text: &str) -> Result<(), String> {
    let browser = app.get_webview(BROWSER_WEBVIEW_LABEL).ok_or_else(|| {
        "Browser WebView is not available. Open ChatGPT before submitting captions.".to_string()
    })?;
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
    return;
  }}

  input.focus();

  if (input instanceof HTMLTextAreaElement) {{
    input.value = text;
    input.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: text }}));
    input.dispatchEvent(new Event('change', {{ bubbles: true }}));
    return;
  }}

  if (input.isContentEditable) {{
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(input);
    selection.removeAllRanges();
    selection.addRange(range);

    if (document.execCommand('insertText', false, text)) {{
      input.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: text }}));
      return;
    }}

    input.textContent = text;
    input.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: text }}));
  }}
}})();
"#
    );

    browser
        .eval(script)
        .map_err(|error| format!("Failed to copy captions into ChatGPT input: {error}"))
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
