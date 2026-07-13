import { FormEvent, useEffect, useMemo, useRef, useState } from 'react';

import {
  applyHotkeySettings,
  debugBrowserLayout,
  getAutomationState,
  focusBrowser,
  getBrowserState,
  getCaptionState,
  getHotkeyState,
  goBrowserBack,
  listenToAutomationState,
  listenToBrowserState,
  listenToCaptionState,
  listenToHotkeyState,
  navigateBrowser,
  openBrowserHome,
  reloadBrowser,
  resizeBrowser,
  setBrowserContentProtected,
  setBrowserWindowOpacity,
  startCaptions,
  stopCaptions,
  submitCaptionsToChatGpt,
} from './lib/tauri/client';
import type {
  AutomationState,
  BrowserDebugLayoutRequest,
  BrowserDebugRect,
  BrowserState,
  CaptionState,
  HotkeyBindingRequest,
} from './lib/tauri/contracts';
import type { HotkeyState } from './lib/tauri/contracts';

type HotkeyAction = HotkeyBindingRequest['action'];

const shortcutFields: Array<{
  action: HotkeyAction;
  label: string;
  defaultAccelerator: string;
}> = [
  {
    action: 'shortcutMode1',
    label: 'Mode 1',
    defaultAccelerator: 'Ctrl+Alt+1',
  },
  {
    action: 'shortcutMode2',
    label: 'Mode 2',
    defaultAccelerator: 'Ctrl+Alt+2',
  },
  {
    action: 'shortcutMode3',
    label: 'Mode 3',
    defaultAccelerator: 'Ctrl+Alt+3',
  },
];

const defaultShortcutDraft = shortcutFields.reduce(
  (draft, field) => {
    draft[field.action] = field.defaultAccelerator;
    return draft;
  },
  {} as Record<HotkeyAction, string>,
);

const initialBrowserState: BrowserState = {
  currentUrl: 'https://chatgpt.com/',
  title: 'ChatGPT',
  isLoading: true,
  isContentProtected: false,
  windowOpacity: 1,
  lastDownload: null,
  lastError: null,
};

const initialCaptionState: CaptionState = {
  isMonitoring: false,
  windowFound: false,
  textElementFound: false,
  launchAttempted: false,
  currentCaptionText: '',
  lastSubmittedCaptionText: '',
  pendingCaptionText: '',
  latestCaption: '',
  captionBuffer: [],
  lastError: null,
};

const initialAutomationState: AutomationState = {
  isRunning: false,
  lastMode: null,
  uploadState: 'idle',
  lastError: null,
};

const initialHotkeyState: HotkeyState = {
  isRunning: false,
  bindings: [],
  lastError: null,
};

const DEBUG_LOG_PREFIX = '[ai-assistant-browser]';
const TRANSPARENCY_CONTROL_WIDTH = 220;
const TRANSPARENCY_CONTROL_MARGIN = 8;

export function App() {
  return <BrowserWindow />;
}

