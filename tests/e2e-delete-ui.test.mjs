import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { JSDOM } from "jsdom";

const ROOT = process.cwd();
const INDEX_HTML = path.join(ROOT, "src", "index.html");

function createMockInvoke(options = {}) {
  const calls = [];
  const includeVscode = options.includeVscode === true;
  const claudeProjects = options.vscodeOnly
    ? []
    : [
        {
          name: "demo-project",
          path: "D:/mock/demo-project",
          cwdPath: "D:/mock/demo-project",
          modifiedMs: Date.now(),
        },
      ];
  const codexProjects = options.vscodeOnly
    ? []
    : [
        {
          name: "demo-project",
          path: "D:/mock/demo-project",
          cwdPath: "D:/mock/demo-project",
          modifiedMs: Date.now() - 500,
        },
      ];
  const claudeEntries = [
    {
      entryType: "session",
      label: "alpha.jsonl",
      path: "D:/mock/demo-project/alpha.jsonl",
      parentSession: null,
      modifiedMs: Date.now(),
      sizeBytes: 120,
    },
    {
      entryType: "session",
      label: "beta.jsonl",
      path: "D:/mock/demo-project/beta.jsonl",
      parentSession: null,
      modifiedMs: Date.now() - 1000,
      sizeBytes: 88,
    },
    {
      entryType: "subagent_session",
      label: "alpha-child.jsonl",
      path: "D:/mock/demo-project/alpha/subagents/alpha-child.jsonl",
      parentSession: "alpha",
      modifiedMs: Date.now(),
      sizeBytes: 96,
    },
    {
      entryType: "memory_file",
      label: "MEMORY.md",
      path: "D:/mock/demo-project/memory/MEMORY.md",
      parentSession: null,
      modifiedMs: Date.now(),
      sizeBytes: 44,
    },
  ];
  const codexEntries = [
    {
      entryType: "session",
      label: "codex-session.jsonl",
      path: "C:/Users/mock/.codex/sessions/2026/03/codex-session.jsonl",
      parentSession: null,
      modifiedMs: Date.now() - 2000,
      sizeBytes: 200,
      source: "codex",
    },
  ];
  if (options.includeHiddenCodexEntries !== false) {
    codexEntries.push({
      entryType: "session",
      label: "hidden-codex-session.jsonl",
      path: "C:/Users/mock/.codex/sessions/2026/03/hidden-codex-session.jsonl",
      parentSession: null,
      modifiedMs: Date.now() - 3000,
      sizeBytes: 80,
      source: "codex",
      hidden: true,
    });
  }
  const vscodeProjects = includeVscode
    ? [
        {
          name: "demo-project",
          path: "D:/mock/demo-project",
          cwdPath: "D:/mock/demo-project",
          modifiedMs: Date.now() - 250,
          source: "vscode",
        },
      ]
    : [];
  const vscodeEntries = includeVscode
    ? [
        {
          entryType: "session",
          label: "vscode-session.json",
          path: "C:/Users/mock/AppData/Roaming/Code/User/workspaceStorage/hash/chatSessions/vscode-session.json",
          parentSession: null,
          modifiedMs: Date.now() - 300,
          sizeBytes: 320,
          source: "vscode",
        },
      ]
    : [];

  const invoke = async (cmd, args = {}) => {
    calls.push({ cmd, args });
    if (cmd === "list_projects") return claudeProjects;
    if (cmd === "list_codex_projects") return codexProjects;
    if (cmd === "list_vscode_copilot_projects") return vscodeProjects;
    if (cmd === "list_project_entries") return claudeEntries;
    if (cmd === "list_codex_project_entries") return codexEntries;
    if (cmd === "list_vscode_copilot_project_entries") return vscodeEntries;
    if (cmd === "get_project_delete_impact") {
      return {
        sessionCount: 2,
        subagentSessionCount: 1,
        memoryFileCount: 1,
        totalFileCount: 4,
        totalSizeBytes: 460,
      };
    }
    if (cmd === "read_session_timeline") {
      return {
        path: args.sessionPath,
        errorCode: null,
        errors: [],
        events: [
          {
            line: 1,
            timestamp: "2026-03-04T10:00:00Z",
            role: "user",
            eventType: "message",
            summary: "hello",
            raw: {
              type: "user",
              timestamp: "2026-03-04T10:00:00Z",
              message: {
                role: "user",
                content: [{ type: "text", text: "hello" }],
              },
            },
          },
          {
            line: 2,
            timestamp: "2026-03-04T10:02:05Z",
            role: "assistant",
            eventType: "message",
            summary: "world",
            raw: {
              type: "assistant",
              timestamp: "2026-03-04T10:02:05Z",
              message: {
                role: "assistant",
                model: "claude-sonnet-4-6",
                content: [
                  { type: "text", text: "world" },
                  { type: "thinking", thinking: "plan silently" },
                  { type: "tool_use", id: "toolu_123", name: "Bash", input: { command: "echo hi" } },
                ],
                usage: {
                  input_tokens: 1,
                  output_tokens: 403,
                },
              },
            },
          },
          {
            line: 21,
            timestamp: "2026-03-04T10:02:10Z",
            role: "assistant",
            eventType: "message",
            summary: "meta note",
            raw: {
              type: "assistant",
              isMeta: true,
              timestamp: "2026-03-04T10:02:10Z",
              message: {
                role: "assistant",
                content: [{ type: "text", text: "meta note" }],
              },
            },
          },
          {
            line: 22,
            timestamp: "2026-03-04T10:02:11Z",
            role: "user",
            eventType: "message",
            summary: "command call",
            raw: {
              type: "user",
              uuid: "cmd-1",
              timestamp: "2026-03-04T10:02:11Z",
              message: {
                role: "user",
                content:
                  "<command-name>/mcp</command-name>\n            <command-message>mcp</command-message>\n            <command-args>disable pencil</command-args>",
              },
            },
          },
          {
            line: 23,
            timestamp: "2026-03-04T10:02:12Z",
            role: "user",
            eventType: "message",
            summary: "local command output",
            raw: {
              type: "user",
              parentUuid: "cmd-1",
              timestamp: "2026-03-04T10:02:12Z",
              message: {
                role: "user",
                content: "<local-command-stdout>MCP server \"pencil\" disabled</local-command-stdout>",
              },
            },
          },
          {
            line: 3,
            timestamp: "2026-03-04T10:02:20Z",
            role: "user",
            eventType: "tool_result",
            summary: "tool result",
            raw: {
              type: "user",
              timestamp: "2026-03-04T10:02:20Z",
              message: {
                role: "user",
                content: [{ type: "tool_result", content: "done" }],
              },
              toolUseResult: {
                commandName: "Bash",
                success: true,
                stdout: "done",
                stderr: "",
                interrupted: false,
                isImage: false,
                noOutputExpected: false,
              },
            },
          },
          {
            line: 4,
            timestamp: "2026-03-04T10:03:00Z",
            role: null,
            eventType: "system",
            summary: "turn duration",
            raw: {
              type: "system",
              subtype: "turn_duration",
              durationMs: 60000,
              timestamp: "2026-03-04T10:03:00Z",
            },
          },
          {
            line: 5,
            timestamp: "2026-03-04T10:04:00Z",
            role: null,
            eventType: "system",
            summary: "turn duration",
            raw: {
              type: "system",
              subtype: "turn_duration",
              durationMs: 120000,
              timestamp: "2026-03-04T10:04:00Z",
            },
          },
        ],
        metadata: {
          modelName: "claude-sonnet-4-5",
          totalInputTokens: 1234,
          totalOutputTokens: 567,
          startTime: "2026-03-04T10:00:00Z",
          endTime: "2026-03-04T10:02:05Z",
        },
      };
    }
    if (cmd === "read_vscode_copilot_session_timeline") {
      return {
        path: args.sessionPath,
        errorCode: null,
        errors: [],
        events: [
          {
            line: 1,
            timestamp: "1779194731541",
            role: "assistant",
            eventType: "vscode_turn",
            requestId: "vscode-req-1",
            summary: "show me the test plan\nVS Code answer",
            raw: {
              type: "vscode_turn",
              vscodeTurn: {
                response: {
                  type: "message",
                  message: {
                    role: "assistant",
                    model: "gpt-5.4",
                    content: [
                      { type: "text", text: "VS Code answer" },
                      { type: "thinking", thinking: "VS Code hidden reasoning" },
                      {
                        type: "tool_use",
                        id: "vscode-tool-1",
                        name: "copilot_readFile",
                        input: { invocationMessage: "Reading package.json" },
                      },
                      {
                        type: "tool_use",
                        id: "vscode-file-1",
                        name: "VSCodeFileChange",
                        input: { file_path: "/tmp/app.js", edit_count: 2, done: true },
                      },
                    ],
                  },
                },
                request: {
                  type: "message",
                  message: {
                    role: "user",
                    content: [{ type: "text", text: "show me the test plan" }],
                  },
                },
              },
            },
          },
        ],
        metadata: {
          modelName: "copilot",
          totalInputTokens: null,
          totalOutputTokens: null,
          startTime: "2026-03-05T09:00:00Z",
          endTime: "2026-03-05T09:01:00Z",
        },
      };
    }
    if (cmd === "read_codex_session_timeline") {
      return {
        path: args.sessionPath,
        errorCode: null,
        errors: [],
        events: [
          {
            line: 1,
            timestamp: "2026-03-06T08:00:00Z",
            role: "user",
            eventType: "event_msg",
            subtype: "user_message",
            summary: "Plan this parser",
            raw: {
              timestamp: "2026-03-06T08:00:00Z",
              type: "event_msg",
              payload: {
                type: "user_message",
                message: "Plan this parser",
              },
            },
          },
          {
            line: 11,
            timestamp: "2026-03-06T08:00:00Z",
            role: "user",
            eventType: "response_item",
            subtype: "message",
            summary: "Plan this parser",
            raw: {
              timestamp: "2026-03-06T08:00:00Z",
              type: "response_item",
              payload: {
                type: "message",
                role: "user",
                content: [
                  { type: "input_text", text: "Plan this parser" },
                ],
              },
            },
          },
          {
            line: 12,
            timestamp: "2026-03-06T08:00:01Z",
            role: "user",
            eventType: "response_item",
            subtype: "message",
            summary: "contextual user fragments",
            raw: {
              timestamp: "2026-03-06T08:00:01Z",
              type: "response_item",
              payload: {
                type: "message",
                role: "user",
                content: [
                  {
                    type: "input_text",
                    text: "# AGENTS.md instructions for D:\\Hank\\Dropbox\\AI-Project\\Unified-AI-Session-Explorer\n\n<INSTRUCTIONS>TXT</INSTRUCTIONS>",
                  },
                  {
                    type: "input_text",
                    text: "<environment_context>\n  <cwd>D:\\Hank\\Dropbox\\AI-Project\\Unified-AI-Session-Explorer</cwd>\n  <shell>powershell</shell>\n</environment_context>",
                  },
                ],
              },
            },
          },
          {
            line: 2,
            timestamp: "2026-03-06T08:00:03Z",
            role: "assistant",
            eventType: "event_msg",
            subtype: "agent_message",
            summary: "Use response_item for full content",
            raw: {
              timestamp: "2026-03-06T08:00:03Z",
              type: "event_msg",
              payload: {
                type: "agent_message",
                message: "Use response_item for full content",
              },
            },
          },
          {
            line: 21,
            timestamp: "2026-03-06T08:00:03Z",
            role: "assistant",
            eventType: "response_item",
            subtype: "message",
            summary: "Use response_item for full content",
            raw: {
              timestamp: "2026-03-06T08:00:03Z",
              type: "response_item",
              payload: {
                type: "message",
                role: "assistant",
                content: [
                  { type: "output_text", text: "Use response_item for full content" },
                ],
              },
            },
          },
          {
            line: 3,
            timestamp: "2026-03-06T08:00:04Z",
            role: "user",
            eventType: "event_msg",
            subtype: "user_message",
            summary: "[Image]",
            raw: {
              timestamp: "2026-03-06T08:00:04Z",
              type: "event_msg",
              payload: {
                type: "user_message",
                message: "",
                local_images: [{ path: "C:/tmp/screenshot.png" }],
              },
            },
          },
          {
            line: 4,
            timestamp: "2026-03-06T08:00:05Z",
            role: "assistant",
            eventType: "response_item",
            subtype: "message",
            summary: "First chunk\nSecond chunk",
            raw: {
              timestamp: "2026-03-06T08:00:05Z",
              type: "response_item",
              payload: {
                type: "message",
                role: "assistant",
                content: [
                  { type: "output_text", text: "First chunk" },
                  { type: "output_text", text: "Second chunk" },
                ],
              },
            },
          },
          {
            line: 5,
            timestamp: "2026-03-06T08:00:06Z",
            role: "assistant",
            eventType: "event_msg",
            subtype: "agent_reasoning",
            summary: "Consider rollout structure",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "event_msg",
              payload: {
                type: "agent_reasoning",
                text: "Consider rollout structure",
              },
            },
          },
          {
            line: 53,
            timestamp: "2026-03-06T08:00:06Z",
            role: "assistant",
            eventType: "event_msg",
            subtype: "agent_reasoning_raw_content",
            summary: "Raw reasoning stream",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "event_msg",
              payload: {
                type: "agent_reasoning_raw_content",
                text: "Raw reasoning stream",
              },
            },
          },
          {
            line: 51,
            timestamp: "2026-03-06T08:00:06Z",
            role: "assistant",
            eventType: "event_msg",
            subtype: "agent_message",
            summary: "I will inspect the rollout parser.",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "event_msg",
              payload: {
                type: "agent_message",
                message: "I will inspect the rollout parser.",
                phase: "commentary",
              },
            },
          },
          {
            line: 54,
            timestamp: "2026-03-06T08:00:06Z",
            role: "user",
            eventType: "response_item",
            subtype: "message",
            summary: "<goal_context>Keep rollout display tidy</goal_context>",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "response_item",
              payload: {
                type: "message",
                role: "user",
                content: [
                  { type: "input_text", text: "<goal_context>Keep rollout display tidy</goal_context>" },
                ],
              },
            },
          },
          {
            line: 55,
            timestamp: "2026-03-06T08:00:06Z",
            role: null,
            eventType: "response_item",
            subtype: "message",
            summary: "<developer_instructions>Switch model guidance</developer_instructions>",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "response_item",
              payload: {
                type: "message",
                role: "developer",
                content: [
                  { type: "input_text", text: "<developer_instructions>Switch model guidance</developer_instructions>" },
                ],
              },
            },
          },
          {
            line: 56,
            timestamp: "2026-03-06T08:00:06Z",
            role: null,
            eventType: "response_item",
            subtype: "local_shell_call",
            summary: "shell command",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "response_item",
              payload: {
                type: "local_shell_call",
                call_id: "local_shell_1",
                status: "completed",
                action: {
                  type: "exec",
                  command: "npm run test:ui",
                },
              },
            },
          },
          {
            line: 52,
            timestamp: "2026-03-06T08:00:06Z",
            role: "assistant",
            eventType: "response_item",
            subtype: "reasoning",
            summary: "",
            raw: {
              timestamp: "2026-03-06T08:00:06Z",
              type: "response_item",
              payload: {
                type: "reasoning",
                encrypted_content: "encrypted",
              },
            },
          },
          {
            line: 6,
            timestamp: "2026-03-06T08:00:07Z",
            role: null,
            eventType: "response_item",
            subtype: "function_call",
            toolUseId: "call_structured",
            operation: "shell_command",
            summary: "shell_command {}",
            raw: {
              timestamp: "2026-03-06T08:00:07Z",
              type: "response_item",
              payload: {
                type: "function_call",
                name: "shell_command",
                arguments: "{\"command\":\"pwd\"}",
                call_id: "call_structured",
              },
            },
          },
          {
            line: 7,
            timestamp: "2026-03-06T08:00:08Z",
            role: null,
            eventType: "response_item",
            subtype: "function_call_output",
            toolUseId: "call_structured",
            summary: "D:/repo/demo",
            raw: {
              timestamp: "2026-03-06T08:00:08Z",
              type: "response_item",
              payload: {
                type: "function_call_output",
                call_id: "call_structured",
                output: {
                  content_items: [
                    { type: "output_text", text: "D:/repo/demo" },
                  ],
                },
              },
            },
          },
        ],
        metadata: {
          modelName: "gpt-5.4",
          totalInputTokens: 0,
          totalOutputTokens: 0,
          startTime: "2026-03-06T08:00:00Z",
          endTime: "2026-03-06T08:00:03Z",
        },
      };
    }
    if (cmd === "read_memory") {
      return { path: args.memoryPath, content: "mock-memory" };
    }
    if (cmd === "delete_session") return null;
    if (cmd === "delete_codex_session") return null;
    if (cmd === "delete_project") return null;
    throw new Error(`Unhandled command: ${cmd}`);
  };

  return { invoke, calls };
}

