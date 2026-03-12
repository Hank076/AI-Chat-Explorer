import {
  buildThemeDatasetValue,
  getStoredThemeMode,
  resolveTheme,
} from "./theme.js";
import {
  SUPPORTED_LOCALES,
  detectLocale,
  getLocaleLabel,
  getStoredLocale,
  t,
} from "./i18n.js";

const { invoke } = window.__TAURI__.core;

const TECH_PREVIEW_COUNT = 20;
const CHAT_PREVIEW_LENGTH = 380;
const SESSION_DELETE_UNDO_MS = 8000;
const THEME_STORAGE_KEY = "claude_history_theme_mode";
const LOCALE_STORAGE_KEY = "claude_history_locale";
const ERROR_TRANSLATION_KEYS = {
  NOT_FOUND: "error.NOT_FOUND",
  READ_FAILED: "error.READ_FAILED",
  PARSE_PARTIAL: "error.PARSE_PARTIAL",
};

const state = {
  projects: [],
  projectSearchQuery: "",
  entries: [],
  selectedProjectPath: "",
  selectedEntryPath: "",
  selectedEntryType: "",
  timelineItems: [],
  parseErrors: [],
  parseErrorCode: "",
  hideSystemEvents: true,
  hideToolEvents: true,
  hideThinkingEvents: true,
  timelineSearchQuery: "",
  techViewState: {},
  entryExpandState: {},
  themeMode: "auto",
  resolvedTheme: "dark",
  locale: "en-US",
  sourceToggles: { claude: true, codex: true },
  allClaudeProjects: [],
  allCodexProjects: [],
  entrySearchQuery: "",
  entrySearchResults: null,
  isSearchingEntries: false,
  entryDateFilter: "all",
  pendingSessionDeletes: [],
  pendingSessionDeleteCandidate: null,
  pendingProjectDelete: null,
  pendingProjectPath: "",
  undoToastAnimationFrameId: null,
  ctxMenuTarget: null,
};

const refs = {
  panelGrid: null,
  projectsPanel: null,
  entriesPanel: null,
  resizerLeft: null,
  resizerMiddle: null,
  projectsList: null,
  projectsSearchInput: null,
  sourceToggleWrap: null,
  sourceToggleClaude: null,
  sourceToggleCodex: null,
  entriesList: null,
  entriesSearchInput: null,
  entryDateFilterWrap: null,
  viewerTitle: null,
  viewerMeta: null,
  viewerMetaPath: null,
  viewerMetaTime: null,
  viewerSearchWrap: null,
  viewerSearchInput: null,
  viewerContent: null,
  status: null,
  hideSystemEventsToggle: null,
  hideToolEventsToggle: null,
  hideThinkingEventsToggle: null,
  hideSystemEventsWrap: null,
  themeButtons: [],
  localeSelect: null,
  aboutButton: null,
  aboutDialog: null,
  aboutCloseButton: null,
  aboutCodexDiscoveryMode: null,
  toastViewport: null,
  projectDeleteDialog: null,
  projectDeleteForm: null,
  projectDeleteImpact: null,
  projectDeleteMessage: null,
  projectDeleteInput: null,
  projectDeleteConfirmButton: null,
  projectDeleteCancelButton: null,
  sessionDeleteDialog: null,
  sessionDeleteForm: null,
  sessionDeleteMessage: null,
  sessionDeleteConfirmButton: null,
  sessionDeleteCancelButton: null,
};

function tt(key, params) {
  return t(state.locale, key, params);
}

function getSubagentToggleSymbol(expanded) {
  return expanded ? "▾" : "▸";
}

function applyStaticTranslations() {
  document.documentElement.lang = state.locale;
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const key = node.getAttribute("data-i18n");
    if (!key) continue;
    node.textContent = tt(key);
  }
  for (const node of document.querySelectorAll("[data-i18n-aria-label]")) {
    const key = node.getAttribute("data-i18n-aria-label");
    if (!key) continue;
    node.setAttribute("aria-label", tt(key));
  }
  for (const node of document.querySelectorAll("[data-i18n-placeholder]")) {
    const key = node.getAttribute("data-i18n-placeholder");
    if (!key) continue;
    node.setAttribute("placeholder", tt(key));
  }
}

function setStatus(message, type = "info") {
  refs.status.textContent = message || "";
  refs.status.dataset.type = type;
}

function formatError(code) {
  const translationKey = ERROR_TRANSLATION_KEYS[code];
  if (translationKey) return tt(translationKey);
  return tt("error.unknown", { code });
}

function setErrorStatus(errorCode) {
  setStatus(formatError(String(errorCode)), "error");
}

function setInfoStatus(key, params = {}) {
  setStatus(tt(key, params), "info");
}

function resetViewerSearchQuery() {
  state.timelineSearchQuery = "";
  if (refs.viewerSearchInput) refs.viewerSearchInput.value = "";
}

function clearViewer() {
  state.timelineItems = [];
  state.parseErrors = [];
  state.parseErrorCode = "";
  resetViewerSearchQuery();
  state.techViewState = {};
  refs.viewerTitle.textContent = tt("panel.viewer");
  renderViewerMeta("", "");
  setStatus("");
  setViewerSearchVisible(false);
  setHideSystemEventsVisible(true);
  refs.viewerContent.innerHTML = `<p class="placeholder">${escapeHtml(tt("placeholder.select"))}</p>`;
}

