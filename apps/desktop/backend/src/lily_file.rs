// SPDX-License-Identifier: AGPL-3.0-or-later
use chrono::{DateTime, Utc};
use crate::provenance::DraftProvenance;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

const LILY_EXT: &str = "lily";
const OLD_SIDECAR_FILENAME: &str = ".lily.json";
const CURRENT_VERSION: u32 = 5;

/// Write content to a file atomically: write to a temp file in the same
/// directory, then rename over the target. Prevents corruption if the
/// process crashes mid-write.
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    atomic_write_bytes(path, content.as_bytes())
}

/// Binary variant of [`atomic_write`]. Used for `.docx` saves and any other
/// non-UTF-8 payload that must not be left half-written on crash.
pub fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Cannot determine parent directory".to_string())?;
    let tmp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string())
    ));
    fs::write(&tmp_path, content).map_err(|e| format!("Failed to write temp file: {}", e))?;
    fs::rename(&tmp_path, path).map_err(|e| {
        // Clean up temp file on rename failure
        let _ = fs::remove_file(&tmp_path);
        format!("Failed to rename temp file: {}", e)
    })?;
    Ok(())
}

/// A contact associated with a client — a person referenced across documents
/// (e.g., a family member, agent, or trustee).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub full_name: String,
    pub first_name: String,
    #[serde(default)]
    pub middle_name: String,
    pub last_name: String,
    pub relationship: String,
    #[serde(default)]
    pub other_relationship: String,
    pub phone: String,
    pub email: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub zip: String,
    #[serde(default)]
    pub is_minor: bool,
}

/// Maps a "role" (e.g., "Healthcare POA Agent") to a contact, plus a mapping
/// from variable display names to contact property keys. When a contact is
/// selected for a role, the mapped variables auto-fill from the contact's
/// properties.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleAssignment {
    /// One person or a shared group. A single ID is a single assignment;
    /// several IDs are people who serve together.
    #[serde(default)]
    pub contact_ids: Vec<String>,
    /// Manual values for this assignment, keyed by contact property name.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactBinding {
    /// The contact ID this role is bound to, or `None` for manual ("Other") entry.
    pub contact_id: Option<String>,
    /// Map from variable display name → contact property key.
    /// e.g., `"POA Agent Full Name" → "full_name"`
    pub variable_mappings: HashMap<String, String>,
    /// For "contact-list" roles: the ordered list of contact IDs selected for a
    /// role that aggregates many contacts into one variable (e.g., a HIPAA
    /// release list). The single `variable_mappings` entry maps the target
    /// variable → the contact property to aggregate; each selected contact's
    /// property value is joined with `"; "` into that variable. `None` for
    /// ordinary single-contact bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_ids: Option<Vec<String>>,
    /// Ordered assignments for a role that can be held in succession. Each
    /// assignment is one person or a shared group. When absent, the single
    /// `contact_id` or `contact_ids` shape is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignments: Option<Vec<RoleAssignment>>,
}

/// Read one property from a contact.
pub fn contact_property(contact: &Contact, property: &str) -> String {
    match property {
        "full_name" => contact.full_name.clone(),
        "first_name" => contact.first_name.clone(),
        "middle_name" => contact.middle_name.clone(),
        "last_name" => contact.last_name.clone(),
        "relationship" => contact.relationship.clone(),
        "other_relationship" => contact.other_relationship.clone(),
        "phone" => contact.phone.clone(),
        "email" => contact.email.clone(),
        "address" => contact.address.clone(),
        "city" => contact.city.clone(),
        "state" => contact.state.clone(),
        "zip" => contact.zip.clone(),
        _ => String::new(),
    }
}

/// Status of a required document in the client workflow.
///
/// Progression: NotStarted → Drafting → Reviewing → Complete → Executed
/// - NotStarted: client needs this doc, but it doesn't exist yet
/// - Drafting: doc exists but still has unfilled variables
/// - Reviewing: all variables filled; ready for attorney review & polish
/// - Complete: reviewed and ready for signature
/// - Executed: signed (locked from editing unless user confirms)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    #[default]
    NotStarted,
    Drafting,
    Reviewing,
    Complete,
    Executed,
}

/// A document that a client needs prepared, with status tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredDocument {
    /// Unique identifier for this requirement.
    pub id: String,
    /// Which template this document is based on (relative to templates dir).
    pub template_rel_path: String,
    /// Current status in the workflow.
    pub status: DocumentStatus,
    /// The filename of the actual document in the working dir, once created.
    /// Links to the `documents` HashMap key in `LilyFile`.
    pub document_filename: Option<String>,
    /// Free-form notes about this requirement.
    #[serde(default)]
    pub notes: String,
}

/// Notes attached to a questionnaire section (client-facing and internal).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SectionNotes {
    /// Notes from/for the client (visible in client-facing tools).
    #[serde(default)]
    pub client: String,
    /// Internal notes for the legal team (not visible to clients).
    #[serde(default)]
    pub internal: String,
}

/// Where the client stands in the engagement workflow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStage {
    /// The attorney is still scoping the engagement.
    #[default]
    Intake,
    /// Documents are selected and the questionnaire is in progress.
    Questionnaire,
    /// The questionnaire is complete and the engagement letter is being prepared.
    Engagement,
    /// The engagement letter is signed.
    Signed,
    /// The selected documents are being drafted.
    Drafting,
    /// The drafts are with the attorney for review.
    Review,
    /// Everything is executed or ready to close.
    Complete,
}

/// Who the engagement is waiting on at the current stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowWaitingOn {
    #[default]
    None,
    Attorney,
    Client,
}

/// Top-level `.lily` project file that lives in each client/working directory.
/// Acts as both a configuration file and (via OS file-type association) a
/// project launcher — analogous to Unreal Engine's `.uproject` files.
///
/// All variables are stored at the client level (shared across every document
/// in the directory). Document entries track only provenance and timestamps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LilyFile {
    /// Discriminator for .lily file types ("client", "questionnaire", etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lily_type: Option<String>,
    /// Schema version for forward compatibility.
    pub lily_version: u32,
    /// Where the client stands in the engagement workflow.
    #[serde(default)]
    pub workflow_stage: WorkflowStage,
    /// Who the current stage is waiting on.
    #[serde(default)]
    pub workflow_waiting_on: WorkflowWaitingOn,
    /// Client-level variable values shared across all documents.
    pub variables: HashMap<String, String>,
    /// DEPRECATED: conditional variable names are now determined by the
    /// template schema. Kept for backward compat parsing of old .lily files.
    #[serde(default, skip_serializing)]
    pub conditional_variables: Vec<String>,
    /// DEPRECATED: conditional definitions now live in the template schema
    /// (.lily sidecar). Kept for backward compat parsing of old .lily files.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    pub conditional_definitions: HashMap<String, Vec<String>>,
    /// Map from document filename to its metadata.
    pub documents: HashMap<String, DocumentMeta>,
    /// Contacts associated with this client.
    #[serde(default)]
    pub contacts: Vec<Contact>,
    /// Contact-to-role bindings, keyed by role name.
    #[serde(default)]
    pub contact_bindings: HashMap<String, ContactBinding>,
    /// Questionnaire notes keyed by section title.
    #[serde(default)]
    pub questionnaire_notes: HashMap<String, SectionNotes>,
    /// ID of the questionnaire definition used for this client.
    #[serde(default)]
    pub questionnaire_id: Option<String>,
    /// Version of the questionnaire definition when it was last applied.
    #[serde(default)]
    pub questionnaire_version: Option<u32>,
    /// Documents required for this client, with status tracking.
    #[serde(default)]
    pub required_documents: Vec<RequiredDocument>,
    /// Non-persisted warnings surfaced to the frontend on load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// A per-document override for a contact role, allowing a document to use
/// a different contact (or custom values) than what the questionnaire set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleOverride {
    /// The contact ID for this override, or `None` for custom manual values.
    pub contact_id: Option<String>,
    /// The specific variable values for this override.
    pub values: HashMap<String, String>,
}

/// Metadata for a single document in the working directory.
/// Variable values are stored at the top-level `LilyFile.variables`, not here.
/// However, the *names* of variables this document uses are stored here so that
/// reopening a saved document (where `{Placeholder}` text has been replaced
/// with actual values) still knows which variables apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    /// Relative path of the source template within the templates directory
    /// (e.g., "Trust Templates/Revocable Trust.docx").
    pub template_rel_path: String,
    /// When the document was first created from the template.
    pub created_at: DateTime<Utc>,
    /// When the document was last saved/modified through Lily.
    pub modified_at: DateTime<Utc>,
    /// Display names of the variables this document uses, recorded when the
    /// template is first processed. Used to restore the variable list after
    /// placeholders have been replaced with real values.
    #[serde(default)]
    pub variable_names: Vec<String>,
    /// Per-document role overrides. When a role is present here, the document
    /// uses the override's values instead of the questionnaire's binding.
    #[serde(default)]
    pub role_overrides: HashMap<String, RoleOverride>,
    /// Per-document variable overrides. When a variable name is present here,
    /// the document uses this value instead of the client-level value.
    #[serde(default)]
    pub variable_overrides: HashMap<String, String>,
    /// What produced the current draft: plan, clauses, client-data
    /// fingerprint, renderer, agent, and approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<DraftProvenance>,
}

impl Default for LilyFile {
    fn default() -> Self {
        Self {
            lily_type: Some("client".to_string()),
            lily_version: CURRENT_VERSION,
            workflow_stage: WorkflowStage::default(),
            workflow_waiting_on: WorkflowWaitingOn::default(),
            variables: HashMap::new(),
            conditional_variables: Vec::new(),
            conditional_definitions: HashMap::new(),
            documents: HashMap::new(),
            contacts: Vec::new(),
            contact_bindings: HashMap::new(),
            questionnaire_notes: HashMap::new(),
            questionnaire_id: None,
            questionnaire_version: None,
            required_documents: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

// ─── Legacy sidecar types (for migration only) ─────────────────────────────

#[derive(Debug, Deserialize)]
struct LegacySidecarFile {
    #[allow(dead_code)]
    version: u32,
    documents: HashMap<String, LegacyDocumentMeta>,
}

#[derive(Debug, Deserialize)]
struct LegacyDocumentMeta {
    template_rel_path: String,
    created_at: DateTime<Utc>,
    modified_at: DateTime<Utc>,
    variable_values: HashMap<String, String>,
}

// ─── File discovery & I/O ───────────────────────────────────────────────────

/// Find the `.lily` file in a working directory.
/// Returns `None` if no `.lily` file exists. If multiple `.lily` files exist,
/// returns the first one found (alphabetically).
fn find_lily_files(working_dir: &str) -> Result<Vec<std::path::PathBuf>, String> {
    let dir = Path::new(working_dir);
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", working_dir));
    }

    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    let mut lily_files: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == LILY_EXT {
                    lily_files.push(path);
                }
            }
        }
    }

    lily_files.sort();
    Ok(lily_files)
}

/// Read the `.lily` file from a working directory.
/// If no `.lily` file exists, checks for a legacy `.lily.json` and migrates it.
/// Returns a default (empty) LilyFile if neither exists.
/// Store a draft provenance record on a document's metadata. Missing
/// document entries are left alone: the render already succeeded, and the
/// provenance is a record rather than a precondition.
pub fn set_document_provenance(
    working_dir: &str,
    filename: &str,
    provenance: DraftProvenance,
) -> Result<(), String> {
    let mut lily = read_lily_file(working_dir)?;
    if let Some(meta) = lily.documents.get_mut(filename) {
        meta.provenance = Some(provenance);
        write_lily_file(working_dir, &lily)?;
    } else {
        tracing::warn!(
            document = filename,
            "No .lily document entry for rendered provenance"
        );
    }
    Ok(())
}

