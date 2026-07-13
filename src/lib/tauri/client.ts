import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type {
  AutomationState,
  BrowserState,
  CaptionState,
  CommandMap,
  HotkeyBindingRequest,
  HotkeyState,
} from './contracts';

type CommandName = keyof CommandMap;
type CommandArgs<TCommand extends CommandName> = CommandMap[TCommand]['args'];
type CommandResponse<TCommand extends CommandName> = CommandMap[TCommand]['response'];

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

export function resizeBrowser(toolbarHeight: number) {
  return invokeCommand('browser_resize', {
    request: { toolbarHeight },
  });
}

export function setBrowserContentProtected(isContentProtected: boolean) {
  return invokeCommand('browser_set_content_protected', {
    request: { isContentProtected },
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

async function invokeCommand<TCommand extends CommandName>(
  command: TCommand,
  ...args: CommandArgs<TCommand> extends undefined ? [] : [CommandArgs<TCommand>]
): Promise<CommandResponse<TCommand>> {
  const payload = (args[0] ?? {}) as Record<string, unknown>;

  return invoke<CommandResponse<TCommand>>(command, payload);
}