function escapeHtml(input) {
  return String(input)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

/**
 * 將 text 中所有符合 query 的片段包在 <mark> 中，其餘部分 escapeHtml。
 * 在原始文字上做比對，再分段 escape，避免 HTML entity 干擾搜尋。
 */
function highlightHtml(text, query) {
  const str = String(text || "");
  if (!query) return escapeHtml(str);
  const escapedQuery = query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const parts = str.split(new RegExp(`(${escapedQuery})`, "gi"));
  return parts
    .map((part, i) =>
      i % 2 === 1
        ? `<mark class="search-highlight">${escapeHtml(part)}</mark>`
        : escapeHtml(part),
    )
    .join("");
}

function decodeProjectLabel(encodedName) {
  const isWindows = navigator.platform.toLowerCase().includes("win");
  const separator = isWindows ? "\\" : "/";

  const driveMatch = encodedName.match(/^([A-Za-z])--(.+)$/);
  if (driveMatch) {
    const drive = `${driveMatch[1]}:${separator}`;
    const rest = driveMatch[2]
      .split("-")
      .filter(Boolean)
      .join(separator);
    return `${drive}${rest}`;
  }

  return encodedName
    .split("-")
    .filter(Boolean)
    .join(separator);
}

function normalizeDisplayPath(path) {
  const isWindows = navigator.platform.toLowerCase().includes("win");
  let value = String(path || "");
  if (isWindows) {
    value = value.replace(/^\\\\\?\\/, "");
    value = value.replace(/^\/\/\?\//, "");
    return value.replaceAll("/", "\\");
  }
  return value.replaceAll("\\", "/");
}

function detectHomePrefix(path) {
  const value = normalizeDisplayPath(path);
  const windowsMatch = value.match(/^[A-Za-z]:\\Users\\[^\\]+/);
  if (windowsMatch) return windowsMatch[0];
  const unixMatch = value.match(/^\/(?:home|Users)\/[^/]+/);
  if (unixMatch) return unixMatch[0];
  return "";
}

function abbreviateHomePath(path) {
  const value = normalizeDisplayPath(path);
  const homePrefix = detectHomePrefix(value);
  if (!homePrefix) return value;
  if (!value.startsWith(homePrefix)) return value;
  const suffix = value.slice(homePrefix.length);
  return suffix ? `~${suffix}` : "~";
}

function getProjectDisplayName(project) {
  const rawName = String(project?.name || "").trim();
  if (rawName) return rawName;

  const normalizedPath = normalizeDisplayPath(project?.path || "");
  const parts = normalizedPath.split(/[\\/]/).filter(Boolean);
  if (parts.length > 0) return parts[parts.length - 1];

  const decoded = decodeProjectLabel(String(project?.name || ""));
  const decodedParts = normalizeDisplayPath(decoded).split(/[\\/]/).filter(Boolean);
  if (decodedParts.length > 0) return decodedParts[decodedParts.length - 1];
  return decoded || tt("panel.projects");
}

function doesProjectMatchSearch(project, query) {
  const normalizedQuery = String(query || "").trim().toLowerCase();
  if (!normalizedQuery) return true;

  const name = getProjectDisplayName(project).toLowerCase();
  const preferredPath = normalizeDisplayPath(
    project?.cwdPath || decodeProjectLabel(String(project?.name || "")),
  ).toLowerCase();
  const abbreviatedPath = abbreviateHomePath(preferredPath).toLowerCase();
  return (
    name.includes(normalizedQuery) ||
    preferredPath.includes(normalizedQuery) ||
    abbreviatedPath.includes(normalizedQuery)
  );
}

function updateInputTexts(input, placeholderKey, ariaLabelKey) {
  if (!input) return;
  input.placeholder = tt(placeholderKey);
  input.setAttribute("aria-label", tt(ariaLabelKey));
}

function updateProjectSearchTexts() {
  updateInputTexts(
    refs.projectsSearchInput,
    "project.searchPlaceholder",
    "project.searchAria",
  );
}

function updateViewerSearchTexts() {
  updateInputTexts(
    refs.viewerSearchInput,
    "viewer.searchPlaceholder",
    "viewer.searchAria",
  );
}

function updateProjectDeleteTexts() {
  updateInputTexts(
    refs.projectDeleteInput,
    "project.delete.inputPlaceholder",
    "project.delete.inputLabel",
  );
}

function getSystemPrefersDark() {
  try {
    return window.matchMedia?.("(prefers-color-scheme: dark)")?.matches ?? true;
  } catch {
    return true;
  }
}

function safeStorageGet(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function safeStorageSet(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Ignore storage errors and keep runtime-only setting.
  }
}

function safeStorageRemove(key) {
  try {
    localStorage.removeItem(key);
  } catch {
    // Ignore storage errors and keep runtime-only setting.
  }
}

function readThemeMode() {
  return getStoredThemeMode(safeStorageGet(THEME_STORAGE_KEY));
}

function saveThemeMode(mode) {
  if (mode === "auto") {
    safeStorageRemove(THEME_STORAGE_KEY);
    return;
  }
  safeStorageSet(THEME_STORAGE_KEY, mode);
}

function updateThemeButtons() {
  for (const button of refs.themeButtons) {
    const active = button.dataset.themeMode === state.themeMode;
    button.dataset.active = active ? "true" : "false";
    button.setAttribute("aria-checked", active ? "true" : "false");
  }
}

function resolveAndApplyTheme() {
  const resolvedTheme = resolveTheme({
    mode: state.themeMode,
    systemPrefersDark: getSystemPrefersDark(),
  });
  const datasetValue = buildThemeDatasetValue(resolvedTheme);
  document.documentElement.dataset.theme = datasetValue;
  state.resolvedTheme = datasetValue;
  updateThemeButtons();
}

function setThemeMode(mode, { persist = true } = {}) {
  state.themeMode = getStoredThemeMode(mode);
  if (persist) saveThemeMode(state.themeMode);
  resolveAndApplyTheme();
}

function initThemeMode() {
  state.themeMode = readThemeMode();
  resolveAndApplyTheme();

  let mediaQuery = null;
  try {
    mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
  } catch {
    mediaQuery = null;
  }
  if (!mediaQuery) return;

  const onSystemThemeChange = () => {
    if (state.themeMode === "auto") {
      resolveAndApplyTheme();
    }
  };

  if (typeof mediaQuery.addEventListener === "function") {
    mediaQuery.addEventListener("change", onSystemThemeChange);
  } else if (typeof mediaQuery.addListener === "function") {
    mediaQuery.addListener(onSystemThemeChange);
  }
}

function readLocale() {
  const saved = safeStorageGet(LOCALE_STORAGE_KEY);
  if (saved) return getStoredLocale(saved);
  return detectLocale(navigator.language);
}

function saveLocale(locale) {
  safeStorageSet(LOCALE_STORAGE_KEY, locale);
}

function setLocale(locale, { persist = true } = {}) {
  state.locale = getStoredLocale(locale);
  if (persist) saveLocale(state.locale);
  if (refs.localeSelect) refs.localeSelect.value = state.locale;
  applyStaticTranslations();
  updateProjectSearchTexts();
  updateViewerSearchTexts();
  updateProjectDeleteTexts();
  renderProjects();
  renderEntries();
  if (!state.selectedEntryPath) clearViewer();
}

function initLocaleSelector() {
  if (!refs.localeSelect) return;
  refs.localeSelect.innerHTML = "";
  for (const locale of SUPPORTED_LOCALES) {
    const option = document.createElement("option");
    option.value = locale;
    option.textContent = getLocaleLabel(locale);
    refs.localeSelect.append(option);
  }
  setLocale(readLocale(), { persist: false });
  refs.localeSelect.addEventListener("change", (event) => {
    setLocale(event.target.value, { persist: true });
  });
}

function isMarkdownPath(path) {
  return /\.md$/i.test(String(path || "").trim());
}

function setHideSystemEventsVisible(visible) {
  if (
    !refs.hideSystemEventsWrap ||
    !refs.hideSystemEventsToggle ||
    !refs.hideToolEventsToggle ||
    !refs.hideThinkingEventsToggle
  ) {
    return;
  }
  refs.hideSystemEventsWrap.style.visibility = visible ? "visible" : "hidden";
  refs.hideSystemEventsWrap.style.pointerEvents = visible ? "auto" : "none";
  refs.hideSystemEventsToggle.disabled = !visible;
  refs.hideToolEventsToggle.disabled = !visible;
  refs.hideThinkingEventsToggle.disabled = !visible;
  refs.hideSystemEventsWrap.setAttribute("aria-hidden", visible ? "false" : "true");
}

function updateTogglePressed(button, active) {
  if (!button) return;
  button.setAttribute("aria-pressed", active ? "true" : "false");
}

function updateEventFilterToggles() {
  updateTogglePressed(refs.hideSystemEventsToggle, state.hideSystemEvents);
  updateTogglePressed(refs.hideToolEventsToggle, state.hideToolEvents);
  updateTogglePressed(refs.hideThinkingEventsToggle, state.hideThinkingEvents);
}

function setViewerSearchVisible(visible) {
  if (!refs.viewerSearchWrap) return;
  refs.viewerSearchWrap.style.display = visible ? "flex" : "none";
  refs.viewerSearchWrap.setAttribute("aria-hidden", visible ? "false" : "true");
}

// ── Context Menu ─────────────────────────────────────────
function hideContextMenu() {
  const menu = refs.ctxMenu;
  if (!menu) return;
  menu.hidden = true;
  menu.setAttribute("aria-hidden", "true");
  menu.replaceChildren();
  state.ctxMenuTarget = null;
}

function showContextMenu(x, y, items) {
  const menu = refs.ctxMenu;
  if (!menu) return;

  menu.replaceChildren();
  for (const { label, onClick, kind } of items) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "ctx-menu-item" + (kind === "danger" ? " ctx-menu-item--danger" : "");
    btn.setAttribute("role", "menuitem");
    btn.textContent = label;
    btn.addEventListener("click", async () => {
      hideContextMenu();
      try {
        await onClick();
      } catch (err) {
        console.error("ctx-menu action failed", err);
      }
    });
    menu.appendChild(btn);
  }

  // Position with viewport overflow guard
  menu.hidden = false;
  menu.removeAttribute("aria-hidden");
  const menuW = menu.offsetWidth;
  const menuH = menu.offsetHeight;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  menu.style.left = `${x + menuW > vw ? Math.max(0, vw - menuW - 8) : x}px`;
  menu.style.top = `${y + menuH > vh ? Math.max(0, y - menuH) : y}px`;

  const firstItem = menu.querySelector(".ctx-menu-item");
  if (firstItem) firstItem.focus();
}

function openProjectContextMenu(event, project) {
  event.preventDefault();
  state.ctxMenuTarget = event.currentTarget;
  showContextMenu(event.clientX, event.clientY, [
    {
      label: tt("action.openFolder"),
      onClick: () => {
        void window.__TAURI__.opener.openPath(project.cwdPath || project.path);
      },
    },
    {
      label: tt("action.deleteProject"),
      kind: "danger",
      onClick: () => {
        void openProjectDeleteDialog(project);
      },
    },
  ]);
}

function openEntryContextMenu(event, entry) {
  event.preventDefault();
  state.ctxMenuTarget = event.currentTarget;
  showContextMenu(event.clientX, event.clientY, [
    {
      label: tt("action.copySessionId"),
      onClick: async () => {
        const sessionId = String(entry.path ?? "")
          .split(/[\\/]/)
          .pop()
          .replace(/\.[^.]+$/, "");
        await navigator.clipboard.writeText(sessionId);
        setInfoStatus("status.sessionIdCopied");
      },
    },
    {
      label: tt("action.deleteConversation"),
      kind: "danger",
      onClick: () => {
        openSessionDeleteDialog(entry);
      },
    },
  ]);
}

function createActionWrap(...buttons) {
  const wrap = createElement("div", "row-actions");
  for (const button of buttons) {
    if (button) wrap.appendChild(button);
  }
  return wrap;
}

function getUndoToastItems() {
  const now = Date.now();
  const sessionItems = state.pendingSessionDeletes.map((pending) => {
    const remainingMs = Math.max(0, (pending.expiresAt || now) - now);
    const remainingSeconds = Math.max(0, Math.ceil(remainingMs / 1000));
    return ({
    id: pending.path,
    kind: "session",
    label: tt("undoToast.sessionLabel"),
    message: tt("session.delete.toast", {
      name: pending.label,
      seconds: remainingSeconds,
    }),
    meta: `${remainingSeconds}s`,
    progress: Math.max(
      0,
      Math.min(100, (remainingMs / Math.max(1, pending.totalMs || 1)) * 100),
    ),
    remainingSeconds,
    onUndo: () => {
      cancelPendingSessionDelete(pending, {
        refreshEntries: true,
        showCancelledStatus: true,
      });
    },
    createdAt: pending.createdAt,
    });
  });

  const projectItem = state.pendingProjectDelete
    ? [{
        id: state.pendingProjectDelete.projectPath,
        kind: "project",
        label: tt("undoToast.projectLabel"),
        get remainingMs() {
          return Math.max(0, (state.pendingProjectDelete.expiresAt || now) - now);
        },
      }]
        .map((pendingItem) => {
          const remainingMs = pendingItem.remainingMs;
          const remainingSeconds = Math.max(0, Math.ceil(remainingMs / 1000));
          return {
        id: state.pendingProjectDelete.projectPath,
        kind: "project",
        label: tt("undoToast.projectLabel"),
        message: tt("project.delete.toast", {
          name: state.pendingProjectDelete.projectName,
          seconds: remainingSeconds,
        }),
        meta: `${remainingSeconds}s`,
        progress: Math.max(
          0,
          Math.min(
            100,
            (remainingMs / Math.max(1, state.pendingProjectDelete.totalMs || 1)) * 100,
          ),
        ),
        remainingSeconds,
        onUndo: () => {
          cancelPendingProjectDelete({ showCancelledStatus: true });
        },
        createdAt: state.pendingProjectDelete.createdAt,
          };
        })
    : [];

  return [...sessionItems, ...projectItem].sort((a, b) => b.createdAt - a.createdAt);
}

function syncUndoToastAnimation() {
  const hasPending = state.pendingSessionDeletes.length > 0 || Boolean(state.pendingProjectDelete);
  const cancelFrame =
    window.cancelAnimationFrame?.bind(window) ||
    ((frameId) => window.clearTimeout(frameId));
  if (!hasPending) {
    if (state.undoToastAnimationFrameId) {
      cancelFrame(state.undoToastAnimationFrameId);
      state.undoToastAnimationFrameId = null;
    }
    return;
  }
  if (state.undoToastAnimationFrameId) return;

  const tick = () => {
    state.undoToastAnimationFrameId = null;
    updateUndoToastVisuals();
    syncUndoToastAnimation();
  };
  state.undoToastAnimationFrameId = window.requestAnimationFrame(tick);
}

function updateUndoToastVisuals() {
  if (!refs.toastViewport || refs.toastViewport.hidden) return;
  const items = getUndoToastItems();
  for (const item of items) {
    const toast = [...refs.toastViewport.children].find((node) => node.dataset.pendingId === item.id);
    if (!toast) continue;
    toast.style.setProperty("--undo-progress", `${item.progress}%`);

    const message = toast.querySelector(".undo-toast__message");
    const meta = toast.querySelector(".undo-toast__meta");
    if (message) message.textContent = item.message;
    if (meta) meta.textContent = item.meta;
  }
}

function renderUndoToasts() {
  if (!refs.toastViewport) return;
  refs.toastViewport.replaceChildren();

  const items = getUndoToastItems();
  refs.toastViewport.hidden = items.length === 0;
  if (items.length === 0) {
    syncUndoToastAnimation();
    return;
  }

  for (const item of items) {
    const toast = createElement("article", "undo-toast");
    toast.dataset.kind = item.kind;
    toast.dataset.pendingId = item.id;
    toast.style.setProperty("--undo-progress", `${item.progress}%`);
    toast.setAttribute("role", "status");
    toast.setAttribute("aria-atomic", "true");

    const body = createElement("div", "undo-toast__body");
    const eyebrow = createElement("div", "undo-toast__eyebrow", item.label);
    const message = createElement("p", "undo-toast__message", item.message);
    const meta = createElement("div", "undo-toast__meta", item.meta);
    body.append(eyebrow, message, meta);

    const undoButton = createElement("button", "undo-toast-btn", tt("action.undo"));
    undoButton.type = "button";
    undoButton.addEventListener("click", item.onUndo);

    toast.append(body, undoButton);
    refs.toastViewport.appendChild(toast);
  }
  syncUndoToastAnimation();
}

function clearPendingTimers(pending) {
  if (!pending) return;
  if (pending.timerId) {
    window.clearTimeout(pending.timerId);
    pending.timerId = null;
  }
  if (pending.countdownIntervalId) {
    window.clearInterval(pending.countdownIntervalId);
    pending.countdownIntervalId = null;
  }
}

function clearPendingSessionTimers(pending) {
  clearPendingTimers(pending);
}

function clearPendingProjectTimers(pending) {
  clearPendingTimers(pending);
}

function getPendingSessionHiddenPaths() {
  const hiddenPaths = new Set();
  for (const pending of state.pendingSessionDeletes) {
    hiddenPaths.add(pending.path);
    for (const path of pending.hiddenPaths || []) {
      hiddenPaths.add(path);
    }
  }
  return hiddenPaths;
}

function applyPendingEntryFilters(entries) {
  const hiddenPaths = getPendingSessionHiddenPaths();
  if (hiddenPaths.size === 0) return entries;
  return entries.filter((entry) => !hiddenPaths.has(entry.path));
}

function cancelPendingProjectDelete({ showCancelledStatus = false } = {}) {
  const pendingProject = state.pendingProjectDelete;
  if (!pendingProject) return;
  clearPendingProjectTimers(pendingProject);
  state.pendingProjectDelete = null;
  renderUndoToasts();
  if (showCancelledStatus) {
    setInfoStatus("status.deleteCancelled");
  }
}

function cancelPendingSessionDelete(
  pending = null,
  {
  executeNow = false,
  refreshEntries = false,
  showCancelledStatus = false,
  } = {},
) {
  const pendingSessions = pending
    ? state.pendingSessionDeletes.filter((item) => item === pending)
    : [...state.pendingSessionDeletes];
  if (pendingSessions.length === 0) return;

  for (const pendingSession of pendingSessions) {
    clearPendingSessionTimers(pendingSession);
  }
  state.pendingSessionDeletes = state.pendingSessionDeletes.filter(
    (item) => !pendingSessions.includes(item),
  );
  renderUndoToasts();

  if (executeNow) {
    for (const pendingSession of pendingSessions) {
      void executeSessionDelete(pendingSession);
    }
    return;
  }
  if (refreshEntries) {
    void refreshEntriesForSelectedProject();
  }
  if (showCancelledStatus) {
    setInfoStatus("status.deleteCancelled");
  }
}

async function refreshEntriesForSelectedProject() {
  if (!state.selectedProjectPath) {
    state.entries = [];
    renderEntries();
    return;
  }
  const entries = await invoke("list_project_entries", {
    projectPath: state.selectedProjectPath,
  });
  state.entries = applyPendingEntryFilters(entries);
  renderEntries();
}

async function refreshEntriesIfProjectSelected(projectPath) {
  if (projectPath !== state.selectedProjectPath) return;
  await refreshEntriesForSelectedProject();
}

function clearSelectedEntryState() {
  state.selectedEntryPath = "";
  state.selectedEntryType = "";
  clearViewer();
}

function clearSelectedEntryIfMatchesPaths(paths) {
  if (!state.selectedEntryPath) return;
  if (!paths.includes(state.selectedEntryPath)) return;
  clearSelectedEntryState();
}

function closeDialogIfOpen(dialog) {
  if (!dialog) return;
  if (dialog.open) dialog.close();
}

function openSessionDeleteDialog(entry) {
  if (!refs.sessionDeleteDialog || !refs.sessionDeleteMessage) return;
  state.pendingSessionDeleteCandidate = entry;
  refs.sessionDeleteMessage.textContent = tt("session.delete.confirmText", {
    name: String(entry.label || ""),
  });
  refs.sessionDeleteDialog.showModal();
}

function closeSessionDeleteDialog() {
  closeDialogIfOpen(refs.sessionDeleteDialog);
  state.pendingSessionDeleteCandidate = null;
}

function openAboutDialog() {
  if (!refs.aboutDialog) return;
  refs.aboutDialog.showModal();
}

function formatCodexProjectDiscoveryMode(payload) {
  const mode = String(payload?.mode || "unavailable");
  const detail = String(payload?.detail || "").trim();
  const label = tt(`about.codexMode.${mode}`);
  return detail ? `${label} (${detail})` : label;
}

async function refreshAboutDialog() {
  if (!refs.aboutCodexDiscoveryMode) return;
  refs.aboutCodexDiscoveryMode.textContent = tt("about.codexProjectDiscoveryLoading");

  try {
    const payload = await invoke("get_codex_project_discovery_mode");
    refs.aboutCodexDiscoveryMode.textContent = formatCodexProjectDiscoveryMode(payload);
  } catch {
    refs.aboutCodexDiscoveryMode.textContent = tt("about.codexMode.unavailable");
  }
}

function closeAboutDialog() {
  closeDialogIfOpen(refs.aboutDialog);
}

function confirmSessionDelete() {
  const entry = state.pendingSessionDeleteCandidate;
  closeSessionDeleteDialog();
  if (!entry) return;
  queueSessionDelete(entry);
}

function queueSessionDelete(entry) {
  const isSession = entry.entryType === "session";
  if (!isSession && entry.entryType !== "subagent_session") return;

  cancelPendingProjectDelete({ showCancelledStatus: true });

  const removedPaths = [entry.path];
  if (isSession) {
    const parentSessionStem = String(entry.label || "").replace(/\.jsonl$/i, "");
    for (const item of state.entries) {
      if (item.entryType === "subagent_session" && item.parentSession === parentSessionStem) {
        removedPaths.push(item.path);
      }
    }
  }
  clearSelectedEntryIfMatchesPaths(removedPaths);
  state.entries = state.entries.filter((item) => !removedPaths.includes(item.path));
  renderEntries();

  const pending = {
    path: entry.path,
    source: entry.source || "claude",
    label: String(entry.label || ""),
    projectPath: state.selectedProjectPath,
    hiddenPaths: removedPaths,
    timerId: null,
    countdownIntervalId: null,
    totalMs: SESSION_DELETE_UNDO_MS,
    expiresAt: Date.now() + SESSION_DELETE_UNDO_MS,
    createdAt: Date.now(),
  };
  pending.timerId = window.setTimeout(() => {
    void executeSessionDelete(pending);
  }, SESSION_DELETE_UNDO_MS);
  state.pendingSessionDeletes = [...state.pendingSessionDeletes, pending];
  renderUndoToasts();
}

async function executeSessionDelete(pending) {
  if (!pending) return;
  clearPendingSessionTimers(pending);
  state.pendingSessionDeletes = state.pendingSessionDeletes.filter((item) => item !== pending);
  renderUndoToasts();
  try {
    if (pending.source === "codex") {
      await invoke("delete_codex_session", { sessionPath: pending.path });
    } else {
      await invoke("delete_session", { sessionPath: pending.path });
    }
    await refreshEntriesIfProjectSelected(pending.projectPath);
    setInfoStatus("status.sessionDeleted");
  } catch (errorCode) {
    await refreshEntriesIfProjectSelected(pending.projectPath);
    setErrorStatus(errorCode);
  }
}

function updateProjectDeleteConfirmState() {
  if (!refs.projectDeleteInput || !refs.projectDeleteConfirmButton) return;
  const selectedProject = findSelectedProject();
  const pendingProject = state.projects.find((item) => item.path === state.pendingProjectPath);
  const project = selectedProject || pendingProject;
  const expectedName = project ? getProjectDisplayName(project) : "";
  const currentValue = String(refs.projectDeleteInput.value || "").trim();
  refs.projectDeleteConfirmButton.disabled = !expectedName || currentValue !== expectedName;
}

function formatProjectDeleteImpactSummary(impact) {
  const readImpactCount = (key) => Number(impact?.[key] || 0);
  const sessionCount = readImpactCount("sessionCount");
  const subagentCount = readImpactCount("subagentSessionCount");
  const memoryCount = readImpactCount("memoryFileCount");
  const totalCount = readImpactCount("totalFileCount");
  const totalSize = formatBytes(readImpactCount("totalSizeBytes"));
  return tt("project.delete.impactSummary", {
    sessionCount,
    subagentCount,
    memoryCount,
    totalCount,
    totalSize,
  });
}

function recomputeVisibleProjects() {
  state.projects = mergeProjects(
    state.sourceToggles.claude ? state.allClaudeProjects : [],
    state.sourceToggles.codex ? state.allCodexProjects : [],
  );
}

function getProjectDeleteTargets(project) {
  return {
    projectPath: project?.claudePath || null,
    codexCwd: project?.codexCwd || null,
  };
}

function removeProjectFromSourceCaches(pending) {
  if (pending.claudePath) {
    state.allClaudeProjects = state.allClaudeProjects.filter(
      (project) => project.path !== pending.claudePath,
    );
  }
  if (pending.codexCwd) {
    state.allCodexProjects = state.allCodexProjects.filter(
      (project) => (project.cwdPath || project.path) !== pending.codexCwd,
    );
  }
}

async function openProjectDeleteDialog(project) {
  if (
    !refs.projectDeleteDialog ||
    !refs.projectDeleteInput ||
    !refs.projectDeleteMessage ||
    !refs.projectDeleteImpact
  ) {
    return;
  }
  state.pendingProjectPath = project.path;
  refs.projectDeleteMessage.textContent = tt("project.delete.confirmText", {
    name: getProjectDisplayName(project),
  });
  refs.projectDeleteImpact.textContent = tt("project.delete.impactLoading");
  refs.projectDeleteInput.value = "";
  updateProjectDeleteConfirmState();
  refs.projectDeleteDialog.showModal();
  refs.projectDeleteInput.focus();
  try {
    const impact = await invoke("get_project_delete_impact", getProjectDeleteTargets(project));
    refs.projectDeleteImpact.textContent = formatProjectDeleteImpactSummary(impact);
  } catch {
    refs.projectDeleteImpact.textContent = tt("project.delete.impactUnavailable");
  }
}

function closeProjectDeleteDialog() {
  closeDialogIfOpen(refs.projectDeleteDialog);
  if (refs.projectDeleteImpact) refs.projectDeleteImpact.textContent = "";
  state.pendingProjectPath = "";
}

async function confirmProjectDelete() {
  const projectPath = state.pendingProjectPath;
  if (!projectPath) return;
  closeProjectDeleteDialog();
  queueProjectDelete(projectPath);
}

function queueProjectDelete(projectPath) {
  const project = findProjectByPath(projectPath);
  if (!project) {
    setStatus(tt("error.NOT_FOUND"), "error");
    return;
  }

  cancelPendingSessionDelete(null, { refreshEntries: true });
  cancelPendingProjectDelete();

  const pending = {
    projectPath,
    projectName: getProjectDisplayName(project),
    claudePath: project.claudePath || null,
    codexCwd: project.codexCwd || null,
    timerId: null,
    countdownIntervalId: null,
    totalMs: SESSION_DELETE_UNDO_MS,
    expiresAt: Date.now() + SESSION_DELETE_UNDO_MS,
    createdAt: Date.now(),
  };
  pending.timerId = window.setTimeout(() => {
    void executeProjectDelete(pending);
  }, SESSION_DELETE_UNDO_MS);
  state.pendingProjectDelete = pending;
  renderUndoToasts();
}

async function executeProjectDelete(pending) {
  if (!pending) return;
  clearPendingProjectTimers(pending);
  if (state.pendingProjectDelete === pending) state.pendingProjectDelete = null;
  renderUndoToasts();
  try {
    await invoke("delete_project", {
      projectPath: pending.claudePath,
      codexCwd: pending.codexCwd,
    });
    const wasSelected = state.selectedProjectPath === pending.projectPath;
    removeProjectFromSourceCaches(pending);
    recomputeVisibleProjects();
    renderProjects();
    if (wasSelected) {
      state.selectedProjectPath = "";
      clearSelectedEntryState();
      state.entries = [];
      renderEntries();
    }
    setInfoStatus("status.projectDeleted");
  } catch (errorCode) {
    setErrorStatus(errorCode);
  }
}

function updateSourceToggleUI() {
  const { claude, codex } = state.sourceToggles;
  if (refs.sourceToggleClaude) {
    refs.sourceToggleClaude.dataset.active = claude ? "true" : "false";
    refs.sourceToggleClaude.setAttribute("aria-pressed", claude ? "true" : "false");
  }
  if (refs.sourceToggleCodex) {
    refs.sourceToggleCodex.dataset.active = codex ? "true" : "false";
    refs.sourceToggleCodex.setAttribute("aria-pressed", codex ? "true" : "false");
  }
}

function renderProjects() {
  refs.projectsList.innerHTML = "";
  const visibleProjects = state.projects.filter((project) =>
    doesProjectMatchSearch(project, state.projectSearchQuery),
  );
  for (const project of visibleProjects) {
    const li = document.createElement("li");
    const button = document.createElement("button");
    button.className = "list-btn project-btn";
    if (state.selectedProjectPath === project.path) button.dataset.active = "true";
    const displayName = getProjectDisplayName(project);
    const sourcePath = project.cwdPath || project.path;
    const fullPath = normalizeDisplayPath(sourcePath);
    const displayPath = abbreviateHomePath(sourcePath);

    const nameNode = createElement("div", "project-name", displayName);
    const pathNode = createElement("div", "project-path", displayPath);
    button.append(nameNode, pathNode);
    if (Array.isArray(project.sources) && project.sources.length > 0) {
      const sourceRow = createElement("div", "project-sources");
      for (const src of project.sources) {
        const badge = createElement("span", `source-badge source-badge--${src}`, src === "claude" ? "Claude" : "Codex");
        sourceRow.appendChild(badge);
      }
      button.append(sourceRow);
    }
    button.title = fullPath;
    button.addEventListener("click", () => selectProject(project.path));
    const row = createElement("div", "list-row");
    row.append(button);
    row.addEventListener("contextmenu", (e) => openProjectContextMenu(e, project));
    li.appendChild(row);
    refs.projectsList.appendChild(li);
  }

  if (visibleProjects.length === 0) {
    const empty = document.createElement("li");
    empty.className = "list-empty";
    empty.textContent = tt("project.empty");
    refs.projectsList.appendChild(empty);
  }
}

function formatEntryTime(modifiedMs) {
  if (!Number.isFinite(modifiedMs)) return tt("common.timeUnknown");
  const date = new Date(modifiedMs);
  if (Number.isNaN(date.getTime())) return tt("common.timeUnknown");
  const yyyy = date.getFullYear();
  const mm = String(date.getMonth() + 1).padStart(2, "0");
  const dd = String(date.getDate()).padStart(2, "0");
  const hh = String(date.getHours()).padStart(2, "0");
  const mi = String(date.getMinutes()).padStart(2, "0");
  const ss = String(date.getSeconds()).padStart(2, "0");
  return `${yyyy}/${mm}/${dd} ${hh}:${mi}:${ss}`;
}

function formatBytes(sizeBytes) {
  if (!Number.isFinite(sizeBytes) || sizeBytes < 0) return "-";
  if (sizeBytes < 1024) return `${sizeBytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = sizeBytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unitIndex]}`;
}

function getEntryHoverTitle(entry) {
  const path = String(entry?.path || "").trim();
  const fileName = path.split(/[\\/]/).pop() || "";
  if (fileName) return fileName;
  return String(entry?.label || "");
}

function findSelectedProject() {
  return state.projects.find((project) => project.path === state.selectedProjectPath) || null;
}

function findProjectByPath(projectPath) {
  return state.projects.find((project) => project.path === projectPath) || null;
}

function findSelectedEntry() {
  return state.entries.find((entry) => entry.path === state.selectedEntryPath) || null;
}

function buildSessionMetaPath(sessionFileName) {
  const project = findSelectedProject();
  const projectName = project ? getProjectDisplayName(project) : tt("panel.projects");
  return `${projectName} / ${sessionFileName || tt("viewer.timeline")}`;
}

function buildMemoryMetaPath(memoryFileName) {
  const project = findSelectedProject();
  const projectName = project ? getProjectDisplayName(project) : tt("panel.projects");
  return `${projectName} / memory / ${memoryFileName || "memory.md"}`;
}

function formatMetaDay(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  const yyyy = date.getFullYear();
  const mm = String(date.getMonth() + 1).padStart(2, "0");
  const dd = String(date.getDate()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd}`;
}

function extractHeaderModelLabel(raw) {
  const message = raw?.message && typeof raw.message === "object" ? raw.message : null;
  const modelCandidate = message?.model ?? raw?.model ?? message?.model_name ?? raw?.model_name;
  const model = String(modelCandidate || "").trim();
  return model ? `model:${model}` : "";
}

function renderViewerMeta(pathText, rightText = "") {
  if (refs.viewerMetaPath) refs.viewerMetaPath.textContent = String(pathText || "");
  if (refs.viewerMetaTime) refs.viewerMetaTime.textContent = String(rightText || "");
}

function getDateFilterRange(filter) {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (filter === "today") {
    return { from: todayStart, to: todayStart + 86400000 };
  }
  if (filter === "yesterday") {
    return { from: todayStart - 86400000, to: todayStart };
  }
  if (filter === "week") {
    const dow = now.getDay(); // 0=Sun
    const daysFromMonday = dow === 0 ? 6 : dow - 1;
    return { from: todayStart - daysFromMonday * 86400000, to: Infinity };
  }
  return null;
}

function isEntryInDateFilter(modifiedMs, filter) {
  if (filter === "all" || !modifiedMs) return filter === "all";
  const range = getDateFilterRange(filter);
  if (!range) return true;
  return modifiedMs >= range.from && modifiedMs < range.to;
}

function updateEntryDateFilterUI() {
  if (!refs.entryDateFilterWrap) return;
  for (const btn of refs.entryDateFilterWrap.querySelectorAll(".entry-date-btn")) {
    const active = btn.dataset.filter === state.entryDateFilter;
    btn.dataset.active = active ? "true" : "false";
    btn.setAttribute("aria-selected", active ? "true" : "false");
  }
}

let _entrySearchTimer = null;

function scheduleEntrySearch(query) {
  clearTimeout(_entrySearchTimer);
  if (!query.trim()) {
    state.entrySearchResults = null;
    state.isSearchingEntries = false;
    renderEntries();
    return;
  }
  // 不在此處 re-render，保留目前顯示結果直到 debounce 結束，避免每個按鍵觸發 I/O 與畫面閃爍
  _entrySearchTimer = setTimeout(async () => {
    const capturedQuery = query;
    const project = findSelectedProject();
    if (!state.selectedProjectPath || !project?.claudePath) {
      state.isSearchingEntries = false;
      state.entrySearchResults = new Map();
      renderEntries();
      return;
    }
    state.isSearchingEntries = true;
    renderEntries();
    try {
      // 若有日期篩選，只傳入符合時間條件的 session 路徑，避免讀取範圍外的檔案
      const invokeArgs = { projectPath: project.claudePath, query: capturedQuery.trim() };
      if (state.entryDateFilter !== "all") {
        invokeArgs.paths = state.entries
          .filter((e) => e.entryType === "session" && isEntryInDateFilter(e.modifiedMs, state.entryDateFilter))
          .map((e) => e.path);
      }
      const results = await invoke("search_sessions", invokeArgs);
      if (state.entrySearchQuery !== capturedQuery) return;
      state.entrySearchResults = new Map(results.map((r) => [r.path, r.matchCount]));
    } catch (_err) {
      state.entrySearchResults = new Map();
    }
    state.isSearchingEntries = false;
    renderEntries();
  }, 300);
}

function renderEntries() {
  refs.entriesList.innerHTML = "";
  const memoryEntries = state.entries
    .filter((entry) => entry.entryType === "memory_file")
    .sort((a, b) => {
      const aIsMain = isTopMemoryFile(a.path, a.label);
      const bIsMain = isTopMemoryFile(b.path, b.label);
      if (aIsMain !== bIsMain) return aIsMain ? -1 : 1;
      return String(a.label || "").localeCompare(String(b.label || ""), "zh-TW");
    });
  const allSessionEntries = state.entries
    .filter((entry) => entry.entryType === "session")
    .sort((a, b) => (b.modifiedMs || 0) - (a.modifiedMs || 0));

  const dateFilteredEntries = state.entryDateFilter !== "all"
    ? allSessionEntries.filter((e) => isEntryInDateFilter(e.modifiedMs, state.entryDateFilter))
    : allSessionEntries;

  const searchResults = state.entrySearchResults;
  const isSearchFiltering = searchResults !== null && !state.isSearchingEntries;
  const sessionEntries = isSearchFiltering
    ? dateFilteredEntries.filter((e) => searchResults.has(e.path))
    : dateFilteredEntries;

  const subagentsByParent = new Map();

  for (const entry of state.entries) {
    if (entry.entryType !== "subagent_session" || !entry.parentSession) continue;
    if (!subagentsByParent.has(entry.parentSession)) {
      subagentsByParent.set(entry.parentSession, []);
    }
    subagentsByParent.get(entry.parentSession).push(entry);
  }

  for (const [parent, children] of subagentsByParent.entries()) {
    children.sort((a, b) => (b.modifiedMs || 0) - (a.modifiedMs || 0));
  }

  if (memoryEntries.length > 0) {
    refs.entriesList.appendChild(
      createEntriesSectionTitle(tt("panel.memoryFiles"), memoryEntries.length, "memory"),
    );
  }

  for (const entry of memoryEntries) {
    const li = document.createElement("li");
    li.appendChild(createEntryButton(entry, { primaryText: entry.label }));
    refs.entriesList.appendChild(li);
  }

  if (memoryEntries.length > 0 && sessionEntries.length > 0) {
    const divider = document.createElement("li");
    divider.className = "entries-divider";
    divider.setAttribute("aria-hidden", "true");
    refs.entriesList.appendChild(divider);
  }

  if (state.isSearchingEntries) {
    const loadingLi = document.createElement("li");
    loadingLi.className = "entries-search-loading";
    loadingLi.setAttribute("aria-live", "polite");
    loadingLi.textContent = tt("status.loadingContent");
    refs.entriesList.appendChild(loadingLi);
    return;
  }

  if (allSessionEntries.length > 0) {
    refs.entriesList.appendChild(
      createEntriesSectionTitle(`${tt("panel.session")} FILES`, sessionEntries.length, "sessions"),
    );
  }

  if (sessionEntries.length === 0 && allSessionEntries.length > 0) {
    const emptyLi = document.createElement("li");
    emptyLi.className = "entries-search-empty";
    emptyLi.textContent = isSearchFiltering
      ? tt("placeholder.entriesNoResults")
      : tt("placeholder.entriesDateNoResults");
    refs.entriesList.appendChild(emptyLi);
    return;
  }

  for (const entry of sessionEntries) {
    const sessionStem = String(entry.label || "").replace(/\.jsonl$/i, "");
    const children = subagentsByParent.get(sessionStem) || [];
    const hasChildren = children.length > 0;
    const expanded = hasChildren ? Boolean(state.entryExpandState[sessionStem]) : false;

    const matchCount = isSearchFiltering && searchResults.has(entry.path) ? searchResults.get(entry.path) : 0;
    const matchLabel = matchCount > 0 ? tt("entry.matchCount", { count: matchCount }) : "";

    const li = document.createElement("li");
    const row = createElement("div", "entry-row list-row");
    if (hasChildren) row.dataset.hasChildren = "true";
    row.appendChild(createEntryButton(entry, { hasChildren, typeLabel: matchLabel }));

    let toggle = null;
    if (hasChildren) {
      toggle = createElement("button", "entry-toggle row-action-btn", getSubagentToggleSymbol(expanded));
      toggle.type = "button";
      toggle.title = expanded ? tt("action.collapseSubagent") : tt("action.expandSubagent");
      toggle.setAttribute("aria-label", `${toggle.title} · ${tt("entry.subagentToggleLabel")}`);
      toggle.setAttribute("aria-expanded", expanded ? "true" : "false");
      toggle.dataset.expanded = expanded ? "true" : "false";
      toggle.dataset.label = tt("entry.subagentToggleLabel");
      toggle.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        state.entryExpandState[sessionStem] = !expanded;
        renderEntries();
      });
    }

    if (toggle) row.append(createActionWrap(toggle));
    row.addEventListener("contextmenu", (e) => openEntryContextMenu(e, entry));
    li.appendChild(row);
    refs.entriesList.appendChild(li);

    if (!expanded) continue;
    for (const child of children) {
      const childLi = document.createElement("li");
      childLi.className = "entry-child-item";
      const childRow = createElement("div", "list-row");
      childRow.appendChild(
        createEntryButton(child, {
          primaryText: tt("entry.childPrefix", { time: formatEntryTime(child.modifiedMs) }),
          isSubagent: true,
        }),
      );
      childRow.addEventListener("contextmenu", (e) => openEntryContextMenu(e, child));
      childLi.appendChild(childRow);
      refs.entriesList.appendChild(childLi);
    }
  }
}

function createEntriesSectionTitle(title, count, iconName = "") {
  const item = document.createElement("li");
  item.className = "entries-section-title";
  if (iconName) item.classList.add(`entries-section-title--${iconName}`);
  item.setAttribute("aria-hidden", "true");

  const headingWrap = createElement("span", "entries-section-heading-wrap");
  const icon = createSectionIcon(iconName);
  if (icon) headingWrap.appendChild(icon);
  headingWrap.append(createElement("span", "entries-section-heading", title));
  const badge = createElement("span", "entries-section-badge", String(count));
  item.append(headingWrap, badge);
  return item;
}

function createSectionIcon(name) {
  const iconName = String(name || "").toLowerCase();
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.classList.add("entries-section-icon");
  if (iconName) svg.classList.add(`entries-section-icon--${iconName}`);

  const addPath = (d) => {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", d);
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", "currentColor");
    path.setAttribute("stroke-width", "1.8");
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("stroke-linejoin", "round");
    svg.appendChild(path);
  };

  if (iconName === "memory") {
    addPath("M8.5 8.5a3.5 3.5 0 0 1 7 0");
    addPath("M8 8H6.8A2.8 2.8 0 0 0 4 10.8v2.6A2.6 2.6 0 0 0 6.6 16H8");
    addPath("M16 8h1.4a2.6 2.6 0 0 1 2.6 2.6v2.8a2.6 2.6 0 0 1-2.6 2.6H16");
    addPath("M12 6.3v2.1");
    addPath("M8 11h2");
    addPath("M14 11h2");
    addPath("M12 15.2v2.5");
    addPath("M10.5 18h3");
    return svg;
  }

  if (iconName === "sessions") {
    addPath("M12.8 3.1a1.4 1.4 0 0 0-1.6 0L3 8l8.2 4.9a1.4 1.4 0 0 0 1.6 0L21 8z");
    addPath("m3.2 11.2 8 4.8a1.4 1.4 0 0 0 1.6 0l8-4.8");
    addPath("m3.2 14.8 8 4.8a1.4 1.4 0 0 0 1.6 0l8-4.8");
    return svg;
  }

  return null;
}

function createEntryButton(
  entry,
  {
    primaryText = formatEntryTime(entry.modifiedMs),
    typeLabel = "",
    isSubagent = false,
    hasChildren = false,
  } = {},
) {
  const button = document.createElement("button");
  button.className = "list-btn entry-btn";
  button.dataset.entryType = entry.entryType;
  button.dataset.subagent = isSubagent ? "true" : "false";
  if (hasChildren) button.dataset.hasChildren = "true";

  const secondaryParts = [formatBytes(entry.sizeBytes)];
  if (typeLabel) secondaryParts.push(typeLabel);

  const primary = createElement("div", "entry-primary", primaryText);
  const secondary = createElement("div", "entry-secondary", secondaryParts.join(" · "));
  button.append(primary, secondary);

  if (entry.source === "claude" || entry.source === "codex") {
    const srcLabel = entry.source === "claude" ? tt("source.claude") : tt("source.codex");
    const badge = createElement("span", `source-badge source-badge--${entry.source}`, srcLabel);
    primary.appendChild(badge);
  }

  button.title = getEntryHoverTitle(entry);
  if (state.selectedEntryPath === entry.path) button.dataset.active = "true";
  button.addEventListener("click", () => selectEntry(entry));
  return button;
}

function isTopMemoryFile(path, label) {
  const fileName = String(label || "").toLowerCase();
  if (fileName !== "memory.md") return false;
  return /[\\/]memory[\\/]memory\.md$/i.test(String(path || ""));
}

function renderMemory(payload) {
  const fileName = String(payload.path || "").split(/[\\/]/).pop() || "memory";
  refs.viewerTitle.textContent = fileName;
  refs.viewerContent.innerHTML = `<pre class="memory-block">${escapeHtml(payload.content)}</pre>`;
}

function formatTimestamp(timestamp) {
  if (!timestamp) return "-";
  const value = new Date(timestamp);
  if (Number.isNaN(value.getTime())) return String(timestamp);
  return value.toLocaleString("zh-TW", { hour12: false });
}

function truncateText(text, limit = 380) {
  const value = String(text || "").trim();
  if (!value) return "";
  if (value.length <= limit) return value;
  return `${value.slice(0, limit)}...`;
}

function extractCommandDisplay(text) {
  const source = String(text || "");
  const commandName = source.match(/<command-name>([\s\S]*?)<\/command-name>/i)?.[1]?.trim();
  const commandMessage = source.match(/<command-message>([\s\S]*?)<\/command-message>/i)?.[1]?.trim();
  const commandArgs = source.match(/<command-args>([\s\S]*?)<\/command-args>/i)?.[1]?.trim();
  const normalizedName = commandName || (commandMessage ? `/${commandMessage.replace(/^\/+/, "")}` : "");
  const parts = [normalizedName, commandArgs].filter(Boolean);
  if (parts.length === 0) return null;
  return `command: ${parts.join(" ")}`;
}

function extractXmlTagContent(source, tagName) {
  const text = String(source || "");
  if (!text.trim()) return "";
  const matcher = new RegExp(`<${tagName}>([\\s\\S]*?)<\\/${tagName}>`, "i");
  const value = text.match(matcher)?.[1];
  return typeof value === "string" ? value.trim() : "";
}

function extractLocalCommandStdout(raw) {
  if (!raw || typeof raw !== "object") return "";
  const candidates = [
    raw["local-command-stdout"],
    raw.localCommandStdout,
    raw?.message?.["local-command-stdout"],
    raw?.message?.localCommandStdout,
  ];
  for (const candidate of candidates) {
    if (typeof candidate === "string" && candidate.trim()) {
      return candidate.trim();
    }
  }
  const messageContent = raw?.message?.content;
  if (typeof messageContent === "string") {
    const tagged = extractXmlTagContent(messageContent, "local-command-stdout");
    if (tagged) return tagged;
  }
  if (typeof raw?.content === "string") {
    const tagged = extractXmlTagContent(raw.content, "local-command-stdout");
    if (tagged) return tagged;
  }
  for (const item of normalizeContentItems(messageContent)) {
    if (!item || typeof item !== "object") continue;
    const text = typeof item.text === "string" ? item.text : typeof item.content === "string" ? item.content : "";
    const tagged = extractXmlTagContent(text, "local-command-stdout");
    if (tagged) return tagged;
  }
  return "";
}

function extractTextFromUnknown(value) {
  if (typeof value === "string") return value.trim();
  if (!value || typeof value !== "object") return "";
  if (typeof value.text === "string") return value.text.trim();
  if (typeof value.content === "string") return value.content.trim();
  return "";
}

function normalizeContentItems(content) {
  if (Array.isArray(content)) return content;
  if (content && typeof content === "object") return [content];
  return [];
}

function parseAskUserQuestions(input) {
  const rawQuestions = input?.questions;
  if (Array.isArray(rawQuestions)) return rawQuestions;
  if (typeof rawQuestions === "string") {
    try {
      const parsed = JSON.parse(rawQuestions);
      if (Array.isArray(parsed)) return parsed;
    } catch {
      return [];
    }
  }
  return [];
}

function buildToolUseDetail(item) {
  if (!item || item.type !== "tool_use") return null;
  const toolName = String(item.name || "unknown");
  const input = item.input && typeof item.input === "object" ? item.input : {};
  const fallbackInput = [tt("tool.fallback.input")];

  if (toolName === "AskUserQuestion") {
    const questions = parseAskUserQuestions(input);
    const lines = [];
    for (const q of questions) {
      const question = typeof q?.question === "string" ? q.question.trim() : "";
      if (question) lines.push(tt("tool.label.question", { text: question }));
      if (Array.isArray(q?.options) && q.options.length > 0) {
        const labels = q.options
          .map((opt) => (typeof opt?.label === "string" ? opt.label.trim() : ""))
          .filter(Boolean);
        if (labels.length > 0) {
          lines.push(tt("tool.label.options", { options: labels.join(" / ") }));
        }
      }
    }
    return {
      toolName,
      title: tt("tool.askUserQuestion.title"),
      lines: lines.length > 0 ? lines : [tt("tool.fallback.questions")],
    };
  }

  if (toolName === "Bash") {
    const command = typeof input.command === "string" ? input.command.trim() : "";
    const description = typeof input.description === "string" ? input.description.trim() : "";
    const lines = [];
    if (description) lines.push(tt("tool.label.description", { text: description }));
    if (command) lines.push(tt("tool.label.command", { text: command }));
    return {
      toolName,
      title: tt("tool.bash.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "Read") {
    const filePath = typeof input.file_path === "string" ? input.file_path.trim() : "";
    const offset = Number.isFinite(input.offset) ? input.offset : null;
    const limit = Number.isFinite(input.limit) ? input.limit : null;
    const lines = [];
    if (filePath) lines.push(tt("tool.label.file", { text: filePath }));
    if (offset !== null || limit !== null) {
      lines.push(
        tt("tool.label.range", {
          offset: offset ?? 0,
          limit: limit ?? "auto",
        }),
      );
    }
    return {
      toolName,
      title: tt("tool.read.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "TaskCreate") {
    const activeForm = typeof input.activeForm === "string" ? input.activeForm.trim() : "";
    const subject = typeof input.subject === "string" ? input.subject.trim() : "";
    const description =
      typeof input.description === "string" ? input.description.trim() : "";
    const lines = [];
    if (activeForm) lines.push(tt("tool.label.phase", { text: activeForm }));
    if (subject) lines.push(tt("tool.label.subject", { text: subject }));
    if (description) lines.push(tt("tool.label.description", { text: description }));
    return {
      toolName,
      title: tt("tool.taskCreate.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "Glob") {
    const pattern = typeof input.pattern === "string" ? input.pattern.trim() : "";
    const path = typeof input.path === "string" ? input.path.trim() : "";
    const lines = [];
    if (pattern) lines.push(tt("tool.label.pattern", { text: pattern }));
    if (path) lines.push(tt("tool.label.path", { text: path }));
    return {
      toolName,
      title: tt("tool.glob.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "Grep") {
    const pattern = typeof input.pattern === "string" ? input.pattern.trim() : "";
    const path = typeof input.path === "string" ? input.path.trim() : "";
    const glob = typeof input.glob === "string" ? input.glob.trim() : "";
    const lines = [];
    if (pattern) lines.push(tt("tool.label.pattern", { text: pattern }));
    if (path) lines.push(tt("tool.label.path", { text: path }));
    if (glob) lines.push(tt("tool.label.glob", { text: glob }));
    return {
      toolName,
      title: tt("tool.grep.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "TaskUpdate") {
    const taskId = typeof input.taskId === "string" ? input.taskId.trim() : "";
    const status = typeof input.status === "string" ? input.status.trim() : "";
    const subject = typeof input.subject === "string" ? input.subject.trim() : "";
    const description =
      typeof input.description === "string" ? input.description.trim() : "";
    const activeForm = typeof input.activeForm === "string" ? input.activeForm.trim() : "";
    const lines = [];
    if (taskId) lines.push(tt("tool.label.taskId", { text: taskId }));
    if (status) lines.push(tt("tool.label.status", { text: status }));
    if (activeForm) lines.push(tt("tool.label.phase", { text: activeForm }));
    if (subject) lines.push(tt("tool.label.subject", { text: subject }));
    if (description) lines.push(tt("tool.label.description", { text: description }));
    return {
      toolName,
      title: tt("tool.taskUpdate.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "TaskList") {
    const filter = typeof input.filter === "string" ? input.filter.trim() : "";
    const status = typeof input.status === "string" ? input.status.trim() : "";
    const lines = [];
    if (filter) lines.push(tt("tool.label.filter", { text: filter }));
    if (status) lines.push(tt("tool.label.status", { text: status }));
    return {
      toolName,
      title: tt("tool.taskList.title"),
      lines: lines.length > 0 ? lines : [tt("tool.fallback.taskList")],
    };
  }

  if (toolName === "TaskGet") {
    const taskId = typeof input.taskId === "string" ? input.taskId.trim() : "";
    const id = typeof input.id === "string" ? input.id.trim() : "";
    const lines = [];
    if (taskId || id) lines.push(tt("tool.label.taskId", { text: taskId || id }));
    return {
      toolName,
      title: tt("tool.taskGet.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "Edit") {
    const filePath = typeof input.file_path === "string" ? input.file_path.trim() : "";
    const oldString = typeof input.old_string === "string" ? input.old_string.trim() : "";
    const newString = typeof input.new_string === "string" ? input.new_string.trim() : "";
    const lines = [];
    if (filePath) lines.push(tt("tool.label.file", { text: filePath }));
    if (oldString) lines.push(tt("tool.label.oldContent", { text: truncateText(oldString, 120) }));
    if (newString) lines.push(tt("tool.label.newContent", { text: truncateText(newString, 120) }));
    return {
      toolName,
      title: tt("tool.edit.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "Write") {
    const filePath = typeof input.file_path === "string" ? input.file_path.trim() : "";
    const content = typeof input.content === "string" ? input.content : "";
    const lines = [];
    if (filePath) lines.push(tt("tool.label.file", { text: filePath }));
    if (content) lines.push(tt("tool.label.contentLength", { count: content.length }));
    return {
      toolName,
      title: tt("tool.write.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "TaskOutput") {
    const taskId = typeof input.taskId === "string" ? input.taskId.trim() : "";
    const shellId = typeof input.shellId === "string" ? input.shellId.trim() : "";
    const lines = [];
    if (taskId) lines.push(tt("tool.label.taskId", { text: taskId }));
    if (shellId) lines.push(tt("tool.label.shellId", { text: shellId }));
    return {
      toolName,
      title: tt("tool.taskOutput.title"),
      lines: lines.length > 0 ? lines : [tt("tool.fallback.taskOutput")],
    };
  }

  if (toolName === "Skill") {
    const skillName = typeof input.skillName === "string" ? input.skillName.trim() : "";
    const name = typeof input.name === "string" ? input.name.trim() : "";
    const skill = typeof input.skill === "string" ? input.skill.trim() : "";
    const lines = [];
    if (skillName || name || skill) {
      lines.push(tt("tool.label.skill", { text: skillName || name || skill }));
    }
    return {
      toolName,
      title: tt("tool.skill.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "Agent") {
    const task = typeof input.task === "string" ? input.task.trim() : "";
    const lines = [];
    if (task) lines.push(tt("tool.label.task", { text: truncateText(task, 180) }));
    return {
      toolName,
      title: tt("tool.agent.title"),
      lines: lines.length > 0 ? lines : [tt("tool.fallback.agent")],
    };
  }

  if (toolName === "WebSearch") {
    const query = typeof input.query === "string" ? input.query.trim() : "";
    const domains = Array.isArray(input.domains) ? input.domains.join(", ") : "";
    const lines = [];
    if (query) lines.push(tt("tool.label.query", { text: query }));
    if (domains) lines.push(tt("tool.label.domains", { text: domains }));
    return {
      toolName,
      title: tt("tool.webSearch.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "WebFetch") {
    const url = typeof input.url === "string" ? input.url.trim() : "";
    const lines = [];
    if (url) lines.push(tt("tool.label.url", { text: url }));
    return {
      toolName,
      title: tt("tool.webFetch.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "MCPSearch") {
    const query = typeof input.query === "string" ? input.query.trim() : "";
    const lines = [];
    if (query) lines.push(tt("tool.label.query", { text: query }));
    return {
      toolName,
      title: tt("tool.mcpSearch.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "KillShell") {
    const shellId = typeof input.shell_id === "string" ? input.shell_id.trim() : "";
    const lines = [];
    if (shellId) lines.push(tt("tool.label.shellId", { text: shellId }));
    return {
      toolName,
      title: tt("tool.killShell.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "ExitPlanMode") {
    const reason = typeof input.reason === "string" ? input.reason.trim() : "";
    const lines = [];
    if (reason) lines.push(tt("tool.label.reason", { text: reason }));
    return {
      toolName,
      title: tt("tool.exitPlanMode.title"),
      lines: lines.length > 0 ? lines : [tt("tool.fallback.exitPlanMode")],
    };
  }

  if (toolName === "NotebookEdit") {
    const notebookPath =
      typeof input.notebook_path === "string" ? input.notebook_path.trim() : "";
    const lines = [];
    if (notebookPath) lines.push(tt("tool.label.notebook", { text: notebookPath }));
    return {
      toolName,
      title: tt("tool.notebookEdit.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  if (toolName === "LSP") {
    const method = typeof input.method === "string" ? input.method.trim() : "";
    const path = typeof input.path === "string" ? input.path.trim() : "";
    const lines = [];
    if (method) lines.push(tt("tool.label.method", { text: method }));
    if (path) lines.push(tt("tool.label.path", { text: path }));
    return {
      toolName,
      title: tt("tool.lsp.title"),
      lines: lines.length > 0 ? lines : fallbackInput,
    };
  }

  return {
    toolName,
    title: tt("tool.default.title", { name: toolName }),
    lines: [tt("tool.fallback.default")],
  };
}

function extractToolResultDetail(item, index = 1) {
  const lines = [];
  if (!item || item.type !== "tool_result") return null;

  const content = item.content;
  if (typeof content === "string" && content.trim()) {
    lines.push(content.trim());
  } else if (Array.isArray(content)) {
    for (const sub of content) {
      if (typeof sub === "string" && sub.trim()) {
        lines.push(sub.trim());
      } else if (
        sub &&
        typeof sub === "object" &&
        typeof sub.text === "string" &&
        sub.text.trim()
      ) {
        lines.push(sub.text.trim());
      }
    }
  }

  if (lines.length === 0 && typeof content === "object" && content !== null) {
    lines.push(tt("tool.result.structured"));
  }

  return {
    title: tt("tool.result.title", { index }),
    lines: lines.length > 0 ? lines : [tt("tool.result.empty")],
  };
}

function stringifyCompact(value) {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value.trim();
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return "";
  }
}

function joinDefinedParts(parts, separator = " · ") {
  return parts
    .map((part) => String(part || "").trim())
    .filter(Boolean)
    .join(separator);
}

function buildToolUseResultDetail(toolUseResult) {
  if (!toolUseResult || typeof toolUseResult !== "object") return null;
  const commandNameRaw = String(toolUseResult.commandName || "").trim();
  const commandName = commandNameRaw || "Unknown";
  const lines = [];
  const addLine = (label, value) => {
    const text = stringifyCompact(value);
    if (!text) return;
    lines.push(`${label}: ${truncateText(text, 200)}`);
  };
  const addJoinedLine = (label, values) => {
    if (!Array.isArray(values) || values.length === 0) return;
    const text = values.map((v) => stringifyCompact(v)).filter(Boolean).join(", ");
    if (!text) return;
    lines.push(`${label}: ${truncateText(text, 220)}`);
  };

  if (/^superpowers:|^skills?:/i.test(commandName)) {
    addLine("success", toolUseResult.success);
    return {
      toolName: "Skill",
      title: `Skill · ${commandName}`,
      lines: lines.length > 0 ? lines : [tt("tool.result.empty")],
    };
  }

  switch (commandName) {
    case "Bash":
      addLine("stdout", toolUseResult.stdout);
      addLine("stderr", toolUseResult.stderr);
      addLine("interrupted", toolUseResult.interrupted);
      addLine("isImage", toolUseResult.isImage);
      addLine("noOutputExpected", toolUseResult.noOutputExpected);
      break;
    case "Read":
      addLine("type", toolUseResult.type);
      addLine("filePath", toolUseResult.file?.filePath);
      addLine("startLine", toolUseResult.file?.startLine);
      addLine("numLines", toolUseResult.file?.numLines);
      addLine("totalLines", toolUseResult.file?.totalLines);
      addLine("content", toolUseResult.file?.content);
      break;
    case "Glob":
      addJoinedLine("filenames", toolUseResult.filenames);
      addLine("numFiles", toolUseResult.numFiles);
      addLine("durationMs", toolUseResult.durationMs);
      addLine("truncated", toolUseResult.truncated);
      break;
    case "Grep":
      addLine("numMatches", toolUseResult.numMatches);
      addLine("durationMs", toolUseResult.durationMs);
      addLine("truncated", toolUseResult.truncated);
      if (Array.isArray(toolUseResult.matches)) {
        for (const match of toolUseResult.matches.slice(0, 3)) {
          const line = joinDefinedParts(
            [match?.filePath, match?.line ? `L${match.line}` : "", match?.preview],
            " | ",
          );
          if (line) lines.push(truncateText(line, 220));
        }
      }
      break;
    case "Edit":
    case "Write":
      addLine("success", toolUseResult.success);
      addLine("filePath", toolUseResult.filePath);
      addLine("replacements", toolUseResult.replacements);
      addLine("bytesWritten", toolUseResult.bytesWritten);
      break;
    case "TaskCreate":
      addLine("task.id", toolUseResult.task?.id);
      addLine("task.subject", toolUseResult.task?.subject);
      break;
    case "TaskUpdate":
      addLine("success", toolUseResult.success);
      addLine("taskId", toolUseResult.taskId);
      addJoinedLine("updatedFields", toolUseResult.updatedFields);
      addLine(
        "statusChange",
        joinDefinedParts(
          [toolUseResult.statusChange?.from, toolUseResult.statusChange?.to],
          " -> ",
        ),
      );
      break;
    case "TaskList":
      addLine("count", toolUseResult.count);
      if (Array.isArray(toolUseResult.tasks)) {
        for (const task of toolUseResult.tasks.slice(0, 3)) {
          const line = joinDefinedParts(
            [
              task?.id ? `#${task.id}` : "",
              task?.status ? `[${task.status}]` : "",
              task?.subject,
            ],
            " ",
          );
          if (line) lines.push(truncateText(line, 220));
        }
      }
      break;
    case "TaskGet":
      addLine("task.id", toolUseResult.task?.id);
      addLine("task.status", toolUseResult.task?.status);
      addLine("task.subject", toolUseResult.task?.subject);
      break;
    case "TaskOutput":
      addLine("taskId", toolUseResult.taskId);
      addLine("shellId", toolUseResult.shellId);
      addLine("stdout", toolUseResult.stdout);
      addLine("stderr", toolUseResult.stderr);
      break;
    case "AskUserQuestion":
      if (Array.isArray(toolUseResult.questions)) {
        for (const q of toolUseResult.questions.slice(0, 3)) {
          addLine("question", q?.question);
          if (Array.isArray(q?.options)) {
            addJoinedLine(
              "options",
              q.options.map((opt) => opt?.label || opt),
            );
          }
        }
      }
      addLine("answers", toolUseResult.answers);
      addLine("annotations", toolUseResult.annotations);
      break;
    case "Agent":
      addLine("taskId", toolUseResult.taskId);
      addLine("status", toolUseResult.status);
      addLine("summary", toolUseResult.summary);
      addLine("durationMs", toolUseResult.durationMs);
      addLine("usage", toolUseResult.usage);
      break;
    case "WebSearch":
    case "WebFetch":
    case "MCPSearch":
      if (Array.isArray(toolUseResult.results)) {
        addLine("count", toolUseResult.count ?? toolUseResult.results.length);
        for (const result of toolUseResult.results.slice(0, 3)) {
          const line = joinDefinedParts(
            [result?.title, result?.url, result?.snippet, result?.status],
            " | ",
          );
          if (line) lines.push(truncateText(line, 220));
        }
      } else {
        addLine("results", toolUseResult.results);
      }
      break;
    case "LSP":
      addLine("method", toolUseResult.method);
      addLine("result", toolUseResult.result);
      break;
    case "NotebookEdit":
      addLine("success", toolUseResult.success);
      addLine("notebookPath", toolUseResult.notebookPath);
      addJoinedLine("updatedCells", toolUseResult.updatedCells);
      break;
    case "KillShell":
      addLine("success", toolUseResult.success);
      addLine("shellId", toolUseResult.shellId);
      break;
    case "ExitPlanMode":
      addLine("success", toolUseResult.success);
      addLine("reason", toolUseResult.reason);
      break;
    default:
      addLine("success", toolUseResult.success);
      addLine("commandName", commandNameRaw);
      for (const [key, value] of Object.entries(toolUseResult)) {
        if (key === "success" || key === "commandName") continue;
        addLine(key, value);
      }
      break;
  }

  return {
    toolName: commandName,
    title: `toolUseResult · ${commandName}`,
    lines: lines.length > 0 ? lines : [tt("tool.result.empty")],
  };
}

