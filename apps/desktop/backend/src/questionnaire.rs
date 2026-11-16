// SPDX-License-Identifier: AGPL-3.0-or-later
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::lily_file::atomic_write;
use tracing::{info, warn};

// ─── Data structures ─────────────────────────────────────────────────────────

/// A single question in a questionnaire definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionDef {
	pub kind: String,
	/// Remaining fields vary by kind — stored as a flat JSON map.
	#[serde(flatten)]
	pub fields: Value,
}

/// A tab in the questionnaire UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabDef {
	pub id: String,
	pub label: String,
}

/// A section grouping related questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionDef {
	pub title: String,
	pub tab: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	/// "standard" or "contacts". Defaults to "standard" if absent.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub kind: Option<String>,
	/// Template paths this section applies to. Omitted or empty means every
	/// engagement.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub documents: Option<Vec<String>>,
	pub questions: Vec<QuestionDef>,
}

/// A complete questionnaire definition file (.lily with lily_type "questionnaire").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionnaireDefFile {
	/// Always "questionnaire" for questionnaire .lily files.
	#[serde(default)]
	pub lily_type: String,
	pub id: String,
	/// Display name — populated from the filename on read, used for filename on write.
	pub name: String,
	pub version: u32,
	pub tabs: Vec<TabDef>,
	pub sections: Vec<SectionDef>,
}

/// An entry in the questionnaire index (derived from scanning the directory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionnaireIndexEntry {
	pub id: String,
	pub name: String,
	pub version: u32,
}

/// The questionnaire index — built by scanning the questionnaires directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionnaireIndex {
	pub questionnaires: Vec<QuestionnaireIndexEntry>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Read the user-configured questionnaires directory from settings.
fn get_questionnaires_dir() -> Result<Option<PathBuf>, String> {
	let settings = crate::settings::load_settings()?;
	match settings.questionnaires_dir {
		Some(dir) => {
			let path = PathBuf::from(&dir);
			if !path.is_dir() {
				return Err(format!(
					"Questionnaires directory does not exist: {}",
					dir
				));
			}
			Ok(Some(path))
		}
		None => Ok(None),
	}
}

/// Require the questionnaires directory to be configured.
fn require_questionnaires_dir() -> Result<PathBuf, String> {
	get_questionnaires_dir()?.ok_or_else(|| "Questionnaires directory not configured".to_string())
}

/// Strip characters that are invalid in filenames on Windows/macOS/Linux.
/// Returns an error if the sanitized name is empty.
fn sanitize_filename(name: &str) -> Result<String, String> {
	let sanitized: String = name
		.chars()
		.filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
		.collect::<String>()
		.trim()
		.to_string();
	if sanitized.is_empty() {
		return Err("Name must contain at least one valid character".to_string());
	}
	Ok(sanitized)
}

/// Extract the display name from a .lily file path (filename stem).
fn name_from_path(path: &Path) -> String {
	path.file_stem()
		.map(|s| s.to_string_lossy().to_string())
		.unwrap_or_default()
}

/// Scan the questionnaires directory for .lily files with lily_type "questionnaire".
fn scan_questionnaires(dir: &Path) -> Result<Vec<(PathBuf, QuestionnaireDefFile)>, String> {
	let mut results = Vec::new();

	let entries =
		fs::read_dir(dir).map_err(|e| format!("Failed to read questionnaires directory: {}", e))?;

	for entry in entries {
		let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
		let path = entry.path();

		if !path.is_file() {
			continue;
		}
		if path.extension().is_none_or(|e| e != "lily") {
			continue;
		}

		let content = match fs::read_to_string(&path) {
			Ok(c) => c,
			Err(_) => continue, // skip unreadable files
		};

		// Check lily_type before full parse
		let parsed: Value = match serde_json::from_str(&content) {
			Ok(v) => v,
			Err(_) => continue, // skip unparseable files
		};

		if parsed.get("lily_type").and_then(|v| v.as_str()) != Some("questionnaire") {
			continue;
		}

		let mut def: QuestionnaireDefFile = serde_json::from_value(parsed)
			.map_err(|e| format!("Failed to parse questionnaire {}: {}", path.display(), e))?;

		// Filename is authoritative for the display name
		def.name = name_from_path(&path);

		results.push((path, def));
	}

	// Sort alphabetically by name (case-insensitive)
	results.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()));

	Ok(results)
}

