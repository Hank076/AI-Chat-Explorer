use serde::Serialize;
use serde_json::Value;
use rusqlite::{params_from_iter, Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_READ_FAILED: &str = "READ_FAILED";
const ERR_PARSE_PARTIAL: &str = "PARSE_PARTIAL";
const NEGATIVE_CWD_CACHE_TTL_MS: u64 = 60_000;
const CWD_CACHE_FALLBACK_TTL_MS: u64 = 300_000;

static PROJECT_CWD_CACHE: OnceLock<Mutex<HashMap<String, ProjectCwdCacheEntry>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct ProjectCwdCacheEntry {
    cwd_path: Option<String>,
    source_session: Option<PathBuf>,
    source_session_mtime_ms: Option<u64>,
    cached_at_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    name: String,
    path: String,
    cwd_path: Option<String>,
    modified_ms: Option<u64>,
    source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    entry_type: String,
    label: String,
    path: String,
    parent_session: Option<String>,
    modified_ms: Option<u64>,
    size_bytes: Option<u64>,
    source: String,
    hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPayload {
    path: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseError {
    line: usize,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    line: usize,
    timestamp: Option<String>,
    role: Option<String>,
    event_type: Option<String>,
    subtype: Option<String>,
    uuid: Option<String>,
    parent_uuid: Option<String>,
    logical_parent_uuid: Option<String>,
    session_id: Option<String>,
    request_id: Option<String>,
    message_id: Option<String>,
    tool_use_id: Option<String>,
    parent_tool_use_id: Option<String>,
    operation: Option<String>,
    is_sidechain: Option<bool>,
    is_meta: Option<bool>,
    summary: String,
    raw: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub model_name: Option<String>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_input_tokens: u64,
    pub total_cache_read_input_tokens: u64,
    pub total_web_search_requests: u64,
    pub total_web_fetch_requests: u64,
    pub service_tier: Option<String>,
    pub speed: Option<String>,
    pub inference_geo: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTimelinePayload {
    pub path: String,
    pub error_code: Option<String>,
    pub errors: Vec<ParseError>,
    pub events: Vec<TimelineEvent>,
    pub metadata: SessionMetadata,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeleteImpact {
    pub session_count: usize,
    pub subagent_session_count: usize,
    pub memory_file_count: usize,
    pub total_file_count: usize,
    pub total_size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProjectDiscoveryMode {
    pub mode: String,
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    path: String,
    match_count: usize,
    source: String,
}

#[derive(Debug, Default)]
struct SessionMetadataAccumulator {
    model_name: Option<String>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_creation_input_tokens: u64,
    total_cache_read_input_tokens: u64,
    total_web_search_requests: u64,
    total_web_fetch_requests: u64,
    service_tier: Option<String>,
    speed: Option<String>,
    inference_geo: Option<String>,
}

impl SessionMetadataAccumulator {
    fn observe_model_name(&mut self, value: &Value) {
        if self.model_name.is_none() {
            self.model_name = extract_string_paths(
                value,
                &[
                    "message.model",
                    "model",
                    "model_name",
                    "request.model",
                    "request.message.model",
                ],
            );
        }
    }

    fn add_usage_from_event(&mut self, value: &Value) {
        let Some(usage) = value
            .get("usage")
            .or_else(|| value.get("message").and_then(|message| message.get("usage")))
        else {
            return;
        };

        self.total_input_tokens += usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        self.total_output_tokens += usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        self.total_cache_creation_input_tokens += usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        self.total_cache_read_input_tokens += usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if let Some(server_tool_use) = usage.get("server_tool_use") {
            self.total_web_search_requests += server_tool_use
                .get("web_search_requests")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            self.total_web_fetch_requests += server_tool_use
                .get("web_fetch_requests")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
        }

        if self.service_tier.is_none() {
            self.service_tier = usage
                .get("service_tier")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
        if self.speed.is_none() {
            self.speed = usage.get("speed").and_then(|v| v.as_str()).map(str::to_string);
        }
        if self.inference_geo.is_none() {
            self.inference_geo = usage
                .get("inference_geo")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
    }

    fn build_metadata(self, start_time: Option<String>, end_time: Option<String>) -> SessionMetadata {
        SessionMetadata {
            model_name: self.model_name,
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
            total_cache_creation_input_tokens: self.total_cache_creation_input_tokens,
            total_cache_read_input_tokens: self.total_cache_read_input_tokens,
            total_web_search_requests: self.total_web_search_requests,
            total_web_fetch_requests: self.total_web_fetch_requests,
            service_tier: self.service_tier,
            speed: self.speed,
            inference_geo: self.inference_geo,
            start_time,
            end_time,
        }
    }
}

fn build_entry(entry_type: &str, path: &Path, label: String, parent_session: Option<String>, source: &str) -> Entry {
    let (modified_ms, size_bytes) = get_file_metadata(path);
    Entry {
        entry_type: entry_type.to_string(),
        label,
        path: path.to_string_lossy().to_string(),
        parent_session,
        modified_ms,
        size_bytes,
        source: source.to_string(),
        hidden: false,
    }
}

fn file_name_or(path: &Path, fallback: &str) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

#[tauri::command]
pub fn list_projects(base_path: Option<String>) -> Result<Vec<Project>, String> {
    let root = resolve_root_path(base_path.as_deref())?;
    let mut projects = Vec::new();

    for item in fs::read_dir(root).map_err(map_read_error)? {
        let item = item.map_err(map_read_error)?;
        let file_type = item.file_type().map_err(map_read_error)?;
        if !file_type.is_dir() {
            continue;
        }

        let path = item.path();
        let cwd_path = infer_project_cwd_path_cached(&path);
        let name = cwd_path
            .as_deref()
            .and_then(project_name_from_cwd)
            .unwrap_or_else(|| item.file_name().to_string_lossy().to_string());
        let (modified_ms, _) = get_file_metadata(&path);
        projects.push(Project {
            name,
            path: path.to_string_lossy().to_string(),
            cwd_path,
            modified_ms,
            source: "claude".to_string(),
        });
    }

    projects.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(projects)
}

#[tauri::command]
pub fn list_project_entries(
    project_path: String,
    base_path: Option<String>,
) -> Result<Vec<Entry>, String> {
    let root = resolve_root_path(base_path.as_deref())?;
    let project = validate_under_root(&root, Path::new(&project_path))?;
    if !project.is_dir() {
        return Err(ERR_NOT_FOUND.to_string());
    }

    let mut entries = Vec::new();
    let memory_dir = project.join("memory");
    if memory_dir.is_dir() {
        let mut memory_files = Vec::new();
        collect_files_recursive(&memory_dir, &mut memory_files)?;
        memory_files.sort_by_key(|path| {
            path.to_string_lossy()
                .to_string()
                .to_lowercase()
        });

        for memory_file in memory_files {
            let label = file_name_or(&memory_file, "unknown");
            entries.push(build_entry("memory_file", &memory_file, label, None, "claude"));
        }
    }

    let mut sessions: Vec<PathBuf> = fs::read_dir(&project)
        .map_err(map_read_error)?
        .filter_map(|item| item.ok().map(|v| v.path()))
        .filter(|path| path.is_file() && has_jsonl_extension(path))
        .collect();
    sessions.sort_by(|a, b| {
        let (a_mod, _) = get_file_metadata(a);
        let (b_mod, _) = get_file_metadata(b);
        b_mod.cmp(&a_mod)
    });

    for session in sessions {
        let stem = session
            .file_stem()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or_default();
        let session_label = file_name_or(&session, "unknown.jsonl");

        entries.push(build_entry("session", &session, session_label, None, "claude"));

        let subagents_dir = project.join(&stem).join("subagents");
        if !subagents_dir.is_dir() {
            continue;
        }

        let mut subagent_files: Vec<PathBuf> = fs::read_dir(subagents_dir)
            .map_err(map_read_error)?
            .filter_map(|item| item.ok().map(|v| v.path()))
            .filter(|path| path.is_file() && has_jsonl_extension(path))
            .collect();
        subagent_files.sort_by(|a, b| {
            let (a_mod, _) = get_file_metadata(a);
            let (b_mod, _) = get_file_metadata(b);
            b_mod.cmp(&a_mod)
        });

        for subagent_file in subagent_files {
            let label = file_name_or(&subagent_file, "unknown.jsonl");
            entries.push(build_entry(
                "subagent_session",
                &subagent_file,
                label,
                Some(stem.clone()),
                "claude",
            ));
        }
    }

    Ok(entries)
}

#[tauri::command]
pub fn read_memory(memory_path: String, base_path: Option<String>) -> Result<MemoryPayload, String> {
    let root = resolve_root_path(base_path.as_deref())?;
    let memory_file = validate_under_root(&root, Path::new(&memory_path))?;
    if !memory_file.is_file() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    let content = fs::read_to_string(&memory_file).map_err(map_read_error)?;
    Ok(MemoryPayload {
        path: memory_file.to_string_lossy().to_string(),
        content,
    })
}

#[tauri::command]
pub fn read_session_timeline(
    session_path: String,
    base_path: Option<String>,
    strict_mode: Option<bool>,
) -> Result<SessionTimelinePayload, String> {
    let root = resolve_root_path(base_path.as_deref())?;
    let session_file = validate_under_root(&root, Path::new(&session_path))?;
    if !session_file.is_file() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    if !has_jsonl_extension(&session_file) {
        return Err(ERR_READ_FAILED.to_string());
    }

    let content = fs::read_to_string(&session_file).map_err(map_read_error)?;
    let mut events = Vec::new();
    let mut errors = Vec::new();

    let mut metadata_accumulator = SessionMetadataAccumulator::default();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                metadata_accumulator.observe_model_name(&value);
                metadata_accumulator.add_usage_from_event(&value);
                events.push(build_timeline_event(line_number, value));
            }
            Err(_) => errors.push(ParseError {
                line: line_number,
                message: "invalid json".to_string(),
            }),
        }
    }

    if !strict_mode.unwrap_or(false) {
        sort_events_by_time(&mut events);
    }

    let start_time = events.first().and_then(|e| e.timestamp.clone());
    let end_time = events.last().and_then(|e| e.timestamp.clone());

    Ok(SessionTimelinePayload {
        path: session_file.to_string_lossy().to_string(),
        error_code: if errors.is_empty() {
            None
        } else {
            Some(ERR_PARSE_PARTIAL.to_string())
        },
        errors,
        events,
        metadata: metadata_accumulator.build_metadata(start_time, end_time),
    })
}

#[tauri::command]
pub fn search_sessions(
    project_path: String,
    query: String,
    base_path: Option<String>,
    paths: Option<Vec<String>>,
) -> Result<Vec<SessionSearchResult>, String> {
    let root = resolve_root_path(base_path.as_deref())?;
    let project = validate_under_root(&root, Path::new(&project_path))?;
    if !project.is_dir() {
        return Err(ERR_NOT_FOUND.to_string());
    }

    let query_lower = query.to_lowercase();
    if query_lower.is_empty() {
        return Ok(vec![]);
    }

    let all_files: Vec<PathBuf> = if let Some(path_strings) = paths {
        // Targeted search: only search caller-specified files (validate each against root)
        let mut files = Vec::new();
        for path_str in path_strings {
            match validate_under_root(&root, Path::new(&path_str)) {
                Ok(canonical) if canonical.is_file() && has_jsonl_extension(&canonical) => {
                    files.push(canonical);
                }
                _ => continue,
            }
        }
        files
    } else {
        // Full discovery: collect top-level session files + subagent files
        let mut session_files: Vec<PathBuf> = Vec::new();
        let mut stems: Vec<String> = Vec::new();

        for item in fs::read_dir(&project).map_err(map_read_error)? {
            let item = item.map_err(map_read_error)?;
            let path = item.path();
            if path.is_file() && has_jsonl_extension(&path) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    stems.push(stem.to_string());
                }
                session_files.push(path);
            }
        }

        let mut all = session_files;
        for stem in &stems {
            let subagents_dir = project.join(stem).join("subagents");
            if !subagents_dir.is_dir() {
                continue;
            }
            for item in fs::read_dir(&subagents_dir).map_err(map_read_error)? {
                let item = item.map_err(map_read_error)?;
                let path = item.path();
                if path.is_file() && has_jsonl_extension(&path) {
                    all.push(path);
                }
            }
        }
        all
    };

    let mut results = Vec::new();
    for file_path in all_files {
        let file = match fs::File::open(&file_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let mut match_count: usize = 0;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.to_lowercase().contains(&query_lower) {
                match_count += 1;
            }
        }
        if match_count > 0 {
            results.push(SessionSearchResult {
                path: file_path.to_string_lossy().to_string(),
                match_count,
                source: "claude".to_string(),
            });
        }
    }

    Ok(results)
}

#[tauri::command]
pub fn delete_session(session_path: String, base_path: Option<String>) -> Result<(), String> {
    let root = resolve_root_path(base_path.as_deref())?;
    let session_file = validate_under_root(&root, Path::new(&session_path))?;
    if !session_file.is_file() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    if !has_jsonl_extension(&session_file) {
        return Err(ERR_READ_FAILED.to_string());
    }

    fs::remove_file(&session_file).map_err(map_read_error)?;

    if let Some(parent) = session_file.parent() {
        let stem = session_file
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        if !stem.is_empty() {
            let subagent_dir = parent.join(&stem);
            if subagent_dir.is_dir() {
                fs::remove_dir_all(&subagent_dir).map_err(map_read_error)?;
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub fn delete_codex_session(session_path: String, base_path: Option<String>) -> Result<(), String> {
    let root = if let Some(path) = base_path {
        let p = PathBuf::from(path);
        if !p.exists() {
            return Err(ERR_NOT_FOUND.to_string());
        }
        p.canonicalize().map_err(map_read_error)?
    } else {
        default_codex_sessions_path()?.canonicalize().map_err(map_read_error)?
    };
    let canonical = Path::new(&session_path).canonicalize().map_err(map_read_error)?;
    if !canonical.starts_with(&root) {
        return Err(ERR_READ_FAILED.to_string());
    }
    if !canonical.is_file() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    if !has_jsonl_extension(&canonical) {
        return Err(ERR_READ_FAILED.to_string());
    }
    fs::remove_file(&canonical).map_err(map_read_error)?;
    Ok(())
}

fn accumulate_project_delete_impact(impact: &mut ProjectDeleteImpact, entries: &[Entry]) {
    for entry in entries {
        match entry.entry_type.as_str() {
            "session" => impact.session_count += 1,
            "subagent_session" => impact.subagent_session_count += 1,
            "memory_file" => impact.memory_file_count += 1,
            _ => {}
        }
        impact.total_file_count += 1;
        impact.total_size_bytes = impact
            .total_size_bytes
            .saturating_add(entry.size_bytes.unwrap_or(0));
    }
}

fn collect_codex_project_entries_for_delete_impact(
    cwd: &str,
    sessions_root: &Path,
) -> Result<Vec<Entry>, String> {
    if !sessions_root.exists() {
        return Ok(vec![]);
    }

    let canonical_root = sessions_root.canonicalize().map_err(map_read_error)?;
    Ok(list_codex_project_entries_from_scan(cwd, &canonical_root))
}

fn delete_codex_project_sessions_in(cwd: &str, sessions_root: &Path) -> Result<(), String> {
    let entries = collect_codex_project_entries_for_delete_impact(cwd, sessions_root)?;
    let canonical_root = sessions_root.canonicalize().map_err(map_read_error)?;
    let mut deleted_paths = HashSet::new();

    for entry in entries {
        if entry.entry_type != "session" || !deleted_paths.insert(entry.path.clone()) {
            continue;
        }

        let session_file = Path::new(&entry.path).canonicalize().map_err(map_read_error)?;
        if !session_file.starts_with(&canonical_root) {
            return Err(ERR_READ_FAILED.to_string());
        }
        if !session_file.is_file() {
            continue;
        }
        if !has_jsonl_extension(&session_file) {
            return Err(ERR_READ_FAILED.to_string());
        }

        fs::remove_file(&session_file).map_err(map_read_error)?;
    }

    Ok(())
}

fn delete_codex_project_sessions(cwd: &str) -> Result<(), String> {
    let sessions_root = default_codex_sessions_path()?;
    if !sessions_root.exists() {
        return Ok(());
    }

    delete_codex_project_sessions_in(cwd, &sessions_root)
}

#[tauri::command]
pub fn delete_project(
    project_path: Option<String>,
    codex_cwd: Option<String>,
    base_path: Option<String>,
) -> Result<(), String> {
    let claude_target = project_path.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let codex_target = codex_cwd.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    if claude_target.is_none() && codex_target.is_none() {
        return Err(ERR_NOT_FOUND.to_string());
    }

    if let Some(project_path) = claude_target {
        let root = resolve_root_path(base_path.as_deref())?;
        let project_dir = validate_under_root(&root, Path::new(&project_path))?;
        if !project_dir.is_dir() {
            return Err(ERR_NOT_FOUND.to_string());
        }

        fs::remove_dir_all(project_dir).map_err(map_read_error)?;
    }

    if let Some(cwd) = codex_target {
        delete_codex_project_sessions(&cwd)?;
    }

    Ok(())
}

#[tauri::command]
pub fn get_project_delete_impact(
    project_path: Option<String>,
    codex_cwd: Option<String>,
    base_path: Option<String>,
) -> Result<ProjectDeleteImpact, String> {
    let claude_target = project_path.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let codex_target = codex_cwd.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    if claude_target.is_none() && codex_target.is_none() {
        return Err(ERR_NOT_FOUND.to_string());
    }

    let mut impact = ProjectDeleteImpact {
        session_count: 0,
        subagent_session_count: 0,
        memory_file_count: 0,
        total_file_count: 0,
        total_size_bytes: 0,
    };

    if let Some(project_path) = claude_target {
        let entries = list_project_entries(project_path, base_path.clone())?;
        accumulate_project_delete_impact(&mut impact, &entries);
    }

    if let Some(cwd) = codex_target {
        let sessions_root = default_codex_sessions_path()?;
        let entries = collect_codex_project_entries_for_delete_impact(&cwd, &sessions_root)?;
        accumulate_project_delete_impact(&mut impact, &entries);
    }

    Ok(impact)
}

fn build_timeline_event(line: usize, raw: Value) -> TimelineEvent {
    let timestamp = extract_string_paths(&raw, &["timestamp", "created_at", "time", "ts"]);
    let role = extract_string_paths(
        &raw,
        &["message.role", "role", "speaker", "author", "actor"],
    );
    let event_type = extract_string_paths(&raw, &["type", "event_type", "event"]);
    let subtype = extract_string_paths(&raw, &["subtype", "data.type"]);
    let uuid = extract_string_paths(&raw, &["uuid"]);
    let parent_uuid = extract_string_paths(&raw, &["parentUuid"]);
    let logical_parent_uuid = extract_string_paths(&raw, &["logicalParentUuid"]);
    let session_id = extract_string_paths(&raw, &["sessionId"]);
    let request_id = extract_string_paths(&raw, &["requestId"]);
    let message_id = extract_string_paths(&raw, &["messageId", "message.id"]);
    let tool_use_id = extract_string_paths(&raw, &["toolUseID", "sourceToolUseID"]);
    let parent_tool_use_id = extract_string_paths(&raw, &["parentToolUseID"]);
    let operation = extract_string_paths(&raw, &["operation"]);
    let is_sidechain = raw.get("isSidechain").and_then(|v| v.as_bool());
    let is_meta = raw.get("isMeta").and_then(|v| v.as_bool());
    let summary = build_summary(&raw);

    TimelineEvent {
        line,
        timestamp,
        role,
        event_type,
        subtype,
        uuid,
        parent_uuid,
        logical_parent_uuid,
        session_id,
        request_id,
        message_id,
        tool_use_id,
        parent_tool_use_id,
        operation,
        is_sidechain,
        is_meta,
        summary,
        raw,
    }
}

fn sort_events_by_time(events: &mut [TimelineEvent]) {
    let has_any_timestamp = events.iter().any(|event| event.timestamp.is_some());
    if !has_any_timestamp {
        return;
    }

    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.line.cmp(&b.line)));
}

fn build_summary(value: &Value) -> String {
    if let Some(text) = extract_string(value, &["content", "message", "text", "summary"]) {
        return truncate(&text, 240);
    }
    truncate(&value.to_string(), 240)
}

fn truncate(input: &str, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }
    let shortened: String = input.chars().take(max_len).collect();
    format!("{shortened}...")
}

fn extract_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let found = value.get(key).and_then(|v| v.as_str());
        if let Some(s) = found {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn extract_string_paths(value: &Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        let found = get_path_value(value, path).and_then(|v| v.as_str());
        if let Some(s) = found {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn get_path_value<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn infer_project_cwd_path_cached(project_dir: &Path) -> Option<String> {
    let key = project_dir.to_string_lossy().to_string();
    let now_ms = now_unix_ms();

    if let Some(cached) = get_cached_project_cwd(&key) {
        if let Some(source) = cached.source_session.as_ref() {
            let current_mtime = get_file_metadata(source).0;
            if current_mtime == cached.source_session_mtime_ms {
                return cached.cwd_path;
            }
        } else if cached.cwd_path.is_none() {
            if now_ms.saturating_sub(cached.cached_at_ms) < NEGATIVE_CWD_CACHE_TTL_MS {
                return None;
            }
        } else if now_ms.saturating_sub(cached.cached_at_ms) < CWD_CACHE_FALLBACK_TTL_MS {
            return cached.cwd_path;
        }
    }

    let fresh = infer_project_cwd_with_source(project_dir, now_ms);
    set_cached_project_cwd(&key, fresh.clone());
    fresh.cwd_path
}

fn infer_project_cwd_with_source(project_dir: &Path, cached_at_ms: u64) -> ProjectCwdCacheEntry {
    let mut session_files: Vec<PathBuf> = match fs::read_dir(project_dir) {
        Ok(read_dir) => read_dir
            .filter_map(|item| item.ok().map(|v| v.path()))
            .filter(|path| path.is_file() && has_jsonl_extension(path))
            .collect(),
        Err(_) => {
            return ProjectCwdCacheEntry {
                cwd_path: None,
                source_session: None,
                source_session_mtime_ms: None,
                cached_at_ms,
            };
        }
    };

    if session_files.is_empty() {
        return ProjectCwdCacheEntry {
            cwd_path: None,
            source_session: None,
            source_session_mtime_ms: None,
            cached_at_ms,
        };
    }

    session_files.sort_by(|a, b| {
        let (a_mod, a_size) = get_file_metadata(a);
        let (b_mod, b_size) = get_file_metadata(b);
        a_size.cmp(&b_size).then(a_mod.cmp(&b_mod)).then(a.cmp(b))
    });

    for session_file in &session_files {
        let session_mtime = get_file_metadata(session_file).0;
        if let Some(cwd) = extract_cwd_from_session_file(&session_file, 300) {
            return ProjectCwdCacheEntry {
                cwd_path: Some(cwd),
                source_session: Some(session_file.clone()),
                source_session_mtime_ms: session_mtime,
                cached_at_ms,
            };
        }
    }

    ProjectCwdCacheEntry {
        cwd_path: None,
        source_session: None,
        source_session_mtime_ms: None,
        cached_at_ms,
    }
}

fn get_cached_project_cwd(key: &str) -> Option<ProjectCwdCacheEntry> {
    let cache = PROJECT_CWD_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let guard = cache.lock().ok()?;
    guard.get(key).cloned()
}

fn set_cached_project_cwd(key: &str, entry: ProjectCwdCacheEntry) {
    let cache = PROJECT_CWD_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key.to_string(), entry);
    }
}

fn extract_cwd_from_session_file(path: &Path, max_lines: usize) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line_result in reader.lines().take(max_lines) {
        let line = line_result.ok()?;
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(cwd) = find_string_by_key_recursive(&value, &["cwd", "current_working_directory"]) {
            return Some(cwd);
        }
    }
    None
}

fn find_string_by_key_recursive(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(|v| v.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
            for child in map.values() {
                if let Some(found) = find_string_by_key_recursive(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_string_by_key_recursive(item, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    let normalized_cwd = display_codex_cwd(cwd);
    let mut last = None;
    for component in Path::new(&normalized_cwd).components() {
        if let Component::Normal(value) = component {
            let text = value.to_string_lossy().trim().to_string();
            if !text.is_empty() {
                last = Some(text);
            }
        }
    }
    last
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn has_jsonl_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
        .unwrap_or(false)
}

fn has_json_or_jsonl_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json") || ext.eq_ignore_ascii_case("jsonl"))
        .unwrap_or(false)
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for item in fs::read_dir(dir).map_err(map_read_error)? {
        let item = item.map_err(map_read_error)?;
        let path = item.path();
        if path.is_dir() {
            collect_files_recursive(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn resolve_root_path(base_path: Option<&str>) -> Result<PathBuf, String> {
    let root = if let Some(path) = base_path {
        PathBuf::from(path)
    } else {
        default_projects_path()?
    };

    if !root.exists() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    root.canonicalize().map_err(map_read_error)
}

fn validate_under_root(root: &Path, target: &Path) -> Result<PathBuf, String> {
    let canonical_target = target.canonicalize().map_err(map_read_error)?;
    if !canonical_target.starts_with(root) {
        return Err(ERR_READ_FAILED.to_string());
    }
    Ok(canonical_target)
}

fn default_projects_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| ERR_READ_FAILED.to_string())?;
    Ok(home.join(".claude").join("projects"))
}

fn default_codex_home_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| ERR_READ_FAILED.to_string())?;
    Ok(home.join(".codex"))
}

fn map_read_error(error: std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => ERR_NOT_FOUND.to_string(),
        _ => ERR_READ_FAILED.to_string(),
    }
}

fn get_file_metadata(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(meta) = fs::metadata(path) else {
        return (None, None);
    };

    let size = Some(meta.len());
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    (modified_ms, size)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(byte);
                    index += 3;
                    continue;
                }
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).to_string()
}

fn decode_vscode_workspace_folder_uri(uri: &str) -> Option<String> {
    let path = percent_decode(uri.trim().strip_prefix("file://")?);
    let bytes = path.as_bytes();

    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
        let drive = (bytes[1] as char).to_ascii_uppercase();
        let rest = path[3..].trim_start_matches('/').replace('/', r"\");
        return Some(format!("{drive}:\\{rest}"));
    }

    Some(path)
}

fn latest_modified_ms_in_dir(dir: &Path) -> Option<u64> {
    let mut latest = None;
    let entries = fs::read_dir(dir).ok()?;

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let modified_ms = get_file_metadata(&entry.path()).0;
        if modified_ms > latest {
            latest = modified_ms;
        }
    }

    latest
}

fn list_vscode_copilot_projects_from_root(root: &Path) -> Result<Vec<Project>, String> {
    if !root.exists() {
        return Ok(vec![]);
    }

    let mut projects = Vec::new();
    for item in fs::read_dir(root).map_err(map_read_error)? {
        let Ok(item) = item else {
            continue;
        };
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let workspace = item.path();
        let chat_sessions = workspace.join("chatSessions");
        if !chat_sessions.is_dir() {
            continue;
        }

        let workspace_json = workspace.join("workspace.json");
        let Ok(content) = fs::read_to_string(workspace_json) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        let Some(cwd_path) = value
            .get("folder")
            .and_then(|folder| folder.as_str())
            .and_then(decode_vscode_workspace_folder_uri)
        else {
            continue;
        };

        let name = project_name_from_cwd(&cwd_path).unwrap_or_else(|| item.file_name().to_string_lossy().to_string());
        projects.push(Project {
            name,
            path: cwd_path.clone(),
            cwd_path: Some(cwd_path),
            modified_ms: latest_modified_ms_in_dir(&chat_sessions),
            source: "vscode".to_string(),
        });
    }

    projects.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(projects)
}

fn default_vscode_workspace_storage_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| ERR_READ_FAILED.to_string())?;
        Ok(appdata.join("Code").join("User").join("workspaceStorage"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| ERR_READ_FAILED.to_string())?;
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("workspaceStorage"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| ERR_READ_FAILED.to_string())?;
        Ok(home.join(".config").join("Code").join("User").join("workspaceStorage"))
    }
}

fn find_vscode_workspace_hash_for_cwd(root: &Path, cwd: &str) -> Result<String, String> {
    if !root.exists() {
        return Err(ERR_NOT_FOUND.to_string());
    }

    let target = normalize_cwd_for_comparison(cwd);
    for item in fs::read_dir(root).map_err(map_read_error)? {
        let Ok(item) = item else {
            continue;
        };
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let workspace_json = item.path().join("workspace.json");
        let Ok(content) = fs::read_to_string(workspace_json) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        let Some(workspace_cwd) = value
            .get("folder")
            .and_then(|folder| folder.as_str())
            .and_then(decode_vscode_workspace_folder_uri)
        else {
            continue;
        };

        if normalize_cwd_for_comparison(&workspace_cwd) == target {
            return Ok(item.file_name().to_string_lossy().to_string());
        }
    }

    Err(ERR_NOT_FOUND.to_string())
}

fn list_vscode_copilot_project_entries_from_root(
    cwd: &str,
    root: &Path,
) -> Result<Vec<Entry>, String> {
    let workspace_hash = find_vscode_workspace_hash_for_cwd(root, cwd)?;
    let workspace = root.join(workspace_hash);
    let chat_sessions = workspace.join("chatSessions");
    let Ok(canonical_workspace) = workspace.canonicalize() else {
        return Ok(vec![]);
    };
    let Ok(canonical_chat_sessions) = chat_sessions.canonicalize() else {
        return Ok(vec![]);
    };
    if !canonical_chat_sessions.starts_with(&canonical_workspace) {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    for item in fs::read_dir(chat_sessions).map_err(map_read_error)? {
        let Ok(item) = item else {
            continue;
        };
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let file = item.path();
        if !has_json_or_jsonl_extension(&file) {
            continue;
        }
        if !vscode_copilot_session_has_visible_timeline(&file) {
            continue;
        }

        let label = file_name_or(&file, "session");
        entries.push(build_entry("session", &file, label, None, "vscode"));
    }

    entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(entries)
}

fn vscode_copilot_session_has_visible_timeline(path: &Path) -> bool {
    read_vscode_copilot_session_timeline_from_path(path)
        .map(|payload| !payload.events.is_empty())
        .unwrap_or(false)
}

#[tauri::command]
pub fn list_vscode_copilot_projects(base_path: Option<String>) -> Result<Vec<Project>, String> {
    let root = match base_path {
        Some(path) => PathBuf::from(path),
        None => default_vscode_workspace_storage_path()?,
    };
    list_vscode_copilot_projects_from_root(&root)
}

#[tauri::command]
pub fn list_vscode_copilot_project_entries(
    cwd: String,
    base_path: Option<String>,
) -> Result<Vec<Entry>, String> {
    let root = match base_path {
        Some(path) => PathBuf::from(path),
        None => default_vscode_workspace_storage_path()?,
    };
    if !root.exists() {
        return Ok(vec![]);
    }
    match list_vscode_copilot_project_entries_from_root(&cwd, &root) {
        Ok(entries) => Ok(entries),
        Err(error) if error == ERR_NOT_FOUND => Ok(vec![]),
        Err(error) => Err(error),
    }
}

fn read_vscode_copilot_state_json(path: &Path) -> Result<(Value, Vec<ParseError>), String> {
    let content = fs::read_to_string(path).map_err(map_read_error)?;
    let state = serde_json::from_str::<Value>(&content).map_err(|_| ERR_READ_FAILED.to_string())?;
    Ok((state, Vec::new()))
}

fn read_vscode_copilot_state_jsonl(path: &Path) -> Result<(Value, Vec<ParseError>), String> {
    let content = fs::read_to_string(path).map_err(map_read_error)?;
    let mut state = Value::Object(serde_json::Map::new());
    let mut errors = Vec::new();
    let mut parsed_entries = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(line) else {
            errors.push(ParseError {
                line: line_number,
                message: "invalid json".to_string(),
            });
            continue;
        };

        parsed_entries.push(value.clone());

        if line_number == 1 {
            if let Some(initial_state) = value.get("v") {
                state = initial_state.clone();
            }
            continue;
        }

        let Some(path) = value.get("k").and_then(|v| v.as_array()) else {
            continue;
        };
        let Some(patch_value) = value.get("v") else {
            continue;
        };
        let kind = value.get("kind").and_then(|v| v.as_i64());
        let insert_index = value
            .get("i")
            .and_then(|v| v.as_u64())
            .and_then(|value| usize::try_from(value).ok());
        apply_vscode_state_patch(&mut state, path, patch_value.clone(), kind, insert_index);
    }

    if parsed_entries.iter().any(is_vscode_agent_event) {
        return Ok((serde_json::json!({ "events": parsed_entries }), errors));
    }

    Ok((state, errors))
}

fn is_vscode_agent_event(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(|v| v.as_str()),
        Some(
            "user.message"
                | "assistant.message"
                | "assistant.message_delta"
                | "assistant.usage"
                | "session.model_change"
        )
    )
}