function pushUniqueDetail(details, detail) {
  if (!detail || typeof detail !== "object") return;
  const signature = `${detail.title}::${Array.isArray(detail.lines) ? detail.lines.join("\n") : ""}`;
  const existing = details.some((item) => {
    const target = `${item.title}::${Array.isArray(item.lines) ? item.lines.join("\n") : ""}`;
    return target === signature;
  });
  if (!existing) details.push(detail);
}


function extractChatOnlySummary(raw) {
  if (raw?.type === "tool_result") return "";

  const toolUseResultDetail = buildToolUseResultDetail(raw?.toolUseResult);
  if (toolUseResultDetail) {
    return joinDefinedParts(
      [toolUseResultDetail.title, ...(Array.isArray(toolUseResultDetail.lines) ? toolUseResultDetail.lines.slice(0, 2) : [])],
      " | ",
    );
  }

  const message = raw?.message;
  const content = message?.content ?? message;
  const chunks = [];

  if (typeof content === "string" && content.trim()) {
    chunks.push(content.trim());
  }

  const items = normalizeContentItems(content);
  for (const item of items) {
    if (!item || typeof item !== "object") continue;
    if (item.type === "text" && typeof item.text === "string" && item.text.trim()) {
      chunks.push(item.text.trim());
    }
  }

  const summary = chunks.join("\n").trim();
  const commandDisplay = extractCommandDisplay(summary);
  return commandDisplay || summary;
}