async function setupApp(options = {}) {
  const html = await fs.readFile(INDEX_HTML, "utf8");
  const dom = new JSDOM(html, { url: "http://localhost/" });
  const { window } = dom;

  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.localStorage = window.localStorage;
  globalThis.HTMLElement = window.HTMLElement;
  globalThis.HTMLDialogElement = window.HTMLDialogElement;
  globalThis.Event = window.Event;
  globalThis.MouseEvent = window.MouseEvent;
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    writable: true,
    value: window.navigator,
  });

  window.matchMedia =
    window.matchMedia ||
    (() => ({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
  const nativeSetTimeout = setTimeout;
  window.setTimeout = (callback, delay = 0, ...args) => {
    if (options.immediateDeleteTimers && delay >= 8000) {
      return nativeSetTimeout(() => callback(...args), 0);
    }
    return nativeSetTimeout(() => callback(...args), delay);
  };
  window.clearTimeout = clearTimeout;
  window.requestAnimationFrame = (callback) => setTimeout(callback, 0);
  globalThis.requestAnimationFrame = window.requestAnimationFrame;

  if (window.HTMLDialogElement) {
    if (!window.HTMLDialogElement.prototype.showModal) {
      window.HTMLDialogElement.prototype.showModal = function showModal() {
        this.open = true;
      };
    }
    if (!window.HTMLDialogElement.prototype.close) {
      window.HTMLDialogElement.prototype.close = function close() {
        this.open = false;
      };
    }
  }

  const mock = createMockInvoke(options);
  const openedPaths = [];
  const revealedPaths = [];
  window.__TAURI__ = {
    core: { invoke: mock.invoke },
    opener: {
      openPath: async (p) => { openedPaths.push(p); },
      revealItemInDir: async (p) => { revealedPaths.push(p); },
    },
  };

  await import(`../src/main.js?e2e=${Date.now()}-${Math.random()}`);
  window.dispatchEvent(new window.Event("DOMContentLoaded"));
  await new Promise((resolve) => setTimeout(resolve, 30));

  return { window, mock, openedPaths, revealedPaths, cleanup: () => dom.window.close() };
}

test("project delete dialog shows impact and confirms by exact name", async () => {
  const app = await setupApp();
  const { window, mock } = app;

  const projectRow = window.document.querySelector(".project-btn")?.closest(".list-row");
  assert.ok(projectRow, "project row should exist");
  projectRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));
  const deleteMenuItem = [...window.document.querySelectorAll(".ctx-menu-item")]
    .find((el) => /delete|刪除/i.test(el.textContent));
  assert.ok(deleteMenuItem, "context menu should have a delete item");
  deleteMenuItem.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const dialog = window.document.querySelector("#project-delete-dialog");
  const impact = window.document.querySelector("#project-delete-impact");
  const input = window.document.querySelector("#project-delete-input");
  const confirm = window.document.querySelector("#project-delete-confirm");

  assert.equal(dialog.open, true);
  assert.match(impact.textContent, /Will delete/i);
  assert.equal(confirm.disabled, true);

  input.value = "wrong-name";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  assert.equal(confirm.disabled, true);

  input.value = "demo-project";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  assert.equal(confirm.disabled, false);

  const impactCalls = mock.calls.filter((call) => call.cmd === "get_project_delete_impact");
  assert.equal(impactCalls.length, 1);
  assert.deepEqual(impactCalls[0].args, {
    projectPath: "D:/mock/demo-project",
    codexCwd: "D:/mock/demo-project",
  });
  app.cleanup();
});

