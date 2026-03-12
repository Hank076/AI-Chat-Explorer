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
        let label = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (modified_ms, size_bytes) = get_file_metadata(&file);
        entries.push(Entry {
            entry_type: "session".to_string(),
            label,
            path: file.to_string_lossy().to_string(),
            parent_session: None,
            modified_ms,
            size_bytes,
            source: "codex".to_string(),
        });
    }

    entries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    entries
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
        entries.push(Entry {
            entry_type: "session".to_string(),
            label: build_codex_entry_label(title, first_user_message, &rollout_path),
            path: rollout_path,
            parent_session: None,
            modified_ms: updated_ms.or(file_modified_ms),
            size_bytes,
            source: "codex".to_string(),
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
        entries.push(Entry {
            entry_type: "session".to_string(),
            label: build_codex_entry_label(title, first_user_message, &rollout_path),
            path: rollout_path,
            parent_session: None,
            modified_ms: updated_ms.or(file_modified_ms),
            size_bytes,
            source: "codex".to_string(),
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
    if let Ok(Some(db_path)) = find_codex_db_path() {
        if let Ok(entries) = list_codex_project_entries_from_db(&cwd, &db_path) {
            return Ok(entries);
        }
    }

    let sessions_root = match default_codex_sessions_path() {
        Ok(path) => path,
        Err(_) => return Ok(vec![]),
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
                if let Some(last) = info.get("last_token_usage") {
                    accum.total_input_tokens += last.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    accum.total_output_tokens += last.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    accum.total_cache_read_input_tokens += last.get("cached_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                }
            }
        }
    }
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
            let msg = payload.get("message").and_then(|v| v.as_str()).unwrap_or("");
            summary = truncate(msg, 240);
        }
        ("event_msg", "agent_message") => {
            role = Some("assistant".to_string());
            let msg = payload.get("message").and_then(|v| v.as_str()).unwrap_or("");
            summary = truncate(msg, 240);
        }
        ("event_msg", "agent_reasoning") => {
            role = Some("assistant".to_string());
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            summary = truncate(text, 240);
        }
        ("response_item", "function_call") => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = payload.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            operation = Some(name.to_string());
            summary = truncate(&format!("{name} {args}"), 240);
        }
        ("response_item", "function_call_output") => {
            let output = payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            summary = truncate(output, 240);
        }
        ("response_item", "custom_tool_call") => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let input = payload.get("input").and_then(|v| v.as_str()).unwrap_or("");
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            operation = Some(name.to_string());
            summary = truncate(&format!("{name}: {input}"), 240);
        }
        ("response_item", "custom_tool_call_output") => {
            let output = payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
            tool_use_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
            summary = truncate(output, 240);
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
            let text = payload
                .get("content")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.iter().find_map(|item| {
                    let t = item.get("type").and_then(|v| v.as_str());
                    if matches!(t, Some("output_text") | Some("input_text") | Some("text")) {
                        item.get("text").and_then(|v| v.as_str()).map(str::to_string)
                    } else {
                        None
                    }
                }))
                .unwrap_or_default();
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
}
