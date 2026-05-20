# AI Chat Explorer

語言：[English](./README.md) | [繁體中文](./README.zh-Hant.md)

`AI Chat Explorer` 是一個以 Tauri 2、Rust 與原生 JavaScript 打造的本機優先桌面應用，用來瀏覽 AI 對話紀錄。目前支援 **Claude**（`~/.claude/projects`）、**Codex CLI**（`~/.codex`），以及 **VS Code / GitHub Copilot Chat** workspace chat sessions。

它的重點不是把資料搬到雲端重新包裝，而是直接在你的本機工作區上，快速、安全地查看專案、Session、Memory 與 Subagent 歷史。

## ✨ 特色摘要

- 本機優先，不依賴雲端同步。
- 支援多來源 AI：**Claude**、**Codex CLI**、**VS Code / GitHub Copilot Chat**，可透過來源切換開關依提供商篩選。
- Rust 後端負責來源探索、JSON/JSONL 解析、rollout replay 與路徑安全驗證。
- 以時間軸方式檢視對話、工具呼叫、thinking 區塊與系統事件。
- 支援父 Session 與 Subagent Session 的樹狀展開。
- 內建專案搜尋與時間軸搜尋。
- 提供專案刪除影響預覽，以及 Session 刪除後短時間復原。
- 內建雙語介面：`en-US`、`zh-Hant-TW`。
- 支援 `auto`、`light`、`dark` 三種主題模式。

## 🧩 目前功能

- 掃描並瀏覽 `~/.claude/projects` 下的 Claude 專案。
- 掃描並瀏覽 `~/.codex` 下的 Codex CLI 專案。
- 掃描並瀏覽 VS Code `User/workspaceStorage/{hash}/chatSessions` 中的 VS Code / GitHub Copilot Chat sessions。
- 來源切換開關：可顯示全部、僅 Claude、僅 Codex 或僅 VS Code 的 Session。
- 專案與 Session 列表顯示來源徽章，一眼辨識 AI 提供商。
- 可從 Session 內容推斷原始工作目錄，優先顯示較可讀的專案名稱。
- 同一介面中查看 Session 與 Memory 檔案。
- 顯示 Session 中繼資料，例如模型、Token 使用量、Web 工具請求數與估計時長。
- 渲染 Codex Session 時間軸，包含對話訊息、thinking 區塊、function call 與系統事件。
- 回放 Codex rollout 記錄，讓 `event_msg` 的 user / assistant 訊息以一般聊天泡泡顯示。
- 渲染 VS Code / GitHub Copilot Chat 的 `.json` 與 `.jsonl` session records。
- 將 `tool_use` 與 `tool_result` 以較易讀的方式整理在時間軸中。
- 可獨立切換系統事件、工具呼叫與 thinking 內容的顯示狀態。
- 支援專案列表搜尋與目前時間軸內容搜尋。
- 將 Subagent Session 掛在父 Session 底下，並提供展開/收合控制。
- 專案右鍵選單可直接開啟資料夾。
- Session 右鍵選單可複製 Session ID。
- 刪除專案前顯示影響檔案數與總容量。
- 刪除 Session 後提供短暫 Undo 緩衝時間。

## 📸 截圖

![主畫面占位圖](./screenshots/main-window-placeholder.png)

## 🛠 技術棧

- 桌面容器：Tauri 2
- 後端：Rust 2021
- 前端：原生 HTML、CSS、ESM JavaScript
- 測試：`node --test`、JSDOM、Rust 單元測試

## 📁 專案結構

```text
src/         前端介面（HTML、CSS、JS、i18n、theme）
src-tauri/   Rust 後端、Tauri commands、打包設定
tests/       前端單元測試與 JSDOM UI 測試
docs/        產品與 UI 規劃文件
```

## 🚀 快速開始

### 環境需求

- Node.js 22 以上
- Rust stable toolchain
- Tauri 系統相依元件：https://tauri.app/start/prerequisites/

### 安裝相依套件

```bash
npm install
```

### 啟動桌面開發模式

```bash
npm run tauri dev
```

### 啟動瀏覽器預覽模式

```bash
npm run dev:web
```

瀏覽器預覽適合做前端樣式與互動調整，但凡是依賴 Tauri API 的功能，例如本機檔案操作、開啟資料夾等，仍需在桌面執行環境中測試。

## ✅ 測試

執行前端測試：

```bash
node --test tests/*.mjs
```

執行聚焦 UI 流程測試：

```bash
npm run test:ui
```

`test:ui` 目前是以 JSDOM 執行的 UI 回歸測試（不是完整真實瀏覽器 E2E），主要涵蓋刪除/復原流程、右鍵選單操作與時間軸事件顯示切換。

執行 Rust 後端測試：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## 🔒 安全模型

- 後端會依資料來源驗證所有請求路徑，在讀取、解析或刪除前拒絕來源允許根目錄之外的檔案。
- Claude sessions 會從 Windows 的 `%USERPROFILE%\\.claude\\projects` 與 Unix-like 系統的 `~/.claude/projects` 探索。
- Codex CLI sessions 會從 `~/.codex` 探索。
- VS Code / GitHub Copilot Chat records 會從 VS Code workspace storage 探索，目前 UI 中以唯讀方式處理。
- Claude 專案刪除會移除整個專案目錄樹。
- Codex 專案刪除會移除該專案對應的 Codex session files。
- Session 刪除是 source-specific，只有後端已有刪除支援的來源才會在 UI 中顯示。

## 🤝 貢獻方式

歡迎提出 Issue 或 Pull Request。建議以小而明確的變更為主，並附上重現步驟與預期結果。

## 📄 授權

MIT