pub fn read_lily_file(working_dir: &str) -> Result<LilyFile, String> {
    info!(working_dir, "Loading .lily file");
    // Check for existing .lily file(s)
    let lily_files = find_lily_files(working_dir)?;
    if let Some(path) = lily_files.first() {
        let mut warnings = Vec::new();
        if lily_files.len() > 1 {
            warn!(working_dir, count = lily_files.len(), "Multiple .lily files found");
            let names: Vec<String> = lily_files
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .collect();
            warnings.push(format!(
                "Multiple .lily files found (using {}): {}",
                names.first().unwrap_or(&String::new()),
                names.join(", ")
            ));
        }
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read .lily file: {}", e))?;
        let mut lily: LilyFile = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse .lily file: {}", e))?;

        // Auto-migrate older versions: new fields have #[serde(default)] so
        // they deserialize as empty; just bump the version and persist.
        if lily.lily_version < CURRENT_VERSION {
            lily.lily_version = CURRENT_VERSION;
            write_lily_file(working_dir, &lily)?;
        }

        lily.warnings = warnings;
        return Ok(lily);
    }

    // Check for legacy .lily.json and migrate
    let legacy_path = Path::new(working_dir).join(OLD_SIDECAR_FILENAME);
    if legacy_path.exists() {
        return migrate_legacy_sidecar(working_dir);
    }

    Ok(LilyFile::default())
}

/// Write a client `.lily` file from another module.
pub fn write_client_lily(working_dir: &str, lily: &LilyFile) -> Result<(), String> {
    write_lily_file(working_dir, lily)
}

/// Copy the client's current `.lily` file to a timestamped backup beside it.
pub fn backup_client_lily(working_dir: &str) -> Result<Option<String>, String> {
    let Some(path) = find_lily_files(working_dir)?.into_iter().next() else {
        return Ok(None);
    };
    let stamp = Utc::now().format("%Y%m%d-%H%M%S%.3f");
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "Client.lily".to_string());
    let backup = path.with_file_name(format!("{}.bak-{}", name, stamp));
    fs::copy(&path, &backup)
        .map_err(|error| format!("Failed to back up {}: {}", path.display(), error))?;
    Ok(Some(backup.to_string_lossy().to_string()))
}

/// Write the `.lily` file to disk.
/// If a `.lily` file already exists in the directory, it is overwritten.
/// Otherwise, a new file is created using the directory name as the filename
/// (e.g., `Doe, Jane.lily` for a directory named `Doe, Jane`).
fn write_lily_file(working_dir: &str, lily: &LilyFile) -> Result<(), String> {
    info!(working_dir, "Writing .lily file");
    let path = match find_lily_files(working_dir)?.into_iter().next() {
        Some(existing) => existing,
        None => {
            // Derive filename from the directory name
            let dir = Path::new(working_dir);
            let dir_name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".to_string());
            dir.join(format!("{}.lily", dir_name))
        }
    };

    let content = serde_json::to_string_pretty(lily)
        .map_err(|e| format!("Failed to serialize .lily file: {}", e))?;
    atomic_write(&path, &content)?;
    Ok(())
}

// ─── Migration ──────────────────────────────────────────────────────────────

/// Migrate a legacy `.lily.json` sidecar to the new `.lily` format.
/// All per-document variable values are merged into the top-level variable pool.
/// The old `.lily.json` file is removed after successful migration.
fn migrate_legacy_sidecar(working_dir: &str) -> Result<LilyFile, String> {
    let legacy_path = Path::new(working_dir).join(OLD_SIDECAR_FILENAME);
    let content = fs::read_to_string(&legacy_path)
        .map_err(|e| format!("Failed to read legacy sidecar: {}", e))?;
    let legacy: LegacySidecarFile = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse legacy sidecar: {}", e))?;

    let mut lily = LilyFile::default();

    for (filename, meta) in legacy.documents {
        // Merge variable values into the top-level pool
        for (key, value) in &meta.variable_values {
            if !value.is_empty() {
                // Only overwrite if the top-level doesn't already have a value
                lily.variables
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }

        // Create a document entry; derive variable_names from the legacy keys
        let variable_names: Vec<String> = meta.variable_values.keys().cloned().collect();
        lily.documents.insert(
            filename,
            DocumentMeta {
                template_rel_path: meta.template_rel_path,
                created_at: meta.created_at,
                modified_at: meta.modified_at,
                variable_names,
                role_overrides: HashMap::new(),
                variable_overrides: HashMap::new(),
                provenance: None,
            },
        );
    }

    // Write the new .lily file
    write_lily_file(working_dir, &lily)?;

    // Remove the old .lily.json
    if let Err(e) = fs::remove_file(&legacy_path) {
        eprintln!(
            "Warning: migrated to .lily but failed to remove old .lily.json: {}",
            e
        );
    }

    Ok(lily)
}

// ─── CRUD operations ────────────────────────────────────────────────────────

/// Record a newly created document in the .lily file.
/// Called when a template is copied into the working directory.
pub fn record_document(
    working_dir: &str,
    filename: &str,
    template_rel_path: &str,
) -> Result<(), String> {
    let mut lily = read_lily_file(working_dir)?;
    let now = Utc::now();
    lily.documents.insert(
        filename.to_string(),
        DocumentMeta {
            template_rel_path: template_rel_path.to_string(),
            created_at: now,
            modified_at: now,
            variable_names: Vec::new(),
            role_overrides: HashMap::new(),
            variable_overrides: HashMap::new(),
            provenance: None,
        },
    );
    write_lily_file(working_dir, &lily)
}

/// Update the client-level variable values and the modified timestamp for a
/// specific document. All variables are stored at the top level (shared across
/// documents), so this merges the provided values into the existing pool.
pub fn update_variables(
    working_dir: &str,
    filename: &str,
    variable_values: HashMap<String, String>,
) -> Result<(), String> {
    let mut lily = read_lily_file(working_dir)?;

    // Merge variable values into the top-level pool
    for (key, value) in variable_values {
        lily.variables.insert(key, value);
    }

    // Update the document's modified timestamp
    if let Some(meta) = lily.documents.get_mut(filename) {
        meta.modified_at = Utc::now();
    }

    write_lily_file(working_dir, &lily)
}

/// Rename a document entry in the .lily file (update the key from old to new filename).
pub fn rename_document_entry(
    working_dir: &str,
    old_filename: &str,
    new_filename: &str,
) -> Result<(), String> {
    let mut lily = read_lily_file(working_dir)?;
    match lily.documents.remove(old_filename) {
        Some(mut meta) => {
            meta.modified_at = Utc::now();
            lily.documents.insert(new_filename.to_string(), meta);
        }
        None => {
            return Err(format!(
                "Document '{}' not found in .lily file",
                old_filename
            ));
        }
    }
    write_lily_file(working_dir, &lily)
}

// ─── Tauri commands ─────────────────────────────────────────────────────────

/// Load the .lily file for a working directory.
/// Returns the full data so the frontend can inspect document metadata and variables.
#[tauri::command]
pub fn load_lily_file_cmd(working_dir: String) -> Result<LilyFile, String> {
    read_lily_file(&working_dir)
}

/// Check whether a .lily file exists on disk in the given directory.
#[tauri::command]
pub fn has_lily_file(working_dir: String) -> Result<bool, String> {
    let files = find_lily_files(&working_dir)?;
    Ok(!files.is_empty())
}

/// Create a new .lily file in the given directory, writing the default
/// structure to disk. The questionnaire is chosen at creation time (the user
/// picks one from the library), so its id/version are stamped immediately.
/// Returns the created LilyFile.
#[tauri::command]
pub fn create_lily_file(
    working_dir: String,
    questionnaire_id: Option<String>,
    questionnaire_version: Option<u32>,
) -> Result<LilyFile, String> {
    let lily = LilyFile {
        questionnaire_id,
        questionnaire_version,
        ..LilyFile::default()
    };
    write_lily_file(&working_dir, &lily)?;
    Ok(lily)
}

/// Save variable values for a document and update its modified timestamp.
#[tauri::command]
pub fn save_variables(
    working_dir: String,
    filename: String,
    variable_values: HashMap<String, String>,
) -> Result<(), String> {
    update_variables(&working_dir, &filename, variable_values)
}

/// Save client-level variable values without associating them with a specific
/// document. Used by the Client Hub for direct variable editing.
#[tauri::command]
pub fn save_client_variables(
    working_dir: String,
    variable_values: HashMap<String, String>,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;

    for (key, value) in variable_values {
        lily.variables.insert(key, value);
    }

    write_lily_file(&working_dir, &lily)
}

/// Add a new variable to the client-level pool with an empty value.
/// Returns an error if the variable already exists (case-sensitive check on the key).
#[tauri::command]
pub fn add_client_variable(working_dir: String, variable_name: String) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;

    if lily.variables.contains_key(&variable_name) {
        return Err(format!("Variable '{}' already exists", variable_name));
    }

    lily.variables.insert(variable_name, String::new());
    write_lily_file(&working_dir, &lily)
}

/// Remove a variable from the client-level pool.
#[tauri::command]
pub fn remove_client_variable(working_dir: String, variable_name: String) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.variables.remove(&variable_name);
    write_lily_file(&working_dir, &lily)
}

/// Store the list of variable names (display names) that a document uses,
/// along with the list of conditional variable names and their full
/// definitions.  Called after extracting variables from a freshly created
/// document so that the variable list and conditional logic survive across
/// save cycles (where placeholders are replaced with real values in the docx).
#[tauri::command]
pub fn set_document_variables(
    working_dir: String,
    filename: String,
    variable_names: Vec<String>,
    conditional_names: Vec<String>,
    _conditional_definitions: HashMap<String, Vec<String>>,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    if let Some(meta) = lily.documents.get_mut(&filename) {
        meta.variable_names = variable_names;
    } else {
        return Err(format!("Document '{}' not found in .lily file", filename));
    }
    // Track conditional variable names (still useful for UI toggle rendering)
    for name in conditional_names {
        if !lily.conditional_variables.contains(&name) {
            lily.conditional_variables.push(name);
        }
    }
    // NOTE: conditional_definitions are no longer stored in the client .lily
    // file — they live in the template schema (.lily sidecar). The parameter
    // is kept for API compatibility but ignored.
    write_lily_file(&working_dir, &lily)
}

/// Delete a document file from disk and remove its entry from the .lily file.
#[tauri::command]
pub fn delete_document(working_dir: String, filename: String) -> Result<(), String> {
    let file_path = Path::new(&working_dir).join(&filename);

    // Remove the file from disk
    if file_path.exists() {
        fs::remove_file(&file_path)
            .map_err(|e| format!("Failed to delete file '{}': {}", filename, e))?;
    }

    // Remove the entry from the .lily file
    let mut lily = read_lily_file(&working_dir)?;
    lily.documents.remove(&filename);
    write_lily_file(&working_dir, &lily)
}