fn get_vscode_patch_target_mut<'a>(state: &'a mut Value, path: &[Value]) -> Option<&'a mut Value> {
    let mut current = state;
    for segment in path {
        if let Some(key) = segment.as_str() {
            current = current.get_mut(key)?;
        } else if let Some(index) = segment.as_u64().and_then(|value| usize::try_from(value).ok()) {
            current = current.get_mut(index)?;
        } else {
            return None;
        }
    }
    Some(current)
}

fn apply_vscode_state_patch(
    state: &mut Value,
    path: &[Value],
    patch_value: Value,
    kind: Option<i64>,
    insert_index: Option<usize>,
) {
    if path.is_empty() {
        *state = patch_value;
        return;
    }

    if kind == Some(2) {
        if let Some(items) = patch_value.as_array() {
            if let Some(target) =
                get_vscode_patch_target_mut(state, path).and_then(|value| value.as_array_mut())
            {
                let index = insert_index.unwrap_or(target.len());
                if index <= target.len() {
                    target.splice(index..index, items.clone());
                    return;
                }
            }
        }
    }

    let mut current = state;
    for segment in &path[..path.len() - 1] {
        if let Some(key) = segment.as_str() {
            let Some(next) = current.get_mut(key) else {
                return;
            };
            current = next;
        } else if let Some(index) = segment.as_u64().and_then(|value| usize::try_from(value).ok()) {
            let Some(next) = current.get_mut(index) else {
                return;
            };
            current = next;
        } else {
            return;
        }
    }

    let last = &path[path.len() - 1];
    if let Some(key) = last.as_str() {
        if let Some(object) = current.as_object_mut() {
            object.insert(key.to_string(), patch_value);
        }
    } else if let Some(index) = last.as_u64().and_then(|value| usize::try_from(value).ok()) {
        if let Some(array) = current.as_array_mut() {
            if index < array.len() {
                array[index] = patch_value;
            } else if index == array.len() {
                array.push(patch_value);
            }
        }
    }
}