function extractToolTagNames(tags) {
  if (!Array.isArray(tags)) return [];
  return tags
    .filter((tag) => typeof tag === "string" && tag.startsWith("tool:"))
    .map((tag) => tag.slice(5))
    .filter(Boolean);
}

function extractTextSummary(raw, resultsMap = null) {
  const message = raw?.message;
  const content = message?.content ?? message;
  const textChunks = [];
  const tags = [];
  let toolUseCount = 0;
  let thinkingCount = 0;
  const toolNames = new Set();
  const thinkingDetails = [];
  const toolUseDetails = [];
  const toolResultDetails = [];
  const toolUseResultDetail = buildToolUseResultDetail(raw?.toolUseResult);

  if (typeof content === "string" && content.trim()) {
    textChunks.push(content.trim());
  }

  if (toolUseResultDetail) {
    tags.push("tool_result");
    if (toolUseResultDetail.toolName) tags.push(`tool:${toolUseResultDetail.toolName}`);
    pushUniqueDetail(toolResultDetails, {
      title: toolUseResultDetail.title,
      lines: toolUseResultDetail.lines,
    });
  }

  const contentItems = normalizeContentItems(content);
  for (const item of contentItems) {
    if (!item || typeof item !== "object") continue;

    if (item.type === "text" && typeof item.text === "string" && item.text.trim()) {
      textChunks.push(item.text.trim());
    }

    if (item.type === "thinking" && typeof item.thinking === "string" && item.thinking.trim()) {
      thinkingDetails.push(item.thinking.trim());
    }

    if (item.type === "tool_result") {
      tags.push("tool_result");
      const detail = extractToolResultDetail(item, toolResultDetails.length + 1);
      if (detail) pushUniqueDetail(toolResultDetails, detail);
    }

    if (item.type === "thinking") {
      thinkingCount += 1;
      tags.push("thinking");
    }
    if (item.type === "tool_use") {
      toolUseCount += 1;
      const toolName = item.name || "unknown";
      toolNames.add(String(toolName));
      tags.push(`tool:${toolName}`);
      const detail = buildToolUseDetail(item);
      if (detail) toolUseDetails.push(detail);

      // 嘗試關聯結果 (Tool Binding)
      if (resultsMap && item.id) {
        const resultItem = resultsMap.get(item.id);
        if (resultItem) {
          const resDetail = extractToolResultDetail(resultItem, toolResultDetails.length + 1);
          if (resDetail) pushUniqueDetail(toolResultDetails, resDetail);
          if (!tags.includes("tool_result")) tags.push("tool_result");
        }
      }
    }
  }

  if (
    textChunks.length === 0 &&
    message &&
    typeof message === "object" &&
    !Array.isArray(message)
  ) {
    const fallback = extractTextFromUnknown(message);
    if (fallback) textChunks.push(fallback);
  }

  if (textChunks.length === 0 && raw?.type === "tool_result") {
    tags.push("tool_result");
    const detail = extractToolResultDetail({ type: "tool_result", content: raw?.content }, 1);
    if (detail) pushUniqueDetail(toolResultDetails, detail);
  }

  const summary = textChunks.join("\n").trim();
  const localCommandStdout = extractLocalCommandStdout(raw);
  if (summary) {
    const commandDisplay = extractCommandDisplay(summary);
    if (commandDisplay && localCommandStdout) {
      pushUniqueDetail(toolUseDetails, {
        title: "",
        kind: "command_stdout",
        lines: [tt("command.stdout", { text: localCommandStdout })],
      });
    }
    return { summary: commandDisplay || summary, tags, thinkingDetails, toolUseDetails, toolResultDetails };
  }

  if (toolUseResultDetail) {
    return {
      summary: joinDefinedParts([toolUseResultDetail.title, ...toolUseResultDetail.lines.slice(0, 1)], " | "),
      tags,
      thinkingDetails,
      toolUseDetails,
      toolResultDetails,
    };
  }

  if (contentItems.some((item) => item.type === "tool_result")) {
    return {
      summary: tt("summary.toolResult"),
      tags,
      thinkingDetails,
      toolUseDetails,
      toolResultDetails,
    };
  }

  if (toolUseCount > 0) {
    const names = Array.from(toolNames).slice(0, 3).join(", ");
    const suffix = names
      ? tt("summary.toolUseNames", { names, more: toolNames.size > 3 ? "..." : "" })
      : "";
    return {
      summary: tt("summary.toolUseCount", { count: toolUseCount, suffix }),
      tags,
      thinkingDetails,
      toolUseDetails,
      toolResultDetails,
    };
  }

  if (thinkingCount > 0) {
    return {
      summary: tt("summary.thinking", { count: thinkingCount }),
      tags,
      thinkingDetails,
      toolUseDetails,
      toolResultDetails,
    };
  }

  return {
    summary: tt("summary.structuredEvent"),
    tags,
    thinkingDetails,
    toolUseDetails,
    toolResultDetails,
  };
}

