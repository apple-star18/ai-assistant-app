# AI Assistant Browser

A Windows 11 desktop mini-browser for ChatGPT, built with Tauri 2, Rust, React, and TypeScript.

The application is designed around a narrow native boundary: the ChatGPT web experience runs in a WebView, while Windows-specific capabilities such as Live Captions access, screenshots, and global shortcuts stay in Rust.

## Current Capabilities

- ChatGPT WebView browser shell
- Persistent ChatGPT session storage
- Back, forward, refresh, home, URL navigation, and session clear controls
- Download event tracking
- Windows Live Captions launch and UI Automation polling
- Caption buffer management and caption cleanup
- Caption submission into ChatGPT
- Automation modes for caption submit, screenshot plus caption submit, and screenshot upload
- Home resets automation and caption collection; Refresh preserves and restores prepared prompt text
- Optional setting to combine an existing ChatGPT prompt with the next Mode 1 or Mode 2 caption batch
- Primary-display screenshot capture using Win32 GDI
- Configurable global automation shortcuts: `Ctrl+Enter`, `Ctrl+Shift+Enter`, and `Ctrl+Shift+S`
- Configurable global window shortcuts: `Ctrl+Arrow` moves the window by 50 pixels and `Ctrl+\\` hides or shows it
- Typed frontend IPC wrappers for Tauri commands

## Architecture

```text
.
+-- src/                         # React + TypeScript frontend
|   +-- App.tsx                  # Browser toolbar and automation controls
|   +-- config/                  # Frontend environment parsing
|   +-- lib/tauri/               # Typed IPC client and command contracts
|   +-- styles/                  # Global application styles
+-- src-tauri/                   # Rust backend and Tauri application shell
|   +-- capabilities/            # Tauri permission boundaries
|   +-- src/
|   |   +-- automation.rs        # Cross-service automation workflows
|   |   +-- browser.rs           # Child WebView, navigation, upload automation
|   |   +-- captions.rs          # Live Captions UIA monitoring and text cleanup
|   |   +-- commands/            # General Tauri commands
|   |   +-- config.rs            # Backend environment config
|   |   +-- hotkeys.rs           # Native global hotkey registration
|   |   +-- lib.rs               # Tauri setup and command registration
|   |   +-- screenshot.rs        # Win32 screen capture and PNG encoding
|   +-- tauri.conf.json          # Window, bundle, and security configuration
+-- package.json                 # Frontend scripts and dependencies
+-- tsconfig.json                # TypeScript configuration
+-- vite.config.ts               # Vite configuration
```

The intended long-term module shape is:

```text
src/frontend/                    # UI layer
src-tauri/src/browser/           # Browser application service
src-tauri/src/caption/           # Caption application service
src-tauri/src/windows/           # Native Windows adapters
src-tauri/src/screenshot/        # Screenshot service
src-tauri/src/automation/        # Workflow orchestration
src-tauri/src/shortcuts/         # Shortcut registration and dispatch
src-tauri/src/security/          # Validation and policy helpers
```

The current implementation is still file-based inside `src-tauri/src/`; splitting those files into folders is a future refactor and should be done as small behavior-preserving commits.

## Runtime Design

The frontend owns only UI state, toolbar interactions, and typed calls into Rust. It does not call raw `invoke` directly outside `src/lib/tauri/client.ts`.

Rust owns:

- WebView creation and bounds management
- URL validation and navigation
- JavaScript injection used for trusted ChatGPT input/upload automation
- Live Captions discovery and UI Automation access
- Screenshot capture and temporary image handling
- Global shortcut registration
- Automation state and workflow sequencing

This keeps native APIs and privileged operations outside the remote ChatGPT page.

## Dependencies

### Frontend

- `@tauri-apps/api`: frontend bridge to Tauri commands and events
- `react`, `react-dom`: UI layer
- `vite`, `typescript`, `eslint`, `prettier`: development, build, and quality tooling

### Rust

- `tauri`: desktop shell, commands, events, WebView integration
- `serde`, `serde_json`: typed command payloads and state serialization
- `windows`: Win32 and UI Automation bindings
- `png`: PNG encoding for captured screenshots
- `base64`: screenshot transfer into browser-side upload automation

Security impact: native capabilities are exposed only through explicit Tauri commands, and command payloads should remain validated at the Rust boundary.

Maintenance impact: Windows integrations are intentionally centralized in Rust so platform-specific risk does not leak into the React UI.

## Development

Install dependencies:

```bash
npm install
```

Run the desktop app in development:

```bash
npm run tauri:dev
```

Run frontend checks:

```bash
npm run typecheck
npm run lint
npm run build
```

Run Rust checks:

```bash
npm run rust:check
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```

Build the Windows installer:

```bash
npm run tauri -- build
```

The current bundle target is NSIS.

## Manual Test Checklist

1. Launch the app with `npm run tauri:dev`.
2. Confirm ChatGPT loads and login persists after restart.
3. Test back, forward, refresh, home, URL navigation, and clear session.
4. Start Windows Live Captions from the toolbar.
5. Confirm caption status updates when Live Captions emits text.
6. Use `Submit Caption` and confirm cleaned caption text appears in ChatGPT input.
7. Test `Ctrl+Enter` for caption submit.
8. Test `Ctrl+Shift+Enter` for screenshot plus caption upload and submit.
9. Test `Ctrl+Shift+S` for screenshot upload only.
10. Test `Ctrl+Arrow` window movement and `Ctrl+\\` window visibility toggle.
11. Confirm temporary screenshot files do not accumulate after successful upload.

## Known Risks

- ChatGPT DOM structure may change, which can break input focus, attachment upload, or upload completion detection.
- Live Captions UI Automation element structure can vary across Windows builds and languages.
- Caption monitoring currently uses bounded UIA polling rather than a robust text-change event subscription.
- Screenshot capture currently targets the primary display through GDI; multi-monitor and protected-content handling need more work.
- Global shortcut registration can fail if another application already owns the same accelerator.
- Screenshots may contain sensitive information, so retention must remain short and cleanup must be reliable.

## Security Notes

- Do not expose broad filesystem, shell, or arbitrary JavaScript execution commands to the frontend.
- Keep all IPC commands small, typed, and validated.
- Treat text from Live Captions and the ChatGPT page as untrusted input.
- Keep screenshot files in an application-controlled temporary location and delete them after use.
- Prefer explicit allowlists for URLs and automation targets.

## Production Roadmap

1. Refactor Rust modules into folder-based services and Windows adapters.
2. Replace heuristic ChatGPT DOM upload tracking with a more resilient observer strategy.
3. Add configurable shortcuts with conflict detection and persistence.
4. Add multi-monitor screenshot support.
5. Add stricter command validation and a dedicated security module.
6. Add integration tests around command contracts and automation state transitions.
7. Add installer signing, release metadata, crash reporting, and update strategy.