/// Find a questionnaire file by UUID in the configured directory.
fn find_questionnaire_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
	for (path, def) in scan_questionnaires(dir)? {
		if def.id == id {
			return Ok(path);
		}
	}
	Err(format!("Questionnaire not found: {}", id))
}

/// Read and parse a single questionnaire .lily file, setting name from filename.
fn read_questionnaire_file(path: &Path) -> Result<QuestionnaireDefFile, String> {
	let content = fs::read_to_string(path)
		.map_err(|e| format!("Failed to read questionnaire: {}", e))?;
	let mut def: QuestionnaireDefFile =
		serde_json::from_str(&content).map_err(|e| format!("Failed to parse questionnaire: {}", e))?;
	def.name = name_from_path(path);
	Ok(def)
}

/// Write a questionnaire definition to a .lily file.
fn write_questionnaire_file(path: &Path, def: &QuestionnaireDefFile) -> Result<(), String> {
	let content = serde_json::to_string_pretty(def)
		.map_err(|e| format!("Failed to serialize questionnaire: {}", e))?;
	atomic_write(path, &content)
}

/// Build the target path for a questionnaire with the given name.
fn questionnaire_file_path(dir: &Path, name: &str) -> PathBuf {
	dir.join(format!("{}.lily", sanitize_filename(name).unwrap_or_else(|_| "Untitled".to_string())))
}

/// Check if a filename would conflict with an existing file (different UUID).
fn check_name_conflict(dir: &Path, name: &str, own_id: &str) -> Result<(), String> {
	let target = questionnaire_file_path(dir, name);
	if target.exists() {
		// Read the existing file to check if it's the same questionnaire (case-only rename)
		if let Ok(existing) = read_questionnaire_file(&target) {
			if existing.id != own_id {
				return Err(format!(
					"A questionnaire named \"{}\" already exists",
					name
				));
			}
		}
	}
	Ok(())
}

// ─── Legacy config-dir helpers (for migration) ──────────────────────────────

/// Path to the old config-dir questionnaires location.
fn legacy_questionnaires_dir() -> Result<PathBuf, String> {
	let config_dir =
		dirs::config_dir().ok_or_else(|| "Could not determine config directory".to_string())?;
	Ok(config_dir.join("lily").join("questionnaires"))
}

/// Legacy index file format.
#[derive(Debug, Deserialize)]
struct LegacyIndex {
	questionnaires: Vec<LegacyIndexEntry>,
}

#[derive(Debug, Deserialize)]
struct LegacyIndexEntry {
	id: String,
	name: String,
	#[allow(dead_code)]
	version: u32,
}

// ─── Tauri commands ──────────────────────────────────────────────────────────

/// Load the questionnaire index by scanning the configured directory.
/// Returns an empty index if the directory is not configured (not an error).
#[tauri::command]
pub fn load_questionnaire_index() -> Result<QuestionnaireIndex, String> {
	info!("Loading questionnaire index");
	let dir = match get_questionnaires_dir()? {
		Some(d) => d,
		None => {
			return Ok(QuestionnaireIndex {
				questionnaires: vec![],
			});
		}
	};

	let entries = scan_questionnaires(&dir)?
		.into_iter()
		.map(|(_, def)| QuestionnaireIndexEntry {
			id: def.id,
			name: def.name,
			version: def.version,
		})
		.collect();

	Ok(QuestionnaireIndex {
		questionnaires: entries,
	})
}