function buildTechSummary(event) {
  const raw = event.raw || {};
  const rawType = raw.type || event.eventType || "unknown";

  if (rawType === "event_msg") {
    const payloadType = raw.payload?.type || "unknown";
    if (payloadType === "token_count") {
      return { subtype: payloadType, summary: buildCodexTokenCountSummary(raw.payload) };
    }
    return { subtype: payloadType, summary: tt("tech.codexEvent", { subtype: payloadType }) };
  }

  if (rawType === "progress") {
    const dataType = raw.data?.type || "progress";
    if (dataType === "hook_progress") {
      const hookName = raw.data?.hookName || "unknown";
      return {
        subtype: dataType,
        summary: tt("tech.hookProgress", { name: hookName }),
      };
    }
    if (dataType === "bash_progress") {
      const lines = raw.data?.totalLines;
      if (typeof lines === "number") {
        return {
          subtype: dataType,
          summary: tt("tech.bashProgress", { count: lines }),
        };
      }
      return {
        subtype: dataType,
        summary: tt("tech.bashProgressUpdate"),
      };
    }
    return {
      subtype: dataType,
      summary: tt("tech.progressUpdate", { type: dataType }),
    };
  }

  if (rawType === "system") {
    const subtype = raw.subtype || "system";
    if (subtype === "local_command") {
      const durationMs = raw.durationMs;
      const suffix = typeof durationMs === "number" ? tt("tech.duration", { ms: durationMs }) : "";
      return { subtype, summary: tt("tech.localCommand", { suffix }) };
    }
    if (subtype === "turn_duration") {
      const durationMs = raw.durationMs;
      const suffix = typeof durationMs === "number" ? tt("tech.duration", { ms: durationMs }) : "";
      return { subtype, summary: tt("tech.turnDuration", { suffix }) };
    }
    return { subtype, summary: tt("tech.systemEvent", { subtype }) };
  }

  if (rawType === "file-history-snapshot") {
    return { subtype: rawType, summary: tt("tech.fileSnapshot") };
  }

  if (rawType === "queue-operation") {
    const operation = raw.operation || "unknown";
    return { subtype: rawType, summary: tt("tech.queueOperation", { operation }) };
  }

  return {
    subtype: rawType,
    summary: tt("tech.generic", { type: rawType }),
  };
}

