# AI Assistant Browser

A compact Windows 11 desktop browser for ChatGPT, built with Tauri 2, Rust, React, and TypeScript.

The app combines ChatGPT with Windows Live Captions, screen capture, reusable prompt profiles, and global keyboard shortcuts. ChatGPT runs in an embedded child WebView, while privileged Windows features remain behind typed Rust commands.

## Features

### Compact ChatGPT browser

- Keeps the ChatGPT login session across app restarts.
- Provides Back, Refresh, Home, and direct URL navigation controls.
- Tracks page loading and download results in the bottom status bar.
- Uses a frameless window with custom minimize, close, and resize controls.
- Stays above other applications while visible.
- Supports hiding and showing the complete app with a global shortcut.

### Live Captions collection

- Opens Windows Live Captions from the toolbar.
- Reads caption text through Windows UI Automation.
- Cleans common caption artifacts and merges rolling updates without repeatedly adding the same text.
- Collects captions until Mode 1 or Mode 2 consumes the current batch.
- The toolbar Clear button discards the current batch and creates a new collection boundary immediately.
- Text still visible in Windows Live Captions at the moment Clear is pressed is treated as already seen, so it is not added back on the next poll.

### Automation modes

| Mode                      | Default shortcut   | Behavior                                                                                                                                                                                             |
| ------------------------- | ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Mode 1: Caption Submit    | `Ctrl+Enter`       | Takes the current caption batch, builds a prompt, inserts it into ChatGPT, and submits it.                                                                                                           |
| Mode 2: Capture + Caption | `Ctrl+Shift+Enter` | Captures the primary display, combines it with the current caption batch, uploads the image, and submits the prompt. Mode 2 can also run with an empty caption batch.                                |
| Mode 3: Capture Only      | `Ctrl+Shift+S`     | Captures the primary display and adds it to the current unsent ChatGPT composer. It does not consume captions or submit automatically, so it can be used repeatedly to build visual project context. |

Additional workflow behavior:

- Refresh preserves an in-progress Mode 1/2 prompt and restores it after ChatGPT reloads.
- Home clears caption and automation runtime state before returning to ChatGPT home.
- A Settings preference can keep the previous submitted Mode 1/2 prompt when new captions are added.
- Workflow guards prevent repeated shortcuts from corrupting an active caption or screenshot batch.
- Temporary screenshot files are removed after the upload workflow finishes.

### Mode 3 for live coding interviews

Mode 3 is designed for interviews where ChatGPT needs to understand an existing project gradually. Instead of trying to fit the entire project into one screenshot, capture the relevant views as they appear during the interview.

A typical workflow is:

1. Open an important project file, architecture view, requirement, error, terminal result, or test output.
2. Press `Ctrl+Shift+S` to add a screenshot of the primary display to the current ChatGPT composer.
3. Navigate to another relevant part of the project and press the shortcut again.
4. Repeat until the composer contains enough screenshots to explain the current project and problem.
5. Type the interview question or instruction in ChatGPT, review the attached screenshots, and submit once manually.

Each Mode 3 capture:

- Adds another screenshot without removing previously attached, unsent screenshots.
- Leaves the Live Captions batch available for a later Mode 1 or Mode 2 action.
- Waits for active screenshot uploads to settle before reporting the batch as ready.
- Coordinates rapid repeated shortcut presses so multiple captures can upload safely.
- Leaves submission under the user's control, allowing the question and screenshot set to be reviewed first.

Mode 3 captures the complete primary display. Before using it, move private messages, credentials, personal information, and unrelated windows away from the captured screen. ChatGPT attachment limits still apply, so capture distinct, useful views rather than many nearly identical screenshots.

### Prompt profiles

Profiles are reusable instructions appended to Mode 1 and Mode 2 prompts.

- Create, rename, edit, save, activate, and delete profiles.
- All profiles and the active-profile selection are persisted in the app data directory.
- Saved profiles and the active selection are restored when the app restarts.
- Clicking a profile row only selects it and loads its saved name and prompt into the editor.
- The Save icon is the only action that persists editor changes.
- The Active icon activates the currently selected profile without silently saving unsaved edits.
- The active profile prompt is read when Mode 1 or Mode 2 starts and appended to the end of the generated prompt.