test("project delete removes hybrid project without source-toggle resurrection", async () => {
  const app = await setupApp({ immediateDeleteTimers: true });
  const { window, mock } = app;

  const projectRow = window.document.querySelector(".project-btn")?.closest(".list-row");
  assert.ok(projectRow, "project row should exist");
  projectRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));

  const deleteMenuItem = [...window.document.querySelectorAll(".ctx-menu-item")]
    .find((el) => /delete|刪除/i.test(el.textContent));
  assert.ok(deleteMenuItem, "context menu should have a delete item");
  deleteMenuItem.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const input = window.document.querySelector("#project-delete-input");
  const confirm = window.document.querySelector("#project-delete-confirm");
  input.value = "demo-project";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  confirm.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const deleteCalls = mock.calls.filter((call) => call.cmd === "delete_project");
  assert.equal(deleteCalls.length, 1);
  assert.deepEqual(deleteCalls[0].args, {
    projectPath: "D:/mock/demo-project",
    codexCwd: "D:/mock/demo-project",
  });

  assert.equal(window.document.querySelector(".project-btn"), null);

  window.document.querySelector("#source-toggle-claude").click();
  window.document.querySelector("#source-toggle-claude").click();
  window.document.querySelector("#source-toggle-codex").click();
  window.document.querySelector("#source-toggle-codex").click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  assert.equal(window.document.querySelector(".project-btn"), null);
  app.cleanup();
});