function getCodexChatRole(raw) {
  if (!raw || typeof raw !== "object") return null;
  const outerType = raw.type;
  const payloadType = raw.payload?.type;
  if (outerType === "response_item" && payloadType === "message") {
    const r = raw.payload?.role;
    if (r === "user" || r === "assistant") return r;
  }
  return null;
}

function extractCodexChatText(raw) {
  const outerType = raw.type;
  const payloadType = raw.payload?.type;
  const payload = raw.payload;
  if (!payload) return "";
  if (outerType === "event_msg" && (payloadType === "user_message" || payloadType === "agent_message")) {
    return String(payload.message || "");
  }
  if (outerType === "response_item" && payloadType === "message") {
    const content = payload.content;
    if (Array.isArray(content)) {
      const textItem = content.find(
        (item) => item?.type === "output_text" || item?.type === "input_text" || item?.type === "text",
      );
      return String(textItem?.text || "");
    }
  }
  return "";
}

function isCodexThinkingEvent(raw) {
  if (!raw || typeof raw !== "object") return false;
  return raw.type === "response_item" && raw.payload?.type === "reasoning";
}

function buildCodexTokenCountSummary(payload) {
  const toK = (n) => {
    if (typeof n !== "number") return "?";
    return `${(n / 1000).toFixed(1)}K`;
  };
  const fmtUnix = (ts) => {
    if (typeof ts !== "number" && typeof ts !== "string") return "?";
    const d = new Date(typeof ts === "number" ? ts * 1000 : ts);
    if (isNaN(d.getTime())) return String(ts);
    const pad = (x) => String(x).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  };
  const fmtUsageLine = (label, usage) => {
    if (!usage) return null;
    let s = `${label}: ↓ ${toK(usage.input_tokens)} ↑ ${toK(usage.output_tokens)}`;
    if (usage.cached_input_tokens) s += ` ${tt("codex.tokenCount.cache")}: ${toK(usage.cached_input_tokens)}`;
    if (usage.reasoning_output_tokens) s += ` ${tt("codex.tokenCount.reasoning")}: ${toK(usage.reasoning_output_tokens)}`;
    return s;
  };

  const primary = payload?.rate_limits?.primary;
  const lastUsage = payload?.info?.last_token_usage;
  const totalUsage = payload?.info?.total_token_usage;
  const lines = [];

  if (primary) {
    const pct = typeof primary.used_percent === "number" ? `${primary.used_percent}%` : null;
    const time = primary.resets_at != null ? fmtUnix(primary.resets_at) : null;
    if (pct || time) {
      lines.push(tt("codex.tokenCount.rateLine", { pct: pct ?? "?", time: time ?? "?" }));
    }
  }

  const lastLine = fmtUsageLine(tt("codex.tokenCount.lastUsage"), lastUsage);
  if (lastLine) lines.push(lastLine);

  const totalLine = fmtUsageLine(tt("codex.tokenCount.totalUsage"), totalUsage);
  if (totalLine) lines.push(totalLine);

  return lines.length > 0 ? lines.join("\n") : tt("tech.generic", { type: "token_count" });
}

function buildCodexFunctionCallDetail(payload) {
  if (!payload) return null;
  const pType = payload.type;
  const toolName = String(payload.name || "unknown");

  let args = {};
  const rawArgs = pType === "function_call" ? payload.arguments : payload.input;
  if (typeof rawArgs === "string") {
    try { args = JSON.parse(rawArgs); } catch { args = { input: rawArgs }; }
  } else if (rawArgs && typeof rawArgs === "object") {
    args = rawArgs;
  }

  if (toolName === "shell" || toolName === "shell_command") {
    const command = typeof args.command === "string" ? args.command.trim() : "";
    return {
      toolName,
      title: tt("tool.codex.shell.title"),
      lines: command ? [tt("tool.label.command", { text: command })] : [tt("tool.fallback.default")],
    };
  }

  if (toolName === "apply_patch") {
    const patch = typeof args.patch === "string" ? args.patch.trim() : "";
    const preview = patch ? patch.split("\n").slice(0, 4).join("\n") : "";
    return {
      toolName,
      title: tt("tool.codex.applyPatch.title"),
      lines: preview ? [preview] : [tt("tool.fallback.default")],
    };
  }

  const lines = Object.entries(args)
    .slice(0, 3)
    .map(([k, v]) => `${k}: ${typeof v === "string" ? v.slice(0, 120) : JSON.stringify(v).slice(0, 120)}`);

  return {
    toolName,
    title: tt("tool.default.title", { name: toolName }),
    lines: lines.length > 0 ? lines : [tt("tool.fallback.default")],
  };
}

function normalizeEvents(events) {
  const normalized = [];
  const toolResultsMap = new Map();
  const consumedResultIds = new Set();
  const localCommandStdoutMap = new Map();
  const codexFunctionOutputMap = new Map();

  // 第一階段：預掃描所有 tool_result 與 local-command-stdout 關聯
  for (const event of events) {
    const raw = event.raw || {};
    const contentItems = normalizeContentItems(raw.message?.content || raw.content);
    for (const item of contentItems) {
      if (item?.type === "tool_result" && item.tool_use_id) {
        toolResultsMap.set(item.tool_use_id, item);
      }
    }
    // 處理外層就是 tool_result 的情況
    if (raw.type === "tool_result" && raw.tool_use_id) {
      toolResultsMap.set(raw.tool_use_id, raw);
    }

    const parentUuid = String(
      event.parentUuid || raw.parentUuid || event.logicalParentUuid || raw.logicalParentUuid || "",
    ).trim();
    const localCommandStdout = extractLocalCommandStdout(raw);
    if (parentUuid && localCommandStdout) {
      const existing = localCommandStdoutMap.get(parentUuid);
      if (existing) {
        localCommandStdoutMap.set(parentUuid, `${existing}\n${localCommandStdout}`);
      } else {
        localCommandStdoutMap.set(parentUuid, localCommandStdout);
      }
    }
    // 預掃描 Codex function_call_output / custom_tool_call_output，以 call_id 建立 map
    if (raw.type === "response_item") {
      const pType = raw.payload?.type;
      if (pType === "function_call_output" || pType === "custom_tool_call_output") {
        const callId = raw.payload?.call_id;
        if (callId) codexFunctionOutputMap.set(callId, raw.payload);
      }
    }
  }

  // 第二階段：正規化事件
  for (const event of events) {
    const raw = event.raw || {};
    const rawType = raw.type || event.eventType || "unknown";
    const role = raw.message?.role;
    const codexRole = getCodexChatRole(raw);
    const contentItems = normalizeContentItems(raw.message?.content || raw.content);
    const parentUuid = String(
      event.parentUuid || raw.parentUuid || event.logicalParentUuid || raw.logicalParentUuid || "",
    ).trim();
    const localCommandStdout = extractLocalCommandStdout(raw);
    const commandDisplayFromRaw = extractCommandDisplay(extractChatOnlySummary(raw));
    if (parentUuid && localCommandStdout && !commandDisplayFromRaw) {
      // 純 stdout 事件已合併到 parentUuid 對應的 command 卡片，不重複顯示
      continue;
    }
    
    // 檢查此事件是否僅包含已「消費」的工具結果
    const onlyConsumedResults = contentItems.length > 0 && contentItems.every(item => 
      item?.type === "tool_result" && item.tool_use_id && toolResultsMap.has(item.tool_use_id)
    );
    const isTopLevelConsumed = rawType === "tool_result" && raw.tool_use_id && toolResultsMap.has(raw.tool_use_id);

    // 如果開啟「隱藏系統事件」且這是純工具結果，則略過渲染（因為它已合併到 tool_use 卡片）
    if (state.hideSystemEvents && (onlyConsumedResults || isTopLevelConsumed)) {
      continue;
    }

    const hasToolResultContent = contentItems.some((item) => item?.type === "tool_result");
    const roleType =
      rawType === "tool_result" || hasToolResultContent
        ? "assistant"
        : codexRole !== null
          ? codexRole
          : role === "user" || role === "assistant"
            ? role
            : rawType;

    if (roleType === "user" || roleType === "assistant") {
      // Codex events: extract text directly from payload
      if (codexRole !== null) {
        const codexText = extractCodexChatText(raw);
        normalized.push({
          kind: roleType === "user" ? "chat_user" : "chat_assistant",
          source: "codex",
          line: event.line,
          timestamp: event.timestamp,
          title: roleType === "user" ? tt("chat.user") : tt("chat.codex"),
          headerModel: "",
          summary: codexText,
          conversationSummary: codexText,
          conversationToolSummary: "",
          tags: [],
          thinkingDetails: [],
          toolUseDetails: [],
          toolResultDetails: [],
          raw,
        });
        continue;
      }

      const text = extractTextSummary(raw, toolResultsMap);
      const eventUuid = String(event.uuid || raw.uuid || "").trim();
      const correlatedLocalStdout = eventUuid ? localCommandStdoutMap.get(eventUuid) : "";
      if (correlatedLocalStdout) {
        const commandDisplay = extractCommandDisplay(text.summary) || text.summary;
        if (commandDisplay) {
          pushUniqueDetail(text.toolUseDetails, {
            title: "",
            kind: "command_stdout",
            lines: [tt("command.stdout", { text: correlatedLocalStdout })],
          });
        }
      }
      normalized.push({
        kind: roleType === "user" ? "chat_user" : "chat_assistant",
        source: "claude",
        line: event.line,
        timestamp: event.timestamp,
        title: roleType === "user" ? tt("chat.user") : tt("chat.assistant"),
        headerModel: extractHeaderModelLabel(raw),
        summary: text.summary,
        conversationSummary: extractChatOnlySummary(raw),
        conversationToolSummary: (() => {
          const names = extractToolTagNames(text.tags);
          if (names.length === 0) return "";
          return tt("summary.toolTags", { names: names.join(", ") });
        })(),
        tags:
          rawType === "tool_result" && !text.tags.includes("tool_result")
            ? [...text.tags, "tool_result"]
            : text.tags,
        thinkingDetails: text.thinkingDetails,
        toolUseDetails: text.toolUseDetails,
        toolResultDetails: text.toolResultDetails,
        raw: raw,
      });
      continue;
    }

    // Codex thinking events → show as assistant bubble with encrypted notice
    if (isCodexThinkingEvent(raw)) {
      normalized.push({
        kind: "chat_assistant",
        source: "codex",
        line: event.line,
        timestamp: event.timestamp,
        title: tt("chat.codex"),
        headerModel: "",
        summary: "",
        conversationSummary: "",
        conversationToolSummary: "",
        tags: ["thinking"],
        thinkingDetails: [tt("codex.thinking.encrypted")],
        toolUseDetails: [],
        toolResultDetails: [],
        raw,
      });
      continue;
    }

    // Codex function_call / custom_tool_call → 視為工具呼叫，渲染為 assistant bubble
    if (rawType === "response_item") {
      const pType = raw.payload?.type;
      if (pType === "function_call" || pType === "custom_tool_call") {
        const callId = raw.payload?.call_id;
        const outputPayload = callId ? codexFunctionOutputMap.get(callId) : null;
        const detail = buildCodexFunctionCallDetail(raw.payload);
        const toolName = String(raw.payload?.name || "unknown");
        const toolResultDetails = [];
        if (outputPayload) {
          const outputText = String(outputPayload.output || "").trim();
          if (outputText) {
            toolResultDetails.push({
              title: tt("tool.result.title", { index: 1 }),
              lines: [outputText],
            });
          }
        }
        normalized.push({
          kind: "chat_assistant",
          source: "codex",
          line: event.line,
          timestamp: event.timestamp,
          title: tt("chat.codex"),
          headerModel: "",
          summary: tt("summary.toolTags", { names: toolName }),
          conversationSummary: "",
          conversationToolSummary: tt("summary.toolTags", { names: toolName }),
          tags: [`tool:${toolName}`],
          thinkingDetails: [],
          toolUseDetails: detail ? [detail] : [],
          toolResultDetails,
          raw,
        });
        continue;
      }
      // function_call_output / custom_tool_call_output 已被合併至對應的 function_call 卡片，略過
      if (pType === "function_call_output" || pType === "custom_tool_call_output") {
        const callId = raw.payload?.call_id;
        if (callId && codexFunctionOutputMap.has(callId)) continue;
      }
    }

    const tech = buildTechSummary(event);
    normalized.push({
      kind: "tech",
      line: event.line,
      timestamp: event.timestamp,
      title: rawType,
      summary: tech.summary,
      techSubtype: tech.subtype,
      raw: raw,
    });
  }

  return normalized;
}

