# AI Chat Explorer

Languages: [English](./README.md) | [繁體中文](./README.zh-Hant.md)

AI Chat Explorer is a local-first desktop app built with Tauri 2, Rust, and vanilla JavaScript for browsing AI chat history. It currently supports **Claude** (`~/.claude/projects`), **Codex CLI** (`~/.codex`), and **VS Code / GitHub Copilot Chat** workspace chat sessions.

It is designed for people who want a faster and safer way to inspect local AI workspaces without uploading session data to a third-party service.

## ✨ Highlights

- Local-first by default. Session data stays on your machine.
- Supports multiple AI sources: **Claude**, **Codex CLI**, and **VS Code / GitHub Copilot Chat**, with a source toggle to filter by provider.
- Rust backend for source discovery, JSON/JSONL parsing, rollout replay, and path validation.
- Timeline viewer for conversations, tool activity, thinking blocks, and system events.
- Tree view for parent sessions and subagent sessions.
- Built-in project and message search.
- Safe deletion flow with project impact preview and session undo window.
- Bilingual UI: `en-US` and `zh-Hant-TW`.
- Theme modes: `auto`, `light`, and `dark`.

## 🧩 Current Features

- Browse Claude projects detected from `~/.claude/projects`.
- Browse Codex CLI projects detected from `~/.codex/sessions`.
- Browse VS Code / GitHub Copilot Chat sessions detected from VS Code `User/workspaceStorage/{hash}/chatSessions`.
- Source toggle to show all sessions, Claude only, Codex only, or VS Code only.
- Source badges on project and session list entries to identify the AI provider at a glance.
- Infer a readable project name from the original working directory when available.
- Open sessions and memory files from the same project workspace.
- Display session metadata including model, token usage, web tool usage, and estimated duration.
- Render Codex session timelines including chat messages, thinking blocks, function calls, and system events.
- Replay Codex rollout records so `event_msg` user and assistant messages render as normal chat bubbles.
- Render VS Code / GitHub Copilot Chat timelines from `.json` and `.jsonl` session records.
- Keep `tool_use` and `tool_result` content grouped in a readable timeline.
- Toggle visibility for system events, tool calls, and thinking content independently.
- Search across the project list and within the selected timeline.
- Show subagent sessions under their parent session with expand/collapse controls.
- Open the project folder from the context menu.
- Copy session IDs from the session context menu.
- Preview delete impact before removing a project.
- Queue session deletion with a short undo grace period.

## 📸 Screenshots

![Main Window Placeholder](./screenshots/main-window-placeholder.png)

## 🛠 Tech Stack

- Desktop shell: Tauri 2
- Backend: Rust 2021
- Frontend: vanilla HTML, CSS, and ESM JavaScript
- Test stack: `node --test`, JSDOM, Rust unit tests

## 📁 Project Layout

```text
src/         Frontend app (HTML, CSS, JS, i18n, theme logic)
src-tauri/   Rust backend, Tauri commands, packaging config
tests/       Frontend unit tests and JSDOM UI tests
docs/        Product and UI notes
```

## 🚀 Getting Started

### Prerequisites

- Node.js 22 or later
- Rust stable toolchain
- Tauri system prerequisites: https://tauri.app/start/prerequisites/

### Install Dependencies

```bash
npm install
```

### Run In Desktop Mode

```bash
npm run tauri dev
```

### Run In Browser Preview

```bash
npm run dev:web
```

The browser preview is useful for frontend iteration, but features that depend on Tauri APIs such as local filesystem commands and folder opening require the desktop runtime.

## ✅ Testing

Run frontend tests:

```bash
node --test tests/*.mjs
```

Run focused UI flow checks:

```bash
npm run test:ui
```

`test:ui` currently runs a JSDOM-based UI regression suite (not a full real-browser E2E suite), mainly covering delete/undo flows, context menu actions, and timeline visibility toggles.

Run Rust backend tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## 🔒 Security Model

- The backend validates requested paths against the allowed root for each source before reading, parsing, or deleting.
- Claude sessions are discovered from `%USERPROFILE%\\.claude\\projects` on Windows and `~/.claude/projects` on Unix-like systems.
- Codex CLI sessions are discovered from `~/.codex`.
- VS Code / GitHub Copilot Chat records are discovered from VS Code workspace storage and are treated as read-only in the current UI.
- Claude project deletion removes the full project directory tree.
- Codex project deletion removes matching Codex session files for that project.
- Session deletion is source-specific and only exposed for sources with backend deletion support.

## 🤝 Contributing

Issues and pull requests are welcome. Prefer small, focused changes with clear reproduction steps and expected behavior.

## 📄 License

MIT