test("vscode copilot projects list entries and render timelines", async () => {
  const app = await setupApp({ includeVscode: true, vscodeOnly: true });
  const { window, mock } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton, "VS Code project row should exist");
  assert.match(projectButton.textContent, /demo-project/);
  assert.equal(window.document.querySelector(".source-badge--vscode")?.textContent, "VS Code");

  projectButton.closest(".list-row").dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));
  let menuItems = [...window.document.querySelectorAll(".ctx-menu-item")];
  assert.equal(menuItems.some((el) => /delete/i.test(el.textContent)), false);
  window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const entryRow = window.document.querySelector(".entry-row.list-row");
  assert.ok(entryRow, "VS Code entry row should exist");
  assert.equal(entryRow.querySelector(".source-badge--vscode")?.textContent, "VS Code");
  assert.equal(
    mock.calls.some((call) => call.cmd === "list_vscode_copilot_project_entries"),
    true,
  );

  entryRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 120, clientY: 120 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));
  menuItems = [...window.document.querySelectorAll(".ctx-menu-item")];
  assert.equal(menuItems.some((el) => /delete/i.test(el.textContent)), false);
  window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

  entryRow.querySelector(".entry-btn").click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  assert.equal(
    mock.calls.some((call) => call.cmd === "read_vscode_copilot_session_timeline"),
    true,
  );
  assert.match(window.document.body.textContent, /VS Code answer/);
  const chatTexts = [...window.document.querySelectorAll(".user-msg-text, .assist-text")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.ok(chatTexts.indexOf("show me the test plan") > -1);
  assert.ok(chatTexts.indexOf("VS Code answer") > -1);
  assert.equal(chatTexts.indexOf("show me the test plan") < chatTexts.indexOf("VS Code answer"), true);
  const vscodeAssistantTitle = [...window.document.querySelectorAll(".role-lbl.assist-role")]
    .map((node) => (node.textContent || "").trim())
    .find((text) => text.startsWith("VS Code"));
  assert.equal(vscodeAssistantTitle, "VS Code (gpt-5.4)");
  const timeLabels = [...window.document.querySelectorAll(".time-lbl")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(timeLabels.some((text) => text.includes("1779194731541")), false);
  assert.equal(timeLabels.some((text) => /\d{4}/.test(text)), true);
  assert.equal(window.document.querySelectorAll(".assistant-thinking-text").length, 0);
  assert.equal(window.document.querySelectorAll(".assistant-tool-title").length, 0);

  window.document.querySelector("#hide-thinking-events-toggle").click();
  window.document.querySelector("#hide-tool-events-toggle").click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const thinkingTexts = [...window.document.querySelectorAll(".assistant-thinking-text")]
    .map((node) => (node.textContent || "").trim());
  const toolTitles = [...window.document.querySelectorAll(".assistant-tool-title")]
    .map((node) => (node.textContent || "").trim());
  const toolLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim());
  assert.equal(thinkingTexts.some((text) => /VS Code hidden reasoning/.test(text)), true);
  assert.equal(toolTitles.some((text) => /(Copilot 工具|Copilot tool): copilot_readFile/.test(text)), true);
  assert.equal(toolTitles.some((text) => /(VS Code 檔案變更|VS Code file changes)/.test(text)), true);
  assert.equal(toolLines.some((text) => /\/tmp\/app\.js/.test(text)), true);

  window.document.querySelector("#source-toggle-vscode").click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  assert.equal(window.document.querySelector("#source-toggle-vscode").getAttribute("aria-pressed"), "false");
  assert.equal(window.document.querySelector(".project-btn"), null);
  app.cleanup();
});