/// Create a new versioned copy of an existing document.
/// The new filename is `{basename}-{YYYYMMDD}.docx`. If that already exists,
/// appends a numeric suffix like `-{YYYYMMDD}-2.docx`.
/// Returns the filename of the new version.
#[tauri::command]
pub fn new_version_document(working_dir: String, filename: String) -> Result<String, String> {
    let src_path = Path::new(&working_dir).join(&filename);
    if !src_path.exists() {
        return Err(format!("Document '{}' not found", filename));
    }

    let mut lily = read_lily_file(&working_dir)?;
    let (template_rel_path, variable_names) = {
        let meta = lily
            .documents
            .get(&filename)
            .ok_or_else(|| format!("Document '{}' not found in .lily file", filename))?;
        (meta.template_rel_path.clone(), meta.variable_names.clone())
    };

    // Build the new filename with today's date
    let basename = Path::new(&filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());
    let date_str = Utc::now().format("%Y%m%d").to_string();
    let mut new_filename = format!("{}-{}.docx", basename, date_str);

    // Handle collision: append a numeric suffix
    let mut counter = 2u32;
    while Path::new(&working_dir).join(&new_filename).exists() {
        new_filename = format!("{}-{}-{}.docx", basename, date_str, counter);
        counter += 1;
    }

    // Copy the file
    let dest_path = Path::new(&working_dir).join(&new_filename);
    fs::copy(&src_path, &dest_path).map_err(|e| format!("Failed to copy document: {}", e))?;

    // Record the new document in the .lily file, sharing the same template origin
    // and variable_names from the source document. Mutate and write the value we
    // already read above — re-reading here would clobber any concurrent writes
    // between the two reads (Tauri commands run on a thread pool).
    let now = Utc::now();
    lily.documents.insert(
        new_filename.clone(),
        DocumentMeta {
            template_rel_path,
            created_at: now,
            modified_at: now,
            variable_names,
            role_overrides: HashMap::new(),
            variable_overrides: HashMap::new(),
            provenance: None,
        },
    );
    write_lily_file(&working_dir, &lily)?;

    Ok(new_filename)
}

/// Open a file using the OS default application (e.g., open a .docx template
/// in Word). Uses `xdg-open` on Linux, `open` on macOS, `start` on Windows.
#[tauri::command]
pub fn open_file_in_os(file_path: String) -> Result<(), String> {
    let path = Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open")
        .arg(&file_path)
        .spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&file_path).spawn();

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", &file_path])
        .spawn();

    result.map_err(|e| format!("Failed to open file: {}", e))?;
    Ok(())
}

// ─── Contact CRUD commands ──────────────────────────────────────────────────

/// Helper: look up a contact property by its key name.
fn get_contact_property(contact: &Contact, key: &str) -> String {
    match key {
        "full_name" => contact.full_name.clone(),
        "first_name" => contact.first_name.clone(),
        "middle_name" => contact.middle_name.clone(),
        "last_name" => contact.last_name.clone(),
        "relationship" => {
            if contact.relationship == "Other" && !contact.other_relationship.is_empty() {
                contact.other_relationship.clone()
            } else {
                contact.relationship.clone()
            }
        }
        "other_relationship" => contact.other_relationship.clone(),
        "phone" => contact.phone.clone(),
        "email" => contact.email.clone(),
        "address" => contact.address.clone(),
        "city" => contact.city.clone(),
        "state" => contact.state.clone(),
        "zip" => contact.zip.clone(),
        _ => String::new(),
    }
}

/// Collect the non-empty `prop_key` values of the contacts identified by `ids`,
/// in order. Used to aggregate a "contact-list" role (e.g. additional HIPAA
/// releases) into a single variable. IDs with no matching contact and contacts
/// whose property is empty are skipped, so the caller can treat an empty result
/// as "no entries" (and mark the role's `Has {role}` conditional false).
fn aggregate_contact_list_values(
    contacts: &[Contact],
    ids: &[String],
    prop_key: &str,
) -> Vec<String> {
    ids.iter()
        .filter_map(|id| contacts.iter().find(|c| &c.id == id))
        .map(|c| get_contact_property(c, prop_key))
        .filter(|v| !v.is_empty())
        .collect()
}

/// Add a new contact to the .lily file. A UUID is generated for the `id` field
/// (any value provided is overwritten). Returns the contact with its assigned ID.
#[tauri::command]
pub fn add_contact(working_dir: String, mut contact: Contact) -> Result<Contact, String> {
    contact.id = Uuid::new_v4().to_string();
    let mut lily = read_lily_file(&working_dir)?;
    lily.contacts.push(contact.clone());
    write_lily_file(&working_dir, &lily)?;
    Ok(contact)
}

/// Update an existing contact by ID. Returns an error if the contact is not found.
#[tauri::command]
pub fn update_contact(working_dir: String, contact: Contact) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    let pos = lily
        .contacts
        .iter()
        .position(|c| c.id == contact.id)
        .ok_or_else(|| format!("Contact '{}' not found", contact.id))?;
    lily.contacts[pos] = contact;
    write_lily_file(&working_dir, &lily)
}

/// Delete a contact by ID. Also removes any contact_bindings that reference it.
#[tauri::command]
pub fn delete_contact(working_dir: String, contact_id: String) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.contacts.retain(|c| c.id != contact_id);
    // Clear bindings that reference this contact
    for binding in lily.contact_bindings.values_mut() {
        if binding.contact_id.as_deref() == Some(&contact_id) {
            binding.contact_id = None;
        }
    }
    write_lily_file(&working_dir, &lily)
}

/// Replace the entire contact_bindings map in the .lily file.
#[tauri::command]
pub fn save_contact_bindings(
    working_dir: String,
    contact_bindings: HashMap<String, ContactBinding>,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.contact_bindings = contact_bindings;
    write_lily_file(&working_dir, &lily)
}

/// Resolve all contact bindings: for each binding with a contact_id, write
/// the contact's property values into the variables pool and persist.
/// Also auto-sets `Has {role}` conditional variables to `"true"` when a
/// contact is bound to a role, or `"false"` when the binding has no contact.
#[tauri::command]
pub fn resolve_contact_variables(working_dir: String) -> Result<Vec<String>, String> {
    let mut lily = read_lily_file(&working_dir)?;
    let mut warnings: Vec<String> = Vec::new();

    // ── Pass 1: Role-based conditionals ("Has {role}") ──────────────────
    for (role, binding) in &lily.contact_bindings {
        let has_key = format!("Has {}", role);

        // Ordered assignments supersede the single-contact shape. A role is
        // present when any assignment names someone or carries a manual value.
        // The first assignment also fills the legacy mapped variables so a
        // plan that has not moved to a role node still renders.
        if let Some(assignments) = &binding.assignments {
            let filled = assignments.iter().any(|assignment| {
                !assignment.contact_ids.is_empty()
                    || assignment.values.values().any(|value| !value.is_empty())
            });
            lily.variables.insert(
                has_key,
                if filled { "true" } else { "false" }.to_string(),
            );
            if !filled {
                for var_name in binding.variable_mappings.keys() {
                    lily.variables.insert(var_name.clone(), String::new());
                }
            }
            if let Some(first) = assignments.first() {
                for (var_name, prop_key) in &binding.variable_mappings {
                    let value = first
                        .values
                        .get(prop_key)
                        .cloned()
                        .or_else(|| {
                            first.contact_ids.first().and_then(|id| {
                                lily.contacts
                                    .iter()
                                    .find(|contact| &contact.id == id)
                                    .map(|contact| get_contact_property(contact, prop_key))
                            })
                        })
                        .unwrap_or_default();
                    lily.variables.insert(var_name.clone(), value);
                }
            }
            continue;
        }

        // Contact-list bindings aggregate several contacts into one variable.
        // The single `variable_mappings` entry maps the target variable → the
        // contact property to collect; each selected contact's value is joined
        // with "; " into that variable. `Has {role}` is "true" when at least
        // one selected contact yields a non-empty value, else "false" (so an
        // empty list self-prunes any conditional list element in the template).
        if let Some(ids) = &binding.contact_ids {
            let prop_key = binding
                .variable_mappings
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| "full_name".to_string());
            let values = aggregate_contact_list_values(&lily.contacts, ids, &prop_key);
            let joined = values.join("; ");
            if let Some(target_var) = binding.variable_mappings.keys().next() {
                lily.variables.insert(target_var.clone(), joined);
            }
            lily.variables.insert(
                has_key,
                if values.is_empty() { "false" } else { "true" }.to_string(),
            );
            continue;
        }

        match &binding.contact_id {
            Some(id) if id == "__none__" => {
                // Explicitly "None" — clear all mapped variables and mark role absent
                for var_name in binding.variable_mappings.keys() {
                    lily.variables.insert(var_name.clone(), String::new());
                }
                lily.variables.insert(has_key, "false".to_string());
            }
            Some(id) => {
                let contact = lily.contacts.iter().find(|c| &c.id == id);
                if let Some(contact) = contact {
                    for (var_name, prop_key) in &binding.variable_mappings {
                        let value = get_contact_property(contact, prop_key);
                        lily.variables.insert(var_name.clone(), value);
                    }
                    lily.variables.insert(has_key, "true".to_string());
                } else {
                    // Binding points at a contact that no longer exists
                    // (e.g., the contact was deleted). Mark the role absent
                    // and surface a warning so the user knows their
                    // auto-filled variables are blank for a reason.
                    lily.variables.insert(has_key, "false".to_string());
                    warnings.push(format!(
                        "Role \"{}\" is bound to a contact that no longer exists. Re-assign or remove the binding to fix.",
                        role
                    ));
                }
            }
            None => {
                // "Other" (manual entry) — properties are set manually,
                // but the role is still considered "has" if any mapped
                // variable has a value.
                let any_filled = binding.variable_mappings.keys().any(|k| {
                    lily.variables.get(k).is_some_and(|v| !v.is_empty())
                });
                lily.variables
                    .insert(has_key, if any_filled { "true" } else { "false" }.to_string());
            }
        }
    }

    // ── Pass 2: Relationship-based conditionals ("Has {relationship}") ──
    // For each unique relationship found among contacts, set a
    // "Has {Relationship}" variable to "true" or "false". This allows
    // templates to use conditionals like `{Has Spouse ?? ... :: ...}`
    // that auto-derive from the client's contact relationships.
    // Also handles "Other" relationships via `other_relationship`.
    let binding_roles: std::collections::HashSet<&str> =
        lily.contact_bindings.keys().map(|k| k.as_str()).collect();
    // Collect all unique relationship names from contacts
    let mut relationship_names: Vec<String> = Vec::new();
    for contact in &lily.contacts {
        let rel = if contact.relationship == "Other" && !contact.other_relationship.is_empty() {
            &contact.other_relationship
        } else {
            &contact.relationship
        };
        if !rel.is_empty() && !binding_roles.contains(rel.as_str()) && !relationship_names.contains(rel) {
            relationship_names.push(rel.clone());
        }
    }
    for relationship in &relationship_names {
        let has_key = format!("Has {}", relationship);
        lily.variables.insert(has_key, "true".to_string());
    }
    // Also check for relationships that WERE present but are no longer
    // (e.g., spouse contact was removed). Clear those to "false".
    let has_prefix_keys: Vec<String> = lily.variables.keys()
        .filter(|k| k.starts_with("Has "))
        .cloned()
        .collect();
    for key in has_prefix_keys {
        let suffix = &key[4..]; // Strip "Has "
        if binding_roles.contains(suffix) {
            continue; // Handled by role-based pass
        }
        if !relationship_names.iter().any(|r| r == suffix) {
            // This relationship no longer exists among contacts
            lily.variables.insert(key, "false".to_string());
        }
    }

    // ── Pass 3: Spouse and children derived variables ─────────────────────
    // Populate variables that templates expect from spouse/children contacts.
    let spouse = lily.contacts.iter().find(|c| c.relationship.eq_ignore_ascii_case("Spouse"));
    lily.variables.insert(
        "Client Spouse Name".to_string(),
        spouse.map_or_else(String::new, |s| s.full_name.clone()),
    );

    let children: Vec<&Contact> = lily
        .contacts
        .iter()
        .filter(|c| c.relationship.eq_ignore_ascii_case("Child"))
        .collect();
    let has_children = !children.is_empty();
    lily.variables.insert(
        "Client has Children".to_string(),
        if has_children { "true" } else { "false" }.to_string(),
    );
    lily.variables.insert(
        "Client Children Full Names".to_string(),
        children.iter().map(|c| c.full_name.as_str()).collect::<Vec<_>>().join(", "),
    );

    let has_minor_children = children.iter().any(|c| c.is_minor);
    lily.variables.insert(
        "Has Minor Children".to_string(),
        if has_minor_children { "true" } else { "false" }.to_string(),
    );

    // ── Pass 4: Co-agent composite helper variables ────────────────────
    // Detect co-agent pairs by naming convention and generate composite
    // variables that templates can use for combined names, phones, verb
    // agreement, and role titles.
    //
    // Convention: "X Co-Agent" is the co-agent for "X Agent", and
    // "X Co-Personal Representative" is the co-agent for "X Personal
    // Representative".
    let roles: Vec<String> = lily.contact_bindings.keys().cloned().collect();
    let mut co_agent_map: Vec<(String, String)> = Vec::new(); // (parent_role, co_role)
    for role in &roles {
        if let Some(parent) = role
            .strip_suffix(" Co-Agent")
            .map(|prefix| format!("{} Agent", prefix.trim_end()))
        {
            if roles.contains(&parent) {
                co_agent_map.push((parent, role.clone()));
            }
        } else if role.contains("Co-Personal Representative") {
            let parent = role.replace("Co-Personal Representative", "Personal Representative");
            if roles.contains(&parent) {
                co_agent_map.push((parent, role.clone()));
            }
        }
    }

    // For every co-agent-capable parent role, generate helpers.
    // First collect all parent roles that COULD have co-agents (even those
    // that don't currently) by also scanning for parent roles without a pair.
    let parents_with_co: std::collections::HashSet<String> =
        co_agent_map.iter().map(|(p, _)| p.clone()).collect();

    for (parent_role, co_role) in &co_agent_map {
        let co_has = lily
            .variables
            .get(&format!("Has {}", co_role))
            .is_some_and(|v| v == "true");

        let (co_name, co_phone) = if co_has {
            let name_key = format!("{} Full Name", co_role);
            let phone_key = format!("{} Phone", co_role);
            let name = lily.variables.get(&name_key).cloned().unwrap_or_default();
            let phone = lily.variables.get(&phone_key).cloned().unwrap_or_default();
            (name, phone)
        } else {
            (String::new(), String::new())
        };

        // Determine role title based on role type
        let (solo_title, co_title) = if parent_role.contains("HPOA") {
            (
                "Healthcare Representative.".to_string(),
                "co-Healthcare Representatives. Either co-representative may act individually if the other is unable or unwilling to serve.".to_string(),
            )
        } else if parent_role.contains("FPOA") {
            (
                "Agent.".to_string(),
                "co-Agents. Either co-agent may act individually if the other is unable or unwilling to serve.".to_string(),
            )
        } else if parent_role.contains("Personal Representative") {
            (
                "Personal Representative.".to_string(),
                "co-Personal Representatives. Either co-representative may act individually if the other is unable or unwilling to serve.".to_string(),
            )
        } else {
            ("representative.".to_string(), "co-representatives.".to_string())
        };

        if co_has && !co_name.is_empty() {
            // Uppercase the co-agent name so it matches the ALL CAPS
            // convention used for names in legal documents.  The " and "
            // conjunction stays lowercase.
            lily.variables.insert(
                format!("{} And Name", co_role),
                format!(" and {}", co_name.to_uppercase()),
            );
            lily.variables.insert(
                format!("{} And Phone", co_role),
                if co_phone.is_empty() {
                    String::new()
                } else {
                    format!(" and {}, respectively", co_phone)
                },
            );
            lily.variables
                .insert(format!("{} Verb", parent_role), "are".to_string());
            lily.variables
                .insert(format!("{} Title", parent_role), co_title);
        } else {
            lily.variables.insert(
                format!("{} And Name", co_role),
                String::new(),
            );
            lily.variables.insert(
                format!("{} And Phone", co_role),
                String::new(),
            );
            lily.variables
                .insert(format!("{} Verb", parent_role), "is".to_string());
            lily.variables
                .insert(format!("{} Title", parent_role), solo_title);
        }
    }

    // Also generate default helpers for parent roles whose co-agent role
    // does not exist in the bindings at all (no co-agent ever assigned).
    for role in &roles {
        if parents_with_co.contains(role) {
            continue;
        }
        let is_co_role = role.contains("Co-Agent") || role.contains("Co-Personal Representative");
        if is_co_role {
            continue;
        }

        // Derive the expected co-role name from the parent role name
        let co_role_name = if role.contains("Personal Representative") {
            role.replace("Personal Representative", "Co-Personal Representative")
        } else if role.ends_with("Agent") {
            // "Primary HPOA Agent" → "Primary HPOA Co-Agent"
            format!("{}Co-Agent", &role[..role.len() - "Agent".len()])
        } else {
            continue;
        };

        let solo_title = if role.contains("HPOA") {
            "Healthcare Representative."
        } else if role.contains("FPOA") {
            "Agent."
        } else if role.contains("Personal Representative") {
            "Personal Representative."
        } else {
            "representative."
        };
        lily.variables
            .entry(format!("{} And Name", co_role_name))
            .or_insert_with(String::new);
        lily.variables
            .entry(format!("{} And Phone", co_role_name))
            .or_insert_with(String::new);
        lily.variables
            .entry(format!("{} Verb", role))
            .or_insert_with(|| "is".to_string());
        lily.variables
            .entry(format!("{} Title", role))
            .or_insert_with(|| solo_title.to_string());
    }

    write_lily_file(&working_dir, &lily)?;
    Ok(warnings)
}

