import {
  FormEvent,
  MouseEvent as ReactMouseEvent,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

import {
  clearCaptions,
  closeMainWindow,
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
  listenToProfileOverlayClosed,
  listenToSettingsOverlayClosed,
  minimizeMainWindow,
  navigateBrowser,
  openBrowserHome,
  reloadBrowser,
  resizeBrowser,
  setBrowserProfileOverlay,
  setBrowserSettingsOverlay,
  setBrowserContentProtected,
  setBrowserTransparencyOverlay,
  startCaptions,
  startMainWindowDragging,
  startMainWindowResizeDragging,
  stopCaptions,
  submitCaptionsToChatGpt,
} from './lib/tauri/client';
import type { AutomationState, BrowserState, CaptionState } from './lib/tauri/contracts';
import type { HotkeyState } from './lib/tauri/contracts';
import type { WindowResizeDirection } from './lib/tauri/client';

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

const TRANSPARENCY_CONTROL_WIDTH = 220;
const TRANSPARENCY_CONTROL_MARGIN = 8;
const TRANSPARENCY_OVERLAY_TOP = 46;
const TRANSPARENCY_OVERLAY_HEIGHT = 43;
const SETTINGS_OVERLAY_WIDTH = 332;
const SETTINGS_OVERLAY_HEIGHT = 500;
const SETTINGS_OVERLAY_MARGIN = 8;
const PROFILE_OVERLAY_WIDTH = 620;
const PROFILE_OVERLAY_HEIGHT = 430;
const PROFILE_OVERLAY_MARGIN = 8;
const WINDOW_CONTENT_INSET = 12;
const WINDOW_RESIZE_HANDLES: ReadonlyArray<{
  direction: WindowResizeDirection;
  position: string;
}> = [
  { direction: 'North', position: 'north' },
  { direction: 'NorthEast', position: 'north-east' },
  { direction: 'East', position: 'east' },
  { direction: 'SouthEast', position: 'south-east' },
  { direction: 'South', position: 'south' },
  { direction: 'SouthWest', position: 'south-west' },
  { direction: 'West', position: 'west' },
  { direction: 'NorthWest', position: 'north-west' },
];

export function App() {
  return <BrowserWindow />;
}

function BrowserWindow() {
  const toolbarRef = useRef<HTMLDivElement | null>(null);
  const settingsButtonRef = useRef<HTMLButtonElement | null>(null);
  const profileButtonRef = useRef<HTMLButtonElement | null>(null);
  const transparencyButtonRef = useRef<HTMLButtonElement | null>(null);
  const isAddressEditingRef = useRef(false);
  const [browserState, setBrowserState] = useState<BrowserState>(initialBrowserState);
  const [captionState, setCaptionState] = useState<CaptionState>(initialCaptionState);
  const [automationState, setAutomationState] = useState<AutomationState>(initialAutomationState);
  const [hotkeyState, setHotkeyState] = useState<HotkeyState>(initialHotkeyState);
  const [address, setAddress] = useState(initialBrowserState.currentUrl);
  const [commandError, setCommandError] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isProfileOpen, setIsProfileOpen] = useState(false);
  const [isTransparencyOpen, setIsTransparencyOpen] = useState(false);
  const [transparencyControlLeft, setTransparencyControlLeft] = useState(
    TRANSPARENCY_CONTROL_MARGIN,
  );

  useEffect(() => {
    let isMounted = true;

    void Promise.allSettled([
      getBrowserState(),
      getCaptionState(),
      getAutomationState(),
      getHotkeyState(),
    ]).then(([browserResult, captionResult, automationResult, hotkeyResult]) => {
      if (!isMounted) {
        return;
      }

      if (browserResult.status === 'fulfilled') {
        setBrowserState(browserResult.value);
        if (!isAddressEditingRef.current) {
          setAddress(browserResult.value.currentUrl);
        }
      }
      if (captionResult.status === 'fulfilled') {
        setCaptionState(captionResult.value);
      }
      if (automationResult.status === 'fulfilled') {
        setAutomationState(automationResult.value);
      }
      if (hotkeyResult.status === 'fulfilled') {
        setHotkeyState(hotkeyResult.value);
      }

      const failedRequest = [browserResult, captionResult, automationResult, hotkeyResult].find(
        (result) => result.status === 'rejected',
      );
      if (failedRequest?.status === 'rejected') {
        setCommandError(getErrorMessage(failedRequest.reason));
      }
    });

    const unlisten = listenToBrowserState((state) => {
      setBrowserState(state);
      if (!isAddressEditingRef.current) {
        setAddress(state.currentUrl);
      }
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
    const listeners = [unlisten, unlistenCaptions, unlistenAutomation, unlistenHotkeys];

    for (const listener of listeners) {
      void listener.catch((error: unknown) => {
        if (isMounted) {
          setCommandError(getErrorMessage(error));
        }
      });
    }

    return () => {
      isMounted = false;

      for (const listener of listeners) {
        void listener.then((dispose) => dispose()).catch(() => undefined);
      }
    };
  }, []);

  useEffect(() => {
    let isMounted = true;
    const unlisten = listenToSettingsOverlayClosed(() => {
      setIsSettingsOpen(false);
    });
    void unlisten.catch((error: unknown) => {
      if (isMounted) {
        setCommandError(getErrorMessage(error));
      }
    });

    return () => {
      isMounted = false;
      void unlisten.then((dispose) => dispose()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    let isMounted = true;
    const unlisten = listenToProfileOverlayClosed(() => {
      setIsProfileOpen(false);
    });
    void unlisten.catch((error: unknown) => {
      if (isMounted) {
        setCommandError(getErrorMessage(error));
      }
    });

    return () => {
      isMounted = false;
      void unlisten.then((dispose) => dispose()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    if (!isTransparencyOpen) {
      return;
    }

    const logTransparencyMetrics = () => {
      updateTransparencyControlPosition();
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
    if (!isSettingsOpen) {
      return;
    }

    let animationFrame: number | null = null;
    const repositionSettings = () => {
      if (animationFrame !== null) {
        return;
      }

      animationFrame = window.requestAnimationFrame(() => {
        animationFrame = null;
        void showSettingsOverlay();
      });
    };

    repositionSettings();

    window.addEventListener('resize', repositionSettings);
    window.visualViewport?.addEventListener('resize', repositionSettings);

    return () => {
      if (animationFrame !== null) {
        window.cancelAnimationFrame(animationFrame);
      }
      window.removeEventListener('resize', repositionSettings);
      window.visualViewport?.removeEventListener('resize', repositionSettings);
    };
  }, [isSettingsOpen]);

  useEffect(() => {
    if (!isProfileOpen) {
      return;
    }

    let animationFrame: number | null = null;
    const repositionProfile = () => {
      if (animationFrame !== null) {
        return;
      }

      animationFrame = window.requestAnimationFrame(() => {
        animationFrame = null;
        void showProfileOverlay();
      });
    };

    repositionProfile();
    window.addEventListener('resize', repositionProfile);
    window.visualViewport?.addEventListener('resize', repositionProfile);

    return () => {
      if (animationFrame !== null) {
        window.cancelAnimationFrame(animationFrame);
      }
      window.removeEventListener('resize', repositionProfile);
      window.visualViewport?.removeEventListener('resize', repositionProfile);
    };
  }, [isProfileOpen]);

  useLayoutEffect(() => {
    let animationFrame: number | null = null;
    let isDisposed = false;
    let lastToolbarHeight: number | null = null;

    const syncBrowserBounds = () => {
      if (animationFrame !== null) {
        return;
      }

      animationFrame = window.requestAnimationFrame(() => {
        animationFrame = null;

        if (isDisposed || !toolbarRef.current) {
          return;
        }

        const toolbarHeight = Math.ceil(toolbarRef.current.getBoundingClientRect().height);

        if (toolbarHeight === lastToolbarHeight) {
          return;
        }

        lastToolbarHeight = toolbarHeight;

        void resizeBrowser(toolbarHeight).catch((error: unknown) => {
          if (!isDisposed) {
            lastToolbarHeight = null;
            setCommandError(getErrorMessage(error));
          }
        });
      });
    };

    const observer = new ResizeObserver(syncBrowserBounds);

    if (toolbarRef.current) {
      observer.observe(toolbarRef.current);
    }

    syncBrowserBounds();

    return () => {
      isDisposed = true;
      observer.disconnect();

      if (animationFrame !== null) {
        window.cancelAnimationFrame(animationFrame);
      }
    };
  }, []);

  useEffect(() => {
    const opacityPercent = Math.round(browserState.windowOpacity * 100);

    void setBrowserTransparencyOverlay({
      isOpen: isTransparencyOpen,
      left: transparencyControlLeft + WINDOW_CONTENT_INSET,
      top: TRANSPARENCY_OVERLAY_TOP + WINDOW_CONTENT_INSET,
      width: TRANSPARENCY_CONTROL_WIDTH,
      height: TRANSPARENCY_OVERLAY_HEIGHT,
      opacityPercent,
    }).catch((error: unknown) => {
      setCommandError(getErrorMessage(error));
    });
  }, [browserState.windowOpacity, isTransparencyOpen, transparencyControlLeft]);

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
        : captionState.pendingCaptionText || captionState.currentCaptionText || statusText),
    [
      automationState.isRunning,
      automationState.lastError,
      browserState.lastError,
      captionState.lastError,
      captionState.currentCaptionText,
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

  async function runWindowCommand(command: () => Promise<void>) {
    setCommandError(null);

    try {
      await command();
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  function handleToolbarMouseDown(event: ReactMouseEvent<HTMLFormElement>) {
    if (event.button !== 0) {
      return;
    }

    const target = event.target;

    if (target instanceof Element && target.closest('button, input, label, a, [role="button"]')) {
      return;
    }

    void runWindowCommand(startMainWindowDragging);
  }

  function handleNavigate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    isAddressEditingRef.current = false;
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

  async function clearCollectedCaptions() {
    setCommandError(null);

    try {
      const nextState = await clearCaptions();
      setCaptionState(nextState);
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
    const nextIsOpen = !isTransparencyOpen;

    if (nextIsOpen) {
      updateTransparencyControlPosition();
    }

    setIsTransparencyOpen(nextIsOpen);
  }

  function openSettings() {
    setCommandError(null);
    if (isProfileOpen) {
      closeProfile();
    }
    setIsSettingsOpen(true);
  }

  function closeSettings() {
    setIsSettingsOpen(false);
    void setBrowserSettingsOverlay({
      isOpen: false,
      left: 0,
      top: 0,
      width: 1,
      height: 1,
      indicatorLeft: 14,
    }).catch((error: unknown) => {
      setCommandError(getErrorMessage(error));
    });
  }

  async function showSettingsOverlay() {
    if (!toolbarRef.current || !settingsButtonRef.current) {
      return;
    }

    const topLayerRect = toolbarRef.current.getBoundingClientRect();
    const buttonRect = settingsButtonRef.current.getBoundingClientRect();
    const width = Math.max(
      260,
      Math.min(SETTINGS_OVERLAY_WIDTH, topLayerRect.width - SETTINGS_OVERLAY_MARGIN * 2),
    );
    const height = SETTINGS_OVERLAY_HEIGHT;
    const buttonCenter = buttonRect.left - topLayerRect.left + buttonRect.width / 2;
    const maxLeft = Math.max(
      SETTINGS_OVERLAY_MARGIN,
      topLayerRect.width - width - SETTINGS_OVERLAY_MARGIN,
    );
    const left = Math.round(
      Math.min(maxLeft, Math.max(SETTINGS_OVERLAY_MARGIN, buttonCenter - width / 2)),
    );
    const top = Math.round(topLayerRect.height + SETTINGS_OVERLAY_MARGIN);
    const indicatorLeft = Math.round(buttonCenter - left);

    try {
      await setBrowserSettingsOverlay({
        isOpen: true,
        left: left + WINDOW_CONTENT_INSET,
        top: top + WINDOW_CONTENT_INSET,
        width,
        height,
        indicatorLeft,
      });
    } catch (error) {
      setCommandError(getErrorMessage(error));
    }
  }

  function openProfile() {
    setCommandError(null);
    if (isSettingsOpen) {
      closeSettings();
    }
    setIsProfileOpen(true);
  }

  function closeProfile() {
    setIsProfileOpen(false);
    void setBrowserProfileOverlay({
      isOpen: false,
      left: 0,
      top: 0,
      width: 1,
      height: 1,
      indicatorLeft: 14,
    }).catch((error: unknown) => {
      setCommandError(getErrorMessage(error));
    });
  }

  async function showProfileOverlay() {
    if (!toolbarRef.current || !profileButtonRef.current) {
      return;
    }

    const topLayerRect = toolbarRef.current.getBoundingClientRect();
    const buttonRect = profileButtonRef.current.getBoundingClientRect();
    const width = Math.max(
      420,
      Math.min(PROFILE_OVERLAY_WIDTH, topLayerRect.width - PROFILE_OVERLAY_MARGIN * 2),
    );
    const buttonCenter = buttonRect.left - topLayerRect.left + buttonRect.width / 2;
    const maxLeft = Math.max(
      PROFILE_OVERLAY_MARGIN,
      topLayerRect.width - width - PROFILE_OVERLAY_MARGIN,
    );
    const left = Math.round(
      Math.min(maxLeft, Math.max(PROFILE_OVERLAY_MARGIN, buttonCenter - width / 2)),
    );
    const top = Math.round(topLayerRect.height + PROFILE_OVERLAY_MARGIN);
    const indicatorLeft = Math.round(buttonCenter - left);

    try {
      await setBrowserProfileOverlay({
        isOpen: true,
        left: left + WINDOW_CONTENT_INSET,
        top: top + WINDOW_CONTENT_INSET,
        width,
        height: PROFILE_OVERLAY_HEIGHT,
        indicatorLeft,
      });
    } catch (error) {
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

    setTransparencyControlLeft(nextLeft);
  }

  return (
    <main
      className={browserState.isContentProtected ? 'browser-shell protected' : 'browser-shell'}
      aria-label="ChatGPT browser"
    >
      <div ref={toolbarRef} className="browser-top-layer">
        <form
          className="browser-toolbar"
          onMouseDown={handleToolbarMouseDown}
          onSubmit={handleNavigate}
        >
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

          <button
            ref={settingsButtonRef}
            className={isSettingsOpen ? 'settings-button active' : 'settings-button'}
            type="button"
            title="Settings"
            aria-expanded={isSettingsOpen}
            onClick={isSettingsOpen ? closeSettings : openSettings}
          >
            &#9881;
          </button>

          <button
            ref={profileButtonRef}
            className={isProfileOpen ? 'profile-button active' : 'profile-button'}
            type="button"
            title="Profile"
            aria-label="Open profiles"
            aria-expanded={isProfileOpen}
            onClick={isProfileOpen ? closeProfile : openProfile}
          >
            <ProfileIcon />
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
              onFocus={(event) => {
                isAddressEditingRef.current = true;
                event.currentTarget.select();
              }}
              onBlur={() => {
                isAddressEditingRef.current = false;
              }}
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
            className="caption-clear-button"
            type="button"
            title="Clear collected captions and start a new batch"
            aria-label="Clear collected captions"
            disabled={!captionState.pendingCaptionText && !captionState.currentCaptionText}
            onClick={() => void clearCollectedCaptions()}
          >
            <ClearCaptionsIcon />
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

          <div className="window-controls" aria-label="Window controls">
            <button
              className="window-minimize-button"
              type="button"
              title="Minimize"
              aria-label="Minimize window"
              onClick={() => void runWindowCommand(minimizeMainWindow)}
            >
              <span aria-hidden="true">&#8722;</span>
            </button>
            <button
              className="window-close-button"
              type="button"
              title="Close"
              aria-label="Close window"
              onClick={() => void runWindowCommand(closeMainWindow)}
            >
              <span aria-hidden="true">&#215;</span>
            </button>
          </div>
        </form>
      </div>
      {WINDOW_RESIZE_HANDLES.map(({ direction, position }) => (
        <div
          key={direction}
          className={`window-resize-handle ${position}`}
          aria-hidden="true"
          onMouseDown={(event) => {
            if (event.button !== 0) {
              return;
            }

            event.preventDefault();
            void runWindowCommand(() => startMainWindowResizeDragging(direction));
          }}
        />
      ))}
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

function ClearCaptionsIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="m4.5 15.5 8.8-10a2 2 0 0 1 2.8-.2l2.6 2.3a2 2 0 0 1 .2 2.8l-7.1 8.1H7.2l-2.5-2.2a.6.6 0 0 1-.2-.8Z" />
      <path d="m10 9.3 6 5.2" />
      <path d="M11.8 18.5h7.7" />
    </svg>
  );
}

function ProfileIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <circle cx="12" cy="8" r="3.25" />
      <path d="M5.5 19c.7-3.3 3-5 6.5-5s5.8 1.7 6.5 5" />
    </svg>
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
