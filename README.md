# AI Assistant App

Production-oriented foundation for a Tauri 2 desktop AI application.

## Architecture

The root project is the React, TypeScript, and Vite frontend. Tauri treats the built frontend as static assets and bundles them into the desktop app.

`src-tauri/` is the Rust backend. It owns native app startup, Tauri configuration, command registration, security capabilities, and future integrations that should not run in the WebView.

The frontend calls Rust through a narrow IPC layer in `src/lib/tauri/`. Command names, argument shapes, and response shapes are centralized there so UI code does not call raw `invoke` directly.

Environment configuration is split by runtime:

- Frontend variables use `VITE_*` and are read through `src/config/env.ts`.
- Rust backend variables are read in `src-tauri/src/config.rs`.
- Tauri app, window, bundle, and security settings live in `src-tauri/tauri.conf.json`.
- Tauri permissions live in `src-tauri/capabilities/default.json`.

For stronger cross-language guarantees as the command surface grows, add generated bindings with `specta`/`tauri-specta` or an equivalent Rust-to-TypeScript generator and replace the handwritten command contract.

## Project Structure

```text
.
├── src/                    # React frontend
│   ├── config/             # Frontend environment parsing
│   ├── lib/tauri/          # Typed IPC boundary
│   └── styles/             # Global styles
├── src-tauri/              # Rust backend and Tauri app shell
│   ├── capabilities/       # Tauri permission boundaries
│   └── src/
│       ├── commands/       # Tauri commands exposed to the frontend
│       └── config.rs       # Backend environment config
├── eslint.config.js
├── package.json
├── tsconfig.json
└── vite.config.ts
```

## Commands

```bash
npm install
npm run tauri:dev
npm run lint
npm run typecheck
npm run build
npm run rust:check
```