/// Set or remove a per-document role override.
/// If `override_data` is `Some`, the role is overridden for this document.
/// If `None`, the override is removed (re-linking to the questionnaire).
#[tauri::command]
pub fn set_role_override(
    working_dir: String,
    filename: String,
    role: String,
    override_data: Option<RoleOverride>,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    let meta = lily
        .documents
        .get_mut(&filename)
        .ok_or_else(|| format!("Document '{}' not found in .lily file", filename))?;
    match override_data {
        Some(data) => {
            meta.role_overrides.insert(role, data);
        }
        None => {
            meta.role_overrides.remove(&role);
        }
    }
    write_lily_file(&working_dir, &lily)
}

/// Set per-document variable overrides.
/// Replaces the full overrides map for the given document.
#[tauri::command]
pub fn set_variable_overrides(
    working_dir: String,
    filename: String,
    overrides: HashMap<String, String>,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    let meta = lily
        .documents
        .get_mut(&filename)
        .ok_or_else(|| format!("Document '{}' not found in .lily file", filename))?;
    meta.variable_overrides = overrides;
    write_lily_file(&working_dir, &lily)
}

/// Save a questionnaire note (client or internal) for a specific section.
#[tauri::command]
pub fn save_questionnaire_note(
    working_dir: String,
    section: String,
    note_kind: String,
    value: String,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    let notes = lily
        .questionnaire_notes
        .entry(section)
        .or_default();
    match note_kind.as_str() {
        "client" => notes.client = value,
        "internal" => notes.internal = value,
        _ => return Err(format!("Invalid note kind: {}", note_kind)),
    }
    write_lily_file(&working_dir, &lily)
}

/// Stamp the questionnaire ID and version into the .lily file for a client.
#[tauri::command]
pub fn set_client_questionnaire(
    working_dir: String,
    questionnaire_id: String,
    questionnaire_version: u32,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.questionnaire_id = Some(questionnaire_id);
    lily.questionnaire_version = Some(questionnaire_version);
    write_lily_file(&working_dir, &lily)
}

/// A single field mapping for variable migration (old name → new name).
#[derive(Debug, Deserialize)]
pub struct FieldMapping {
    pub from: String,
    pub to: String,
}

/// Apply variable migration: copy values from old variable names to new ones,
/// optionally remove orphaned variables, and bump questionnaire_version.
/// Returns the updated LilyFile.
#[tauri::command]
pub fn apply_variable_migration(
    working_dir: String,
    mappings: Vec<FieldMapping>,
    remove_orphaned: Vec<String>,
    new_questionnaire_version: u32,
) -> Result<LilyFile, String> {
    let mut lily = read_lily_file(&working_dir)?;

    // Apply mappings: copy value from old key to new key
    for mapping in &mappings {
        if let Some(value) = lily.variables.get(&mapping.from).cloned() {
            lily.variables.insert(mapping.to.clone(), value);
        }
    }

    // Remove mapped source keys (they've been transferred)
    for mapping in &mappings {
        lily.variables.remove(&mapping.from);
    }

    // Remove orphaned variables the user chose to discard
    for key in &remove_orphaned {
        lily.variables.remove(key);
    }

    // Bump questionnaire version
    lily.questionnaire_version = Some(new_questionnaire_version);

    write_lily_file(&working_dir, &lily)?;
    Ok(lily)
}

/// Export client data as a JSON file to the given path.
#[tauri::command]
pub fn export_client_data(working_dir: String, export_path: String) -> Result<(), String> {
    let lily = read_lily_file(&working_dir)?;
    let content = serde_json::to_string_pretty(&lily)
        .map_err(|e| format!("Failed to serialize client data: {}", e))?;
    atomic_write(Path::new(&export_path), &content)
}

/// Zip the whole client folder, including the `.lily` source and every
/// document. The archive contains a top-level folder named after the client so
/// extracting it does not scatter files into the current directory.
#[tauri::command]
pub fn export_client_archive(working_dir: String, export_path: String) -> Result<(), String> {
    let directory = Path::new(&working_dir);
    if !directory.is_dir() {
        return Err(format!("Client folder not found: {}", directory.display()));
    }
    let archive = Path::new(&export_path);
    if archive.starts_with(directory) {
        return Err("Choose an export location outside the client folder.".to_string());
    }
    let file = fs::File::create(archive)
        .map_err(|error| format!("Failed to create archive: {}", error))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let root_name = directory
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("client");
    writer
        .add_directory(format!("{}/", root_name), options)
        .map_err(|error| format!("Failed to write archive: {}", error))?;
    add_archive_directory(&mut writer, directory, directory, root_name, options)?;
    writer
        .finish()
        .map_err(|error| format!("Failed to finish archive: {}", error))?;
    Ok(())
}

fn add_archive_directory(
    writer: &mut zip::ZipWriter<fs::File>,
    root: &Path,
    current: &Path,
    root_name: &str,
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    let entries = fs::read_dir(current)
        .map_err(|error| format!("Failed to read {}: {}", current.display(), error))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("Failed to read folder entry: {}", error))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("Failed to make archive path: {}", error))?;
        let name = format!(
            "{}/{}",
            root_name,
            relative.to_string_lossy().replace('\\', "/")
        );
        if path.is_dir() {
            writer
                .add_directory(format!("{}/", name), options)
                .map_err(|error| format!("Failed to write archive: {}", error))?;
            add_archive_directory(writer, root, &path, root_name, options)?;
        } else {
            let mut source = fs::File::open(&path)
                .map_err(|error| format!("Failed to read {}: {}", path.display(), error))?;
            writer
                .start_file(name, options)
                .map_err(|error| format!("Failed to write archive entry: {}", error))?;
            std::io::copy(&mut source, writer)
                .map_err(|error| format!("Failed to write archive data: {}", error))?;
        }
    }
    Ok(())
}

// ─── Document consistency check ─────────────────────────────────────────