test("codex event_msg user and agent messages render as chat bubbles", async () => {
  const app = await setupApp();
  const { window, mock } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const codexEntryButton = window.document.querySelector('.entry-btn[title="codex-session.jsonl"]');
  assert.ok(codexEntryButton, "Codex entry should exist");
  assert.equal(
    window.document.querySelector('.entry-btn[title="hidden-codex-session.jsonl"]'),
    null,
    "hidden Codex entry should be hidden by default",
  );
  assert.equal(
    window.document.querySelector(".entries-show-hidden-toggle"),
    null,
    "FILES title should not include show hidden checkbox",
  );
  codexEntryButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  assert.equal(
    mock.calls.some((call) => call.cmd === "read_codex_session_timeline"),
    true,
  );
  const chatTexts = [...window.document.querySelectorAll(".user-msg-text, .assist-text")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.ok(chatTexts.includes("Plan this parser"));
  assert.equal(chatTexts.filter((text) => text === "Plan this parser").length, 1);
  assert.equal(chatTexts.some((text) => text.includes("AGENTS.md instructions")), false);
  assert.equal(chatTexts.some((text) => text.includes("<environment_context>")), false);
  assert.ok(chatTexts.includes("Use response_item for full content"));
  assert.ok(chatTexts.includes("[Image]"));
  assert.ok(chatTexts.includes("First chunk\nSecond chunk"));
  assert.equal(
    chatTexts.filter((text) => text === "Use response_item for full content").length,
    1,
  );
  assert.equal(chatTexts.includes("I will inspect the rollout parser."), false);

  window.document.querySelector("#hide-thinking-events-toggle").click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  const thinkingTexts = [...window.document.querySelectorAll(".assistant-thinking-text")]
    .map((node) => (node.textContent || "").trim());
  assert.ok(thinkingTexts.includes("Consider rollout structure"));
  assert.ok(thinkingTexts.includes("Raw reasoning stream"));
  assert.ok(thinkingTexts.includes("I will inspect the rollout parser."));
  assert.equal(
    thinkingTexts.filter((text) => text === "I will inspect the rollout parser.").length,
    1,
  );
  assert.equal(
    thinkingTexts.some((text) => /encrypted|已加密/i.test(text)),
    false,
  );
  window.document.querySelector("#hide-tool-events-toggle").click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  const toolLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim());
  assert.ok(toolLines.includes("D:/repo/demo"));
  window.document.querySelector("#hide-system-events-toggle").click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  const techHeaders = [...window.document.querySelectorAll(".tool-hdr-txt")]
    .map((node) => (node.textContent || "").trim());
  assert.ok(techHeaders.some((text) => /contextual_user_fragments/i.test(text)));
  assert.ok(techHeaders.some((text) => /developer_message/i.test(text)));
  assert.ok(techHeaders.some((text) => /local_shell_call/i.test(text)));
  assert.equal(chatTexts.some((text) => text.includes("<goal_context>")), false);

  app.cleanup();
});