/// Load a single questionnaire definition by UUID.
#[tauri::command]
pub fn load_questionnaire(id: String) -> Result<QuestionnaireDefFile, String> {
	let dir = require_questionnaires_dir()?;
	let path = find_questionnaire_path(&dir, &id)?;
	read_questionnaire_file(&path)
}

/// Save/update a questionnaire definition. Bumps the version number.
/// If the name changed, renames the file on disk.
#[tauri::command]
pub fn save_questionnaire(mut questionnaire: QuestionnaireDefFile) -> Result<(), String> {
	info!(id = %questionnaire.id, name = %questionnaire.name, "Saving questionnaire");
	let dir = require_questionnaires_dir()?;
	let old_path = find_questionnaire_path(&dir, &questionnaire.id)?;
	let old_name = name_from_path(&old_path);

	questionnaire.version += 1;
	questionnaire.lily_type = "questionnaire".to_string();

	// Handle rename if name changed: write to new path first, then remove old file
	let target_path = if questionnaire.name != old_name {
		check_name_conflict(&dir, &questionnaire.name, &questionnaire.id)?;
		let new_path = questionnaire_file_path(&dir, &questionnaire.name);
		write_questionnaire_file(&new_path, &questionnaire)?;
		if let Err(e) = fs::remove_file(&old_path) {
			warn!("Failed to remove old questionnaire file after rename: {}", e);
		}
		return Ok(());
	} else {
		old_path
	};

	write_questionnaire_file(&target_path, &questionnaire)
}

/// Create a new empty questionnaire with the given name.
#[tauri::command]
pub fn create_questionnaire(name: String) -> Result<QuestionnaireDefFile, String> {
	info!(%name, "Creating questionnaire");
	let dir = require_questionnaires_dir()?;
	let target = questionnaire_file_path(&dir, &name);

	if target.exists() {
		return Err(format!(
			"A questionnaire named \"{}\" already exists",
			name
		));
	}

	let id = Uuid::new_v4().to_string();
	let q = QuestionnaireDefFile {
		lily_type: "questionnaire".to_string(),
		id: id.clone(),
		name: sanitize_filename(&name)?,
		version: 1,
		tabs: vec![
			TabDef {
				id: "client-info".into(),
				label: "Client Info".into(),
			},
			TabDef {
				id: "contacts".into(),
				label: "Client Contacts".into(),
			},
			TabDef {
				id: "assignments".into(),
				label: "Assignments & Decisions".into(),
			},
		],
		sections: vec![],
	};
	write_questionnaire_file(&target, &q)?;

	Ok(q)
}

/// Duplicate an existing questionnaire with a new name.
#[tauri::command]
pub fn duplicate_questionnaire(id: String, name: String) -> Result<QuestionnaireDefFile, String> {
	let dir = require_questionnaires_dir()?;
	let target = questionnaire_file_path(&dir, &name);

	if target.exists() {
		return Err(format!(
			"A questionnaire named \"{}\" already exists",
			name
		));
	}

	let source_path = find_questionnaire_path(&dir, &id)?;
	let source = read_questionnaire_file(&source_path)?;

	let new_id = Uuid::new_v4().to_string();
	let q = QuestionnaireDefFile {
		lily_type: "questionnaire".to_string(),
		id: new_id,
		name: sanitize_filename(&name)?,
		version: 1,
		tabs: source.tabs,
		sections: source.sections,
	};
	write_questionnaire_file(&target, &q)?;

	Ok(q)
}

/// Delete a questionnaire definition by UUID.
#[tauri::command]
pub fn delete_questionnaire(id: String) -> Result<(), String> {
	info!(%id, "Deleting questionnaire");
	let dir = require_questionnaires_dir()?;
	let path = find_questionnaire_path(&dir, &id)?;

	fs::remove_file(&path).map_err(|e| format!("Failed to delete questionnaire: {}", e))?;

	Ok(())
}