The bottom status bar also provides:

- The active profile name.
- A single-line prompt preview, truncated with an ellipsis when necessary.
- A profile dropdown that changes and persists the active profile immediately.

### Settings and global shortcuts

Settings supports configurable shortcuts with duplicate detection:

| Action                    | Default shortcut   |
| ------------------------- | ------------------ |
| Mode 1: Caption Submit    | `Ctrl+Enter`       |
| Mode 2: Capture + Caption | `Ctrl+Shift+Enter` |
| Mode 3: Capture Only      | `Ctrl+Shift+S`     |
| Move window               | `Ctrl+Arrow`       |
| Hide / Show window        | `Ctrl+Backslash`   |

Window movement uses 50-pixel steps and is clamped to the current monitor work area.

### Privacy and appearance

- Transparency control adjusts the complete app window from 40% to 100% opacity.
- Content protection asks Windows to exclude the app from supported screen-capture workflows.
- The browser WebView is kept separate from the toolbar and bottom status bar, so app controls never modify the ChatGPT page layout.
- Transparency, Settings, and Profiles popovers close when focus moves elsewhere.
- Dismissing Settings or Profiles by clicking elsewhere does not apply or save pending changes.

## Toolbar

From left to right, the toolbar contains:

1. Back, Refresh, and Home.
2. Settings and Profiles.
3. Content protection and transparency.
4. Address bar.
5. Start/Stop Live Captions and Clear collected captions.
6. Minimize and Close.

The bottom bar contains operational status, the active-profile dropdown, and the truncated active prompt.

## Architecture

    .
    +-- public/                      # Settings, Profiles, and transparency popovers
    +-- src/
    |   +-- App.tsx                 # Toolbar, bottom status bar, and shell state
    |   +-- lib/tauri/              # Typed IPC client and command contracts
    |   +-- styles/                 # Main application styles
    +-- src-tauri/
    |   +-- capabilities/           # Tauri permission boundaries
    |   +-- src/
    |   |   +-- automation.rs       # Mode 1/2/3 workflow orchestration
    |   |   +-- browser.rs          # Child WebViews, navigation, and browser automation
    |   |   +-- captions.rs         # Live Captions monitoring, cleanup, and batching
    |   |   +-- hotkeys.rs          # Global shortcut registration and window movement
    |   |   +-- profiles.rs         # Profile persistence and active-profile state
    |   |   +-- screenshot.rs       # Primary-display capture and PNG encoding
    |   |   +-- lib.rs              # Tauri setup and command registration
    |   +-- tauri.conf.json         # Window, bundle, and security configuration
    +-- package.json

The React app owns presentation state and typed calls into Rust. Rust owns:

- WebView creation, focus, visibility, and bounds.
- URL validation and trusted ChatGPT automation.
- Windows UI Automation access.
- Screen capture and temporary image handling.
- Global shortcuts and window movement.
- Caption, automation, and profile state.
- Profile and preference persistence.

This boundary prevents the remote ChatGPT page from directly accessing native APIs, screenshots, shortcut registration, or saved profiles.

## Requirements

- Windows 11.
- Windows Live Captions for caption workflows.
- Node.js 20.11 or newer.
- npm 10 or newer.
- A stable Rust toolchain with the MSVC Windows target.
- Microsoft Edge WebView2 Runtime.

## Development

Install dependencies:

    npm install

Run the desktop app:

    npm run tauri:dev

Run frontend checks:

    npm run typecheck
    npm run lint
    npm run build
    npm run format:check

Run Rust checks:

    cargo check --manifest-path src-tauri/Cargo.toml
    cargo test --manifest-path src-tauri/Cargo.toml
    cargo fmt --manifest-path src-tauri/Cargo.toml --check

Build the NSIS Windows installer:

    npm run tauri -- build

## Automated GitHub Checks and Releases

The repository includes two GitHub Actions workflows:

- `CI` runs on every push and pull request to `main`. It installs dependencies, lints and builds the frontend, checks Rust formatting, and runs the Rust tests on a Windows runner.
- `Publish Windows release` runs only when a version tag such as `v0.1.0` is pushed. It verifies that the tag matches every project version, builds the NSIS installer, creates a public GitHub Release, and uploads the installer automatically.