test("session delete requires confirmation modal and supports undo", async () => {
  const app = await setupApp();
  const { window, mock } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const entryRow = window.document.querySelector(".entry-row.list-row");
  assert.ok(entryRow, "entry row should exist");
  entryRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));
  const deleteMenuItem = [...window.document.querySelectorAll(".ctx-menu-item")]
    .find((el) => /delete|刪除/i.test(el.textContent));
  assert.ok(deleteMenuItem, "context menu should have a delete item");
  deleteMenuItem.click();

  const sessionDialog = window.document.querySelector("#session-delete-dialog");
  assert.equal(sessionDialog.open, true);
  assert.equal(mock.calls.some((call) => call.cmd === "delete_session"), false);

  const confirm = window.document.querySelector("#session-delete-confirm");
  confirm.click();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(sessionDialog.open, false);
  assert.equal(mock.calls.some((call) => call.cmd === "delete_session"), false);

  const toastViewport = window.document.querySelector("#undo-toast-viewport");
  assert.equal(toastViewport.hidden, false);
  const toast = toastViewport.querySelector(".undo-toast");
  assert.ok(toast);
  const undo = toast.querySelector(".undo-toast-btn");
  undo.click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(mock.calls.some((call) => call.cmd === "delete_session"), false);
  assert.equal(mock.calls.some((call) => call.cmd === "list_project_entries"), true);

  app.cleanup();
});

test("multiple session deletes keep separate undo toasts", async () => {
  const app = await setupApp();
  const { window, mock } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButtons = [...window.document.querySelectorAll('.entry-btn[data-entry-type="session"]')];
  assert.equal(sessionButtons.length >= 2, true);

  for (const sessionButton of sessionButtons.slice(0, 2)) {
    sessionButton.dispatchEvent(
      new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
    );
    await new Promise((resolve) => setTimeout(resolve, 10));

    const deleteMenuItem = [...window.document.querySelectorAll(".ctx-menu-item")]
      .find((el) => /delete|刪除/i.test(el.textContent));
    assert.ok(deleteMenuItem, "context menu should have a delete item");
    deleteMenuItem.click();

    const confirm = window.document.querySelector("#session-delete-confirm");
    confirm.click();
    await new Promise((resolve) => setTimeout(resolve, 10));
  }

  const toastViewport = window.document.querySelector("#undo-toast-viewport");
  const toasts = [...toastViewport.querySelectorAll(".undo-toast")];
  assert.equal(toasts.length, 2);
  assert.match(toasts[0].textContent || "", /alpha|beta/i);
  assert.match(toasts[1].textContent || "", /alpha|beta/i);

  const secondUndo = toasts[1].querySelector(".undo-toast-btn");
  secondUndo.click();
  await new Promise((resolve) => setTimeout(resolve, 60));

  const remainingToasts = [...toastViewport.querySelectorAll(".undo-toast")];
  assert.equal(remainingToasts.length, 1);
  assert.equal(mock.calls.some((call) => call.cmd === "delete_session"), false);
  // 只計算非 Codex 的 sessions（Codex session 不在本次刪除範圍內）
  const visibleClaudeSessionButtons = [
    ...window.document.querySelectorAll('.entry-btn[data-entry-type="session"]'),
  ].filter((el) => !el.querySelector('.source-badge--codex'))
    .map((el) => el.textContent || "");
  assert.equal(visibleClaudeSessionButtons.length, 1, JSON.stringify(visibleClaudeSessionButtons));

  const finalUndo = remainingToasts[0].querySelector(".undo-toast-btn");
  finalUndo.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  app.cleanup();
});