function BrowserWindow() {
  const toolbarRef = useRef<HTMLDivElement | null>(null);
  const transparencyButtonRef = useRef<HTMLButtonElement | null>(null);
  const [browserState, setBrowserState] = useState<BrowserState>(initialBrowserState);
  const [captionState, setCaptionState] = useState<CaptionState>(initialCaptionState);
  const [automationState, setAutomationState] = useState<AutomationState>(initialAutomationState);
  const [hotkeyState, setHotkeyState] = useState<HotkeyState>(initialHotkeyState);
  const [address, setAddress] = useState(initialBrowserState.currentUrl);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isTransparencyOpen, setIsTransparencyOpen] = useState(false);
  const [transparencyControlLeft, setTransparencyControlLeft] = useState(TRANSPARENCY_CONTROL_MARGIN);
  const [shortcutDraft, setShortcutDraft] =
    useState<Record<HotkeyAction, string>>(defaultShortcutDraft);

  useEffect(() => {
    let isMounted = true;

    void getBrowserState()
      .then((state) => {
        if (isMounted) {
          setBrowserState(state);
          setAddress(state.currentUrl);
        }
      })
      .catch((error: unknown) => {
        setCommandError(getErrorMessage(error));
      });

    void getCaptionState()
      .then((state) => {
        if (isMounted) {
          setCaptionState(state);
        }
      })
      .catch((error: unknown) => {
        setCommandError(getErrorMessage(error));
      });

    void getAutomationState()
      .then((state) => {
        if (isMounted) {
          setAutomationState(state);
        }
      })
      .catch((error: unknown) => {
        setCommandError(getErrorMessage(error));
      });

    void getHotkeyState()
      .then((state) => {
        if (isMounted) {
          setHotkeyState(state);
        }
      })
      .catch((error: unknown) => {
        setCommandError(getErrorMessage(error));
      });

    const unlisten = listenToBrowserState((state) => {
      setBrowserState(state);
      setAddress(state.currentUrl);
      setCommandError(null);
    });

    const unlistenCaptions = listenToCaptionState((state) => {
      setCaptionState(state);
    });

    const unlistenAutomation = listenToAutomationState((state) => {
      setAutomationState(state);
      if (state.lastError) {
        setCommandError(state.lastError);
      }
    });

    const unlistenHotkeys = listenToHotkeyState((state) => {
      setHotkeyState(state);
    });

    return () => {
      isMounted = false;
      void unlisten.then((dispose) => {
        dispose();
      });
      void unlistenCaptions.then((dispose) => {
        dispose();
      });
      void unlistenAutomation.then((dispose) => {
        dispose();
      });
      void unlistenHotkeys.then((dispose) => {
        dispose();
      });
    };
  }, []);

  useEffect(() => {
    if (!isSettingsOpen) {
      setShortcutDraft(shortcutDraftFromHotkeys(hotkeyState));
    }
  }, [hotkeyState, isSettingsOpen]);

  useEffect(() => {
    if (!isTransparencyOpen) {
      return;
    }

    const logTransparencyMetrics = () => {
      updateTransparencyControlPosition();
      logElementMetrics('transparency:top-layer-metrics', '.browser-top-layer');
      logElementMetrics('transparency:button-metrics', '#transparency-button');
      logElementMetrics('transparency:popover-row-metrics', '.transparency-popover-row');
      logElementMetrics('transparency:control-metrics', '#transparency-controls');
      logElementMetrics('transparency:range-metrics', '#transparency-opacity-range');
      void sendBrowserDebugLayout('transparency:metrics');
    };

    logTransparencyMetrics();
    const firstFrame = window.requestAnimationFrame(() => {
      logTransparencyMetrics();
    });
    const settleTimer = window.setTimeout(logTransparencyMetrics, 150);

    window.addEventListener('resize', logTransparencyMetrics);
    window.visualViewport?.addEventListener('resize', logTransparencyMetrics);

    return () => {
      window.cancelAnimationFrame(firstFrame);
      window.clearTimeout(settleTimer);
      window.removeEventListener('resize', logTransparencyMetrics);
      window.visualViewport?.removeEventListener('resize', logTransparencyMetrics);
    };
  }, [isTransparencyOpen]);

  useEffect(() => {
    let animationFrame: number | null = null;
    const settleTimers: number[] = [];
    let isDisposed = false;

    const syncBrowserBounds = () => {
      if (animationFrame !== null) {
        window.cancelAnimationFrame(animationFrame);
      }

      animationFrame = window.requestAnimationFrame(() => {
        animationFrame = null;

        if (isDisposed || !toolbarRef.current) {
          return;
        }

        const toolbarHeight = Math.ceil(toolbarRef.current.getBoundingClientRect().height);
        logDebug('resize:measured-top-layer', {
          toolbarHeight,
          isSettingsOpen,
          isTransparencyOpen,
        });

        void resizeBrowser(toolbarHeight)
          .then((state) => {
            if (!isDisposed) {
              logDebug('resize:complete', {
                toolbarHeight,
                windowOpacity: state.windowOpacity,
              });
              setBrowserState(state);
            }
          })
          .catch((error: unknown) => {
            if (!isDisposed) {
              logDebug('resize:failed', {
                toolbarHeight,
                error: getErrorMessage(error),
              });
              setCommandError(getErrorMessage(error));
            }
          });
      });
    };

    const syncBrowserBoundsUntilSettled = () => {
      while (settleTimers.length > 0) {
        const timer = settleTimers.pop();

        if (timer !== undefined) {
          window.clearTimeout(timer);
        }
      }

      syncBrowserBounds();

      for (const delay of [50, 150, 300, 600]) {
        const timer = window.setTimeout(syncBrowserBounds, delay);
        settleTimers.push(timer);
      }
    };

    const observer = new ResizeObserver(syncBrowserBounds);

    if (toolbarRef.current) {
      observer.observe(toolbarRef.current);
    }

    window.addEventListener('resize', syncBrowserBoundsUntilSettled);
    window.visualViewport?.addEventListener('resize', syncBrowserBoundsUntilSettled);
    syncBrowserBoundsUntilSettled();

    return () => {
      isDisposed = true;
      observer.disconnect();
      window.removeEventListener('resize', syncBrowserBoundsUntilSettled);
      window.visualViewport?.removeEventListener('resize', syncBrowserBoundsUntilSettled);

      for (const timer of settleTimers) {
        window.clearTimeout(timer);
      }

      if (animationFrame !== null) {
        window.cancelAnimationFrame(animationFrame);
      }
    };
  }, [isSettingsOpen, isTransparencyOpen]);

  const statusText = useMemo(() => {
    if (browserState.isLoading) {
      return 'Loading';
    }

    if (browserState.lastDownload?.success === true) {
      return 'Download complete';
    }

    if (browserState.lastDownload?.success === false) {
      return 'Download failed';
    }

    return 'Ready';
  }, [browserState.isLoading, browserState.lastDownload]);

  const statusMessage = useMemo(
    () =>
      commandError ??
      hotkeyState.lastError ??
      automationState.lastError ??
      captionState.lastError ??
      browserState.lastError ??
      (automationState.isRunning
        ? 'Automation running'
        : captionState.pendingCaptionText || captionState.latestCaption || statusText),
    [
      automationState.isRunning,
      automationState.lastError,
      browserState.lastError,
      captionState.lastError,
      captionState.latestCaption,
      captionState.pendingCaptionText,
      commandError,
      hotkeyState.lastError,
      statusText,
    ],
  );

  async function runBrowserCommand(command: () => Promise<BrowserState | undefined>) {
    setCommandError(null);

    try {
      const nextState = await command();

      if (nextState) {
        setBrowserState(nextState);
        setAddress(nextState.currentUrl);
      }

      await focusBrowser();
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  function handleNavigate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void runBrowserCommand(() => navigateBrowser(address));
  }

  async function toggleCaptions() {
    setCommandError(null);

    try {
      const nextState = captionState.isMonitoring ? await stopCaptions() : await startCaptions();
      setCaptionState(nextState);
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  async function submitCaptions() {
    setCommandError(null);

    try {
      const nextState = await submitCaptionsToChatGpt();
      setCaptionState(nextState);
      await focusBrowser();
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  async function toggleContentProtection() {
    const requestedContentProtected = !browserState.isContentProtected;
    setCommandError(null);

    try {
      const nextState = await setBrowserContentProtected(requestedContentProtected);
      setBrowserState(nextState);
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  function toggleTransparencyControls() {
    setIsTransparencyOpen((isOpen) => {
      const nextIsOpen = !isOpen;
      logDebug('transparency:toggle', {
        isOpen: nextIsOpen,
        zIndex: getTopLayerZIndex(),
      });
      return nextIsOpen;
    });
    scheduleBrowserResizeFromTopLayer();
  }

  async function updateWindowOpacity(opacityPercent: number) {
    const opacity = opacityPercent / 100;
    setCommandError(null);
    logDebug('transparency:opacity-requested', { opacityPercent, opacity });
    setBrowserState((current) => ({
      ...current,
      windowOpacity: opacity,
    }));

    try {
      const nextState = await setBrowserWindowOpacity(opacity);
      logDebug('transparency:opacity-applied', {
        opacityPercent: Math.round(nextState.windowOpacity * 100),
        opacity: nextState.windowOpacity,
      });
      setBrowserState(nextState);
    } catch (error) {
      logDebug('transparency:opacity-failed', { error: getErrorMessage(error) });
      setCommandError(getErrorMessage(error));
    }
  }

  function openSettings() {
    setCommandError(null);
    setShortcutDraft(shortcutDraftFromHotkeys(hotkeyState));
    setIsSettingsOpen(true);
    scheduleBrowserResizeFromTopLayer();
  }

  function closeSettings() {
    setIsSettingsOpen(false);
    scheduleBrowserResizeFromTopLayer();
  }

  function scheduleBrowserResizeFromTopLayer() {
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => {
        if (toolbarRef.current) {
          updateTransparencyControlPosition();
          const topHeight = Math.ceil(toolbarRef.current.getBoundingClientRect().height);
          logDebug('resize:scheduled-top-layer', {
            topHeight,
            isSettingsOpen,
            isTransparencyOpen,
          });
          void sendBrowserDebugLayout('resize:scheduled-top-layer');
          void resizeBrowserToTopHeight(topHeight);
        }
      });
    });
  }

  function updateShortcutDraft(action: HotkeyAction, accelerator: string) {
    setShortcutDraft((current) => ({
      ...current,
      [action]: accelerator,
    }));
  }

  async function applyShortcutSettings(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setCommandError(null);

    try {
      const nextState = await applyHotkeySettings(
        shortcutFields.map((field) => ({
          action: field.action,
          accelerator: shortcutDraft[field.action].trim(),
        })),
      );

      setHotkeyState(nextState);
      closeSettings();
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  async function resizeBrowserToTopHeight(topHeight: number) {
    try {
      const nextState = await resizeBrowser(topHeight);
      logDebug('resize:scheduled-complete', { topHeight });
      setBrowserState(nextState);
    } catch (error) {
      logDebug('resize:scheduled-failed', { topHeight, error: getErrorMessage(error) });
      setCommandError(getErrorMessage(error));
    }
  }

  function updateTransparencyControlPosition() {
    if (!toolbarRef.current || !transparencyButtonRef.current) {
      return;
    }

    const topLayerRect = toolbarRef.current.getBoundingClientRect();
    const buttonRect = transparencyButtonRef.current.getBoundingClientRect();
    const buttonCenter = buttonRect.left - topLayerRect.left + buttonRect.width / 2;
    const maxLeft = Math.max(
      TRANSPARENCY_CONTROL_MARGIN,
      topLayerRect.width - TRANSPARENCY_CONTROL_WIDTH - TRANSPARENCY_CONTROL_MARGIN,
    );
    const nextLeft = Math.round(
      Math.min(
        maxLeft,
        Math.max(TRANSPARENCY_CONTROL_MARGIN, buttonCenter - TRANSPARENCY_CONTROL_WIDTH / 2),
      ),
    );

    logDebug('transparency:position', {
      buttonCenter: Math.round(buttonCenter),
      topLayerWidth: Math.round(topLayerRect.width),
      controlLeft: nextLeft,
      maxLeft: Math.round(maxLeft),
    });
    setTransparencyControlLeft(nextLeft);
  }

  const opacityPercent = Math.round(browserState.windowOpacity * 100);

  return (
    <main className="browser-shell" aria-label="ChatGPT browser">
      <div ref={toolbarRef} className="browser-top-layer">
        <form className="browser-toolbar" onSubmit={handleNavigate}>
          <div className="window-title">
            <span className="app-mark" aria-hidden="true">
              AI
            </span>
            <span>{browserState.title}</span>
          </div>

          <div className="navigation-controls" aria-label="Navigation controls">
            <button
              type="button"
              title="Back"
              onClick={() => void runBrowserCommand(goBrowserBack)}
            >
              &#8592;
            </button>
            <button
              type="button"
              title="Refresh"
              onClick={() => void runBrowserCommand(reloadBrowser)}
            >
              &#8635;
            </button>
            <button
              type="button"
              title="Home"
              onClick={() => void runBrowserCommand(openBrowserHome)}
            >
              &#8962;
            </button>
          </div>

          <button className="settings-button" type="button" title="Settings" onClick={openSettings}>
            &#9881;
          </button>

          <button
            className={
              browserState.isContentProtected ? 'protection-button protected' : 'protection-button'
            }
            type="button"
            title={
              browserState.isContentProtected
                ? 'Content is hidden from screen capture.'
                : 'Content is visible to screen capture.'
            }
            aria-pressed={browserState.isContentProtected}
            onClick={() => void toggleContentProtection()}
          >
            {browserState.isContentProtected ? <EyeOffIcon /> : <EyeIcon />}
          </button>

          <button
            id="transparency-button"
            ref={transparencyButtonRef}
            className={
              browserState.windowOpacity < 1
                ? 'transparency-button adjusted'
                : 'transparency-button'
            }
            type="button"
            title="Transparency"
            aria-expanded={isTransparencyOpen}
            aria-controls="transparency-controls"
            onClick={toggleTransparencyControls}
          >
            <TransparencyIcon />
          </button>

          <label className="address-bar">
            <span className="visually-hidden">URL</span>
            <input
              value={address}
              inputMode="url"
              spellCheck={false}
              autoCapitalize="none"
              onChange={(event) => setAddress(event.target.value)}
              onFocus={(event) => event.currentTarget.select()}
            />
          </label>

          <button
            className="caption-button"
            type="button"
            title={captionState.isMonitoring ? 'Stop captions' : 'Start captions'}
            onClick={() => void toggleCaptions()}
          >
            <CaptionsIcon />
          </button>

          <button
            className="caption-submit-button"
            type="button"
            title="Send captions"
            disabled={!captionState.pendingCaptionText && !captionState.currentCaptionText}
            onClick={() => void submitCaptions()}
          >
            <SendIcon />
          </button>

          <div className="browser-status" role="status">
            <span
              className={
                browserState.isLoading || automationState.isRunning
                  ? 'status-dot loading'
                  : 'status-dot'
              }
            />
            <span>{statusMessage}</span>
          </div>
        </form>

        {isTransparencyOpen ? (
          <div className="transparency-popover-row">
            <label
              id="transparency-controls"
              className="transparency-popover-control"
              style={{ left: `${transparencyControlLeft}px` }}
              aria-label="Transparency controls"
            >
              <span>Opacity</span>
              <input
                id="transparency-opacity-range"
                type="range"
                min="40"
                max="100"
                step="5"
                value={opacityPercent}
                onChange={(event) => void updateWindowOpacity(Number(event.currentTarget.value))}
              />
              <output>{opacityPercent}%</output>
            </label>
          </div>
        ) : null}

        {isSettingsOpen ? (
          <div className="settings-modal-region">
            <form
              className="settings-dialog in-window"
              aria-modal="true"
              aria-labelledby="settings-title"
              role="dialog"
              onSubmit={(event) => void applyShortcutSettings(event)}
            >
              <div className="settings-dialog-header">
                <h2 id="settings-title">Settings</h2>
                <button type="button" title="Close settings" onClick={closeSettings}>
                  X
                </button>
              </div>

              <div className="shortcut-settings">
                {shortcutFields.map((field) => {
                  const binding = hotkeyState.bindings.find(
                    (candidate) => candidate.action === field.action,
                  );

                  return (
                    <label className="shortcut-field" key={field.action}>
                      <span>{field.label}</span>
                      <input
                        value={shortcutDraft[field.action]}
                        spellCheck={false}
                        autoCapitalize="none"
                        onChange={(event) =>
                          updateShortcutDraft(field.action, event.currentTarget.value)
                        }
                      />
                      <span
                        className={binding?.error ? 'shortcut-status error' : 'shortcut-status'}
                      >
                        {binding?.error ?? (binding?.registered ? 'Registered' : 'Not registered')}
                      </span>
                    </label>
                  );
                })}
              </div>

              <div className="settings-actions">
                <button type="button" onClick={closeSettings}>
                  Cancel
                </button>
                <button className="settings-apply-button" type="submit">
                  Apply
                </button>
              </div>
            </form>
          </div>
        ) : null}
      </div>
    </main>
  );
}

function EyeIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="M2.5 12s3.5-6 9.5-6 9.5 6 9.5 6-3.5 6-9.5 6-9.5-6-9.5-6Z" />
      <circle cx="12" cy="12" r="2.75" />
    </svg>
  );
}

function EyeOffIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="M3 3l18 18" />
      <path d="M8.6 5.5A10.5 10.5 0 0 1 12 5c6 0 9.5 7 9.5 7a18 18 0 0 1-2.4 3.2" />
      <path d="M15.2 18.1A10.5 10.5 0 0 1 12 18c-6 0-9.5-6-9.5-6a17 17 0 0 1 3.1-3.7" />
      <path d="M10 10.2a2.75 2.75 0 0 0 3.8 3.8" />
    </svg>
  );
}

function TransparencyIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="M12 3.5 7 10a6 6 0 1 0 10 0L12 3.5Z" />
      <path d="M8.8 15.5a4.4 4.4 0 0 0 6.4 0" />
    </svg>
  );
}

function CaptionsIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <rect x="3.5" y="5.5" width="17" height="13" rx="2.5" />
      <path d="M7.5 10.5h3" />
      <path d="M13.5 10.5h3" />
      <path d="M7.5 14h4" />
      <path d="M13.5 14h3" />
    </svg>
  );
}

function SendIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="M4 12 20 4l-5 16-3.2-6.8L4 12Z" />
      <path d="m11.8 13.2 3.6-3.6" />
    </svg>
  );
}

function shortcutDraftFromHotkeys(hotkeyState: HotkeyState): Record<HotkeyAction, string> {
  const draft = { ...defaultShortcutDraft };

  for (const binding of hotkeyState.bindings) {
    draft[binding.action] = binding.accelerator;
  }

  return draft;
}

function getErrorMessage(error: unknown) {
  if (typeof error === 'string') {
    return error;
  }

  if (error && typeof error === 'object' && 'message' in error) {
    return String(error.message);
  }

  return 'The browser command failed.';
}

function getTopLayerZIndex() {
  const topLayer = document.querySelector('.browser-top-layer');

  if (!(topLayer instanceof HTMLElement)) {
    return 'unavailable';
  }

  return window.getComputedStyle(topLayer).zIndex;
}

function logElementMetrics(event: string, selector: string) {
  const element = document.querySelector(selector);

  if (!(element instanceof HTMLElement)) {
    logDebug(event, { selector, found: false });
    return;
  }

  const rect = element.getBoundingClientRect();
  const style = window.getComputedStyle(element);

  logDebug(event, {
    selector,
    found: true,
    rect: {
      x: Math.round(rect.x),
      y: Math.round(rect.y),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
      top: Math.round(rect.top),
      bottom: Math.round(rect.bottom),
    },
    display: style.display,
    visibility: style.visibility,
    opacity: style.opacity,
    zIndex: style.zIndex,
    color: style.color,
    backgroundColor: style.backgroundColor,
    pointerEvents: style.pointerEvents,
  });
}

