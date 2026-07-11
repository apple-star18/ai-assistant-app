import { FormEvent, useEffect, useMemo, useRef, useState } from 'react';

import {
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
  startCaptions,
  stopCaptions,
  submitAfterUpload,
  submitCaptionsToChatGpt,
} from './lib/tauri/client';
import type { AutomationState, BrowserState, CaptionState } from './lib/tauri/contracts';
import type { HotkeyState } from './lib/tauri/contracts';

const initialBrowserState: BrowserState = {
  currentUrl: 'https://chatgpt.com/',
  title: 'ChatGPT',
  isLoading: true,
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
  const toolbarRef = useRef<HTMLFormElement | null>(null);
  const [browserState, setBrowserState] = useState<BrowserState>(initialBrowserState);
  const [captionState, setCaptionState] = useState<CaptionState>(initialCaptionState);
  const [automationState, setAutomationState] = useState<AutomationState>(initialAutomationState);
  const [hotkeyState, setHotkeyState] = useState<HotkeyState>(initialHotkeyState);
  const [address, setAddress] = useState(initialBrowserState.currentUrl);
  const [commandError, setCommandError] = useState<string | null>(null);

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
        const viewportWidth = Math.ceil(window.visualViewport?.width ?? window.innerWidth);
        const viewportHeight = Math.ceil(window.visualViewport?.height ?? window.innerHeight);

        void resizeBrowser(toolbarHeight, viewportWidth, viewportHeight)
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
  }, []);

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

  return (
    <main className="browser-shell" aria-label="ChatGPT browser">
      <form ref={toolbarRef} className="browser-toolbar" onSubmit={handleNavigate}>
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
    </main>
  );
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