function buildTimelineItems(events) {
  const normalized = normalizeEvents(events);
  const items = [];
  let currentTechGroup = null;
  let nextGroupId = 1;

  for (const item of normalized) {
    if (item.kind === "tech") {
      if (!currentTechGroup) {
        const groupId = `tech-group-${nextGroupId++}`;
        currentTechGroup = {
          kind: "tech_group",
          id: groupId,
          events: [],
        };
        items.push(currentTechGroup);
        if (!state.techViewState[groupId]) {
          state.techViewState[groupId] = {
            expanded: false,
            visibleCount: TECH_PREVIEW_COUNT,
          };
        }
      }
      currentTechGroup.events.push(item);
      continue;
    }

    currentTechGroup = null;
    items.push(item);
  }

  return items;
}

function createElement(tag, className, text) {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (typeof text === "string") el.textContent = text;
  return el;
}

function initColumnResizers() {
  const MIN_LEFT = 180;
  const MIN_MIDDLE = 220;
  const MIN_RIGHT = 320;
  const SPLITTER_TOTAL = 16;

  const grid = refs.panelGrid;
  if (!grid || !refs.projectsPanel || !refs.entriesPanel) return;

  const setSizes = (left, middle) => {
    grid.style.setProperty("--col-left", `${Math.round(left)}px`);
    grid.style.setProperty("--col-middle", `${Math.round(middle)}px`);
  };

  const startDrag = (type, downEvent) => {
    downEvent.preventDefault();
    const startX = downEvent.clientX;
    const startLeft = refs.projectsPanel.getBoundingClientRect().width;
    const startMiddle = refs.entriesPanel.getBoundingClientRect().width;

    const onMove = (moveEvent) => {
      const dx = moveEvent.clientX - startX;
      const containerWidth = grid.getBoundingClientRect().width;

      if (type === "left") {
        const maxLeft = Math.max(MIN_LEFT, containerWidth - startMiddle - MIN_RIGHT - SPLITTER_TOTAL);
        const nextLeft = Math.min(Math.max(startLeft + dx, MIN_LEFT), maxLeft);
        setSizes(nextLeft, startMiddle);
        return;
      }

      const maxMiddle = Math.max(
        MIN_MIDDLE,
        containerWidth - startLeft - MIN_RIGHT - SPLITTER_TOTAL,
      );
      const nextMiddle = Math.min(Math.max(startMiddle + dx, MIN_MIDDLE), maxMiddle);
      setSizes(startLeft, nextMiddle);
    };

    const onUp = () => {
      document.body.classList.remove("resizing-columns");
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    document.body.classList.add("resizing-columns");
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  refs.resizerLeft?.addEventListener("pointerdown", (event) => startDrag("left", event));
  refs.resizerMiddle?.addEventListener("pointerdown", (event) => startDrag("middle", event));
}

function renderRawDetails(raw) {
  const details = document.createElement("details");
  const summary = document.createElement("summary");
  summary.textContent = "Raw JSON";
  const pre = document.createElement("pre");
  pre.textContent = JSON.stringify(raw, null, 2);
  details.append(summary, pre);
  return details;
}

function renderChatItem(item) {
  const rowClass = item.kind === "chat_user" ? "user-row" : "assist-row";
  const msgClass = item.kind === "chat_user" ? "user-msg" : "assist-msg";
  const article = createElement("article", rowClass);
  article.dataset.anchor = item.line;
  const bubble = createElement("section", msgClass);
  const header = createElement("header", "msg-header");
  const tags = Array.isArray(item.tags) ? item.tags : [];
  const visibleTags = tags
    .filter((tag) => tag !== "thinking" && tag !== "tool_result")
    .filter((tag) => !(typeof tag === "string" && tag.startsWith("tool:")));
  const roleClass = item.kind === "chat_user" ? "user-role" : "assist-role";
  const timeClass = item.kind === "chat_user" ? "user-time" : "assist-time";
  const tagClass = item.kind === "chat_user" ? "user-tag" : "assist-tag";
  header.append(createElement("span", `role-lbl ${roleClass}`, item.title));
  header.append(createElement("span", `time-lbl ${timeClass}`, formatTimestamp(item.timestamp)));
  if (visibleTags.length > 0) {
    for (const tag of visibleTags.slice(0, 2)) {
      header.append(createElement("span", `tag-badge ${tagClass}`, tag));
    }
  }
  if (item.headerModel) {
    header.append(createElement("span", `tag-badge ${tagClass}`, item.headerModel));
  }
  header.append(createElement("span", "line", `line ${item.line}`));

  const fullText = String(
    state.hideSystemEvents
      ? item.conversationSummary || (state.hideToolEvents ? "" : item.conversationToolSummary || "")
      : item.summary || "",
  );
  const isLong = fullText.length > CHAT_PREVIEW_LENGTH;
  let expanded = false;
  const searchQuery = String(state.timelineSearchQuery || "").trim().toLowerCase();
  const textClass = item.kind === "chat_user" ? "msg-text user-msg-text" : "assist-text";
  const body = createElement("p", textClass);
  body.innerHTML = highlightHtml(
    isLong ? truncateText(fullText, CHAT_PREVIEW_LENGTH) : fullText,
    searchQuery,
  );

  let contentRoot = bubble;
  if (item.kind === "chat_assistant") {
    const assistHeader = createElement("div", "assist-hdr");
    assistHeader.append(header);
    const assistBody = createElement("div", "assist-body");
    assistBody.append(body);
    bubble.append(assistHeader, assistBody);
    contentRoot = assistBody;
  } else {
    bubble.append(header, body);
  }
  article.append(bubble);

  if (isLong) {
    const toggle = createElement("button", "expand-btn", tt("action.expandContent"));
    toggle.type = "button";
    toggle.addEventListener("click", () => {
      expanded = !expanded;
      body.innerHTML = highlightHtml(
        expanded ? fullText : truncateText(fullText, CHAT_PREVIEW_LENGTH),
        searchQuery,
      );
      toggle.textContent = expanded ? tt("action.collapseContent") : tt("action.expandContent");
    });
    contentRoot.append(toggle);
  }

  if (item.kind !== "chat_assistant" && visibleTags.length > 0) {
    const tagRow = createElement("div", "tag-row");
    for (const tag of visibleTags.slice(0, 4)) {
      tagRow.append(createElement("span", "tag", tag));
    }
    contentRoot.append(tagRow);
  }

  if (
    !state.hideThinkingEvents &&
    item.kind === "chat_assistant" &&
    Array.isArray(item.thinkingDetails) &&
    item.thinkingDetails.length > 0
  ) {
    const thinkingBox = createElement("details", "assistant-fold assistant-fold--thinking");
    thinkingBox.append(
      createElement("summary", "assistant-fold-summary", tt("section.thinking", { count: item.thinkingDetails.length })),
    );
    const content = createElement("section", "assistant-thinking");
    for (const text of item.thinkingDetails.slice(0, 2)) {
      content.append(createElement("p", "assistant-thinking-text", truncateText(text, 800)));
    }
    thinkingBox.append(content);
    contentRoot.append(thinkingBox);
  }

  if (
    (item.kind === "chat_assistant" || item.kind === "chat_user") &&
    Array.isArray(item.toolUseDetails) &&
    item.toolUseDetails.length > 0
  ) {
    const visibleToolUseDetails = state.hideToolEvents
      ? item.toolUseDetails.filter((detail) =>
          detail?.kind === "command_stdout",
        )
      : item.toolUseDetails;
    for (const detail of visibleToolUseDetails.slice(0, 4)) {
      const cardClass = "assistant-tool-card assistant-tool-card--direct";
      const card = createElement("div", cardClass);
      if (String(detail?.title || "").trim()) {
        card.append(createElement("div", "assistant-tool-title", detail.title));
      }
      for (const line of detail.lines.slice(0, 5)) {
        card.append(createElement("div", "assistant-tool-line", line));
      }
      contentRoot.append(card);
    }
  }

  if (
    !state.hideToolEvents &&
    Array.isArray(item.toolResultDetails) &&
    item.toolResultDetails.length > 0
  ) {
    const resultBox = createElement("details", "assistant-fold");
    resultBox.append(
      createElement(
        "summary",
        "assistant-fold-summary",
        tt("section.toolResult", { count: item.toolResultDetails.length }),
      ),
    );
    const content = createElement("section", "assistant-tools");
    for (const detail of item.toolResultDetails.slice(0, 4)) {
      const card = createElement("div", "assistant-tool-card");
      card.append(createElement("div", "assistant-tool-title", detail.title));
      for (const line of detail.lines.slice(0, 6)) {
        card.append(createElement("div", "assistant-tool-line", line));
      }
      content.append(card);
    }
    resultBox.append(content);
    contentRoot.append(resultBox);
  }

  contentRoot.append(renderRawDetails(item.raw));
  return article;
}

function renderTechGroup(group) {
  const wrapper = createElement("section", "tool-row");
  wrapper.dataset.anchor = group.id;
  const block = createElement("section", "tool-block");
  const viewState = state.techViewState[group.id] || {
    expanded: false,
    visibleCount: TECH_PREVIEW_COUNT,
  };

  const subtypeCount = {};
  for (const item of group.events) {
    const key = item.techSubtype || "unknown";
    subtypeCount[key] = (subtypeCount[key] || 0) + 1;
  }
  const subtypeSummary = Object.entries(subtypeCount)
    .slice(0, 3)
    .map(([key, count]) => `${key} ${count}`)
    .join(" / ");

  const header = createElement("header", "tool-block-hdr");
  header.append(
    createElement(
      "span",
      "tool-hdr-txt",
      tt("tech.groupTitle", {
        count: group.events.length,
        detail: subtypeSummary ? tt("tech.groupDetail", { detail: subtypeSummary }) : "",
      }),
    ),
  );
  block.append(header);

  const headBtn = createElement(
    "button",
    "tool-raw-btn",
    viewState.expanded ? tt("action.collapseContent") : tt("action.expandContent"),
  );
  headBtn.type = "button";
  headBtn.dataset.expanded = viewState.expanded ? "true" : "false";
  headBtn.addEventListener("click", () => {
    state.techViewState[group.id] = {
      ...viewState,
      expanded: !viewState.expanded,
    };
    renderTimelineView(true);
  });
  header.append(headBtn);

  if (!viewState.expanded) {
    wrapper.append(block);
    return wrapper;
  }

  const body = createElement("div", "tool-block-body");
  const list = createElement("ul", "tech-event-list");
  const visible = Math.min(viewState.visibleCount, group.events.length);
  for (const event of group.events.slice(0, visible)) {
    const row = createElement("li", "tool-card tech-event-row");
    const meta = createElement(
      "div",
      "tech-meta",
      `${formatTimestamp(event.timestamp)} | ${event.title} | line ${event.line}`,
    );
    const summary = createElement("div", "tech-summary", event.summary);
    row.append(meta, summary, renderRawDetails(event.raw));
    list.append(row);
  }
  body.append(list);

  if (visible < group.events.length) {
    const more = createElement(
      "button",
      "load-more-btn",
      tt("action.loadMore", { count: group.events.length - visible }),
    );
    more.type = "button";
    more.addEventListener("click", () => {
      state.techViewState[group.id] = {
        ...viewState,
        visibleCount: Math.min(group.events.length, viewState.visibleCount + TECH_PREVIEW_COUNT),
      };
      renderTimelineView(true);
    });
    body.append(more);
  }

  block.append(body);
  wrapper.append(block);
  return wrapper;
}

function renderTimelineView(preserveScroll = false) {
  let anchorKey = null;
  let anchorOffset = 0;
  if (preserveScroll) {
    const containerRect = refs.viewerContent.getBoundingClientRect();
    const anchors = refs.viewerContent.querySelectorAll("[data-anchor]");
    for (const el of anchors) {
      const rect = el.getBoundingClientRect();
      if (rect.bottom > containerRect.top) {
        anchorKey = el.dataset.anchor;
        anchorOffset = rect.top - containerRect.top;
        break;
      }
    }
  }
  refs.viewerContent.innerHTML = "";

  if (state.parseErrorCode === "PARSE_PARTIAL") {
    const warning = createElement(
      "div",
      "warning",
      tt("error.parseLines", { message: formatError(state.parseErrorCode), lines: state.parseErrors.map((item) => item.line).join(", ") }),


    );
    refs.viewerContent.append(warning);
  }

  let renderedCount = 0;
  const query = String(state.timelineSearchQuery || "").trim().toLowerCase();

  for (const item of state.timelineItems) {
    if (query && !doesTimelineItemMatchSearch(item, query)) {
      continue;
    }

    if (item.kind.startsWith("chat")) {
      const isMetaChatItem =
        item?.raw?.isMeta === true || item?.raw?.message?.isMeta === true;
      if (state.hideThinkingEvents && isMetaChatItem) {
        continue;
      }
      if (state.hideSystemEvents && item.kind === "chat_assistant") {
        const summaryText = String(
          item.conversationSummary || (state.hideToolEvents ? "" : item.conversationToolSummary || ""),
        ).trim();
        const hasVisibleToolDetails =
          !state.hideToolEvents &&
          ((Array.isArray(item.toolUseDetails) && item.toolUseDetails.length > 0) ||
            (Array.isArray(item.toolResultDetails) && item.toolResultDetails.length > 0));
        const hasVisibleThinkingDetails =
          !state.hideThinkingEvents &&
          Array.isArray(item.thinkingDetails) &&
          item.thinkingDetails.length > 0;
        if (!summaryText && !hasVisibleToolDetails && !hasVisibleThinkingDetails) {
          continue;
        }
      }
      refs.viewerContent.append(renderChatItem(item));
      renderedCount += 1;
      continue;
    }

    if (!state.hideSystemEvents && item.kind === "tech_group") {
      refs.viewerContent.append(renderTechGroup(item));
      renderedCount += 1;
    }
  }

  if (renderedCount === 0) {
    const messageKey =
      query && state.timelineItems.length > 0
        ? "placeholder.searchNoResults"
        : "placeholder.emptySession";
    refs.viewerContent.innerHTML = `<p class="placeholder">${escapeHtml(tt(messageKey))}</p>`;
  } else {
    requestAnimationFrame(() => {
      if (preserveScroll && anchorKey !== null) {
        const target = refs.viewerContent.querySelector(`[data-anchor="${CSS.escape(String(anchorKey))}"]`);
        if (target) {
          const containerRect = refs.viewerContent.getBoundingClientRect();
          const targetRect = target.getBoundingClientRect();
          refs.viewerContent.scrollTop += targetRect.top - containerRect.top - anchorOffset;
        }
      } else if (!preserveScroll) {
        refs.viewerContent.scrollTop = refs.viewerContent.scrollHeight;
      }
    });
  }
}

function doesTimelineItemMatchSearch(item, query) {
  if (!query) return true;
  if (!item || typeof item !== "object") return false;

  if (String(item.kind || "").startsWith("chat")) {
    const tags = Array.isArray(item.tags) ? item.tags.join(" ") : "";
    const haystack = [
      item.title,
      item.summary,
      item.conversationSummary,
      item.conversationToolSummary,
      tags,
      formatTimestamp(item.timestamp),
    ]
      .map((part) => String(part || "").toLowerCase())
      .join("\n");
    return haystack.includes(query);
  }

  if (item.kind === "tech_group") {
    return item.events.some((event) => {
      const haystack = [
        event.title,
        event.summary,
        event.techSubtype,
        formatTimestamp(event.timestamp),
      ]
        .map((part) => String(part || "").toLowerCase())
        .join("\n");
      return haystack.includes(query);
    });
  }

  return false;
}

function calculateSessionDurationMinutes(events) {
  if (!Array.isArray(events) || events.length === 0) return null;
  let totalDurationMs = 0;

  for (const event of events) {
    const raw = event?.raw;
    if (!raw || typeof raw !== "object") continue;
    if (raw.type !== "system" || raw.subtype !== "turn_duration") continue;
    const durationMs = Number(raw.durationMs);
    if (!Number.isFinite(durationMs) || durationMs < 0) continue;
    totalDurationMs += durationMs;
  }

  if (totalDurationMs <= 0) return null;
  return Math.ceil(totalDurationMs / 60000);
}

function renderTimeline(payload) {
  const entry = findSelectedEntry();
  const rawSessionName = entry?.label
    ? String(entry.label)
    : String(payload.path || "").split(/[\\/]/).pop() || "";
  const metaPath = buildSessionMetaPath(rawSessionName);
  const events = Array.isArray(payload.events) ? payload.events : [];
  const eventCount = events.length;
  const totalMinutes = calculateSessionDurationMinutes(events);
  const metaDate = formatMetaDay(entry?.modifiedMs) || "";
  const metaRight = buildViewerMetaSummary({
    date: metaDate,
    count: eventCount,
    minutes: totalMinutes,
  });

  refs.viewerTitle.textContent = tt("viewer.timeline");
  renderViewerMeta(metaPath, metaRight);

  state.parseErrorCode = payload.errorCode || "";
  state.parseErrors = Array.isArray(payload.errors) ? payload.errors : [];
  state.techViewState = {};
  state.timelineItems = buildTimelineItems(events);

  renderTimelineView();
}

function buildViewerMetaSummary({ date, count, minutes = null }) {
  if (minutes) {
    return date
      ? tt("viewer.metaSummaryWithDuration", { date, count, minutes })
      : tt("viewer.metaSummaryNoDateWithDuration", { count, minutes });
  }
  return date
    ? tt("viewer.metaSummary", { date, count })
    : tt("viewer.metaSummaryNoDate", { count });
}

function buildLoadingMetaRight(modifiedMs) {
  const metaDate = formatMetaDay(modifiedMs) || "";
  return buildViewerMetaSummary({ date: metaDate, count: "-" });
}

function renderLoadingMeta(entry) {
  if (entry.entryType === "session") {
    refs.viewerTitle.textContent = tt("viewer.timeline");
    renderViewerMeta(
      buildSessionMetaPath(String(entry.label || "")),
      buildLoadingMetaRight(entry.modifiedMs),
    );
    return;
  }

  if (entry.entryType === "memory_file") {
    renderViewerMeta(
      buildMemoryMetaPath(String(entry.label || "")),
      buildLoadingMetaRight(entry.modifiedMs),
    );
  }
}

function bindDialogCancel(dialog, onClose) {
  dialog?.addEventListener("cancel", (event) => {
    event.preventDefault();
    onClose();
  });
}

function bindDialogSubmit(form) {
  form?.addEventListener("submit", (event) => {
    event.preventDefault();
  });
}

function bindClick(element, handler) {
  element?.addEventListener("click", handler);
}

function bindInputText(input, onChange) {
  if (!input) return;
  input.addEventListener("input", (event) => {
    onChange(String(event.target.value || ""));
  });
}

function bindTimelineFilterToggle(button, stateKey) {
  bindClick(button, () => {
    state[stateKey] = !state[stateKey];
    updateEventFilterToggles();
    renderTimelineView(true);
  });
}

function initDomRefs() {
  const selectorMap = {
    panelGrid: "#panel-grid",
    projectsPanel: "#projects-panel",
    entriesPanel: "#entries-panel",
    resizerLeft: "#resizer-left",
    resizerMiddle: "#resizer-middle",
    projectsList: "#projects-list",
    projectsSearchInput: "#projects-search-input",
    sourceToggleWrap: "#source-toggle-wrap",
    sourceToggleClaude: "#source-toggle-claude",
    sourceToggleCodex: "#source-toggle-codex",
    entriesList: "#entries-list",
    entriesSearchInput: "#entries-search-input",
    entryDateFilterWrap: "#entry-date-filter",
    viewerTitle: "#viewer-title",
    viewerMeta: "#viewer-meta",
    viewerMetaPath: "#viewer-meta-path",
    viewerMetaTime: "#viewer-meta-time",
    viewerSearchWrap: "#viewer-search",
    viewerSearchInput: "#viewer-search-input",
    viewerContent: "#viewer-content",
    status: "#status",
    hideSystemEventsToggle: "#hide-system-events-toggle",
    hideToolEventsToggle: "#hide-tool-events-toggle",
    hideThinkingEventsToggle: "#hide-thinking-events-toggle",
    hideSystemEventsWrap: "#viewer-event-toggle-group",
    localeSelect: "#locale-select",
    aboutButton: "#about-button",
    aboutDialog: "#about-dialog",
    aboutCloseButton: "#about-close",
    aboutCodexDiscoveryMode: "#about-codex-discovery-mode",
    toastViewport: "#undo-toast-viewport",
    projectDeleteDialog: "#project-delete-dialog",
    projectDeleteForm: "#project-delete-form",
    projectDeleteImpact: "#project-delete-impact",
    projectDeleteMessage: "#project-delete-message",
    projectDeleteInput: "#project-delete-input",
    projectDeleteConfirmButton: "#project-delete-confirm",
    projectDeleteCancelButton: "#project-delete-cancel",
    sessionDeleteDialog: "#session-delete-dialog",
    sessionDeleteForm: "#session-delete-form",
    sessionDeleteMessage: "#session-delete-message",
    sessionDeleteConfirmButton: "#session-delete-confirm",
    sessionDeleteCancelButton: "#session-delete-cancel",
    ctxMenu: "#ctx-menu",
  };

  for (const [key, selector] of Object.entries(selectorMap)) {
    refs[key] = document.querySelector(selector);
  }
  refs.themeButtons = Array.from(document.querySelectorAll(".theme-btn"));
}

function mergeProjects(claudeProjects, codexProjects) {
  const merged = new Map(); // normalized cwd → merged project object

  for (const p of claudeProjects) {
    const key = (p.cwdPath || p.path).replace(/\\/g, "/").toLowerCase().replace(/\/$/, "");
    merged.set(key, {
      name: p.name,
      path: p.path,
      cwdPath: p.cwdPath,
      claudePath: p.path,
      codexCwd: null,
      modifiedMs: p.modifiedMs,
      sources: ["claude"],
    });
  }

  for (const p of codexProjects) {
    const key = (p.cwdPath || p.path).replace(/\\/g, "/").toLowerCase().replace(/\/$/, "");
    if (merged.has(key)) {
      const existing = merged.get(key);
      existing.codexCwd = p.cwdPath || p.path;
      existing.sources.push("codex");
      if ((p.modifiedMs || 0) > (existing.modifiedMs || 0)) {
        existing.modifiedMs = p.modifiedMs;
      }
    } else {
      merged.set(key, {
        name: p.name,
        path: p.cwdPath || p.path,
        cwdPath: p.cwdPath,
        claudePath: null,
        codexCwd: p.cwdPath || p.path,
        modifiedMs: p.modifiedMs,
        sources: ["codex"],
      });
    }
  }

  return Array.from(merged.values()).sort((a, b) => (b.modifiedMs || 0) - (a.modifiedMs || 0));
}

async function loadProjects() {
  setInfoStatus("status.loadingProjects");
  try {
    const [claudeProjects, codexProjects] = await Promise.all([
      invoke("list_projects").catch(() => []),
      invoke("list_codex_projects").catch(() => []),
    ]);
    state.allClaudeProjects = claudeProjects;
    state.allCodexProjects = codexProjects;
    recomputeVisibleProjects();
    renderProjects();
    setInfoStatus("status.projectsLoaded", { count: state.projects.length });
  } catch (errorCode) {
    setErrorStatus(errorCode);
  }
}

async function selectProject(projectPath) {
  state.selectedProjectPath = projectPath;
  state.entrySearchQuery = "";
  state.entrySearchResults = null;
  state.isSearchingEntries = false;
  state.entryDateFilter = "all";
  if (refs.entriesSearchInput) refs.entriesSearchInput.value = "";
  updateEntryDateFilterUI();
  clearSelectedEntryState();
  renderProjects();
  setInfoStatus("status.loadingEntries");

  const project = findSelectedProject();

  try {
    const fetches = [];
    if (project?.claudePath && state.sourceToggles.claude) {
      fetches.push(invoke("list_project_entries", { projectPath: project.claudePath }));
    } else {
      fetches.push(Promise.resolve([]));
    }
    if (project?.codexCwd && state.sourceToggles.codex) {
      fetches.push(invoke("list_codex_project_entries", { cwd: project.codexCwd }));
    } else {
      fetches.push(Promise.resolve([]));
    }

    const [claudeEntries, codexEntries] = await Promise.all(fetches);
    const all = [...claudeEntries, ...codexEntries];
    all.sort((a, b) => (b.modifiedMs || 0) - (a.modifiedMs || 0));

    state.entries = all;
    state.entryExpandState = {};
    for (const entry of state.entries) {
      if (entry.entryType === "subagent_session" && entry.parentSession) {
        state.entryExpandState[entry.parentSession] = false;
      }
    }
    renderEntries();
    setInfoStatus("status.entriesLoaded", { count: state.entries.length });
  } catch (errorCode) {
    state.entries = [];
    renderEntries();
    setErrorStatus(errorCode);
  }
}

async function selectEntry(entry) {
  state.selectedEntryPath = entry.path;
  state.selectedEntryType = entry.entryType;
  renderEntries();
  setInfoStatus("status.loadingContent");
  const hideSystemEventsControl = entry.entryType === "memory_file";
  setHideSystemEventsVisible(!hideSystemEventsControl);
  setViewerSearchVisible(entry.entryType !== "memory_file");
  const carryQuery = entry.entryType !== "memory_file" ? state.entrySearchQuery.trim() : "";
  if (carryQuery) {
    state.timelineSearchQuery = carryQuery;
    if (refs.viewerSearchInput) refs.viewerSearchInput.value = carryQuery;
  } else {
    resetViewerSearchQuery();
  }
  renderLoadingMeta(entry);

  try {
    if (entry.entryType === "memory_file") {
      const payload = await invoke("read_memory", { memoryPath: entry.path });
      renderMemory(payload);
      setInfoStatus("status.memoryLoaded");
      return;
    }

    const payload = entry.source === "codex"
      ? await invoke("read_codex_session_timeline", { sessionPath: entry.path })
      : await invoke("read_session_timeline", { sessionPath: entry.path, strictMode: true });
    setHideSystemEventsVisible(true);
    renderTimeline(payload);
    if (payload.errorCode) {
      setStatus(formatError(payload.errorCode), "warn");
    } else {
      setInfoStatus("status.sessionLoaded");
    }
  } catch (errorCode) {
    if (entry.entryType !== "memory_file") {
      setHideSystemEventsVisible(true);
    }
    setErrorStatus(errorCode);
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  initDomRefs();

  for (const button of refs.themeButtons) {
    bindClick(button, () => {
      setThemeMode(button.dataset.themeMode, { persist: true });
    });
  }

  bindClick(refs.sourceToggleClaude, () => {
    state.sourceToggles.claude = !state.sourceToggles.claude;
    updateSourceToggleUI();
    recomputeVisibleProjects();
    renderProjects();
  });
  bindClick(refs.sourceToggleCodex, () => {
    state.sourceToggles.codex = !state.sourceToggles.codex;
    updateSourceToggleUI();
    recomputeVisibleProjects();
    renderProjects();
  });

  bindInputText(refs.projectsSearchInput, (value) => {
    state.projectSearchQuery = value;
    renderProjects();
  });
  bindInputText(refs.viewerSearchInput, (value) => {
    state.timelineSearchQuery = value;
    renderTimelineView();
  });
  bindInputText(refs.entriesSearchInput, (value) => {
    state.entrySearchQuery = value;
    scheduleEntrySearch(value);
  });
  bindClick(refs.entryDateFilterWrap, (e) => {
    const btn = e.target.closest(".entry-date-btn");
    if (!btn) return;
    state.entryDateFilter = btn.dataset.filter;
    updateEntryDateFilterUI();
    renderEntries();
  });
  bindClick(refs.projectDeleteCancelButton, () => {
    closeProjectDeleteDialog();
  });
  refs.projectDeleteInput?.addEventListener("input", () => {
    updateProjectDeleteConfirmState();
  });
  bindClick(refs.projectDeleteConfirmButton, async () => {
    await confirmProjectDelete();
  });
  bindDialogCancel(refs.projectDeleteDialog, closeProjectDeleteDialog);
  bindDialogSubmit(refs.projectDeleteForm);
  bindClick(refs.sessionDeleteCancelButton, () => {
    closeSessionDeleteDialog();
  });
  bindClick(refs.sessionDeleteConfirmButton, () => {
    confirmSessionDelete();
  });
  bindDialogCancel(refs.sessionDeleteDialog, closeSessionDeleteDialog);
  bindDialogSubmit(refs.sessionDeleteForm);
  bindClick(refs.aboutButton, async () => {
    await refreshAboutDialog();
    openAboutDialog();
  });
  bindClick(refs.aboutCloseButton, () => {
    closeAboutDialog();
  });
  bindDialogCancel(refs.aboutDialog, closeAboutDialog);

  // Context menu global close handlers
  document.addEventListener("click", (e) => {
    if (refs.ctxMenu && !refs.ctxMenu.contains(e.target)) {
      hideContextMenu();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (!refs.ctxMenu || refs.ctxMenu.hidden) return;
    if (e.key === "Escape") {
      hideContextMenu();
      state.ctxMenuTarget?.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = Array.from(refs.ctxMenu.querySelectorAll(".ctx-menu-item"));
      const idx = items.indexOf(document.activeElement);
      const next = e.key === "ArrowDown"
        ? items[(idx + 1) % items.length]
        : items[(idx - 1 + items.length) % items.length];
      next?.focus();
    }
  });
  refs.ctxMenu?.addEventListener("contextmenu", (e) => e.preventDefault());

  renderUndoToasts();
  state.hideSystemEvents = true;
  state.hideToolEvents = true;
  state.hideThinkingEvents = true;
  updateEventFilterToggles();
  bindTimelineFilterToggle(refs.hideSystemEventsToggle, "hideSystemEvents");
  bindTimelineFilterToggle(refs.hideToolEventsToggle, "hideToolEvents");
  bindTimelineFilterToggle(refs.hideThinkingEventsToggle, "hideThinkingEvents");
  initLocaleSelector();
  initThemeMode();
  initColumnResizers();

  clearViewer();
  updateSourceToggleUI();
  await loadProjects();
});