fn vscode_value_to_string(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if value.is_number() || value.is_boolean() {
        return Some(value.to_string());
    }
    None
}

fn extract_vscode_message_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("text").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let parts = message.get("parts").and_then(|v| v.as_array())?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() { None } else { Some(text) }
}

fn extract_vscode_response_value_text(item: &Value) -> Option<&str> {
    item.get("value")
        .or_else(|| item.get("message"))
        .or_else(|| item.get("markdownContent"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn vscode_tool_input_from_item(item: &Value, keys: &[&str]) -> Value {
    let mut input = serde_json::Map::new();
    for key in keys {
        if let Some(value) = item.get(*key) {
            input.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(input)
}

fn vscode_file_change_input(item: &Value) -> Value {
    let mut input = serde_json::Map::new();
    if let Some(uri) = item.get("uri") {
        input.insert("uri".to_string(), uri.clone());
        if let Some(path) = uri.get("path").and_then(|v| v.as_str()) {
            input.insert("file_path".to_string(), Value::String(path.to_string()));
        } else if let Some(uri_text) = uri.as_str() {
            input.insert("file_path".to_string(), Value::String(uri_text.to_string()));
        }
    }
    if let Some(edits) = item.get("edits") {
        input.insert("edits".to_string(), edits.clone());
        if let Some(count) = edits.as_array().map(|items| items.len()) {
            input.insert("edit_count".to_string(), serde_json::json!(count));
        }
    }
    if let Some(done) = item.get("done") {
        input.insert("done".to_string(), done.clone());
    }
    Value::Object(input)
}

fn vscode_response_item_to_content(item: &Value) -> Option<Value> {
    let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");

    if kind == "thinking" {
        let text = extract_vscode_response_value_text(item)?;
        return Some(serde_json::json!({
            "type": "thinking",
            "thinking": text,
        }));
    }

    if kind == "toolInvocationSerialized" {
        let tool_name = item
            .get("toolId")
            .and_then(|v| v.as_str())
            .unwrap_or("VSCodeTool");
        let tool_id = item
            .get("toolCallId")
            .and_then(|v| v.as_str())
            .unwrap_or(tool_name);
        return Some(serde_json::json!({
            "type": "tool_use",
            "id": tool_id,
            "name": tool_name,
            "input": vscode_tool_input_from_item(
                item,
                &[
                    "invocationMessage",
                    "pastTenseMessage",
                    "generatedTitle",
                    "resultDetails",
                    "toolSpecificData",
                    "source",
                    "presentation",
                    "isComplete",
                    "isConfirmed"
                ],
            ),
        }));
    }

    if matches!(kind, "textEditGroup" | "workspaceEdit" | "codeblockUri") {
        return Some(serde_json::json!({
            "type": "tool_use",
            "id": item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("vscode-file-change"),
            "name": "VSCodeFileChange",
            "input": vscode_file_change_input(item),
        }));
    }

    if matches!(kind, "progressTaskSerialized" | "progressMessage" | "questionCarousel" | "elicitationSerialized") {
        return Some(serde_json::json!({
            "type": "tool_use",
            "id": item
                .get("resolveId")
                .or_else(|| item.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("vscode-system"),
            "name": "VSCodeSystem",
            "input": item,
        }));
    }

    extract_vscode_response_value_text(item).map(|text| {
        serde_json::json!({
            "type": "text",
            "text": text,
        })
    })
}

fn extract_vscode_response_sections(response: &Value) -> (Option<String>, Vec<Value>) {
    let Some(items) = response.as_array() else {
        return (None, Vec::new());
    };
    let content = items
        .iter()
        .filter_map(vscode_response_item_to_content)
        .collect::<Vec<_>>();
    let text = content
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("text"))
        .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let text = if text.is_empty() { None } else { Some(text) };
    (text, content)
}

fn build_vscode_assistant_raw_with_model(
    content: Vec<Value>,
    model: Option<&str>,
    original: &Value,
) -> Value {
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": content,
    });
    if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
        if let Some(object) = message.as_object_mut() {
            object.insert("model".to_string(), Value::String(model.to_string()));
        }
    }
    serde_json::json!({
        "type": "message",
        "message": message,
        "vscodeRaw": original,
    })
}

fn build_vscode_turn_raw(
    request_text: Option<&str>,
    response_content: Vec<Value>,
    response_model: Option<&str>,
    original: &Value,
) -> Value {
    let request_raw = request_text.map(|text| {
        serde_json::json!({
            "type": "message",
            "message": {
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": text,
                }],
            },
            "vscodeRaw": original.get("message").unwrap_or(&Value::Null),
        })
    });
    let response_raw = if response_content.is_empty() {
        None
    } else {
        Some(build_vscode_assistant_raw_with_model(
            response_content,
            response_model,
            original.get("response").unwrap_or(&Value::Null),
        ))
    };

    serde_json::json!({
        "type": "vscode_turn",
        "vscodeTurn": {
            "request": request_raw,
            "response": response_raw,
        },
        "vscodeRaw": original,
    })
}

fn extract_vscode_request_model(request: &Value) -> Option<String> {
    [
        ["result", "metadata", "resolvedModel"].as_slice(),
        ["result", "details"].as_slice(),
        ["modelId"].as_slice(),
        ["model"].as_slice(),
    ]
    .iter()
    .find_map(|path| {
        let mut current = request;
        for segment in *path {
            current = current.get(*segment)?;
        }
        vscode_value_to_string(current).map(|value| {
            value
                .strip_prefix("copilot/")
                .unwrap_or(value.as_str())
                .to_string()
        })
    })
}

fn build_vscode_event_request_raw(text: &str, original: &Value, model: Option<&str>) -> Value {
    let mut message = serde_json::json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": text,
        }],
    });
    if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
        if let Some(object) = message.as_object_mut() {
            object.insert("model".to_string(), Value::String(model.to_string()));
        }
    }
    serde_json::json!({
        "type": "message",
        "message": message,
        "vscodeRaw": original,
    })
}

fn build_vscode_event_turn_raw(
    request_raw: Option<Value>,
    response_raw: Option<Value>,
    originals: Vec<Value>,
) -> Value {
    serde_json::json!({
        "type": "vscode_turn",
        "vscodeTurn": {
            "request": request_raw,
            "response": response_raw,
        },
        "vscodeRaw": originals,
    })
}

fn extract_vscode_agent_content_text(data: &Value) -> Option<String> {
    if let Some(text) = data.get("content").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(text) = data.get("deltaContent").and_then(|v| v.as_str()) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn vscode_agent_mode_prefix(mode: Option<&str>) -> Option<&'static str> {
    match mode {
        Some("autopilot") => Some("/autopilot"),
        Some("plan") => Some("/plan"),
        _ => None,
    }
}