/// Report of file consistency between the .lily metadata and the actual
/// files on disk in the working directory.
#[derive(Debug, Serialize)]
pub struct ConsistencyReport {
    /// Filenames in the .lily documents map that no longer exist on disk.
    pub missing_on_disk: Vec<String>,
    /// .docx files found on disk that are not tracked in the .lily file.
    pub untracked_on_disk: Vec<String>,
}

/// Compare the .lily file's document list against actual files on disk.
#[tauri::command]
pub fn check_document_consistency(working_dir: String) -> Result<ConsistencyReport, String> {
    let lily = read_lily_file(&working_dir)?;

    // Scan directory for actual .docx files
    let dir = Path::new(&working_dir);
    let actual_files: std::collections::HashSet<String> = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory: {}", e))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().ends_with(".docx") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let tracked: std::collections::HashSet<&str> =
        lily.documents.keys().map(|k| k.as_str()).collect();

    let missing_on_disk = tracked
        .iter()
        .filter(|name| !actual_files.contains(**name))
        .map(|s| s.to_string())
        .collect();

    let untracked_on_disk = actual_files
        .iter()
        .filter(|name| !tracked.contains(name.as_str()))
        .cloned()
        .collect();

    Ok(ConsistencyReport {
        missing_on_disk,
        untracked_on_disk,
    })
}

// ─── Required document CRUD ──────────────────────────────────────────────

/// Set the client's place in the engagement workflow.
#[tauri::command]
pub fn set_workflow_stage(working_dir: String, stage: WorkflowStage) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.workflow_stage = stage;
    write_lily_file(&working_dir, &lily)
}

/// Set who the current engagement stage is waiting on.
#[tauri::command]
pub fn set_workflow_waiting_on(
    working_dir: String,
    waiting_on: WorkflowWaitingOn,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.workflow_waiting_on = waiting_on;
    write_lily_file(&working_dir, &lily)
}

/// Link a drafted document to the selection entry for its template. If the
/// selection has not been recorded yet, add the entry. This is the bridge
/// between `required_documents` (the client's plan) and the actual `.docx`.
#[tauri::command]
pub fn ensure_required_document(
    working_dir: String,
    template_rel_path: String,
    document_filename: String,
    status: DocumentStatus,
) -> Result<RequiredDocument, String> {
    let mut lily = read_lily_file(&working_dir)?;
    let position = lily.required_documents.iter().position(|document| {
        document.template_rel_path == template_rel_path
            && document
                .document_filename
                .as_deref()
                .is_none_or(|filename| filename == document_filename)
    });
    let document = if let Some(index) = position {
        &mut lily.required_documents[index]
    } else {
        lily.required_documents.push(RequiredDocument {
            id: Uuid::new_v4().to_string(),
            template_rel_path: template_rel_path.clone(),
            status: status.clone(),
            document_filename: Some(document_filename.clone()),
            notes: String::new(),
        });
        lily.required_documents
            .last_mut()
            .expect("required document just pushed")
    };
    document.template_rel_path = template_rel_path;
    document.document_filename = Some(document_filename);
    document.status = status;
    let result = document.clone();
    write_lily_file(&working_dir, &lily)?;
    Ok(result)
}

/// Backfill the selection spine for clients created before required documents
/// were linked to their drafted files. Existing entries are linked rather than
/// duplicated, and the number of changed entries is returned.
#[tauri::command]
pub fn backfill_required_documents(working_dir: String) -> Result<usize, String> {
    let mut lily = read_lily_file(&working_dir)?;
    let documents: Vec<(String, String)> = lily
        .documents
        .iter()
        .map(|(filename, meta)| (filename.clone(), meta.template_rel_path.clone()))
        .collect();
    let mut changed = 0;
    for (filename, template_rel_path) in documents {
        let position = lily.required_documents.iter().position(|document| {
            document.template_rel_path == template_rel_path
                && document
                    .document_filename
                    .as_deref()
                    .is_none_or(|existing| existing == filename)
        });
        if let Some(index) = position {
            let document = &mut lily.required_documents[index];
            if document.document_filename.is_none() {
                document.document_filename = Some(filename);
                document.status = DocumentStatus::Drafting;
                changed += 1;
            }
        } else {
            lily.required_documents.push(RequiredDocument {
                id: Uuid::new_v4().to_string(),
                template_rel_path,
                status: DocumentStatus::Drafting,
                document_filename: Some(filename),
                notes: String::new(),
            });
            changed += 1;
        }
    }
    if changed > 0 {
        write_lily_file(&working_dir, &lily)?;
    }
    Ok(changed)
}

/// Add a required document to the client's .lily file.
#[tauri::command]
pub fn add_required_document(
    working_dir: String,
    template_rel_path: String,
    notes: String,
) -> Result<RequiredDocument, String> {
    let mut lily = read_lily_file(&working_dir)?;
    let doc = RequiredDocument {
        id: Uuid::new_v4().to_string(),
        template_rel_path,
        status: DocumentStatus::NotStarted,
        document_filename: None,
        notes,
    };
    lily.required_documents.push(doc.clone());
    write_lily_file(&working_dir, &lily)?;
    Ok(doc)
}

/// Update the status of a required document.
#[tauri::command]
pub fn update_required_document_status(
    working_dir: String,
    document_id: String,
    status: DocumentStatus,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    let doc = lily
        .required_documents
        .iter_mut()
        .find(|d| d.id == document_id)
        .ok_or_else(|| format!("Required document '{}' not found", document_id))?;
    doc.status = status;
    write_lily_file(&working_dir, &lily)
}

/// Update the notes of a required document.
#[tauri::command]
pub fn update_required_document_notes(
    working_dir: String,
    document_id: String,
    notes: String,
) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    let doc = lily
        .required_documents
        .iter_mut()
        .find(|d| d.id == document_id)
        .ok_or_else(|| format!("Required document '{}' not found", document_id))?;
    doc.notes = notes;
    write_lily_file(&working_dir, &lily)
}

/// Remove a required document by ID.
#[tauri::command]
pub fn remove_required_document(working_dir: String, document_id: String) -> Result<(), String> {
    let mut lily = read_lily_file(&working_dir)?;
    lily.required_documents.retain(|d| d.id != document_id);
    write_lily_file(&working_dir, &lily)
}

// ─── Status auto-detection ──────────────────────────────────────────────

/// Detect the status of a required document based on filesystem heuristics.
fn detect_single_status(
    working_dir: &str,
    req: &RequiredDocument,
    lily: &LilyFile,
) -> DocumentStatus {
    let dir = Path::new(working_dir);

    // If no document file linked or file doesn't exist → NotStarted
    let filename = match &req.document_filename {
        Some(f) if dir.join(f).exists() => f.clone(),
        _ => return DocumentStatus::NotStarted,
    };

    let basename = Path::new(&filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    // Check for EXECUTED PDF (case-insensitive "executed" in filename)
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".pdf")
                && name.contains("executed")
                && name.contains(&basename.to_lowercase())
            {
                return DocumentStatus::Executed;
            }
        }
    }

    // Check for regular PDF → Complete
    let pdf_name = format!("{}.pdf", basename);
    if dir.join(&pdf_name).exists() {
        return DocumentStatus::Complete;
    }

    // Check variable fill state
    if let Some(doc_meta) = lily.documents.get(&filename) {
        if !doc_meta.variable_names.is_empty() {
            let all_filled = doc_meta.variable_names.iter().all(|name| {
                lily.variables
                    .get(name)
                    .is_some_and(|v| !v.is_empty())
            });
            if all_filled {
                return DocumentStatus::Reviewing;
            }
        }
    }

    DocumentStatus::Drafting
}

/// Run auto-detection for all required documents, returning id + detected status.
#[tauri::command]
pub fn detect_document_statuses(
    working_dir: String,
) -> Result<Vec<(String, DocumentStatus)>, String> {
    let lily = read_lily_file(&working_dir)?;
    let results: Vec<(String, DocumentStatus)> = lily
        .required_documents
        .iter()
        .map(|req| {
            let status = detect_single_status(&working_dir, req, &lily);
            (req.id.clone(), status)
        })
        .collect();
    Ok(results)
}

// ─── Client summary for aggregate views ─────────────────────────────────

/// Lightweight summary of a client for the Clients module.
#[derive(Debug, Serialize)]
pub struct ClientSummary {
    pub directory: String,
    pub client_name: String,
    pub total_documents: usize,
    pub required_documents: Vec<RequiredDocumentSummary>,
    pub contacts_count: usize,
    pub has_questionnaire: bool,
}

/// Lightweight summary of a required document's status.
#[derive(Debug, Serialize)]
pub struct RequiredDocumentSummary {
    pub template_rel_path: String,
    pub status: DocumentStatus,
    pub document_filename: Option<String>,
}

/// Extract the folder name from a directory path.
fn folder_name(dir: &str) -> String {
    Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Build a ClientSummary from a .lily file in the given directory.
/// Returns None if the directory doesn't exist or has no .lily file data.
fn summarize_client(directory: &str) -> Option<ClientSummary> {
    let lily = read_lily_file(directory).ok()?;
    Some(ClientSummary {
        directory: directory.to_string(),
        client_name: folder_name(directory),
        total_documents: lily.documents.len(),
        required_documents: lily
            .required_documents
            .iter()
            .map(|r| RequiredDocumentSummary {
                template_rel_path: r.template_rel_path.clone(),
                status: r.status.clone(),
                document_filename: r.document_filename.clone(),
            })
            .collect(),
        contacts_count: lily.contacts.len(),
        has_questionnaire: lily.questionnaire_id.is_some(),
    })
}

/// Load summaries for multiple client directories.
#[tauri::command]
pub fn load_client_summaries(directories: Vec<String>) -> Vec<ClientSummary> {
    directories
        .iter()
        .filter_map(|dir| summarize_client(dir))
        .collect()
}

/// Discover clients in a library directory by scanning for subdirectories
/// containing `.lily` files.
#[tauri::command]
pub fn list_clients_in_library(library_dir: String) -> Result<Vec<ClientSummary>, String> {
    let path = Path::new(&library_dir);
    if !path.is_dir() {
        return Err(format!("Not a directory: {}", library_dir));
    }

    let entries =
        fs::read_dir(path).map_err(|e| format!("Failed to read directory: {}", e))?;

    let mut summaries = Vec::new();
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }
        let dir_str = entry_path.to_string_lossy().to_string();
        // Check if this subdirectory has a .lily file
        if let Ok(lily_files) = find_lily_files(&dir_str) {
            if !lily_files.is_empty() {
                if let Some(summary) = summarize_client(&dir_str) {
                    summaries.push(summary);
                }
            }
        }
    }
    summaries.sort_by(|a, b| a.client_name.cmp(&b.client_name));
    Ok(summaries)
}

// ─── Client library folder tree ──────────────────────────────────────────

/// A node in the client library folder tree.
/// Every subdirectory is included (even those without .lily files).
#[derive(Debug, Serialize)]
pub struct ClientTreeNode {
    pub name: String,
    pub path: String,
    pub is_client: bool,
    pub client_summary: Option<ClientSummary>,
    pub children: Vec<ClientTreeNode>,
}

/// Recursively build a tree of all subdirectories under the given path.
fn build_library_tree(dir: &Path) -> Vec<ClientTreeNode> {
    let mut nodes = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return nodes,
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let name = entry_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Skip hidden directories
        if name.starts_with('.') {
            continue;
        }

        let dir_str = entry_path.to_string_lossy().to_string();
        let has_lily = find_lily_files(&dir_str)
            .map(|files| !files.is_empty())
            .unwrap_or(false);

        let summary = if has_lily {
            summarize_client(&dir_str)
        } else {
            None
        };

        let children = build_library_tree(&entry_path);

        nodes.push(ClientTreeNode {
            name,
            path: dir_str,
            is_client: has_lily,
            client_summary: summary,
            children,
        });
    }

    // Sort: folders with children first, then alphabetical
    nodes.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    nodes
}