async function sendBrowserDebugLayout(source: string) {
  const request = buildBrowserDebugLayoutRequest(source);
  logDebug('debug-layout:send', {
    source,
    topHeight: request.frontend.topHeight,
    topLayerRect: request.frontend.topLayerRect,
    transparencyRowRect: request.frontend.transparencyRowRect,
    transparencyControlRect: request.frontend.transparencyControlRect,
    transparencyRangeRect: request.frontend.transparencyRangeRect,
  });

  try {
    await debugBrowserLayout(request);
  } catch (error) {
    logDebug('debug-layout:failed', { source, error: getErrorMessage(error) });
  }
}

function buildBrowserDebugLayoutRequest(source: string): BrowserDebugLayoutRequest {
  const topLayer = document.querySelector('.browser-top-layer');
  const transparencyControl = document.querySelector('#transparency-controls');

  return {
    source,
    frontend: {
      isTransparencyOpen: Boolean(document.querySelector('#transparency-controls')),
      topHeight:
        topLayer instanceof HTMLElement
          ? Math.ceil(topLayer.getBoundingClientRect().height)
          : null,
      topLayerRect: getDebugRect('.browser-top-layer'),
      transparencyButtonRect: getDebugRect('#transparency-button'),
      transparencyRowRect: getDebugRect('.transparency-popover-row'),
      transparencyControlRect: getDebugRect('#transparency-controls'),
      transparencyRangeRect: getDebugRect('#transparency-opacity-range'),
      topLayerZIndex:
        topLayer instanceof HTMLElement ? window.getComputedStyle(topLayer).zIndex : 'unavailable',
      transparencyControlZIndex:
        transparencyControl instanceof HTMLElement
          ? window.getComputedStyle(transparencyControl).zIndex
          : 'unavailable',
    },
  };
}

function getDebugRect(selector: string): BrowserDebugRect | null {
  const element = document.querySelector(selector);

  if (!(element instanceof HTMLElement)) {
    return null;
  }

  const rect = element.getBoundingClientRect();

  return {
    x: Math.round(rect.x),
    y: Math.round(rect.y),
    width: Math.round(rect.width),
    height: Math.round(rect.height),
    top: Math.round(rect.top),
    bottom: Math.round(rect.bottom),
  };
}

function logDebug(event: string, details: Record<string, unknown>) {
  console.info(DEBUG_LOG_PREFIX, event, details);
}