fn extract_vscode_agent_user_text(event: &Value) -> Option<String> {
    let data = event.get("data").unwrap_or(event);
    let text = extract_vscode_agent_content_text(data)?;
    let mode = data.get("agentMode").and_then(|v| v.as_str());
    match vscode_agent_mode_prefix(mode) {
        Some(prefix) if !text.starts_with(prefix) => Some(format!("{prefix} {text}")),
        _ => Some(text),
    }
}

fn extract_vscode_agent_event_timestamp(event: &Value) -> Option<String> {
    event
        .get("timestamp")
        .or_else(|| event.get("data").and_then(|data| data.get("timestamp")))
        .and_then(vscode_value_to_string)
}

fn extract_vscode_agent_event_model(event: &Value) -> Option<String> {
    let data = event.get("data").unwrap_or(event);
    data.get("model")
        .or_else(|| data.get("modelId"))
        .or_else(|| event.get("model"))
        .and_then(vscode_value_to_string)
}

fn extract_vscode_agent_request_id(event: &Value) -> Option<String> {
    event
        .get("id")
        .or_else(|| event.get("requestId"))
        .or_else(|| event.get("data").and_then(|data| data.get("requestId")))
        .and_then(vscode_value_to_string)
}

fn is_vscode_child_agent_event(event: &Value) -> bool {
    event
        .get("parentToolCallId")
        .or_else(|| event.get("data").and_then(|data| data.get("parentToolCallId")))
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn vscode_agent_assistant_content(event: &Value) -> Vec<Value> {
    let data = event.get("data").unwrap_or(event);
    if let Some(items) = data.get("content").and_then(|v| v.as_array()) {
        return items
            .iter()
            .filter_map(|item| match item.get("type").and_then(|v| v.as_str()) {
                Some("text") => item
                    .get("text")
                    .and_then(|text| text.as_str())
                    .map(|text| serde_json::json!({ "type": "text", "text": text })),
                Some("thinking") => item
                    .get("thinking")
                    .and_then(|text| text.as_str())
                    .map(|text| serde_json::json!({ "type": "thinking", "thinking": text })),
                Some("tool_use") => Some(item.clone()),
                _ => None,
            })
            .collect();
    }

    extract_vscode_agent_content_text(data)
        .map(|text| vec![serde_json::json!({ "type": "text", "text": text })])
        .unwrap_or_default()
}

fn build_vscode_agent_event_timeline_events(events: &[Value]) -> Vec<TimelineEvent> {
    struct PendingTurn {
        line: usize,
        timestamp: Option<String>,
        request_id: Option<String>,
        request_text: String,
        request_raw: Value,
        response_content: Vec<Value>,
        response_model: Option<String>,
        originals: Vec<Value>,
    }

    fn finalize_pending(pending: Option<PendingTurn>, output: &mut Vec<TimelineEvent>) {
        let Some(pending) = pending else {
            return;
        };
        let response_text = pending
            .response_content
            .iter()
            .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("text"))
            .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let summary = [Some(pending.request_text.as_str()), Some(response_text.as_str())]
            .into_iter()
            .flatten()
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let request_raw = Some(pending.request_raw);
        let response_raw = if pending.response_content.is_empty() {
            None
        } else {
            Some(build_vscode_assistant_raw_with_model(
                pending.response_content,
                pending.response_model.as_deref(),
                &Value::Array(pending.originals.clone()),
            ))
        };
        output.push(TimelineEvent {
            line: pending.line,
            timestamp: pending.timestamp,
            role: Some("assistant".to_string()),
            event_type: Some("vscode_turn".to_string()),
            subtype: Some("turn".to_string()),
            uuid: None,
            parent_uuid: None,
            logical_parent_uuid: None,
            session_id: None,
            request_id: pending.request_id,
            message_id: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            is_sidechain: None,
            is_meta: None,
            summary: truncate(&summary, 240),
            raw: build_vscode_event_turn_raw(request_raw, response_raw, pending.originals),
        });
    }

    let mut output = Vec::new();
    let mut pending: Option<PendingTurn> = None;
    let mut current_model: Option<String> = None;

    for (index, event) in events.iter().enumerate() {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "session.model_change" => {
                current_model = extract_vscode_agent_event_model(event);
            }
            "user.message" => {
                finalize_pending(pending.take(), &mut output);
                let Some(request_text) = extract_vscode_agent_user_text(event) else {
                    continue;
                };
                let request_model = current_model.as_deref();
                pending = Some(PendingTurn {
                    line: index + 1,
                    timestamp: extract_vscode_agent_event_timestamp(event),
                    request_id: extract_vscode_agent_request_id(event),
                    request_raw: build_vscode_event_request_raw(&request_text, event, request_model),
                    request_text,
                    response_content: Vec::new(),
                    response_model: current_model.clone(),
                    originals: vec![event.clone()],
                });
            }
            "assistant.message" | "assistant.message_delta" => {
                if is_vscode_child_agent_event(event) {
                    continue;
                }
                let content = vscode_agent_assistant_content(event);
                if content.is_empty() {
                    continue;
                }
                if let Some(turn) = pending.as_mut() {
                    turn.response_content.extend(content);
                    turn.originals.push(event.clone());
                }
            }
            "assistant.usage" => {
                if let Some(model) = extract_vscode_agent_event_model(event) {
                    if let Some(turn) = pending.as_mut() {
                        turn.response_model = Some(model.clone());
                        turn.originals.push(event.clone());
                    }
                    current_model = Some(model);
                }
            }
            _ => {}
        }
    }

    finalize_pending(pending, &mut output);
    output
}

fn build_vscode_timeline_events(state: &Value) -> Vec<TimelineEvent> {
    let mut events = Vec::new();
    if let Some(agent_events) = state.get("events").and_then(|v| v.as_array()) {
        return build_vscode_agent_event_timeline_events(agent_events);
    }

    let Some(requests) = state.get("requests").and_then(|v| v.as_array()) else {
        return events;
    };

    for (index, request) in requests.iter().enumerate() {
        let line = index + 1;
        let timestamp = request.get("timestamp").and_then(vscode_value_to_string);
        let request_id = request.get("requestId").and_then(vscode_value_to_string);
        let response_model = extract_vscode_request_model(request);

        let request_text = request
            .get("message")
            .and_then(extract_vscode_message_text);
        let (response_text, response_content) = request
            .get("response")
            .map(extract_vscode_response_sections)
            .unwrap_or_else(|| (None, Vec::new()));
        if request_text.is_none() && response_content.is_empty() {
            continue;
        }
        let summary = [request_text.as_deref(), response_text.as_deref()]
            .into_iter()
            .flatten()
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let summary = if summary.is_empty() {
            "VS Code assistant sections".to_string()
        } else {
            summary
        };

        events.push(TimelineEvent {
            line,
            timestamp,
            role: Some("assistant".to_string()),
            event_type: Some("vscode_turn".to_string()),
            subtype: Some("turn".to_string()),
            uuid: None,
            parent_uuid: None,
            logical_parent_uuid: None,
            session_id: None,
            request_id,
            message_id: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            is_sidechain: None,
            is_meta: None,
            summary: truncate(&summary, 240),
            raw: build_vscode_turn_raw(
                request_text.as_deref(),
                response_content,
                response_model.as_deref(),
                request,
            ),
        });
    }

    events
}

fn read_vscode_copilot_session_timeline_from_path(
    path: &Path,
) -> Result<SessionTimelinePayload, String> {
    let (state, errors) = if has_jsonl_extension(path) {
        read_vscode_copilot_state_jsonl(path)?
    } else {
        read_vscode_copilot_state_json(path)?
    };

    let events = build_vscode_timeline_events(&state);
    let start_time = events.first().and_then(|event| event.timestamp.clone());
    let end_time = events.last().and_then(|event| event.timestamp.clone());

    Ok(SessionTimelinePayload {
        path: path.to_string_lossy().to_string(),
        error_code: if errors.is_empty() {
            None
        } else {
            Some(ERR_PARSE_PARTIAL.to_string())
        },
        errors,
        events,
        metadata: SessionMetadataAccumulator::default().build_metadata(start_time, end_time),
    })
}

fn validate_vscode_copilot_session_file(target: &Path) -> Result<PathBuf, String> {
    if !has_json_or_jsonl_extension(target) {
        return Err(ERR_READ_FAILED.to_string());
    }

    let root = default_vscode_workspace_storage_path()?
        .canonicalize()
        .map_err(map_read_error)?;
    let session_file = target.canonicalize().map_err(map_read_error)?;
    if !session_file.starts_with(&root) {
        return Err(ERR_READ_FAILED.to_string());
    }
    if !session_file.is_file() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    if session_file
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        != Some("chatSessions")
    {
        return Err(ERR_READ_FAILED.to_string());
    }

    Ok(session_file)
}

#[tauri::command]
pub fn read_vscode_copilot_session_timeline(
    session_path: String,
) -> Result<SessionTimelinePayload, String> {
    let target = Path::new(&session_path);
    let session_file = validate_vscode_copilot_session_file(target)?;
    read_vscode_copilot_session_timeline_from_path(&session_file)
}

fn default_codex_sessions_path() -> Result<PathBuf, String> {
    Ok(default_codex_home_path()?.join("sessions"))
}

fn strip_windows_extended_path_prefix(path: &str) -> &str {
    path.trim()
        .strip_prefix(r"\\?\")
        .or_else(|| path.trim().strip_prefix("//?/"))
        .unwrap_or(path.trim())
}

fn display_codex_cwd(cwd: &str) -> String {
    strip_windows_extended_path_prefix(cwd).trim().to_string()
}

fn is_windows_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn parse_state_db_version(path: &Path) -> Option<u32> {
    let file_name = path.file_name()?.to_str()?;
    let version = file_name
        .strip_prefix("state_")?
        .strip_suffix(".sqlite")?;
    version.parse::<u32>().ok()
}

fn find_codex_db_path_in(codex_root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(codex_root).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter_map(|path| parse_state_db_version(&path).map(|version| (version, path)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, path)| path)
}

fn find_codex_db_path() -> Result<Option<PathBuf>, String> {
    Ok(find_codex_db_path_in(&default_codex_home_path()?))
}

fn get_codex_project_discovery_mode_impl() -> CodexProjectDiscoveryMode {
    if let Ok(Some(db_path)) = find_codex_db_path() {
        if open_codex_database(&db_path).is_ok() {
            return CodexProjectDiscoveryMode {
                mode: "sqlite".to_string(),
                detail: db_path.file_name().and_then(|name| name.to_str()).map(str::to_string),
            };
        }
    }

    if let Ok(sessions_root) = default_codex_sessions_path() {
        if sessions_root.exists() {
            return CodexProjectDiscoveryMode {
                mode: "scan".to_string(),
                detail: Some("sessions".to_string()),
            };
        }
    }

    CodexProjectDiscoveryMode {
        mode: "unavailable".to_string(),
        detail: None,
    }
}

fn open_codex_database(db_path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_millis(1_000))?;
    connection.pragma_update(None, "query_only", "ON")?;
    Ok(connection)
}

fn codex_cwd_query_candidates(cwd: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    let mut push_candidate = |value: String| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return;
        }
        if !candidates.iter().any(|existing| existing == trimmed) {
            candidates.push(trimmed.to_string());
        }
    };

    let raw = cwd.trim().to_string();
    let display = display_codex_cwd(cwd);
    let backslash_display = display.replace('/', "\\");
    let slash_display = display.replace('\\', "/");

    push_candidate(raw);
    push_candidate(display.clone());
    push_candidate(backslash_display.clone());
    push_candidate(slash_display);
    if is_windows_absolute_path(&backslash_display) {
        push_candidate(format!(r"\\?\{}", backslash_display));
    }

    candidates
}

fn normalize_cwd_for_comparison(cwd: &str) -> String {
    display_codex_cwd(cwd)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

fn validate_under_codex_root(target: &Path) -> Result<PathBuf, String> {
    let codex_root = default_codex_sessions_path()?
        .canonicalize()
        .map_err(map_read_error)?;
    let canonical_target = target.canonicalize().map_err(map_read_error)?;
    if !canonical_target.starts_with(&codex_root) {
        return Err(ERR_READ_FAILED.to_string());
    }
    Ok(canonical_target)
}

fn to_optional_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|number| u64::try_from(number).ok())
}