test("chat title shows model before line number", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButton = window.document.querySelector('.entry-btn[data-entry-type="session"]');
  assert.ok(sessionButton);
  sessionButton.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const assistantHeader = window.document.querySelector(".assist-row .msg-header");
  assert.ok(assistantHeader);
  const headerText = assistantHeader.textContent || "";
  const modelIndex = headerText.indexOf("model:claude-sonnet-4-6");
  const lineIndex = headerText.indexOf("line 2");
  assert.ok(modelIndex >= 0);
  assert.ok(lineIndex >= 0);
  assert.ok(modelIndex < lineIndex);

  app.cleanup();
});

test("chat header does not show tool:* badges", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButton = window.document.querySelector('.entry-btn[data-entry-type="session"]');
  assert.ok(sessionButton);
  sessionButton.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const badgeTexts = [...window.document.querySelectorAll(".msg-header .tag-badge")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(badgeTexts.some((text) => /^tool:/i.test(text)), false);

  app.cleanup();
});

test("command xml text is rendered as compact command line", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButton = window.document.querySelector('.entry-btn[data-entry-type="session"]');
  assert.ok(sessionButton);
  sessionButton.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const toolToggle = window.document.querySelector("#hide-tool-events-toggle");
  assert.ok(toolToggle);
  toolToggle.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const assistantTexts = [...window.document.querySelectorAll(".assist-text, .user-msg-text")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(
    assistantTexts.some((text) => /command:\s*\/mcp\s+disable\s+pencil/i.test(text)),
    true,
  );
  const toolTitles = [...window.document.querySelectorAll(".assistant-tool-title")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(toolTitles.some((text) => /command:\s*\/mcp\s+disable\s+pencil/i.test(text)), false);
  const toolLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(toolLines.some((text) => /(回傳結果|Result):\s*MCP server \"pencil\" disabled/i.test(text)), true);

  app.cleanup();
});

test("toolUseResult commandName is parsed and rendered in tool result panel", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButton = window.document.querySelector('.entry-btn[data-entry-type="session"]');
  assert.ok(sessionButton);
  sessionButton.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const toolToggle = window.document.querySelector("#hide-tool-events-toggle");
  assert.ok(toolToggle);
  toolToggle.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const toolResultLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(toolResultLines.some((text) => /stdout:\s*done/i.test(text)), true);

  app.cleanup();
});

test("event toggles independently control tool/thinking while system events are hidden", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButton = window.document.querySelector('.entry-btn[data-entry-type="session"]');
  assert.ok(sessionButton);
  sessionButton.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const systemToggle = window.document.querySelector("#hide-system-events-toggle");
  const toolToggle = window.document.querySelector("#hide-tool-events-toggle");
  const thinkingToggle = window.document.querySelector("#hide-thinking-events-toggle");
  assert.ok(systemToggle);
  assert.ok(toolToggle);
  assert.ok(thinkingToggle);
  assert.equal(systemToggle.getAttribute("aria-pressed"), "true");
  assert.equal(toolToggle.getAttribute("aria-pressed"), "true");
  assert.equal(thinkingToggle.getAttribute("aria-pressed"), "true");

  let toolResultLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(toolResultLines.some((text) => /stdout:\s*done/i.test(text)), false);
  assert.equal(window.document.querySelectorAll(".assistant-thinking-text").length, 0);
  let assistantTexts = [...window.document.querySelectorAll(".assist-text")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(assistantTexts.includes("meta note"), false);

  toolToggle.click();
  thinkingToggle.click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  toolResultLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(toolResultLines.some((text) => /stdout:\s*done/i.test(text)), true);
  assert.equal(window.document.querySelectorAll(".assistant-thinking-text").length > 0, true);
  assistantTexts = [...window.document.querySelectorAll(".assist-text")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(assistantTexts.includes("meta note"), true);

  toolToggle.click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  toolResultLines = [...window.document.querySelectorAll(".assistant-tool-line")]
    .map((node) => (node.textContent || "").trim())
    .filter(Boolean);
  assert.equal(toolResultLines.some((text) => /stdout:\s*done/i.test(text)), false);
  assert.equal(
    toolResultLines.some((text) => /(回傳結果|Result):\s*MCP server \"pencil\" disabled/i.test(text)),
    true,
  );

  thinkingToggle.click();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(window.document.querySelectorAll(".assistant-thinking-text").length, 0);

  app.cleanup();
});

test("viewer meta shows total minutes from system turn_duration events", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const sessionButton = window.document.querySelector('.entry-btn[data-entry-type="session"]');
  assert.ok(sessionButton);
  sessionButton.click();
  await new Promise((resolve) => setTimeout(resolve, 30));

  const metaText = (window.document.querySelector("#viewer-meta-time")?.textContent || "").trim();
  assert.equal(/3\s*(?:分鐘|min)/i.test(metaText), true);

  app.cleanup();
});

test("subagent toggle button has clear state and purpose", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const toggle = window.document.querySelector(".entry-toggle");
  assert.ok(toggle);
  assert.equal(toggle.getAttribute("aria-expanded"), "false");
  assert.equal(/(子對話|Subagent)/.test(toggle.getAttribute("aria-label") || ""), true);
  assert.equal(/(子對話|Subagent)/.test(toggle.getAttribute("data-label") || ""), true);
  assert.equal((toggle.textContent || "").includes("▸"), true);

  toggle.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const expandedToggle = window.document.querySelector(".entry-toggle");
  assert.ok(expandedToggle);
  assert.equal(expandedToggle.getAttribute("aria-expanded"), "true");
  assert.equal(/(子對話|Subagent)/.test(expandedToggle.getAttribute("aria-label") || ""), true);
  assert.equal(/(子對話|Subagent)/.test(expandedToggle.getAttribute("data-label") || ""), true);
  assert.equal((expandedToggle.textContent || "").includes("▾"), true);

  app.cleanup();
});

test("project context menu shows open folder and delete options", async () => {
  const app = await setupApp();
  const { window, openedPaths } = app;

  const projectRow = window.document.querySelector(".project-btn")?.closest(".list-row");
  assert.ok(projectRow, "project row should exist");
  projectRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));

  const items = window.document.querySelectorAll(".ctx-menu-item");
  assert.equal(items.length, 2, "project context menu should have 2 items");
  assert.match(items[0].textContent, /Open folder|開啟資料夾/i);
  assert.match(items[1].textContent, /Delete|刪除/i);

  // 點擊開啟資料夾
  items[0].click();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(openedPaths.length, 1, "opener.openPath should be called once");
  assert.match(openedPaths[0], /demo-project/i);

  // 選單應已關閉
  assert.equal(window.document.querySelector("#ctx-menu").hidden, true, "menu should be hidden after click");

  app.cleanup();
});