/// List all subdirectories in a library directory as a tree structure.
/// Includes all folders, even those without .lily files.
#[tauri::command]
pub fn list_library_tree(library_dir: String) -> Result<Vec<ClientTreeNode>, String> {
    let path = Path::new(&library_dir);
    if !path.is_dir() {
        return Err(format!("Not a directory: {}", library_dir));
    }
    Ok(build_library_tree(path))
}

// ─── Export / Import ─────────────────────────────────────────────────────

/// Import client data from a JSON file, merging into the existing .lily file.
/// Variables, contacts, and contact bindings from the import are merged (import wins on conflict).
#[tauri::command]
pub fn import_client_data(working_dir: String, import_path: String) -> Result<LilyFile, String> {
    let content = fs::read_to_string(&import_path)
        .map_err(|e| format!("Failed to read import file: {}", e))?;
    let imported: LilyFile = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse import file: {}", e))?;

    let mut lily = read_lily_file(&working_dir)?;

    // Merge variables (import wins on conflict)
    for (key, value) in imported.variables {
        lily.variables.insert(key, value);
    }

    // Merge contacts (skip duplicates by id)
    let existing_ids: std::collections::HashSet<String> =
        lily.contacts.iter().map(|c| c.id.clone()).collect();
    for contact in imported.contacts {
        if !existing_ids.contains(&contact.id) {
            lily.contacts.push(contact);
        }
    }

    // Merge contact bindings (import wins on conflict)
    for (role, binding) in imported.contact_bindings {
        lily.contact_bindings.insert(role, binding);
    }

    // Merge questionnaire notes (import wins on conflict)
    for (section, notes) in imported.questionnaire_notes {
        lily.questionnaire_notes.insert(section, notes);
    }

    write_lily_file(&working_dir, &lily)?;
    read_lily_file(&working_dir)
}

// ─── Copy from spouse ────────────────────────────────────────────────────

/// `Client *` variables that swap with the promoted spouse contact's
/// properties. The first element is the variable name; the second is the
/// matching `Contact` property key recognized by `get_contact_property`.
const CLIENT_VAR_TO_CONTACT_PROP: &[(&str, &str)] = &[
    ("Client Full Name", "full_name"),
    ("Client First Name", "first_name"),
    ("Client Middle Name", "middle_name"),
    ("Client Last Name", "last_name"),
    ("Client Phone", "phone"),
    ("Client Email", "email"),
    ("Client Address", "address"),
    ("Client City", "city"),
    ("Client State", "state"),
    ("Client Zip", "zip"),
];

/// Variables that `resolve_contact_variables` regenerates from contacts.
/// Carrying these from the source would briefly seed stale values; instead
/// we drop them and let resolve_contact_variables repopulate.
const DERIVED_VARS_TO_DROP: &[&str] = &[
    "Client Spouse Name",
    "Client Children Full Names",
    "Client has Children",
    "Has Minor Children",
];

/// True if the variable name is a derived helper that
/// `resolve_contact_variables` regenerates (so we should drop it on copy).
fn is_derived_helper_var(name: &str) -> bool {
    name.starts_with("Has ")
        || name.ends_with(" And Name")
        || name.ends_with(" And Phone")
        || name.ends_with(" Verb")
        || name.ends_with(" Title")
        || DERIVED_VARS_TO_DROP.contains(&name)
}

/// True if a `LilyFile` has no user-meaningful content — i.e., looks like a
/// freshly-created or default project file. A freshly-created client now always
/// has a `questionnaire_id`/`questionnaire_version` stamped at creation (the
/// Create Client pane pre-selects one), so those fields don't count as content;
/// only actual client data does.
fn is_empty_project(lily: &LilyFile) -> bool {
    lily.variables.is_empty()
        && lily.documents.is_empty()
        && lily.contacts.is_empty()
        && lily.contact_bindings.is_empty()
        && lily.questionnaire_notes.is_empty()
        && lily.required_documents.is_empty()
}

/// Build the swapped `LilyFile` for the target, given the source and the
/// chosen spouse contact. Pure: does no I/O. The new spouse contact (the
/// original client's "demoted" entry) gets `new_spouse_id` as its UUID so
/// callers can retarget bindings predictably.
fn build_swapped_lily(source: &LilyFile, spouse: &Contact, new_spouse_id: &str) -> LilyFile {
    let mut new_lily = LilyFile::default();

    // Copy variables, dropping client-identity vars (we'll set fresh below)
    // and derived helpers (`resolve_contact_variables` regenerates them).
    for (k, v) in &source.variables {
        if CLIENT_VAR_TO_CONTACT_PROP.iter().any(|(name, _)| name == k) {
            continue;
        }
        if is_derived_helper_var(k) {
            continue;
        }
        new_lily.variables.insert(k.clone(), v.clone());
    }

    // Promote spouse contact's properties into Client * variables.
    for (var_name, prop_key) in CLIENT_VAR_TO_CONTACT_PROP {
        let value = get_contact_property(spouse, prop_key);
        new_lily.variables.insert(var_name.to_string(), value);
    }

    // Carry over all contacts except the promoted spouse, preserving IDs.
    // Then add a new contact representing the original client (relationship
    // = "Spouse"), built from source's `Client *` variables.
    for c in &source.contacts {
        if c.id == spouse.id {
            continue;
        }
        new_lily.contacts.push(c.clone());
    }
    let original_client_as_spouse = Contact {
        id: new_spouse_id.to_string(),
        full_name: source
            .variables
            .get("Client Full Name")
            .cloned()
            .unwrap_or_default(),
        first_name: source
            .variables
            .get("Client First Name")
            .cloned()
            .unwrap_or_default(),
        middle_name: source
            .variables
            .get("Client Middle Name")
            .cloned()
            .unwrap_or_default(),
        last_name: source
            .variables
            .get("Client Last Name")
            .cloned()
            .unwrap_or_default(),
        relationship: "Spouse".to_string(),
        other_relationship: String::new(),
        phone: source
            .variables
            .get("Client Phone")
            .cloned()
            .unwrap_or_default(),
        email: source
            .variables
            .get("Client Email")
            .cloned()
            .unwrap_or_default(),
        address: source
            .variables
            .get("Client Address")
            .cloned()
            .unwrap_or_default(),
        city: source
            .variables
            .get("Client City")
            .cloned()
            .unwrap_or_default(),
        state: source
            .variables
            .get("Client State")
            .cloned()
            .unwrap_or_default(),
        zip: source
            .variables
            .get("Client Zip")
            .cloned()
            .unwrap_or_default(),
        is_minor: false,
    };
    new_lily.contacts.push(original_client_as_spouse);

    // Copy bindings, retargeting any that pointed at the promoted spouse.
    for (role, binding) in &source.contact_bindings {
        let mut new_binding = binding.clone();
        if new_binding.contact_id.as_deref() == Some(&spouse.id) {
            new_binding.contact_id = Some(new_spouse_id.to_string());
        }
        new_lily.contact_bindings.insert(role.clone(), new_binding);
    }

    new_lily.questionnaire_id = source.questionnaire_id.clone();
    new_lily.questionnaire_version = source.questionnaire_version;
    new_lily.questionnaire_notes = source.questionnaire_notes.clone();

    // Required documents: new IDs, reset status. Keep document_filename so
    // recreated docs (same filenames) stay linked.
    for req in &source.required_documents {
        new_lily.required_documents.push(RequiredDocument {
            id: Uuid::new_v4().to_string(),
            template_rel_path: req.template_rel_path.clone(),
            status: DocumentStatus::NotStarted,
            document_filename: req.document_filename.clone(),
            notes: req.notes.clone(),
        });
    }

    new_lily
}

/// Result of a copy-from-spouse operation.
#[derive(Debug, Serialize)]
pub struct CopyFromSpouseResult {
    /// The fully resolved target `.lily` file after the swap and document
    /// recreation.
    pub lily: LilyFile,
    /// Filenames successfully recreated in the target directory.
    pub copied_documents: Vec<String>,
    /// Filenames that could not be recreated (e.g., template missing).
    pub skipped_documents: Vec<String>,
    /// Non-fatal warnings to surface to the user.
    pub warnings: Vec<String>,
}

/// Rename a copied document filename so it reflects the target client's
/// name instead of the source client's. Tries the full "First Last" form
/// first (most common, produced by the frontend's `buildDocumentFilename`),
/// then a first-name-only fallback, then a last-name-only fallback. Returns
/// the original filename unchanged if no source name appears in it.
///
/// Only the first occurrence is replaced (matches `replacen(..., 1)`), so
/// names that recur elsewhere in the filename — unlikely but possible —
/// won't be double-substituted.
fn swap_client_name_in_filename(
    filename: &str,
    source_first: &str,
    source_last: &str,
    new_first: &str,
    new_last: &str,
) -> String {
    let source_full = format!("{} {}", source_first.trim(), source_last.trim());
    let new_full = format!("{} {}", new_first.trim(), new_last.trim());

    if !source_first.trim().is_empty()
        && !source_last.trim().is_empty()
        && filename.contains(&source_full)
    {
        return filename.replacen(&source_full, &new_full, 1);
    }
    if !source_first.trim().is_empty()
        && !new_first.trim().is_empty()
        && filename.contains(source_first.trim())
    {
        return filename.replacen(source_first.trim(), new_first.trim(), 1);
    }
    if !source_last.trim().is_empty()
        && !new_last.trim().is_empty()
        && filename.contains(source_last.trim())
    {
        return filename.replacen(source_last.trim(), new_last.trim(), 1);
    }
    filename.to_string()
}

/// Ensure `filename` doesn't collide with anything in `dir` or with names
/// already in `taken`. If it does, appends ` (2)`, ` (3)`, ... before the
/// extension. Records the chosen name in `taken`.
fn dedupe_filename(
    dir: &Path,
    filename: &str,
    taken: &mut std::collections::HashSet<String>,
) -> String {
    let original = Path::new(filename);
    let stem = original
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = original
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let mut candidate = filename.to_string();
    let mut counter = 2u32;
    while taken.contains(&candidate) || dir.join(&candidate).exists() {
        candidate = format!("{} ({}){}", stem, counter, ext);
        counter += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// Resolve which contact in `source` should be promoted to the client of
/// `target`. If `spouse_contact_id` is provided, verify it matches a real
/// contact. Otherwise auto-pick the single contact with relationship
/// "Spouse"; error if there's not exactly one.
fn resolve_spouse_contact<'a>(
    source: &'a LilyFile,
    spouse_contact_id: Option<&str>,
) -> Result<&'a Contact, String> {
    if let Some(id) = spouse_contact_id {
        return source
            .contacts
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("Contact '{}' not found in source .lily", id));
    }
    let candidates: Vec<&Contact> = source
        .contacts
        .iter()
        .filter(|c| c.relationship.eq_ignore_ascii_case("Spouse"))
        .collect();
    match candidates.len() {
        0 => Err("Source .lily has no contact with relationship 'Spouse'".to_string()),
        1 => Ok(candidates[0]),
        n => Err(format!(
            "Source .lily has {} 'Spouse' contacts; specify spouse_contact_id to choose",
            n
        )),
    }
}

