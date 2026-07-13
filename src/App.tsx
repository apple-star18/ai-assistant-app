import { FormEvent, useEffect, useMemo, useRef, useState } from 'react';

import {
  applyHotkeySettings,
  clearBrowserSession,
  getAutomationState,
  focusBrowser,
  getBrowserState,
  getCaptionState,
  getHotkeyState,
  goBrowserBack,
  goBrowserForward,
  listenToAutomationState,
  listenToBrowserState,
  listenToCaptionState,
  listenToHotkeyState,
  navigateBrowser,
  openBrowserHome,
  reloadBrowser,
  resizeBrowser,
  runShortcutMode1,
  runShortcutMode2,
  runShortcutMode3,
  setBrowserContentProtected,
  startCaptions,
  stopCaptions,
  submitAfterUpload,
  submitCaptionsToChatGpt,
} from './lib/tauri/client';
import type {
  AutomationState,
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

export function App() {
  return <BrowserWindow />;
}

function BrowserWindow() {
  const toolbarRef = useRef<HTMLDivElement | null>(null);
  const [browserState, setBrowserState] = useState<BrowserState>(initialBrowserState);
  const [captionState, setCaptionState] = useState<CaptionState>(initialCaptionState);
  const [automationState, setAutomationState] = useState<AutomationState>(initialAutomationState);
  const [hotkeyState, setHotkeyState] = useState<HotkeyState>(initialHotkeyState);
  const [address, setAddress] = useState(initialBrowserState.currentUrl);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
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

        void resizeBrowser(toolbarHeight)
          .then((state) => {
            if (!isDisposed) {
              setBrowserState(state);
            }
          })
          .catch((error: unknown) => {
            if (!isDisposed) {
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
  }, [isSettingsOpen]);

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

  async function runAutomation(command: () => Promise<AutomationState>) {
    setCommandError(null);

    try {
      const nextState = await command();
      setAutomationState(nextState);
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
          void resizeBrowserToTopHeight(
            Math.ceil(toolbarRef.current.getBoundingClientRect().height),
          );
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
      setBrowserState(nextState);
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

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
            <button type="button" title="Back" onClick={() => void runBrowserCommand(goBrowserBack)}>
              &#8592;
            </button>
            <button
              type="button"
              title="Forward"
              onClick={() => void runBrowserCommand(goBrowserForward)}
            >
              &#8594;
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

          <button
            className="settings-button"
            type="button"
            title="Settings"
            onClick={openSettings}
          >
            Set
          </button>

          <button
            className={
              browserState.isContentProtected
                ? 'protection-button protected'
                : 'protection-button'
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
            {browserState.isContentProtected ? 'Protected' : 'Visible'}
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

          <button className="go-button" type="submit">
            Go
          </button>

          <button
            className="danger-button"
            type="button"
            title="Clear ChatGPT cookies and browser data"
            onClick={() => void runBrowserCommand(clearBrowserSession)}
          >
            Clear
          </button>

          <button className="caption-button" type="button" onClick={() => void toggleCaptions()}>
            {captionState.isMonitoring ? 'Stop Caption' : 'Start Caption'}
          </button>

          <button
            className="caption-submit-button"
            type="button"
            disabled={!captionState.pendingCaptionText && !captionState.currentCaptionText}
            onClick={() => void submitCaptions()}
          >
            Submit Caption
          </button>

          <div className="automation-controls" aria-label="Automation shortcuts">
            <button type="button" onClick={() => void runAutomation(runShortcutMode1)}>
              1
            </button>
            <button type="button" onClick={() => void runAutomation(runShortcutMode2)}>
              2
            </button>
            <button type="button" onClick={() => void runAutomation(runShortcutMode3)}>
              3
            </button>
            <button type="button" onClick={() => void runAutomation(submitAfterUpload)}>
              Enter
            </button>
          </div>

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