fn build_codex_entry_label(title: Option<String>, first_user_message: Option<String>, rollout_path: &str) -> String {
    for candidate in [title, first_user_message] {
        if let Some(text) = candidate {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    file_name_or(Path::new(rollout_path), "session")
}

#[derive(Debug, Clone, Default)]
struct CodexRolloutHeadSummary {
    saw_session_meta: bool,
    preview: Option<String>,
    first_user_message: Option<String>,
}

fn codex_strip_user_message_prefix(message: &str) -> &str {
    const USER_MESSAGE_BEGIN: &str = "USER_MESSAGE_BEGIN";
    message
        .strip_prefix(USER_MESSAGE_BEGIN)
        .unwrap_or(message)
        .trim()
}

fn codex_event_msg_preview(payload: &Value) -> Option<String> {
    match payload.get("type").and_then(|v| v.as_str()) {
        Some("user_message") => {
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .map(codex_strip_user_message_prefix)
                .unwrap_or("");
            if !message.is_empty() {
                return Some(message.to_string());
            }
            let has_images = payload
                .get("images")
                .and_then(|v| v.as_array())
                .is_some_and(|arr| !arr.is_empty())
                || payload
                    .get("local_images")
                    .and_then(|v| v.as_array())
                    .is_some_and(|arr| !arr.is_empty());
            has_images.then(|| "[Image]".to_string())
        }
        Some("thread_goal_updated") => payload
            .get("goal")
            .and_then(|v| v.get("objective"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|objective| !objective.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn read_codex_rollout_head_summary(path: &Path) -> CodexRolloutHeadSummary {
    const HEAD_RECORD_LIMIT: usize = 10;
    const USER_EVENT_SCAN_LIMIT: usize = 200;

    let Ok(file) = fs::File::open(path) else {
        return CodexRolloutHeadSummary::default();
    };
    let reader = BufReader::new(file);
    let mut summary = CodexRolloutHeadSummary::default();
    let mut lines_scanned = 0usize;

    for line_result in reader.lines() {
        if lines_scanned >= HEAD_RECORD_LIMIT
            && !(summary.saw_session_meta
                && (summary.preview.is_none() || summary.first_user_message.is_none())
                && lines_scanned < HEAD_RECORD_LIMIT + USER_EVENT_SCAN_LIMIT)
        {
            break;
        }

        let Ok(line) = line_result else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines_scanned += 1;

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        match value.get("type").and_then(|v| v.as_str()) {
            Some("session_meta") => {
                summary.saw_session_meta = true;
            }
            Some("event_msg") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if let Some(preview) = codex_event_msg_preview(payload) {
                    if summary.preview.is_none() {
                        summary.preview = Some(preview.clone());
                    }
                    if payload.get("type").and_then(|v| v.as_str()) == Some("user_message")
                        && summary.first_user_message.is_none()
                    {
                        summary.first_user_message = Some(preview);
                    }
                }
            }
            _ => {}
        }
    }

    summary
}

fn codex_rollout_is_visible_in_thread_list(summary: &CodexRolloutHeadSummary) -> bool {
    summary.saw_session_meta && summary.preview.is_some()
}

fn collect_codex_session_files_from_scan(sessions_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(year_entries) = fs::read_dir(sessions_root) else { return files };
    for year_entry in year_entries.flatten() {
        let year_path = year_entry.path();
        if !year_path.is_dir() { continue; }
        let Ok(months) = fs::read_dir(&year_path) else { continue };
        for month_entry in months.flatten() {
            let month_path = month_entry.path();
            if !month_path.is_dir() { continue; }
            let Ok(days) = fs::read_dir(&month_path) else { continue };
            for day_entry in days.flatten() {
                let day_path = day_entry.path();
                if !day_path.is_dir() { continue; }
                let Ok(session_files) = fs::read_dir(&day_path) else { continue };
                for sf in session_files.flatten() {
                    let p = sf.path();
                    if p.is_file() && has_jsonl_extension(&p) {
                        files.push(p);
                    }
                }
            }
        }
    }
    files
}

fn extract_codex_session_cwd_from_scan(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line_result in reader.lines().take(20) {
        let Ok(line) = line_result else { continue };
        if line.trim().is_empty() { continue; }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
            return value
                .get("payload")
                .and_then(|p| p.get("cwd"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
    }
    None
}

fn list_codex_projects_from_scan(sessions_root: &Path) -> Vec<Project> {
    let mut cwd_map: HashMap<String, (String, Option<u64>)> = HashMap::new();
    for file in collect_codex_session_files_from_scan(sessions_root) {
        let Some(raw_cwd) = extract_codex_session_cwd_from_scan(&file) else { continue };
        let normalized_cwd = display_codex_cwd(&raw_cwd);
        let key = normalize_cwd_for_comparison(&normalized_cwd);
        let (modified_ms, _) = get_file_metadata(&file);

        let entry = cwd_map
            .entry(key)
            .or_insert_with(|| (normalized_cwd.clone(), modified_ms));
        if modified_ms > entry.1 {
            entry.1 = modified_ms;
        }
    }

    let mut projects: Vec<Project> = cwd_map
        .into_values()
        .map(|(cwd, modified_ms)| Project {
            name: project_name_from_cwd(&cwd).unwrap_or_else(|| cwd.clone()),
            path: cwd.clone(),
            cwd_path: Some(cwd),
            modified_ms,
            source: "codex".to_string(),
        })
        .collect();
    projects.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    projects
}

fn list_codex_project_entries_from_scan(cwd: &str, sessions_root: &Path) -> Vec<Entry> {
    let norm_target = normalize_cwd_for_comparison(cwd);
    let all_files = collect_codex_session_files_from_scan(sessions_root);

    let mut entries = Vec::new();
    for file in all_files {
        let Some(file_cwd) = extract_codex_session_cwd_from_scan(&file) else { continue };
        if normalize_cwd_for_comparison(&file_cwd) != norm_target {
            continue;
        }
        let summary = read_codex_rollout_head_summary(&file);
        let fallback_label = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let label = summary
            .preview
            .clone()
            .filter(|preview| !preview.trim().is_empty())
            .unwrap_or(fallback_label);
        let (modified_ms, size_bytes) = get_file_metadata(&file);
        entries.push(Entry {
            entry_type: "session".to_string(),
            label,
            path: file.to_string_lossy().to_string(),
            parent_session: None,
            modified_ms,
            size_bytes,
            source: "codex".to_string(),
            hidden: !codex_rollout_is_visible_in_thread_list(&summary),
        });
    }

    entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    entries
}

fn append_hidden_codex_project_entries_from_scan(
    entries: &mut Vec<Entry>,
    cwd: &str,
    sessions_root: &Path,
) {
    let mut seen_paths: HashSet<String> = entries.iter().map(|entry| entry.path.clone()).collect();
    let norm_target = normalize_cwd_for_comparison(cwd);
    for file in collect_codex_session_files_from_scan(sessions_root) {
        let path = file.to_string_lossy().to_string();
        if seen_paths.contains(&path) {
            continue;
        }
        let Some(file_cwd) = extract_codex_session_cwd_from_scan(&file) else {
            continue;
        };
        if normalize_cwd_for_comparison(&file_cwd) != norm_target {
            continue;
        }
        let summary = read_codex_rollout_head_summary(&file);
        if codex_rollout_is_visible_in_thread_list(&summary) {
            continue;
        }
        let label = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (modified_ms, size_bytes) = get_file_metadata(&file);
        entries.push(Entry {
            entry_type: "session".to_string(),
            label,
            path: path.clone(),
            parent_session: None,
            modified_ms,
            size_bytes,
            source: "codex".to_string(),
            hidden: true,
        });
        seen_paths.insert(path);
    }
}

fn query_codex_project_entries_by_candidates(
    connection: &Connection,
    target_norm: &str,
    candidates: &[String],
) -> rusqlite::Result<Vec<Entry>> {
    let placeholders = std::iter::repeat("?")
        .take(candidates.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT rollout_path, title, first_user_message, cwd, updated_at * 1000 AS updated_ms \
         FROM threads \
         WHERE archived = 0 \
           AND cwd IN ({placeholders}) \
           AND rollout_path IS NOT NULL \
           AND TRIM(rollout_path) != '' \
         ORDER BY updated_at DESC"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(candidates.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            to_optional_u64(row.get::<_, Option<i64>>(4)?),
        ))
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let (rollout_path, title, first_user_message, row_cwd, updated_ms) = row?;
        if normalize_cwd_for_comparison(&row_cwd) != target_norm {
            continue;
        }
        let path = Path::new(&rollout_path);
        if !path.exists() {
            continue;
        }
        let (file_modified_ms, size_bytes) = get_file_metadata(path);
        let hidden = !codex_rollout_is_visible_in_thread_list(&read_codex_rollout_head_summary(path));
        entries.push(Entry {
            entry_type: "session".to_string(),
            label: build_codex_entry_label(title, first_user_message, &rollout_path),
            path: rollout_path,
            parent_session: None,
            modified_ms: updated_ms.or(file_modified_ms),
            size_bytes,
            source: "codex".to_string(),
            hidden,
        });
    }

    Ok(entries)
}

fn query_all_codex_project_entries(connection: &Connection, target_norm: &str) -> rusqlite::Result<Vec<Entry>> {
    let mut statement = connection.prepare(
        "SELECT rollout_path, title, first_user_message, cwd, updated_at * 1000 AS updated_ms \
         FROM threads \
         WHERE archived = 0 \
           AND rollout_path IS NOT NULL \
           AND TRIM(rollout_path) != '' \
         ORDER BY updated_at DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            to_optional_u64(row.get::<_, Option<i64>>(4)?),
        ))
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let (rollout_path, title, first_user_message, row_cwd, updated_ms) = row?;
        if normalize_cwd_for_comparison(&row_cwd) != target_norm {
            continue;
        }
        let path = Path::new(&rollout_path);
        if !path.exists() {
            continue;
        }
        let (file_modified_ms, size_bytes) = get_file_metadata(path);
        let hidden = !codex_rollout_is_visible_in_thread_list(&read_codex_rollout_head_summary(path));
        entries.push(Entry {
            entry_type: "session".to_string(),
            label: build_codex_entry_label(title, first_user_message, &rollout_path),
            path: rollout_path,
            parent_session: None,
            modified_ms: updated_ms.or(file_modified_ms),
            size_bytes,
            source: "codex".to_string(),
            hidden,
        });
    }

    Ok(entries)
}

fn list_codex_project_entries_from_db(cwd: &str, db_path: &Path) -> rusqlite::Result<Vec<Entry>> {
    let target_norm = normalize_cwd_for_comparison(cwd);
    if target_norm.is_empty() {
        return Ok(vec![]);
    }

    let connection = open_codex_database(db_path)?;
    let candidates = codex_cwd_query_candidates(cwd);
    let mut entries = query_codex_project_entries_by_candidates(&connection, &target_norm, &candidates)?;
    if entries.is_empty() {
        entries = query_all_codex_project_entries(&connection, &target_norm)?;
    }
    entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(entries)
}

fn list_codex_projects_from_db(db_path: &Path) -> rusqlite::Result<Vec<Project>> {
    let connection = open_codex_database(db_path)?;
    let mut statement = connection.prepare(
        "SELECT cwd, rollout_path, updated_at * 1000 AS modified_ms \
         FROM threads \
         WHERE archived = 0 \
           AND cwd IS NOT NULL \
           AND TRIM(cwd) != '' \
           AND rollout_path IS NOT NULL \
           AND TRIM(rollout_path) != '' \
         ORDER BY modified_ms DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            to_optional_u64(row.get::<_, Option<i64>>(2)?),
        ))
    })?;

    let mut deduped: HashMap<String, (String, Option<u64>)> = HashMap::new();
    for row in rows {
        let (raw_cwd, rollout_path, modified_ms) = row?;
        if !Path::new(&rollout_path).exists() {
            continue;
        }
        let cwd = display_codex_cwd(&raw_cwd);
        let key = normalize_cwd_for_comparison(&cwd);
        if key.is_empty() {
            continue;
        }

        let entry = deduped
            .entry(key)
            .or_insert_with(|| (cwd.clone(), modified_ms));
        if modified_ms > entry.1 {
            entry.0 = cwd;
            entry.1 = modified_ms;
        }
    }

    let mut projects: Vec<Project> = deduped
        .into_values()
        .map(|(cwd, modified_ms)| Project {
            name: project_name_from_cwd(&cwd).unwrap_or_else(|| cwd.clone()),
            path: cwd.clone(),
            cwd_path: Some(cwd),
            modified_ms,
            source: "codex".to_string(),
        })
        .collect();
    projects.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(projects)
}

#[tauri::command]
pub fn list_codex_project_entries(cwd: String) -> Result<Vec<Entry>, String> {
    let sessions_root = default_codex_sessions_path().ok();
    if let Ok(Some(db_path)) = find_codex_db_path() {
        if let Ok(mut entries) = list_codex_project_entries_from_db(&cwd, &db_path) {
            if let Some(sessions_root) = sessions_root.as_ref().filter(|path| path.exists()) {
                append_hidden_codex_project_entries_from_scan(&mut entries, &cwd, sessions_root);
                entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
            }
            return Ok(entries);
        }
    }

    let Some(sessions_root) = sessions_root else {
        return Ok(vec![]);
    };
    if !sessions_root.exists() {
        return Ok(vec![]);
    }
    Ok(list_codex_project_entries_from_scan(&cwd, &sessions_root))
}

#[tauri::command]
pub fn get_codex_project_discovery_mode() -> Result<CodexProjectDiscoveryMode, String> {
    Ok(get_codex_project_discovery_mode_impl())
}

#[tauri::command]
pub fn list_codex_projects() -> Result<Vec<Project>, String> {
    if let Ok(Some(db_path)) = find_codex_db_path() {
        if let Ok(projects) = list_codex_projects_from_db(&db_path) {
            return Ok(projects);
        }
    }

    let sessions_root = match default_codex_sessions_path() {
        Ok(path) => path,
        Err(_) => return Ok(vec![]),
    };
    if !sessions_root.exists() {
        return Ok(vec![]);
    }
    Ok(list_codex_projects_from_scan(&sessions_root))
}