/// Migrate questionnaires from the old config-dir storage to the user-configured
/// questionnaires directory. Returns the number of migrated files.
#[tauri::command]
pub fn migrate_questionnaires() -> Result<u32, String> {
	info!("Migrating legacy questionnaires");
	let target_dir = require_questionnaires_dir()?;
	let legacy_dir = legacy_questionnaires_dir()?;

	if !legacy_dir.is_dir() {
		return Ok(0);
	}

	// Read the old index to recover each questionnaire's display name.
	let index_path = legacy_dir.join("index.json");
	let legacy_index: Option<LegacyIndex> = if index_path.exists() {
		let content = fs::read_to_string(&index_path).ok();
		content.and_then(|c| serde_json::from_str(&c).ok())
	} else {
		None
	};

	// Build a map of id → name from the legacy index
	let name_map: std::collections::HashMap<String, String> = legacy_index
		.as_ref()
		.map(|idx| {
			idx.questionnaires
				.iter()
				.map(|e| (e.id.clone(), e.name.clone()))
				.collect()
		})
		.unwrap_or_default();

	let mut count: u32 = 0;

	// Scan for .json files in the legacy dir (excluding index.json)
	let entries = fs::read_dir(&legacy_dir)
		.map_err(|e| format!("Failed to read legacy questionnaires directory: {}", e))?;

	for entry in entries {
		let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
		let path = entry.path();

		if !path.is_file() {
			continue;
		}
		if path.extension().is_none_or(|e| e != "json") {
			continue;
		}
		if path.file_name().is_some_and(|n| n == "index.json") {
			continue;
		}

		// Read and parse the old-format questionnaire
		let content = match fs::read_to_string(&path) {
			Ok(c) => c,
			Err(_) => continue,
		};
		let mut parsed: Value = match serde_json::from_str(&content) {
			Ok(v) => v,
			Err(_) => continue,
		};

		// Get the id and name
		let id = parsed
			.get("id")
			.and_then(|v| v.as_str())
			.unwrap_or("")
			.to_string();
		if id.is_empty() {
			continue;
		}

		let name = name_map
			.get(&id)
			.cloned()
			.or_else(|| {
				parsed
					.get("name")
					.and_then(|v| v.as_str())
					.map(|s| s.to_string())
			})
			.unwrap_or_else(|| id.clone());

		// Inject lily_type
		if let Some(obj) = parsed.as_object_mut() {
			obj.insert(
				"lily_type".to_string(),
				Value::String("questionnaire".to_string()),
			);
		}

		// Determine target filename, handling conflicts
		let sanitized = sanitize_filename(&name)?;
		let mut target_name = sanitized.clone();
		let mut suffix = 2;
		while questionnaire_file_path(&target_dir, &target_name).exists() {
			target_name = format!("{} ({})", sanitized, suffix);
			suffix += 1;
		}

		let target_path = questionnaire_file_path(&target_dir, &target_name);
		let content = serde_json::to_string_pretty(&parsed)
			.map_err(|e| format!("Failed to serialize migrated questionnaire: {}", e))?;
		atomic_write(&target_path, &content)?;

		count += 1;
	}

	Ok(count)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn section_and_question_documents_survive_round_trip() {
		let text = r#"{"lily_type":"questionnaire","id":"x","name":"n","version":1,"tabs":[],"sections":[{"title":"S","tab":"t","documents":["A.docx"],"questions":[{"kind":"text","variable":"V","label":"L","documents":["B.docx"]}]}]}"#;
		let def: QuestionnaireDefFile = serde_json::from_str(text).expect("definition");
		assert_eq!(
			def.sections[0].documents.as_deref(),
			Some(&["A.docx".to_string()][..])
		);
		let value = serde_json::to_value(&def).expect("json");
		assert_eq!(value["sections"][0]["documents"][0], "A.docx");
		assert_eq!(value["sections"][0]["questions"][0]["documents"][0], "B.docx");
	}
}
