// SPDX-License-Identifier: AGPL-3.0-or-later
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::lily_file::atomic_write;

/// A navigation entry persisted across sessions for the "recent pages" list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedNavEntry {
    pub step: String,
    pub working_dir: Option<String>,
    pub document_path: Option<String>,
    pub template_rel_path: Option<String>,
    pub label: String,
    /// Unix timestamp in milliseconds when this page was visited.
    pub visited_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AppSettings {
    /// Path to the directory containing template .docx files.
    pub templates_dir: Option<String>,
    /// Last-used working directory.
    pub last_working_dir: Option<String>,
    /// Recently-used working directories (most-recent first).
    #[serde(default)]
    pub recent_directories: Vec<String>,
    /// Remembered window width from last session.
    #[serde(default)]
    pub window_width: Option<u32>,
    /// Remembered window height from last session.
    #[serde(default)]
    pub window_height: Option<u32>,
    /// UI theme: "light" or "dark".
    #[serde(default)]
    pub theme: Option<String>,
    /// UI zoom level as a percentage (100 = normal). Persisted across sessions.
    #[serde(default)]
    pub zoom: Option<u32>,
    /// Status bar size: "small", "medium", or "large".
    #[serde(default)]
    pub footer_size: Option<String>,
    /// Last workflow step the user was on (for "pick up where you left off").
    #[serde(default)]
    pub last_step: Option<String>,
    /// Whether editors auto-save changes. Defaults to true when absent.
    #[serde(default)]
    pub autosave: Option<bool>,
    /// Path to the directory containing questionnaire definition .lily files.
    #[serde(default)]
    pub questionnaires_dir: Option<String>,
    /// Directories containing client folders (like templates_dir but for clients).
    #[serde(default)]
    pub client_library_dirs: Vec<String>,
    /// Persisted navigation history for "recent pages" on the hub.
    #[serde(default)]
    pub navigation_history: Vec<PersistedNavEntry>,
    /// Command that starts the external drafting agent (default: opencode).
    #[serde(default)]
    pub agent_command: Option<String>,
    /// Optional model passed to the drafting agent.
    #[serde(default)]
    pub agent_model: Option<String>,
    /// Directory where the Lily repository and its CLI live, for the agent.
    #[serde(default)]
    pub agent_repo_dir: Option<String>,
}

fn settings_path() -> Result<PathBuf, String> {
    let config_dir =
        dirs::config_dir().ok_or_else(|| "Could not determine config directory".to_string())?;
    let app_dir = config_dir.join("lily");
    if !app_dir.exists() {
        fs::create_dir_all(&app_dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    Ok(app_dir.join("settings.json"))
}

#[tauri::command]
pub fn load_settings() -> Result<AppSettings, String> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let content =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read settings: {}", e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse settings: {}", e))
}

#[tauri::command]
pub fn save_settings(settings: AppSettings) -> Result<(), String> {
    let path = settings_path()?;
    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    atomic_write(&path, &content)?;
    Ok(())
}