### One-time GitHub setup

1. Push this repository to GitHub and open it in the browser.
2. Open **Settings > Actions > General**.
3. Under **Actions permissions**, allow the actions used by the workflows: GitHub's official actions, `tauri-apps/tauri-action`, `dtolnay/rust-toolchain`, and `Swatinem/rust-cache`. Selecting **Allow all actions and reusable workflows** is the simplest option for a personal repository.
4. Under **Workflow permissions**, keep the default read-only option. The CI workflow requests only `contents: read`, and the release workflow grants only the required `contents: write` permission to its own short-lived token.
5. Open the **Actions** tab and confirm that the `CI` workflow is listed.
6. Optional but recommended: open **Settings > Branches**, add a protection rule for `main`, require a pull request, and require the `Validate Windows app` check before merging.

GitHub supplies the short-lived `GITHUB_TOKEN` automatically. Do not create a personal access token or commit credentials for this workflow.

### Publish a release

1. Choose the next semantic version, for example `0.1.0`.
2. Set that exact version in all three files:
   - `src-tauri/tauri.conf.json`
   - `src-tauri/Cargo.toml`
   - `package.json` (run `npm install --package-lock-only` afterward so `package-lock.json` matches)
3. Commit the version change and push it to `main`.
4. Wait for `CI` to pass in the GitHub **Actions** tab.
5. Create and push a tag with a leading `v`:

       git tag v0.1.0
       git push origin v0.1.0

6. Open **Actions > Publish Windows release** and follow the running job.
7. When it succeeds, open the repository's **Releases** page. The new release and Windows NSIS setup executable are published there automatically.

The tag must match the configured version exactly. For example, tag `v0.2.0` requires version `0.2.0` in all three project files. A mismatch stops the workflow before anything is published.

### Windows signing

The generated installer is usable without a signing certificate, but Windows SmartScreen may warn users about an unknown publisher. For public distribution, obtain a Windows code-signing certificate and store its certificate, password, and signing keys as encrypted GitHub Actions secrets. Never add signing credentials to the repository.

## Manual Test Checklist

1. Launch the app and confirm the window stays above another application.
2. Confirm ChatGPT loads and its login session survives an app restart.
3. Test Back, Refresh, Home, URL navigation, minimize, close, and resizing.
4. Test transparency and content protection.
5. Confirm Transparency, Settings, and Profiles close after clicking elsewhere.
6. Confirm outside-click dismissal does not save profile edits or apply settings.
7. Start Windows Live Captions and verify new speech is collected.
8. Press Clear, continue speaking, and verify Mode 1/2 uses only captions collected after Clear.
9. Test all three automation modes with their configured shortcuts.
10. Create and save multiple profiles.
11. Confirm selecting a profile row does not activate it.
12. Activate the selected profile with the Active icon.
13. Restart and confirm all profiles and the active selection are restored.
14. Change the active profile from the bottom dropdown.
15. Confirm Mode 1/2 appends the active profile prompt at the end.
16. Change shortcuts and verify duplicate-shortcut validation.
17. Test window movement and the Hide / Show shortcut.
18. Confirm temporary screenshot files do not accumulate after successful uploads.

## Known Limitations

- ChatGPT DOM changes can break prompt insertion, attachment upload, or upload-completion detection.
- Windows Live Captions UI Automation structure can vary between Windows versions and languages.
- Caption capture uses bounded polling instead of a native text-change subscription.
- Screenshot capture currently targets the primary display.
- Global shortcut registration can fail when another application owns the same accelerator.
- Content protection depends on Windows and capture-tool support.

## Security Notes

- Keep native capabilities behind small, typed, validated Tauri commands.
- Treat Live Captions text and ChatGPT page content as untrusted input.
- Restrict browser automation and navigation to explicitly allowed targets.
- Keep screenshots in an application-controlled temporary location and delete them promptly.
- Do not expose broad filesystem, shell, or arbitrary JavaScript execution capabilities to the remote page.

## License

This project is available under the [MIT License](LICENSE).
