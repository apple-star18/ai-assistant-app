import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

import type {
  AutomationState,
  BrowserProfileOverlayRequest,
  BrowserSettingsOverlayRequest,
  BrowserState,
  BrowserTransparencyOverlayRequest,
  CaptionState,
  CommandMap,
  HotkeyBindingRequest,
  HotkeyState,
  ProfileState,
} from './contracts';

type CommandName = keyof CommandMap;
type CommandArgs<TCommand extends CommandName> = CommandMap[TCommand]['args'];
type CommandResponse<TCommand extends CommandName> = CommandMap[TCommand]['response'];

const mainWindow = getCurrentWindow();

export type WindowResizeDirection =
  'East' | 'North' | 'NorthEast' | 'NorthWest' | 'South' | 'SouthEast' | 'SouthWest' | 'West';

export function minimizeMainWindow() {
  return mainWindow.minimize();
}

export function closeMainWindow() {
  return mainWindow.close();
}

export function startMainWindowDragging() {
  return mainWindow.startDragging();
}

export function startMainWindowResizeDragging(direction: WindowResizeDirection) {
  return mainWindow.startResizeDragging(direction);
}

export function getAppHealth() {
  return invokeCommand('get_app_health');
}

export function getBrowserState() {
  return invokeCommand('browser_get_state');
}

export function openBrowserHome() {
  return invokeCommand('browser_open_home');
}

export function navigateBrowser(url: string) {
  return invokeCommand('browser_navigate', { request: { url } });
}

export function reloadBrowser() {
  return invokeCommand('browser_reload');
}

export function goBrowserBack() {
  return invokeCommand('browser_go_back');
}

export function goBrowserForward() {
  return invokeCommand('browser_go_forward');
}

export function focusBrowser() {
  return invokeCommand('browser_focus');
}

export function clearBrowserSession() {
  return invokeCommand('browser_clear_session');
}

export function resizeBrowser(toolbarHeight: number, statusBarHeight: number) {
  return invokeCommand('browser_resize', {
    request: { toolbarHeight, statusBarHeight },
  });
}

export function setBrowserContentProtected(isContentProtected: boolean) {
  return invokeCommand('browser_set_content_protected', {
    request: { isContentProtected },
  });
}

export function setBrowserWindowOpacity(opacity: number) {
  return invokeCommand('browser_set_window_opacity', {
    request: { opacity },
  });
}

export function setBrowserTransparencyOverlay(request: BrowserTransparencyOverlayRequest) {
  return invokeCommand('browser_set_transparency_overlay', {
    request,
  });
}

export function setBrowserSettingsOverlay(request: BrowserSettingsOverlayRequest) {
  return invokeCommand('browser_set_settings_overlay', {
    request,
  });
}

export function setBrowserProfileOverlay(request: BrowserProfileOverlayRequest) {
  return invokeCommand('browser_set_profile_overlay', {
    request,
  });
}

export function getCaptionState() {
  return invokeCommand('captions_get_state');
}

export function startCaptions() {
  return invokeCommand('captions_start');
}

export function stopCaptions() {
  return invokeCommand('captions_stop');
}

export function clearCaptions() {
  return invokeCommand('captions_clear');
}

export function submitCaptionsToChatGpt() {
  return invokeCommand('captions_submit_to_chatgpt');
}

export function getAutomationState() {
  return invokeCommand('automation_get_state');
}

export function runShortcutMode1() {
  return invokeCommand('automation_shortcut_mode_1');
}

export function runShortcutMode2() {
  return invokeCommand('automation_shortcut_mode_2');
}

export function runShortcutMode3() {
  return invokeCommand('automation_shortcut_mode_3');
}

export function submitAfterUpload() {
  return invokeCommand('automation_submit_after_upload');
}

export function getHotkeyState() {
  return invokeCommand('hotkeys_get_state');
}

export function getProfileState() {
  return invokeCommand('profiles_get_state');
}

export function activateProfile(id: number) {
  return invokeCommand('profiles_activate', { request: { id } });
}

export function applyHotkeySettings(bindings: HotkeyBindingRequest[]) {
  return invokeCommand('hotkeys_apply_settings', { request: { bindings } });
}

export function listenToBrowserState(onState: (state: BrowserState) => void) {
  return listen<BrowserState>('browser://state', (event) => {
    onState(event.payload);
  });
}

export function listenToCaptionState(onState: (state: CaptionState) => void) {
  return listen<CaptionState>('captions://state', (event) => {
    onState(event.payload);
  });
}

export function listenToAutomationState(onState: (state: AutomationState) => void) {
  return listen<AutomationState>('automation://state', (event) => {
    onState(event.payload);
  });
}

export function listenToHotkeyState(onState: (state: HotkeyState) => void) {
  return listen<HotkeyState>('hotkeys://state', (event) => {
    onState(event.payload);
  });
}

export function listenToProfileState(onState: (state: ProfileState) => void) {
  return listen<ProfileState>('profiles://state', (event) => {
    onState(event.payload);
  });
}

export function listenToSettingsOverlayClosed(onClose: () => void) {
  return listen('settings-overlay://closed', () => {
    onClose();
  });
}

export function listenToProfileOverlayClosed(onClose: () => void) {
  return listen('profile-overlay://closed', () => {
    onClose();
  });
}

export function listenToTransparencyOverlayClosed(onClose: () => void) {
  return listen('transparency-overlay://closed', () => {
    onClose();
  });
}

async function invokeCommand<TCommand extends CommandName>(
  command: TCommand,
  ...args: CommandArgs<TCommand> extends undefined ? [] : [CommandArgs<TCommand>]
): Promise<CommandResponse<TCommand>> {
  const payload = (args[0] ?? {}) as Record<string, unknown>;

  return invoke<CommandResponse<TCommand>>(command, payload);
}