test("session context menu shows copy session id, open file location, and delete options", async () => {
  const app = await setupApp();
  const { window, revealedPaths } = app;

  // 先選取專案，讓 entries 出現
  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton, "project button should exist");
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  const entryRow = window.document.querySelector(".entry-row.list-row");
  assert.ok(entryRow, "entry row should exist");
  entryRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));

  const items = window.document.querySelectorAll(".ctx-menu-item");
  assert.equal(items.length, 3, "session context menu should have 3 items");
  assert.match(items[0].textContent, /Copy Session ID|複製 Session ID/i);
  assert.match(items[1].textContent, /Open file location|開啟檔案位置/i);
  assert.match(items[2].textContent, /Delete|刪除/i);

  items[1].click();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(revealedPaths, ["D:/mock/demo-project/alpha.jsonl"]);

  app.cleanup();
});

test("context menu closes on Escape key", async () => {
  const app = await setupApp();
  const { window } = app;

  const projectRow = window.document.querySelector(".project-btn")?.closest(".list-row");
  assert.ok(projectRow, "project row should exist");
  projectRow.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));

  const menu = window.document.querySelector("#ctx-menu");
  assert.equal(menu.hidden, false, "menu should be visible after contextmenu event");

  window.document.dispatchEvent(
    new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));

  assert.equal(menu.hidden, true, "menu should be hidden after Escape");

  app.cleanup();
});

test("codex session delete calls delete_codex_session, not delete_session", async () => {
  const app = await setupApp();
  const { window, mock } = app;

  const projectButton = window.document.querySelector(".project-btn");
  assert.ok(projectButton);
  projectButton.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  // 找到 source=codex 的 entry button (title 設為 entry.label)
  const codexEntryBtn = window.document.querySelector('.entry-btn[title="codex-session.jsonl"]');
  assert.ok(codexEntryBtn, "codex session entry button should exist");

  codexEntryBtn.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, clientX: 100, clientY: 100 }),
  );
  await new Promise((resolve) => setTimeout(resolve, 10));

  const deleteMenuItem = [...window.document.querySelectorAll(".ctx-menu-item")]
    .find((el) => /delete|刪除/i.test(el.textContent));
  assert.ok(deleteMenuItem, "context menu should have a delete item");
  deleteMenuItem.click();

  const sessionDialog = window.document.querySelector("#session-delete-dialog");
  assert.equal(sessionDialog.open, true);

  const confirm = window.document.querySelector("#session-delete-confirm");
  confirm.click();
  await new Promise((resolve) => setTimeout(resolve, 10));

  // 確認 undo toast 出現，且尚未呼叫後端
  assert.equal(mock.calls.some((call) => call.cmd === "delete_codex_session"), false);
  assert.equal(mock.calls.some((call) => call.cmd === "delete_session"), false);

  const toastViewport = window.document.querySelector("#undo-toast-viewport");
  assert.equal(toastViewport.hidden, false);

  // 點擊 undo 清除計時器，避免 8000ms timer 拖延測試
  const undo = toastViewport.querySelector(".undo-toast-btn");
  assert.ok(undo, "undo button should exist");
  undo.click();
  await new Promise((resolve) => setTimeout(resolve, 20));

  // undo 後不應呼叫任何刪除命令
  assert.equal(mock.calls.some((call) => call.cmd === "delete_codex_session"), false);
  assert.equal(mock.calls.some((call) => call.cmd === "delete_session"), false);

  app.cleanup();
});