/// Recreate a single document in the target directory from its source
/// template through the plan engine. Returns `Ok(())` on success or
/// `Err(message)` to record as a warning.
fn recreate_document_from_template(
    target_dir: &str,
    templates_dir: &str,
    filename: &str,
    template_rel_path: &str,
) -> Result<(), String> {
    let template_full = Path::new(templates_dir).join(template_rel_path);
    if !template_full.exists() {
        return Err(format!(
            "template not found at '{}'",
            template_rel_path
        ));
    }
    let template_path_str = template_full.to_string_lossy().to_string();
    let schema = crate::docx_ops::load_template_schema(
        templates_dir.to_string(),
        template_rel_path.to_string(),
    )?;
    if schema.engine.as_deref() != Some("plan") {
        return Err(format!(
            "template '{}' is not plan-driven",
            template_rel_path
        ));
    }
    let plan_path = schema
        .plan_path
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("template '{}' has no plan path", template_rel_path))?;
    let plan_full = Path::new(templates_dir).join(plan_path);
    if !plan_full.is_file() {
        return Err(format!("plan not found at '{}'", plan_path));
    }

    let docx_path = crate::docx_ops::copy_template(
        template_path_str,
        target_dir.to_string(),
        filename.to_string(),
        template_rel_path.to_string(),
    )?;

    let mut variable_names: Vec<String> = schema.variables.keys().cloned().collect();
    variable_names.sort_by_key(|name| name.to_lowercase());
    {
        let mut tl = read_lily_file(target_dir)?;
        if let Some(meta) = tl.documents.get_mut(filename) {
            meta.variable_names = variable_names;
        }
        write_lily_file(target_dir, &tl)?;
    }

    let report = crate::plan::render_plan(
        &plan_full,
        Path::new(target_dir),
        &template_full,
        Path::new(&docx_path),
        Some(Path::new(filename)),
    )?;
    set_document_provenance(target_dir, filename, report.provenance)?;
    Ok(())
}

