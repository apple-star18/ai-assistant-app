export type AppEnvironment = 'development' | 'staging' | 'production';

export interface AppHealth {
  status: 'ok';
  version: string;
  environment: AppEnvironment;
}

export interface BrowserDownload {
  url: string;
  destination: string | null;
  success: boolean | null;
}

export interface BrowserState {
  currentUrl: string;
  title: string;
  isLoading: boolean;
  isContentProtected: boolean;
  windowOpacity: number;
  lastDownload: BrowserDownload | null;
  lastError: string | null;
}

export interface BrowserTransparencyOverlayRequest {
  isOpen: boolean;
  left: number;
  top: number;
  width: number;
  height: number;
  opacityPercent: number;
}

export interface BrowserSettingsOverlayRequest {
  isOpen: boolean;
  left: number;
  top: number;
  width: number;
  height: number;
  indicatorLeft: number;
}

export interface BrowserProfileOverlayRequest {
  isOpen: boolean;
  left: number;
  top: number;
  width: number;
  height: number;
  indicatorLeft: number;
}

export interface CaptionState {
  isMonitoring: boolean;
  windowFound: boolean;
  textElementFound: boolean;
  launchAttempted: boolean;
  currentCaptionText: string;
  lastSubmittedCaptionText: string;
  pendingCaptionText: string;
  latestCaption: string;
  captionBuffer: string[];
  lastError: string | null;
}

export type AutomationMode = 'captionSubmit' | 'screenshotCaptionSubmit' | 'screenshotOnly';
export type UploadState = 'idle' | 'uploading' | 'ready' | 'failed';

export interface AutomationState {
  isRunning: boolean;
  lastMode: AutomationMode | null;
  uploadState: UploadState;
  lastError: string | null;
}

export interface HotkeyBindingState {
  action:
    | 'shortcutMode1'
    | 'shortcutMode2'
    | 'shortcutMode3'
    | 'shortcutMoveUp'
    | 'shortcutMoveDown'
    | 'shortcutMoveRight'
    | 'shortcutMoveLeft'
    | 'shortcutToggleWindow';
  accelerator: string;
  registered: boolean;
  error: string | null;
}

export interface HotkeyBindingRequest {
  action: HotkeyBindingState['action'];
  accelerator: string;
}

export interface HotkeyState {
  isRunning: boolean;
  bindings: HotkeyBindingState[];
  lastError: string | null;
}

export interface ProfileState {
  profiles: Array<{
    id: number;
    name: string;
    prompt: string;
  }>;
  activeProfileId: number | null;
  nextId: number;
}

export interface CommandMap {
  get_app_health: {
    args: undefined;
    response: AppHealth;
  };
  browser_get_state: {
    args: undefined;
    response: BrowserState;
  };
  browser_open_home: {
    args: undefined;
    response: BrowserState;
  };
  browser_navigate: {
    args: {
      request: {
        url: string;
      };
    };
    response: BrowserState;
  };
  browser_reload: {
    args: undefined;
    response: BrowserState;
  };
  browser_go_back: {
    args: undefined;
    response: BrowserState;
  };
  browser_go_forward: {
    args: undefined;
    response: BrowserState;
  };
  browser_focus: {
    args: undefined;
    response: undefined;
  };
  browser_clear_session: {
    args: undefined;
    response: BrowserState;
  };
  browser_resize: {
    args: {
      request: {
        toolbarHeight: number;
      };
    };
    response: BrowserState;
  };
  browser_set_content_protected: {
    args: {
      request: {
        isContentProtected: boolean;
      };
    };
    response: BrowserState;
  };
  browser_set_window_opacity: {
    args: {
      request: {
        opacity: number;
      };
    };
    response: BrowserState;
  };
  browser_set_settings_overlay: {
    args: {
      request: BrowserSettingsOverlayRequest;
    };
    response: undefined;
  };
  browser_set_profile_overlay: {
    args: {
      request: BrowserProfileOverlayRequest;
    };
    response: undefined;
  };
  browser_set_transparency_overlay: {
    args: {
      request: BrowserTransparencyOverlayRequest;
    };
    response: undefined;
  };
  captions_get_state: {
    args: undefined;
    response: CaptionState;
  };
  captions_start: {
    args: undefined;
    response: CaptionState;
  };
  captions_stop: {
    args: undefined;
    response: CaptionState;
  };
  captions_submit_to_chatgpt: {
    args: undefined;
    response: CaptionState;
  };
  automation_get_state: {
    args: undefined;
    response: AutomationState;
  };
  automation_shortcut_mode_1: {
    args: undefined;
    response: AutomationState;
  };
  automation_shortcut_mode_2: {
    args: undefined;
    response: AutomationState;
  };
  automation_shortcut_mode_3: {
    args: undefined;
    response: AutomationState;
  };
  automation_submit_after_upload: {
    args: undefined;
    response: AutomationState;
  };
  hotkeys_get_state: {
    args: undefined;
    response: HotkeyState;
  };
  hotkeys_apply_settings: {
    args: {
      request: {
        bindings: HotkeyBindingRequest[];
      };
    };
    response: HotkeyState;
  };
  profiles_get_state: {
    args: undefined;
    response: ProfileState;
  };
  profiles_add: {
    args: undefined;
    response: ProfileState;
  };
  profiles_save: {
    args: {
      request: {
        id: number;
        name: string;
        prompt: string;
      };
    };
    response: ProfileState;
  };
  profiles_delete: {
    args: {
      request: {
        id: number;
      };
    };
    response: ProfileState;
  };
  profiles_activate: {
    args: {
      request: {
        id: number;
      };
    };
    response: ProfileState;
  };
}