fn accumulate_codex_metadata(accum: &mut SessionMetadataAccumulator, raw: &Value) {
    let outer_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let payload = raw.get("payload");

    if outer_type == "turn_context" {
        if accum.model_name.is_none() {
            accum.model_name = payload
                .and_then(|p| p.get("model"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
    }

    if outer_type == "event_msg" {
        let payload_type = payload.and_then(|p| p.get("type")).and_then(|v| v.as_str());
        if payload_type == Some("token_count") {
            if let Some(info) = payload.and_then(|p| p.get("info")) {
                if let Some(total) = info.get("total_token_usage") {
                    accum.total_input_tokens =
                        total.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    accum.total_output_tokens =
                        total.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    accum.total_cache_read_input_tokens =
                        total.get("cached_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                } else if let Some(last) = info.get("last_token_usage") {
                    accum.total_input_tokens += last.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    accum.total_output_tokens += last.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    accum.total_cache_read_input_tokens += last.get("cached_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                }
            }
        }
    }
}

fn codex_event_msg_text(payload: &Value) -> String {
    let message = payload.get("message").and_then(|v| v.as_str()).unwrap_or("");
    if !message.trim().is_empty() {
        return message.to_string();
    }
    let has_images = payload
        .get("images")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| !arr.is_empty())
        || payload
            .get("local_images")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| !arr.is_empty());
    if has_images {
        "[Image]".to_string()
    } else {
        String::new()
    }
}

fn codex_reasoning_event_text(payload: &Value) -> String {
    payload
        .get("text")
        .or_else(|| payload.get("content"))
        .or_else(|| payload.get("delta"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn codex_response_message_text(payload: &Value) -> String {
    payload
        .get("content")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let t = item.get("type").and_then(|v| v.as_str());
                    if matches!(t, Some("output_text") | Some("input_text") | Some("text")) {
                        item.get("text").and_then(|v| v.as_str())
                    } else {
                        None
                    }
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn codex_structured_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(codex_structured_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                return text.to_string();
            }
            if let Some(content) = map.get("content") {
                let text = codex_structured_text(content);
                if !text.is_empty() {
                    return text;
                }
            }
            if let Some(content_items) = map.get("content_items") {
                let text = codex_structured_text(content_items);
                if !text.is_empty() {
                    return text;
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn codex_tool_output_text(payload: &Value) -> String {
    payload
        .get("output")
        .map(codex_structured_text)
        .unwrap_or_default()
}

fn build_codex_timeline_event(line: usize, raw: Value) -> TimelineEvent {
    let timestamp = raw.get("timestamp").and_then(|v| v.as_str()).map(str::to_string);
    let outer_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
    let payload = raw.get("payload").cloned().unwrap_or(Value::Null);
    let payload_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");

    let mut role: Option<String> = None;
    let event_type: Option<String> = Some(outer_type.to_string());
    let subtype: Option<String> = if payload_type.is_empty() { None } else { Some(payload_type.to_string()) };
    let mut tool_use_id: Option<String> = None;
    let mut operation: Option<String> = None;
    let summary: String;

    match (outer_type, payload_type) {
        ("event_msg", "user_message") => {
            role = Some("user".to_string());
            summary = truncate(&codex_event_msg_text(&payload), 240);
        }
        ("event_msg", "agent_message") => {
            role = Some("assistant".to_string());
            summary = truncate(&codex_event_msg_text(&payload), 240);
        }
        ("event_msg", "agent_reasoning") => {
            role = Some("assistant".to_string());
            let text = codex_reasoning_event_text(&payload);
            summary = truncate(&text, 240);
        }
        ("event_msg", "agent_reasoning_raw_content") => {
            role = Some("assistant".to_string());
            let text = codex_reasoning_event_text(&payload);
            summary = truncate(&text, 240);
        }
        ("response_item", "function_call") => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = payload.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            operation = Some(name.to_string());
            summary = truncate(&format!("{name} {args}"), 240);
        }
        ("response_item", "function_call_output") => {
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            summary = truncate(&codex_tool_output_text(&payload), 240);
        }
        ("response_item", "custom_tool_call") => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let input = payload.get("input").and_then(|v| v.as_str()).unwrap_or("");
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            operation = Some(name.to_string());
            summary = truncate(&format!("{name}: {input}"), 240);
        }
        ("response_item", "custom_tool_call_output") => {
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            summary = truncate(&codex_tool_output_text(&payload), 240);
        }
        ("response_item", "local_shell_call") => {
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            operation = Some("local_shell_call".to_string());
            let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let command = payload
                .get("action")
                .and_then(|v| v.get("command").or_else(|| v.get("cmd")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            summary = truncate(&format!("{status} {command}").trim(), 240);
        }
        ("response_item", "reasoning") => {
            role = Some("assistant".to_string());
            let text = payload
                .get("summary")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            summary = truncate(text, 240);
        }
        ("response_item", "message") => {
            role = payload.get("role").and_then(|v| v.as_str()).map(str::to_string);
            let text = codex_response_message_text(&payload);
            summary = truncate(&text, 240);
        }
        ("session_meta", _) => {
            let cwd = payload.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            let ver = payload.get("cli_version").and_then(|v| v.as_str()).unwrap_or("");
            summary = truncate(&format!("cwd:{cwd} v{ver}"), 240);
        }
        ("turn_context", _) => {
            let model = payload.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let cwd = payload.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            summary = truncate(&format!("model:{model} cwd:{cwd}"), 240);
        }
        _ => {
            summary = truncate(&payload.to_string(), 240);
        }
    }

    let session_id = raw
        .get("payload")
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    TimelineEvent {
        line,
        timestamp,
        role,
        event_type,
        subtype,
        uuid: None,
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id,
        request_id: None,
        message_id: None,
        tool_use_id,
        parent_tool_use_id: None,
        operation,
        is_sidechain: None,
        is_meta: None,
        summary,
        raw,
    }
}

fn is_codex_legacy_ghost_snapshot_rollout_line(raw: &Value) -> bool {
    raw.get("type").and_then(|v| v.as_str()) == Some("response_item")
        && raw
            .get("payload")
            .and_then(|p| p.get("type"))
            .and_then(|v| v.as_str())
            == Some("ghost_snapshot")
}

#[tauri::command]
pub fn read_codex_session_timeline(session_path: String) -> Result<SessionTimelinePayload, String> {
    let target = Path::new(&session_path);
    let session_file = validate_under_codex_root(target)?;

    if !session_file.is_file() {
        return Err(ERR_NOT_FOUND.to_string());
    }
    if !has_jsonl_extension(&session_file) {
        return Err(ERR_READ_FAILED.to_string());
    }

    let content = fs::read_to_string(&session_file).map_err(map_read_error)?;
    let mut events = Vec::new();
    let mut errors = Vec::new();
    let mut metadata_accumulator = SessionMetadataAccumulator::default();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() { continue; }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                if is_codex_legacy_ghost_snapshot_rollout_line(&value) {
                    continue;
                }
                accumulate_codex_metadata(&mut metadata_accumulator, &value);
                events.push(build_codex_timeline_event(line_number, value));
            }
            Err(_) => errors.push(ParseError {
                line: line_number,
                message: "invalid json".to_string(),
            }),
        }
    }

    sort_events_by_time(&mut events);
    let start_time = events.first().and_then(|e| e.timestamp.clone());
    let end_time = events.last().and_then(|e| e.timestamp.clone());

    Ok(SessionTimelinePayload {
        path: session_file.to_string_lossy().to_string(),
        error_code: if errors.is_empty() { None } else { Some(ERR_PARSE_PARTIAL.to_string()) },
        errors,
        events,
        metadata: metadata_accumulator.build_metadata(start_time, end_time),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("claude-projects-browser-{name}-{ts}"));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, content).expect("write file");
    }

    fn clear_project_cwd_cache() {
        let cache = PROJECT_CWD_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        cache.lock().expect("lock cache").clear();
    }

    #[test]
    fn test_decode_vscode_workspace_folder_uri_windows_path() {
        let decoded = decode_vscode_workspace_folder_uri("file:///d%3A/Hank/Dropbox/AI-Project/Unified-AI-Session-Explorer")
            .expect("decode workspace URI");
        assert_eq!(decoded, r"D:\Hank\Dropbox\AI-Project\Unified-AI-Session-Explorer");
    }

    #[test]
    fn test_list_vscode_projects_from_workspace_storage() {
        let root = unique_temp_dir("vscode-workspace-storage");
        let workspace = root.join("812a28887692c48029817b9ae7c9cddf");
        let chat = workspace.join("chatSessions");
        fs::create_dir_all(&chat).expect("create chat sessions dir");
        write_file(
            &workspace.join("workspace.json"),
            r#"{"folder":"file:///d%3A/Hank/Dropbox/AI-Project/Unified-AI-Session-Explorer"}"#,
        );
        write_file(
            &chat.join("session-one.json"),
            r#"{"version":3,"sessionId":"session-one","creationDate":1710000000000,"requests":[]}"#,
        );

        let projects = list_vscode_copilot_projects_from_root(&root).expect("list vscode projects");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].source, "vscode");
        assert_eq!(projects[0].name, "Unified-AI-Session-Explorer");
        assert_eq!(
            projects[0].cwd_path.as_deref(),
            Some(r"D:\Hank\Dropbox\AI-Project\Unified-AI-Session-Explorer")
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_list_vscode_project_entries_from_root() {
        let root = unique_temp_dir("vscode-project-entries");
        let workspace = root.join("812a28887692c48029817b9ae7c9cddf");
        let chat = workspace.join("chatSessions");
        fs::create_dir_all(&chat).expect("create chat sessions dir");
        write_file(
            &workspace.join("workspace.json"),
            r#"{"folder":"file:///d%3A/project/demo"}"#,
        );
        write_file(&chat.join("empty.json"), r#"{"version":3,"requests":[]}"#);
        write_file(
            &chat.join("visible.json"),
            r#"{"requests":[{"message":{"text":"hello"},"response":[{"value":"hi"}]}]}"#,
        );

        let entries = list_vscode_copilot_project_entries_from_root(r"D:\project\demo", &root)
            .expect("list vscode project entries");
        let labels: Vec<&str> = entries.iter().map(|entry| entry.label.as_str()).collect();

        assert_eq!(entries.len(), 1);
        assert!(entries.iter().all(|entry| entry.entry_type == "session"));
        assert!(entries.iter().all(|entry| entry.source == "vscode"));
        assert!(labels.contains(&"visible.json"));
        assert!(!labels.contains(&"empty.json"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_json_session_timeline() {
        let root = unique_temp_dir("vscode-json-timeline");
        let session = root.join("session.json");
        write_file(
            &session,
            r#"{"requests":[{"requestId":"req-1","timestamp":1710000000000,"modelId":"copilot/claude-sonnet-4.6","result":{"metadata":{"resolvedModel":"claude-sonnet-4-6"}},"message":{"text":"Hello Copilot"},"response":[{"value":"Hello from VS Code"}]}]}"#,
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");

        assert_eq!(payload.events.len(), 1);
        assert_eq!(payload.events[0].role.as_deref(), Some("assistant"));
        assert_eq!(payload.events[0].event_type.as_deref(), Some("vscode_turn"));
        assert_eq!(payload.events[0].subtype.as_deref(), Some("turn"));
        assert_eq!(payload.events[0].summary, "Hello Copilot\nHello from VS Code");
        assert_eq!(
            payload.events[0]
                .raw
                .get("vscodeTurn")
                .and_then(|turn| turn.get("request"))
                .and_then(|request| request.get("message"))
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(|text| text.as_str()),
            Some("Hello Copilot")
        );
        assert_eq!(
            payload.events[0]
                .raw
                .get("vscodeTurn")
                .and_then(|turn| turn.get("response"))
                .and_then(|response| response.get("message"))
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(|text| text.as_str()),
            Some("Hello from VS Code")
        );
        assert_eq!(
            payload.events[0]
                .raw
                .get("vscodeTurn")
                .and_then(|turn| turn.get("response"))
                .and_then(|response| response.get("message"))
                .and_then(|message| message.get("model"))
                .and_then(|model| model.as_str()),
            Some("claude-sonnet-4-6")
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_jsonl_session_replays_state() {
        let root = unique_temp_dir("vscode-jsonl-timeline");
        let session = root.join("session.jsonl");
        write_file(
            &session,
            "{\"kind\":0,\"v\":{\"requests\":[]}}\n{\"kind\":2,\"k\":[\"requests\"],\"v\":[{\"message\":{\"text\":\"Question\"},\"response\":[{\"value\":\"Answer\"}]}]}\n",
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");

        assert_eq!(payload.events.len(), 1);
        assert_eq!(payload.events[0].summary, "Question\nAnswer");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_jsonl_session_splices_requests() {
        let root = unique_temp_dir("vscode-jsonl-request-splice");
        let session = root.join("session.jsonl");
        write_file(
            &session,
            "{\"kind\":0,\"v\":{\"requests\":[]}}\n{\"kind\":2,\"k\":[\"requests\"],\"v\":[{\"message\":{\"text\":\"First question\"},\"response\":[{\"value\":\"First answer\"}]}]}\n{\"kind\":2,\"k\":[\"requests\"],\"v\":[{\"message\":{\"text\":\"Second question\"},\"response\":[{\"value\":\"Second answer\"}]}]}\n",
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");
        let summaries: Vec<&str> = payload
            .events
            .iter()
            .map(|event| event.summary.as_str())
            .collect();

        assert_eq!(summaries, vec!["First question\nFirst answer", "Second question\nSecond answer"]);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_jsonl_session_splices_response_chunks_at_index() {
        let root = unique_temp_dir("vscode-jsonl-response-splice");
        let session = root.join("session.jsonl");
        write_file(
            &session,
            "{\"kind\":0,\"v\":{\"requests\":[]}}\n{\"kind\":2,\"k\":[\"requests\"],\"v\":[{\"message\":{\"text\":\"Question\"},\"response\":[{\"value\":\"A\"},{\"value\":\"D\"}]}]}\n{\"kind\":2,\"k\":[\"requests\",0,\"response\"],\"i\":1,\"v\":[{\"value\":\"B\"},{\"value\":\"C\"}]}\n",
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");

        assert_eq!(payload.events.len(), 1);
        assert_eq!(payload.events[0].summary, "Question\nA\nB\nC\nD");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_session_splits_assistant_sections() {
        let root = unique_temp_dir("vscode-section-split");
        let session = root.join("session.json");
        write_file(
            &session,
            r#"{"requests":[{"requestId":"req-1","timestamp":1710000000000,"message":{"text":"Explain"},"response":[{"value":"Visible answer"},{"kind":"thinking","value":"Hidden reasoning"},{"kind":"toolInvocationSerialized","toolCallId":"tool-1","toolId":"copilot_readFile","invocationMessage":"Reading file","pastTenseMessage":"Read file","isComplete":true},{"kind":"textEditGroup","uri":{"path":"/tmp/app.js"},"edits":[{"newText":"console.log(1);"}],"done":true}]}]}"#,
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");
        let assistant = payload.events.first().expect("assistant turn");
        let content = assistant
            .raw
            .get("vscodeTurn")
            .and_then(|turn| turn.get("response"))
            .and_then(|response| response.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_array())
            .expect("normalized content");

        assert_eq!(assistant.summary, "Explain\nVisible answer");
        assert_eq!(content[0].get("type").and_then(|v| v.as_str()), Some("text"));
        assert_eq!(content[0].get("text").and_then(|v| v.as_str()), Some("Visible answer"));
        assert_eq!(content[1].get("type").and_then(|v| v.as_str()), Some("thinking"));
        assert_eq!(
            content[1].get("thinking").and_then(|v| v.as_str()),
            Some("Hidden reasoning")
        );
        assert_eq!(content[2].get("type").and_then(|v| v.as_str()), Some("tool_use"));
        assert_eq!(content[2].get("id").and_then(|v| v.as_str()), Some("tool-1"));
        assert_eq!(content[2].get("name").and_then(|v| v.as_str()), Some("copilot_readFile"));
        assert_eq!(content[3].get("type").and_then(|v| v.as_str()), Some("tool_use"));
        assert_eq!(content[3].get("name").and_then(|v| v.as_str()), Some("VSCodeFileChange"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_session_preserves_request_order_over_timestamps() {
        let root = unique_temp_dir("vscode-request-order");
        let session = root.join("session.json");
        write_file(
            &session,
            r#"{"requests":[{"requestId":"req-1","timestamp":2000,"message":{"text":"Q1"},"response":[{"value":"A1"}]},{"requestId":"req-2","timestamp":1000,"message":{"text":"Q2"},"response":[{"value":"A2"}]}]}"#,
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");
        let summaries: Vec<&str> = payload
            .events
            .iter()
            .map(|event| event.summary.as_str())
            .collect();

        assert_eq!(summaries, vec!["Q1\nA1", "Q2\nA2"]);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_jsonl_session_appends_array_index_patch() {
        let root = unique_temp_dir("vscode-jsonl-append-patch");
        let session = root.join("session.jsonl");
        write_file(
            &session,
            "{\"kind\":0,\"v\":{\"requests\":[]}}\n{\"kind\":2,\"k\":[\"requests\",0],\"v\":{\"message\":{\"text\":\"Append question\"},\"response\":[{\"value\":\"Append answer\"}]}}\n",
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode timeline");

        assert_eq!(payload.events.len(), 1);
        assert_eq!(payload.events[0].summary, "Append question\nAppend answer");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn test_read_vscode_jsonl_agent_events_builds_turns() {
        let root = unique_temp_dir("vscode-jsonl-agent-events");
        let session = root.join("session.jsonl");
        write_file(
            &session,
            concat!(
                "{\"type\":\"session.model_change\",\"data\":{\"model\":\"gpt-5.4\"}}\n",
                "{\"type\":\"user.message\",\"id\":\"sdk-req-1\",\"timestamp\":1779194731541,\"data\":{\"content\":\"Fix the bug\",\"agentMode\":\"plan\"}}\n",
                "{\"type\":\"assistant.message\",\"id\":\"a1\",\"data\":{\"messageId\":\"msg-1\",\"content\":\"Top-level reply\"}}\n",
                "{\"type\":\"assistant.message_delta\",\"id\":\"a2\",\"data\":{\"messageId\":\"msg-2\",\"deltaContent\":\"sub-agent thinking\",\"parentToolCallId\":\"task-1\"}}\n",
                "{\"type\":\"assistant.message\",\"id\":\"a3\",\"data\":{\"messageId\":\"msg-3\",\"content\":\"sub-agent result\",\"parentToolCallId\":\"task-1\"}}\n",
                "{\"type\":\"assistant.message\",\"id\":\"a4\",\"data\":{\"messageId\":\"msg-4\",\"content\":\"Final answer\"}}\n",
                "{\"type\":\"assistant.usage\",\"data\":{\"model\":\"gpt-5.4\",\"inputTokens\":10,\"outputTokens\":5}}\n",
                "{\"type\":\"user.message\",\"id\":\"sdk-req-2\",\"data\":{\"content\":\"Next question\",\"agentMode\":\"autopilot\"}}\n",
                "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Next answer\"}}\n",
            ),
        );

        let payload = read_vscode_copilot_session_timeline_from_path(&session)
            .expect("read vscode agent event timeline");
        let summaries: Vec<&str> = payload
            .events
            .iter()
            .map(|event| event.summary.as_str())
            .collect();

        assert_eq!(
            summaries,
            vec![
                "/plan Fix the bug\nTop-level reply\nFinal answer",
                "/autopilot Next question\nNext answer",
            ]
        );
        assert_eq!(payload.events[0].timestamp.as_deref(), Some("1779194731541"));
        assert_eq!(payload.events[0].request_id.as_deref(), Some("sdk-req-1"));

        let first_turn = payload.events[0].raw.get("vscodeTurn").expect("first turn");
        assert_eq!(
            first_turn
                .get("request")
                .and_then(|request| request.get("message"))
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(|text| text.as_str()),
            Some("/plan Fix the bug")
        );
        assert_eq!(
            first_turn
                .get("response")
                .and_then(|response| response.get("message"))
                .and_then(|message| message.get("model"))
                .and_then(|model| model.as_str()),
            Some("gpt-5.4")
        );
        let response_text = first_turn
            .get("response")
            .and_then(|response| response.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_array())
            .expect("response content")
            .iter()
            .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(response_text, "Top-level reply\nFinal answer");
        assert!(!response_text.contains("sub-agent"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn list_entries_memory_first_and_include_subagents() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("entries");
        let project = root.join("demo");
        fs::create_dir_all(&project).expect("create project");
        write_file(&project.join("memory").join("MEMORY.md"), "# memory");
        write_file(
            &project.join("alpha.jsonl"),
            "{\"timestamp\":\"2026-03-01T00:00:00Z\",\"content\":\"hello\"}",
        );
        write_file(&project.join("alpha").join("subagents").join("s1.jsonl"), "{\"content\":\"sub\"}");

        let entries = list_project_entries(
            project.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
        )
        .expect("list entries");

        assert!(!entries.is_empty());
        assert_eq!(entries[0].entry_type, "memory_file");
        assert!(entries.iter().any(|v| v.entry_type == "session"));
        assert!(entries.iter().any(|v| v.entry_type == "subagent_session"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn parse_session_keeps_valid_events_and_reports_partial_error() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("timeline");
        let session = root.join("mixed.jsonl");
        write_file(
            &session,
            "{\"timestamp\":\"2026-03-01T00:00:00Z\",\"content\":\"ok1\"}\n{broken}\n{\"timestamp\":\"2026-03-02T00:00:00Z\",\"content\":\"ok2\"}\n",
        );

        let payload = read_session_timeline(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
            None,
        )
        .expect("parse session");

        assert_eq!(payload.error_code.as_deref(), Some(ERR_PARSE_PARTIAL));
        assert_eq!(payload.errors.len(), 1);
        assert_eq!(payload.events.len(), 2);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn strict_mode_preserves_original_line_order() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("strict-order");
        let session = root.join("order.jsonl");
        write_file(
            &session,
            "{\"timestamp\":\"2026-03-02T00:00:00Z\",\"content\":\"first\"}\n{\"timestamp\":\"2026-03-01T00:00:00Z\",\"content\":\"second\"}\n",
        );

        let strict_payload = read_session_timeline(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
            Some(true),
        )
        .expect("parse strict session");

        let legacy_payload = read_session_timeline(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
            Some(false),
        )
        .expect("parse legacy session");

        assert_eq!(strict_payload.events[0].line, 1);
        assert_eq!(legacy_payload.events[0].line, 2);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn parse_session_extracts_model_and_usage_details() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("metadata");
        let session = root.join("meta.jsonl");
        write_file(
            &session,
            "{\"type\":\"assistant\",\"sessionId\":\"sid-1\",\"timestamp\":\"2026-03-01T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":10,\"output_tokens\":20,\"cache_creation_input_tokens\":30,\"cache_read_input_tokens\":40,\"server_tool_use\":{\"web_search_requests\":2,\"web_fetch_requests\":3},\"service_tier\":\"standard\",\"speed\":\"standard\",\"inference_geo\":\"not_available\"}}}\n",
        );

        let payload = read_session_timeline(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
            Some(true),
        )
        .expect("parse session metadata");

        assert_eq!(payload.metadata.model_name.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(payload.metadata.total_input_tokens, 10);
        assert_eq!(payload.metadata.total_output_tokens, 20);
        assert_eq!(payload.metadata.total_cache_creation_input_tokens, 30);
        assert_eq!(payload.metadata.total_cache_read_input_tokens, 40);
        assert_eq!(payload.metadata.total_web_search_requests, 2);
        assert_eq!(payload.metadata.total_web_fetch_requests, 3);
        assert_eq!(payload.metadata.service_tier.as_deref(), Some("standard"));
        assert_eq!(payload.metadata.speed.as_deref(), Some("standard"));
        assert_eq!(payload.metadata.inference_geo.as_deref(), Some("not_available"));
        assert_eq!(payload.events.len(), 1);
        assert_eq!(payload.events[0].role.as_deref(), Some("assistant"));
        assert_eq!(payload.events[0].session_id.as_deref(), Some("sid-1"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn list_projects_prefers_cwd_name_when_available() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("projects-cwd");
        let encoded = root.join("d-Hank-Dropbox-Claude-History");
        fs::create_dir_all(&encoded).expect("create encoded project");
        write_file(
            &encoded.join("a.jsonl"),
            "{\"cwd\":\"D:\\\\Hank\\\\Dropbox\\\\Claude-History\\\\actual-project\"}\n",
        );

        let projects = list_projects(Some(root.to_string_lossy().to_string())).expect("list projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "actual-project");
        assert_eq!(
            projects[0].cwd_path.as_deref(),
            Some("D:\\Hank\\Dropbox\\Claude-History\\actual-project")
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn delete_session_removes_jsonl_and_subagent_folder() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("delete-session");
        let project = root.join("demo");
        fs::create_dir_all(&project).expect("create project");
        let session = project.join("main.jsonl");
        write_file(&session, "{\"content\":\"hello\"}\n");
        let subagent = project.join("main").join("subagents").join("child.jsonl");
        write_file(&subagent, "{\"content\":\"child\"}\n");

        delete_session(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
        )
        .expect("delete session");

        assert!(!session.exists());
        assert!(!project.join("main").exists());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn delete_project_removes_directory_tree() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("delete-project");
        let project = root.join("demo");
        fs::create_dir_all(&project).expect("create project");
        write_file(&project.join("nested").join("a.jsonl"), "{\"content\":\"x\"}");

        delete_project(
            Some(project.to_string_lossy().to_string()),
            None,
            Some(root.to_string_lossy().to_string()),
        )
        .expect("delete project");

        assert!(!project.exists());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn get_project_delete_impact_counts_entries_and_size() {
        clear_project_cwd_cache();
        let root = unique_temp_dir("delete-impact");
        let project = root.join("demo");
        fs::create_dir_all(&project).expect("create project");
        write_file(&project.join("memory").join("MEMORY.md"), "# memory");
        write_file(&project.join("a.jsonl"), "{\"content\":\"root session\"}");
        write_file(
            &project.join("a").join("subagents").join("s1.jsonl"),
            "{\"content\":\"child\"}",
        );

        let impact = get_project_delete_impact(
            Some(project.to_string_lossy().to_string()),
            None,
            Some(root.to_string_lossy().to_string()),
        )
        .expect("get impact");

        assert_eq!(impact.session_count, 1);
        assert_eq!(impact.subagent_session_count, 1);
        assert_eq!(impact.memory_file_count, 1);
        assert_eq!(impact.total_file_count, 3);
        assert!(impact.total_size_bytes > 0);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn delete_project_removes_matching_codex_sessions() {
        let sessions_root = unique_temp_dir("delete-project-codex-sessions");
        let day_dir = sessions_root.join("2026").join("03").join("12");
        fs::create_dir_all(&day_dir).expect("create day dir");
        let matching = day_dir.join("match.jsonl");
        let other = day_dir.join("other.jsonl");
        write_file(
            &matching,
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
        );
        write_file(
            &other,
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\other\"}}\n",
        );

        delete_codex_project_sessions_in(r"D:\repo\demo", &sessions_root)
            .expect("delete codex project sessions");

        assert!(!matching.exists());
        assert!(other.exists());

        fs::remove_dir_all(sessions_root).expect("cleanup");
    }

    #[test]
    fn list_codex_project_entries_marks_rollouts_without_thread_preview_hidden() {
        let sessions_root = unique_temp_dir("codex-hidden-rollouts");
        let day_dir = sessions_root.join("2026").join("03").join("12");
        fs::create_dir_all(&day_dir).expect("create day dir");
        write_file(
            &day_dir.join("visible.jsonl"),
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"visible prompt\"}}\n"
            ),
        );
        write_file(
            &day_dir.join("no-preview.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
        );
        write_file(
            &day_dir.join("no-meta.jsonl"),
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"missing meta\"}}\n",
        );

        let entries = list_codex_project_entries_from_scan(r"D:\repo\demo", &sessions_root);
        let visible = entries
            .iter()
            .find(|entry| entry.label == "visible prompt")
            .expect("visible entry");
        let hidden = entries
            .iter()
            .find(|entry| entry.label == "no-preview.jsonl")
            .expect("hidden entry");

        assert_eq!(visible.hidden, false);
        assert_eq!(hidden.hidden, true);
        assert!(entries.iter().all(|entry| entry.label != "no-meta.jsonl"));

        fs::remove_dir_all(sessions_root).expect("cleanup");
    }

    #[test]
    fn get_project_delete_impact_combines_claude_and_codex_entries() {
        clear_project_cwd_cache();
        let claude_root = unique_temp_dir("delete-impact-claude");
        let project = claude_root.join("demo");
        fs::create_dir_all(&project).expect("create project");
        write_file(&project.join("memory").join("MEMORY.md"), "# memory");
        write_file(&project.join("a.jsonl"), "{\"content\":\"root session\"}");
        write_file(
            &project.join("a").join("subagents").join("s1.jsonl"),
            "{\"content\":\"child\"}",
        );

        let codex_root = unique_temp_dir("delete-impact-codex-sessions");
        let day_dir = codex_root.join("2026").join("03").join("12");
        fs::create_dir_all(&day_dir).expect("create codex day dir");
        write_file(
            &day_dir.join("codex-a.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
        );
        write_file(
            &day_dir.join("codex-b.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
        );

        let mut impact = get_project_delete_impact(
            Some(project.to_string_lossy().to_string()),
            None,
            Some(claude_root.to_string_lossy().to_string()),
        )
        .expect("get claude impact");
        let codex_entries = collect_codex_project_entries_for_delete_impact(r"D:\repo\demo", &codex_root)
            .expect("collect codex impact entries");
        accumulate_project_delete_impact(&mut impact, &codex_entries);

        assert_eq!(impact.session_count, 3);
        assert_eq!(impact.subagent_session_count, 1);
        assert_eq!(impact.memory_file_count, 1);
        assert_eq!(impact.total_file_count, 5);
        assert!(impact.total_size_bytes > 0);

        fs::remove_dir_all(claude_root).expect("cleanup claude root");
        fs::remove_dir_all(codex_root).expect("cleanup codex root");
    }

    #[test]
    fn delete_codex_session_removes_file() {
        let root = unique_temp_dir("delete-codex-session");
        let day_dir = root.join("2026").join("03");
        fs::create_dir_all(&day_dir).expect("create day dir");
        let session = day_dir.join("session.jsonl");
        write_file(&session, "{\"type\":\"session_meta\"}\n");

        delete_codex_session(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
        )
        .expect("delete codex session");

        assert!(!session.exists());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn delete_codex_session_rejects_path_outside_root() {
        let root = unique_temp_dir("delete-codex-outside");
        let other = unique_temp_dir("delete-codex-other");
        let session = other.join("evil.jsonl");
        write_file(&session, "{}\n");

        let result = delete_codex_session(
            session.to_string_lossy().to_string(),
            Some(root.to_string_lossy().to_string()),
        );
        assert!(result.is_err());

        fs::remove_dir_all(root).expect("cleanup root");
        fs::remove_dir_all(other).expect("cleanup other");
    }

    #[test]
    fn test_normalize_cwd_for_comparison() {
        assert_eq!(normalize_cwd_for_comparison("D:\\project\\foo\\"), "d:/project/foo");
        assert_eq!(normalize_cwd_for_comparison("/home/user/project/"), "/home/user/project");
        assert_eq!(normalize_cwd_for_comparison("D:\\project"), "d:/project");
    }

        fn build_test_codex_db(db_path: &Path) {
        let connection = Connection::open(db_path).expect("open sqlite db");
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    cwd TEXT,
                    rollout_path TEXT,
                    title TEXT,
                    first_user_message TEXT,
                    created_at INTEGER,
                    updated_at INTEGER,
                    archived INTEGER DEFAULT 0
                );",
            )
            .expect("create threads table");
    }

    #[test]
    fn test_find_codex_db_path_picks_highest_version() {
        let dir = unique_temp_dir("codex-db-version");
        write_file(&dir.join("state_3.sqlite"), "");
        write_file(&dir.join("state_7.sqlite"), "");
        write_file(&dir.join("state_5.sqlite"), "");

        let found = find_codex_db_path_in(&dir).expect("find db path");
        assert_eq!(found.file_name().and_then(|value| value.to_str()), Some("state_7.sqlite"));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_get_codex_project_discovery_mode_impl_unavailable_without_sources() {
        let mode = get_codex_project_discovery_mode_impl();
        assert!(matches!(mode.mode.as_str(), "sqlite" | "scan" | "unavailable"));
    }

    #[test]
    fn test_list_codex_projects_from_db_dedupes_prefixed_paths() {
        let dir = unique_temp_dir("codex-projects-db");
        let db_path = dir.join("state_5.sqlite");
        build_test_codex_db(&db_path);
        let rollout_dir = dir.join("rollouts");
        fs::create_dir_all(&rollout_dir).expect("create rollout dir");
        let session_one = rollout_dir.join("one.jsonl");
        let session_two = rollout_dir.join("two.jsonl");
        write_file(&session_one, "{\"type\":\"session_meta\"}\n");
        write_file(&session_two, "{\"type\":\"session_meta\"}\n");
        let connection = Connection::open(&db_path).expect("open sqlite db");

        connection
            .execute(
                "INSERT INTO threads (id, cwd, rollout_path, title, first_user_message, created_at, updated_at, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                (
                    "thread-1",
                    r"\\?\D:\repo\demo",
                    session_one.to_string_lossy().to_string(),
                    "Session One",
                    "hello",
                    1_i64,
                    10_i64,
                ),
            )
            .expect("insert thread 1");
        connection
            .execute(
                "INSERT INTO threads (id, cwd, rollout_path, title, first_user_message, created_at, updated_at, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                (
                    "thread-2",
                    r"D:\repo\demo",
                    session_two.to_string_lossy().to_string(),
                    "Session Two",
                    "hello again",
                    2_i64,
                    20_i64,
                ),
            )
            .expect("insert thread 2");

        let projects = list_codex_projects_from_db(&db_path).expect("list codex projects from db");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "demo");
        assert_eq!(projects[0].path, r"D:\repo\demo");
        assert_eq!(projects[0].cwd_path.as_deref(), Some(r"D:\repo\demo"));
        assert_eq!(projects[0].modified_ms, Some(20_000));

        drop(connection);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_list_codex_projects_from_db_ignores_missing_rollouts() {
        let dir = unique_temp_dir("codex-projects-db-missing");
        let db_path = dir.join("state_5.sqlite");
        build_test_codex_db(&db_path);
        let connection = Connection::open(&db_path).expect("open sqlite db");

        connection
            .execute(
                "INSERT INTO threads (id, cwd, rollout_path, title, first_user_message, created_at, updated_at, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                (
                    "thread-missing",
                    r"D:\repo\ghost",
                    dir.join("missing.jsonl").to_string_lossy().to_string(),
                    "Missing",
                    "ghost",
                    1_i64,
                    10_i64,
                ),
            )
            .expect("insert missing thread");

        let projects = list_codex_projects_from_db(&db_path).expect("list codex projects from db");
        assert!(projects.is_empty());

        drop(connection);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_list_codex_project_entries_from_db_filters_by_normalized_cwd() {
        let dir = unique_temp_dir("codex-entries-db");
        let db_path = dir.join("state_5.sqlite");
        build_test_codex_db(&db_path);
        let session_a = dir.join("rollout-a.jsonl");
        let session_b = dir.join("rollout-b.jsonl");
        write_file(&session_a, "{\"type\":\"session_meta\"}\n");
        write_file(&session_b, "{\"type\":\"session_meta\"}\n");

        let connection = Connection::open(&db_path).expect("open sqlite db");
        connection
            .execute(
                "INSERT INTO threads (id, cwd, rollout_path, title, first_user_message, created_at, updated_at, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                (
                    "thread-a",
                    r"\\?\D:\proj\alpha",
                    session_a.to_string_lossy().to_string(),
                    "Alpha Title",
                    "Alpha message",
                    1_i64,
                    30_i64,
                ),
            )
            .expect("insert thread a");
        connection
            .execute(
                "INSERT INTO threads (id, cwd, rollout_path, title, first_user_message, created_at, updated_at, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                (
                    "thread-b",
                    r"D:\proj\beta",
                    session_b.to_string_lossy().to_string(),
                    "",
                    "Beta fallback",
                    1_i64,
                    40_i64,
                ),
            )
            .expect("insert thread b");

        let result = list_codex_project_entries_from_db(r"D:\proj\alpha", &db_path)
            .expect("list codex entries from db");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, "codex");
        assert_eq!(result[0].entry_type, "session");
        assert_eq!(result[0].label, "Alpha Title");
        assert_eq!(result[0].path, session_a.to_string_lossy().to_string());
        assert_eq!(result[0].modified_ms, Some(30_000));

        drop(connection);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_append_hidden_codex_entries_from_scan_adds_rollouts_missing_from_db() {
        let dir = unique_temp_dir("codex-db-hidden-scan");
        let db_path = dir.join("state_5.sqlite");
        build_test_codex_db(&db_path);
        let day_dir = dir.join("sessions").join("2026").join("03").join("12");
        fs::create_dir_all(&day_dir).expect("create day dir");
        let visible_session = day_dir.join("visible.jsonl");
        let hidden_session = day_dir.join("hidden.jsonl");
        write_file(
            &visible_session,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"visible prompt\"}}\n"
            ),
        );
        write_file(
            &hidden_session,
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:\\\\repo\\\\demo\"}}\n",
        );

        let connection = Connection::open(&db_path).expect("open sqlite db");
        connection
            .execute(
                "INSERT INTO threads (id, cwd, rollout_path, title, first_user_message, created_at, updated_at, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                (
                    "thread-visible",
                    r"D:\repo\demo",
                    visible_session.to_string_lossy().to_string(),
                    "Visible Title",
                    "visible prompt",
                    1_i64,
                    30_i64,
                ),
            )
            .expect("insert visible thread");

        let mut entries = list_codex_project_entries_from_db(r"D:\repo\demo", &db_path)
            .expect("list codex entries from db");
        append_hidden_codex_project_entries_from_scan(&mut entries, r"D:\repo\demo", &dir.join("sessions"));
        entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.label == "Visible Title" && !entry.hidden));
        assert!(entries.iter().any(|entry| entry.label == "hidden.jsonl" && entry.hidden));

        drop(connection);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_build_codex_timeline_event_user_message() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "Hello world"
            }
        });
        let event = build_codex_timeline_event(1, raw);
        assert_eq!(event.role, Some("user".to_string()));
        assert_eq!(event.event_type, Some("event_msg".to_string()));
        assert_eq!(event.subtype, Some("user_message".to_string()));
        assert_eq!(event.summary, "Hello world");
    }

    #[test]
    fn test_build_codex_timeline_event_agent_message() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "I can help with that."
            }
        });
        let event = build_codex_timeline_event(2, raw);
        assert_eq!(event.role, Some("assistant".to_string()));
        assert_eq!(event.summary, "I can help with that.");
    }

    #[test]
    fn test_build_codex_timeline_event_agent_reasoning_raw_content() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_reasoning_raw_content",
                "text": "Raw reasoning stream"
            }
        });
        let event = build_codex_timeline_event(2, raw);
        assert_eq!(event.role, Some("assistant".to_string()));
        assert_eq!(event.subtype, Some("agent_reasoning_raw_content".to_string()));
        assert_eq!(event.summary, "Raw reasoning stream");
    }

    #[test]
    fn test_build_codex_timeline_event_image_user_message() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "",
                "local_images": [{"path": "C:/tmp/screenshot.png"}]
            }
        });
        let event = build_codex_timeline_event(2, raw);
        assert_eq!(event.role, Some("user".to_string()));
        assert_eq!(event.summary, "[Image]");
    }

    #[test]
    fn test_build_codex_timeline_event_message_concatenates_text_blocks() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:01Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "output_text", "text": "First chunk"},
                    {"type": "output_text", "text": "Second chunk"}
                ]
            }
        });
        let event = build_codex_timeline_event(2, raw);
        assert_eq!(event.role, Some("assistant".to_string()));
        assert_eq!(event.summary, "First chunk\nSecond chunk");
    }

    #[test]
    fn test_build_codex_timeline_event_function_call() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:02Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "shell_command",
                "arguments": "{\"command\":\"ls\"}",
                "call_id": "call_abc123"
            }
        });
        let event = build_codex_timeline_event(3, raw);
        assert_eq!(event.role, None);
        assert_eq!(event.subtype, Some("function_call".to_string()));
        assert_eq!(event.tool_use_id, Some("call_abc123".to_string()));
        assert_eq!(event.operation, Some("shell_command".to_string()));
    }

    #[test]
    fn test_build_codex_timeline_event_local_shell_call() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:02Z",
            "type": "response_item",
            "payload": {
                "type": "local_shell_call",
                "call_id": "call_local123",
                "status": "completed",
                "action": {
                    "type": "exec",
                    "command": "npm run test:ui"
                }
            }
        });
        let event = build_codex_timeline_event(3, raw);
        assert_eq!(event.role, None);
        assert_eq!(event.subtype, Some("local_shell_call".to_string()));
        assert_eq!(event.tool_use_id, Some("call_local123".to_string()));
        assert_eq!(event.operation, Some("local_shell_call".to_string()));
        assert_eq!(event.summary, "completed npm run test:ui");
    }

    #[test]
    fn test_build_codex_timeline_event_structured_function_output() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:03Z",
            "type": "response_item",
            "payload": {
                "type": "function_call_output",
                "call_id": "call_abc123",
                "output": {
                    "content_items": [
                        {"type": "output_text", "text": "first"},
                        {"type": "output_text", "text": "second"}
                    ]
                }
            }
        });

        let event = build_codex_timeline_event(4, raw);
        assert_eq!(event.tool_use_id, Some("call_abc123".to_string()));
        assert_eq!(event.summary, "first\nsecond");
    }

    #[test]
    fn test_codex_legacy_ghost_snapshot_rollout_line_is_skipped() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:02Z",
            "type": "response_item",
            "payload": {
                "type": "ghost_snapshot",
                "ghost_commit": {
                    "id": "deadbeef",
                    "preexisting_untracked_dirs": [],
                    "preexisting_untracked_files": []
                }
            }
        });

        assert_eq!(is_codex_legacy_ghost_snapshot_rollout_line(&raw), true);
    }

    #[test]
    fn test_codex_token_count_metadata_uses_total_usage() {
        let raw = serde_json::json!({
            "timestamp": "2026-01-01T00:00:02Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 100,
                        "output_tokens": 40,
                        "cached_input_tokens": 20,
                        "reasoning_output_tokens": 7
                    }
                }
            }
        });

        let mut accum = SessionMetadataAccumulator::default();
        accumulate_codex_metadata(&mut accum, &raw);
        let metadata = accum.build_metadata(None, None);

        assert_eq!(metadata.total_input_tokens, 100);
        assert_eq!(metadata.total_output_tokens, 40);
        assert_eq!(metadata.total_cache_read_input_tokens, 20);
    }
}