/// Create a new client `.lily` in `target_dir` by promoting a Spouse
/// contact in `source_dir` to the client of the new file (and demoting the
/// original client to a Spouse contact). Carries over questionnaire data,
/// non-spouse contacts, contact bindings, and required documents; recreates
/// each of source's documents from template with the swapped variable
/// values. Manual edits made to source's `.docx` files are NOT preserved.
#[tauri::command]
pub fn copy_from_spouse_lily(
    target_dir: String,
    source_dir: String,
    spouse_contact_id: Option<String>,
    templates_dir: String,
) -> Result<CopyFromSpouseResult, String> {
    info!(%source_dir, %target_dir, "Copying from spouse");

    if source_dir == target_dir {
        return Err("Source and target directories must be different".to_string());
    }

    // Guard: target must not already have a non-empty .lily project.
    let existing = read_lily_file(&target_dir)?;
    if !is_empty_project(&existing) {
        return Err(
            "Target directory already has a non-empty .lily project. Choose an empty folder."
                .to_string(),
        );
    }

    let source = read_lily_file(&source_dir)?;
    let spouse = resolve_spouse_contact(&source, spouse_contact_id.as_deref())?;
    let new_spouse_id = Uuid::new_v4().to_string();
    let new_lily = build_swapped_lily(&source, spouse, &new_spouse_id);

    // Persist the swapped .lily and let resolve_contact_variables compute
    // derived values (Client Spouse Name, Has *, co-agent helpers).
    write_lily_file(&target_dir, &new_lily)?;
    resolve_contact_variables(target_dir.clone())?;

    // Re-read so we have the resolved variable pool to feed into document
    // recreation.
    let resolved = read_lily_file(&target_dir)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut copied_documents: Vec<String> = Vec::new();
    let mut skipped_documents: Vec<String> = Vec::new();

    // Build name-swap inputs: source filenames embed the source client's
    // name (per `buildDocumentFilename` on the frontend), and we want them
    // to embed the new client's name in the target.
    let source_first = source
        .variables
        .get("Client First Name")
        .cloned()
        .unwrap_or_default();
    let source_last = source
        .variables
        .get("Client Last Name")
        .cloned()
        .unwrap_or_default();
    let new_first = resolved
        .variables
        .get("Client First Name")
        .cloned()
        .unwrap_or_default();
    let new_last = resolved
        .variables
        .get("Client Last Name")
        .cloned()
        .unwrap_or_default();

    // Recreate each document from template with the swapped variables.
    // Iterate in stable sorted order so warnings/output are deterministic.
    let mut doc_entries: Vec<(&String, &DocumentMeta)> = source.documents.iter().collect();
    doc_entries.sort_by(|a, b| a.0.cmp(b.0));

    let target_path = Path::new(&target_dir);
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Old → new filename map so we can update required_documents.document_filename.
    let mut renamed: HashMap<String, String> = HashMap::new();

    for (filename, meta) in doc_entries {
        let swapped = swap_client_name_in_filename(
            filename,
            &source_first,
            &source_last,
            &new_first,
            &new_last,
        );
        let final_name = dedupe_filename(target_path, &swapped, &mut taken);

        match recreate_document_from_template(
            &target_dir,
            &templates_dir,
            &final_name,
            &meta.template_rel_path,
        ) {
            Ok(()) => {
                if &final_name != filename {
                    renamed.insert(filename.clone(), final_name.clone());
                }
                copied_documents.push(final_name);
            }
            Err(msg) => {
                warn!(%filename, %msg, "Skipped document during copy-from-spouse");
                warnings.push(format!("Skipped '{}' — {}", filename, msg));
                skipped_documents.push(filename.clone());
            }
        }
    }

    // Retarget required_documents to the renamed copies so status detection
    // stays linked to the actual files on disk.
    if !renamed.is_empty() {
        let mut tl = read_lily_file(&target_dir)?;
        for req in tl.required_documents.iter_mut() {
            if let Some(old_name) = req.document_filename.clone() {
                if let Some(new_name) = renamed.get(&old_name) {
                    req.document_filename = Some(new_name.clone());
                }
            }
        }
        write_lily_file(&target_dir, &tl)?;
    }

    let final_lily = read_lily_file(&target_dir)?;
    Ok(CopyFromSpouseResult {
        lily: final_lily,
        copied_documents,
        skipped_documents,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_contact(id: &str, first: &str, last: &str, relationship: &str) -> Contact {
        Contact {
            id: id.to_string(),
            full_name: format!("{} {}", first, last),
            first_name: first.to_string(),
            middle_name: String::new(),
            last_name: last.to_string(),
            relationship: relationship.to_string(),
            other_relationship: String::new(),
            phone: format!("555-{}", first),
            email: format!("{}@x", first.to_lowercase()),
            address: format!("{} St", first),
            city: "Denver".to_string(),
            state: "CO".to_string(),
            zip: "80202".to_string(),
            is_minor: false,
        }
    }

    fn source_with_jane_and_john() -> (LilyFile, Contact) {
        // Jane is the client; John is her spouse.
        let mut lily = LilyFile::default();
        lily.variables.insert("Client Full Name".into(), "Jane Doe".into());
        lily.variables.insert("Client First Name".into(), "Jane".into());
        lily.variables.insert("Client Last Name".into(), "Doe".into());
        lily.variables.insert("Client Phone".into(), "555-Jane".into());
        lily.variables.insert("Client Email".into(), "jane@x".into());
        lily.variables.insert("Client Address".into(), "Jane St".into());
        lily.variables.insert("Client City".into(), "Denver".into());
        lily.variables.insert("Client State".into(), "CO".into());
        lily.variables.insert("Client Zip".into(), "80202".into());
        // Custom user variable that should carry over.
        lily.variables.insert("Engagement Date".into(), "2024-06-01".into());
        // Derived helpers from a previous resolve — these should be dropped.
        lily.variables.insert("Has Spouse".into(), "true".into());
        lily.variables.insert("Client Spouse Name".into(), "John Doe".into());
        lily.variables.insert("Primary HPOA Agent Title".into(), "Healthcare Representative.".into());

        let john = make_contact("john-id", "John", "Doe", "Spouse");
        let kid = make_contact("kid-id", "Kelly", "Doe", "Child");
        lily.contacts.push(john.clone());
        lily.contacts.push(kid);

        // A binding that references the spouse directly — should retarget.
        let mut spouse_binding = ContactBinding {
            contact_id: Some("john-id".into()),
            variable_mappings: HashMap::new(),
            contact_ids: None,
            assignments: None,
        };
        spouse_binding
            .variable_mappings
            .insert("Primary HPOA Agent Full Name".into(), "full_name".into());
        lily.contact_bindings
            .insert("Primary HPOA Agent".into(), spouse_binding);
        // A binding that references a non-spouse — should pass through unchanged.
        let mut other_binding = ContactBinding {
            contact_id: Some("kid-id".into()),
            variable_mappings: HashMap::new(),
            contact_ids: None,
            assignments: None,
        };
        other_binding
            .variable_mappings
            .insert("Personal Representative Full Name".into(), "full_name".into());
        lily.contact_bindings
            .insert("Personal Representative".into(), other_binding);

        lily.questionnaire_id = Some("elder-law".into());
        lily.questionnaire_version = Some(7);
        lily.questionnaire_notes.insert(
            "Family".into(),
            SectionNotes {
                client: "client note".into(),
                internal: "internal note".into(),
            },
        );

        lily.required_documents.push(RequiredDocument {
            id: "req-1".into(),
            template_rel_path: "Estate/Will.docx".into(),
            status: DocumentStatus::Complete,
            document_filename: Some("Will.docx".into()),
            notes: "n".into(),
        });

        (lily, john)
    }

    #[test]
    fn aggregate_contact_list_values_joins_and_skips() {
        let contacts = vec![
            make_contact("a", "Ada", "Smith", "Friend"),
            make_contact("b", "Bo", "Jones", "Friend"),
            make_contact("c", "Cy", "Lee", "Friend"),
        ];

        // Selected in order, with one id (the middle) skipped — order is
        // preserved and only the chosen contacts are included.
        let ids = vec!["c".to_string(), "a".to_string()];
        let values = aggregate_contact_list_values(&contacts, &ids, "full_name");
        assert_eq!(values, vec!["Cy Lee".to_string(), "Ada Smith".to_string()]);
        assert_eq!(values.join("; "), "Cy Lee; Ada Smith");

        // Missing IDs and empty property values are skipped.
        let mut blank = make_contact("d", "Di", "Poe", "Friend");
        blank.full_name = String::new();
        let mut contacts2 = contacts.clone();
        contacts2.push(blank);
        let ids2 = vec!["missing".to_string(), "d".to_string(), "b".to_string()];
        let values2 = aggregate_contact_list_values(&contacts2, &ids2, "full_name");
        assert_eq!(values2, vec!["Bo Jones".to_string()]);

        // No selections → empty (caller treats this as "Has {role}" == false).
        assert!(aggregate_contact_list_values(&contacts, &[], "full_name").is_empty());
    }

    #[test]
    fn is_derived_helper_var_classification() {
        assert!(is_derived_helper_var("Has Spouse"));
        assert!(is_derived_helper_var("Has Minor Children"));
        assert!(is_derived_helper_var("Client Spouse Name"));
        assert!(is_derived_helper_var("Primary HPOA Agent Title"));
        assert!(is_derived_helper_var("Primary HPOA Co-Agent And Name"));
        assert!(!is_derived_helper_var("Engagement Date"));
        assert!(!is_derived_helper_var("Client Full Name"));
    }

    #[test]
    fn build_swapped_lily_promotes_spouse_to_client() {
        let (source, john) = source_with_jane_and_john();
        let new_id = "new-spouse-uuid";
        let result = build_swapped_lily(&source, &john, new_id);

        // Client * variables now reflect John's properties.
        assert_eq!(result.variables.get("Client Full Name").unwrap(), "John Doe");
        assert_eq!(result.variables.get("Client First Name").unwrap(), "John");
        assert_eq!(result.variables.get("Client Phone").unwrap(), "555-John");
        // Custom variable carries over.
        assert_eq!(result.variables.get("Engagement Date").unwrap(), "2024-06-01");
        // Derived helpers are dropped (will be regenerated by resolve).
        assert!(!result.variables.contains_key("Has Spouse"));
        assert!(!result.variables.contains_key("Client Spouse Name"));
        assert!(!result.variables.contains_key("Primary HPOA Agent Title"));
    }

    #[test]
    fn build_swapped_lily_demotes_client_into_spouse_contact() {
        let (source, john) = source_with_jane_and_john();
        let new_id = "new-spouse-uuid";
        let result = build_swapped_lily(&source, &john, new_id);

        // John (the original spouse) is no longer in contacts.
        assert!(result.contacts.iter().all(|c| c.id != "john-id"));
        // The non-spouse contact (Kelly) keeps her ID.
        assert!(result.contacts.iter().any(|c| c.id == "kid-id"));
        // A new "Spouse" contact exists with the new ID and Jane's data.
        let new_spouse = result
            .contacts
            .iter()
            .find(|c| c.id == new_id)
            .expect("new spouse contact should be present");
        assert_eq!(new_spouse.relationship, "Spouse");
        assert_eq!(new_spouse.full_name, "Jane Doe");
        assert_eq!(new_spouse.first_name, "Jane");
        assert_eq!(new_spouse.last_name, "Doe");
        assert_eq!(new_spouse.phone, "555-Jane");
    }

    #[test]
    fn build_swapped_lily_retargets_bindings_pointing_at_promoted_spouse() {
        let (source, john) = source_with_jane_and_john();
        let new_id = "new-spouse-uuid";
        let result = build_swapped_lily(&source, &john, new_id);

        // The binding that pointed at the promoted spouse is retargeted.
        let hpoa = result
            .contact_bindings
            .get("Primary HPOA Agent")
            .expect("Primary HPOA Agent binding should be present");
        assert_eq!(hpoa.contact_id.as_deref(), Some(new_id));
        // Bindings pointing elsewhere are unchanged.
        let pr = result
            .contact_bindings
            .get("Personal Representative")
            .expect("Personal Representative binding should be present");
        assert_eq!(pr.contact_id.as_deref(), Some("kid-id"));
    }

    #[test]
    fn build_swapped_lily_carries_questionnaire_and_resets_required_docs() {
        let (source, john) = source_with_jane_and_john();
        let result = build_swapped_lily(&source, &john, "new-id");

        assert_eq!(result.questionnaire_id.as_deref(), Some("elder-law"));
        assert_eq!(result.questionnaire_version, Some(7));
        assert_eq!(
            result
                .questionnaire_notes
                .get("Family")
                .map(|n| n.client.as_str()),
            Some("client note")
        );

        assert_eq!(result.required_documents.len(), 1);
        let req = &result.required_documents[0];
        assert_ne!(req.id, "req-1", "required document id should be regenerated");
        assert_eq!(req.status, DocumentStatus::NotStarted);
        assert_eq!(req.template_rel_path, "Estate/Will.docx");
        assert_eq!(req.document_filename.as_deref(), Some("Will.docx"));
    }

    #[test]
    fn build_swapped_lily_drops_documents_and_overrides() {
        let (mut source, john) = source_with_jane_and_john();
        source.documents.insert(
            "Will.docx".into(),
            DocumentMeta {
                template_rel_path: "Estate/Will.docx".into(),
                created_at: Utc::now(),
                modified_at: Utc::now(),
                variable_names: vec!["Client Full Name".into()],
                role_overrides: HashMap::new(),
                variable_overrides: HashMap::new(),
                provenance: None,
            },
        );

        let result = build_swapped_lily(&source, &john, "new-id");
        assert!(result.documents.is_empty());
    }

    #[test]
    fn resolve_spouse_contact_auto_picks_single_spouse() {
        let (source, _) = source_with_jane_and_john();
        let picked = resolve_spouse_contact(&source, None).unwrap();
        assert_eq!(picked.id, "john-id");
    }

    #[test]
    fn resolve_spouse_contact_errors_when_multiple_spouses_without_id() {
        let (mut source, _) = source_with_jane_and_john();
        source.contacts.push(make_contact("ex-id", "Alex", "Ex", "Spouse"));
        let err = resolve_spouse_contact(&source, None).unwrap_err();
        assert!(err.contains("2"), "error should mention count: {}", err);
    }

    #[test]
    fn resolve_spouse_contact_uses_provided_id_over_relationship() {
        let (mut source, _) = source_with_jane_and_john();
        // A non-Spouse contact picked explicitly should still resolve.
        source
            .contacts
            .push(make_contact("partner-id", "Pat", "Partner", "Other"));
        let picked = resolve_spouse_contact(&source, Some("partner-id")).unwrap();
        assert_eq!(picked.id, "partner-id");
    }

    #[test]
    fn resolve_spouse_contact_errors_for_unknown_id() {
        let (source, _) = source_with_jane_and_john();
        let err = resolve_spouse_contact(&source, Some("nope")).unwrap_err();
        assert!(err.to_lowercase().contains("not found"));
    }

    #[test]
    fn swap_client_name_in_filename_full_match() {
        let out = swap_client_name_in_filename(
            "GPOA - Jack Doe.docx",
            "Jack",
            "Doe",
            "Jane",
            "Doe",
        );
        assert_eq!(out, "GPOA - Jane Doe.docx");
    }

    #[test]
    fn swap_client_name_in_filename_different_last_names() {
        let out = swap_client_name_in_filename(
            "Will - Jack Smith.docx",
            "Jack",
            "Smith",
            "Jane",
            "Doe",
        );
        assert_eq!(out, "Will - Jane Doe.docx");
    }

    #[test]
    fn swap_client_name_in_filename_first_name_only() {
        let out = swap_client_name_in_filename(
            "GPOA - Jack.docx",
            "Jack",
            "Doe",
            "Jane",
            "Doe",
        );
        assert_eq!(out, "GPOA - Jane.docx");
    }

    #[test]
    fn swap_client_name_in_filename_unchanged_when_no_match() {
        let out = swap_client_name_in_filename(
            "Estate Plan.docx",
            "Jack",
            "Doe",
            "Jane",
            "Doe",
        );
        assert_eq!(out, "Estate Plan.docx");
    }

    #[test]
    fn swap_client_name_in_filename_unchanged_when_source_names_blank() {
        let out =
            swap_client_name_in_filename("GPOA - Jack Doe.docx", "", "", "Jane", "Doe");
        assert_eq!(out, "GPOA - Jack Doe.docx");
    }

    #[test]
    fn swap_client_name_in_filename_replaces_only_first_occurrence() {
        // Defensive: a name appearing twice (very unusual) should only be
        // replaced once so we never produce a filename with mixed casing or
        // double-substituted text.
        let out = swap_client_name_in_filename(
            "Jack - For Jack.docx",
            "Jack",
            "Doe",
            "Jane",
            "Doe",
        );
        assert_eq!(out, "Jane - For Jack.docx");
    }

    #[test]
    fn dedupe_filename_appends_suffix_on_collision() {
        use std::collections::HashSet;
        let dir = std::env::temp_dir();
        let mut taken: HashSet<String> = HashSet::new();
        taken.insert("GPOA - Jane Doe.docx".to_string());

        let out = dedupe_filename(&dir, "GPOA - Jane Doe.docx", &mut taken);
        assert_eq!(out, "GPOA - Jane Doe (2).docx");
        assert!(taken.contains("GPOA - Jane Doe (2).docx"));
    }

    #[test]
    fn dedupe_filename_passes_through_when_unique() {
        use std::collections::HashSet;
        let dir = std::env::temp_dir();
        let mut taken: HashSet<String> = HashSet::new();
        let out = dedupe_filename(&dir, "Unique-12345abcdef.docx", &mut taken);
        assert_eq!(out, "Unique-12345abcdef.docx");
    }

    #[test]
    fn is_empty_project_recognizes_default_lily() {
        let empty = LilyFile::default();
        assert!(is_empty_project(&empty));

        let mut not_empty = LilyFile::default();
        not_empty.variables.insert("X".into(), "y".into());
        assert!(!is_empty_project(&not_empty));

        // A freshly-created client is stamped with a questionnaire id/version at
        // creation but has no actual content — it must still count as empty so
        // copy-from-spouse can target it.
        let mut fresh = LilyFile::default();
        fresh.questionnaire_id = Some("3c74a6ef-1edc-4fc1-8a3e-4971a9072089".into());
        fresh.questionnaire_version = Some(4);
        assert!(is_empty_project(&fresh));
    }

    #[test]
    fn exports_the_client_folder_as_a_zip() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let client = std::env::temp_dir().join(format!("lily-export-{}", unique));
        std::fs::create_dir_all(&client).expect("client folder");
        std::fs::write(client.join("Client.lily"), b"{}").expect("lily");
        std::fs::write(client.join("Will.docx"), b"docx").expect("docx");
        let archive = std::env::temp_dir().join(format!("lily-export-{}.zip", unique));
        export_client_archive(
            client.to_string_lossy().to_string(),
            archive.to_string_lossy().to_string(),
        )
        .expect("archive");
        let file = std::fs::File::open(&archive).expect("archive file");
        let mut zip = zip::ZipArchive::new(file).expect("zip");
        let names: Vec<String> = (0..zip.len())
            .map(|index| zip.by_index(index).expect("entry").name().to_string())
            .collect();
        let prefix = client
            .file_name()
            .expect("name")
            .to_string_lossy()
            .to_string();
        assert!(names
            .iter()
            .any(|name| name == &format!("{}/Client.lily", prefix)));
        assert!(names
            .iter()
            .any(|name| name == &format!("{}/Will.docx", prefix)));
        std::fs::remove_dir_all(&client).ok();
        std::fs::remove_file(&archive).ok();
    }

    #[test]
    fn workflow_stage_defaults_and_serializes_in_snake_case() {
        assert_eq!(LilyFile::default().workflow_stage, WorkflowStage::Intake);
        assert_eq!(
            serde_json::to_string(&WorkflowStage::Questionnaire).expect("stage json"),
            "\"questionnaire\""
        );
    }

    #[test]
    fn ensure_required_document_links_and_reuses_the_selection() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("lily-required-{}", unique));
        std::fs::create_dir_all(&directory).expect("client folder");
        let working = directory.to_string_lossy().to_string();
        write_lily_file(&working, &LilyFile::default()).expect("lily file");

        let first = ensure_required_document(
            working.clone(),
            "03 - Will Templates/Last Will and Testament Template.docx".to_string(),
            "Will.docx".to_string(),
            DocumentStatus::Drafting,
        )
        .expect("first link");
        assert_eq!(first.document_filename.as_deref(), Some("Will.docx"));

        let second = ensure_required_document(
            working.clone(),
            "03 - Will Templates/Last Will and Testament Template.docx".to_string(),
            "Will.docx".to_string(),
            DocumentStatus::Reviewing,
        )
        .expect("second link");
        assert_eq!(first.id, second.id);
        assert_eq!(second.status, DocumentStatus::Reviewing);

        set_workflow_stage(working.clone(), WorkflowStage::Signed).expect("stage");
        let loaded = read_lily_file(&working).expect("reload");
        assert_eq!(loaded.required_documents.len(), 1);
        assert_eq!(loaded.required_documents[0].status, DocumentStatus::Reviewing);
        assert_eq!(loaded.workflow_stage, WorkflowStage::Signed);
        std::fs::remove_dir_all(&directory).ok();
    }
}
