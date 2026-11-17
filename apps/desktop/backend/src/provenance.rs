// SPDX-License-Identifier: AGPL-3.0-or-later
//! Provenance records for generated drafts.
//!
//! A draft records which plan and clauses produced it, a fingerprint of the
//! client data it was rendered from, the renderer version, and — when the
//! drafting agent was involved — the model, prompt fingerprint, and approval
//! event. The record lives on the document's `.lily` metadata so it survives a
//! rename and a copy of the client folder.

use crate::lily_file::LilyFile;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The renderer version recorded on every draft.
pub const RENDERER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What produced a generated document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftProvenance {
    /// When the document was rendered.
    pub generated_at: DateTime<Utc>,
    /// Plan identifier from the plan JSON.
    pub plan_id: String,
    /// Plan version from the plan JSON.
    pub plan_version: u32,
    /// Lily renderer version.
    pub renderer_version: String,
    /// Clause IDs and versions that contributed text, in ID order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub clauses: BTreeMap<String, u32>,
    /// SHA-256 of the client data the renderer read.
    pub client_data_hash: String,
    /// SHA-256 of the template the document was rendered from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_hash: Option<String>,
    /// SHA-256 of the rendered document bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_hash: Option<String>,
    /// Present when the drafting agent proposed the values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentProvenance>,
    /// Present once an attorney approves a candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalEvent>,
}

/// The agent invocation behind a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProvenance {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// SHA-256 of the prompt sent to the agent.
    pub prompt_hash: String,
    /// Where the agent wrote `proposal.lily`.
    pub proposal_path: String,
    pub flags: FlagSummary,
}

/// Counts of the agent's flags by severity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlagSummary {
    pub total: usize,
    pub gaps: usize,
    pub warnings: usize,
    pub info: usize,
}

/// The attorney's approval of a candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEvent {
    pub approved_at: DateTime<Utc>,
    /// `"agent-proposal"` or `"plan-candidate"`.
    pub method: String,
    /// The attorney named in settings when the candidate was approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

impl DraftProvenance {
    /// Mark this record as approved and name the path that produced it.
    pub fn approve(&mut self, approved_by: Option<String>) {
        self.approval = Some(ApprovalEvent {
            approved_at: Utc::now(),
            method: if self.agent.is_some() {
                "agent-proposal".to_string()
            } else {
                "plan-candidate".to_string()
            },
            approved_by,
        });
    }
}

/// SHA-256 rendered as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// SHA-256 of the client data that affects rendering: the shared variable
/// pool, contacts, role bindings, questionnaire identity, and the current
/// document's overrides. Workflow state and timestamps are excluded so the
/// fingerprint changes only when a rendered value could change.
pub fn client_data_hash(lily: &LilyFile, document: Option<&str>) -> String {
    let variables = serde_json::to_value(&lily.variables).unwrap_or(Value::Null);
    let contacts = serde_json::to_value(&lily.contacts).unwrap_or(Value::Null);
    let bindings = serde_json::to_value(&lily.contact_bindings).unwrap_or(Value::Null);
    let mut payload = format!(
        "variables={};contacts={};bindings={};questionnaire_id={};questionnaire_version={}",
        canonical_json(&variables),
        canonical_json(&contacts),
        canonical_json(&bindings),
        lily.questionnaire_id.as_deref().unwrap_or(""),
        lily.questionnaire_version
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    if let Some(name) = document {
        if let Some(meta) = lily.documents.get(name) {
            let variable_overrides =
                serde_json::to_value(&meta.variable_overrides).unwrap_or(Value::Null);
            let role_overrides =
                serde_json::to_value(&meta.role_overrides).unwrap_or(Value::Null);
            payload.push_str(&format!(
                ";variable_overrides={};role_overrides={}",
                canonical_json(&variable_overrides),
                canonical_json(&role_overrides)
            ));
        }
    }
    sha256_hex(payload.as_bytes())
}

/// Serialize a JSON value with object keys in sorted order so equal data
/// always produces the same fingerprint.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(&map[key])
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// Sidecar path for a candidate document: `Name.docx` becomes
/// `Name.provenance.json`.
fn candidate_provenance_path(candidate: &Path) -> PathBuf {
    let mut name = candidate
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(".provenance.json");
    candidate.with_file_name(name)
}

/// Write the provenance that will be attached when a candidate is approved.
pub fn write_candidate_provenance(
    candidate: &Path,
    provenance: &DraftProvenance,
) -> Result<(), String> {
    let path = candidate_provenance_path(candidate);
    let text = serde_json::to_string_pretty(provenance)
        .map_err(|error| format!("Failed to serialize provenance: {}", error))?;
    fs::write(&path, text)
        .map_err(|error| format!("Failed to write provenance {}: {}", path.display(), error))
}

/// Read a candidate's provenance sidecar, when one exists.
pub fn read_candidate_provenance(candidate: &Path) -> Option<DraftProvenance> {
    let path = candidate_provenance_path(candidate);
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remove a candidate's provenance sidecar.
pub fn remove_candidate_provenance(candidate: &Path) {
    let _ = fs::remove_file(candidate_provenance_path(candidate));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lily_file::DocumentMeta;

    fn provenance(plan_id: &str) -> DraftProvenance {
        DraftProvenance {
            generated_at: Utc::now(),
            plan_id: plan_id.to_string(),
            plan_version: 1,
            renderer_version: RENDERER_VERSION.to_string(),
            clauses: BTreeMap::new(),
            client_data_hash: String::new(),
            template_hash: None,
            document_hash: None,
            agent: None,
            approval: None,
        }
    }

    #[test]
    fn hash_is_stable_across_map_order() {
        let mut left = LilyFile::default();
        left.variables.insert("A".to_string(), "1".to_string());
        left.variables.insert("B".to_string(), "2".to_string());
        let mut right = LilyFile::default();
        right.variables.insert("B".to_string(), "2".to_string());
        right.variables.insert("A".to_string(), "1".to_string());
        assert_eq!(
            client_data_hash(&left, None),
            client_data_hash(&right, None)
        );
    }

    #[test]
    fn hash_changes_when_a_value_changes() {
        let mut lily = LilyFile::default();
        lily.variables.insert("A".to_string(), "1".to_string());
        let before = client_data_hash(&lily, None);
        lily.variables.insert("A".to_string(), "2".to_string());
        assert_ne!(before, client_data_hash(&lily, None));
    }

    #[test]
    fn document_overrides_change_the_hash() {
        let mut lily = LilyFile::default();
        lily.documents.insert(
            "Will.docx".to_string(),
            DocumentMeta {
                template_rel_path: "Will.docx".to_string(),
                created_at: Utc::now(),
                modified_at: Utc::now(),
                variable_names: Vec::new(),
                role_overrides: Default::default(),
                variable_overrides: Default::default(),
                client_key: None,
                provenance: None,
            },
        );
        let before = client_data_hash(&lily, Some("Will.docx"));
        lily.documents
            .get_mut("Will.docx")
            .unwrap()
            .variable_overrides
            .insert("Client City".to_string(), "Aurora".to_string());
        assert_ne!(before, client_data_hash(&lily, Some("Will.docx")));
    }

    #[test]
    fn approval_records_the_agent_path() {
        let mut record = provenance("will");
        record.agent = Some(AgentProvenance {
            command: "opencode".to_string(),
            model: None,
            prompt_hash: "abc".to_string(),
            proposal_path: "/tmp/proposal.lily".to_string(),
            flags: FlagSummary::default(),
        });
        record.approve(None);
        assert_eq!(record.approval.unwrap().method, "agent-proposal");
        let mut manual = provenance("will");
        manual.approve(None);
        assert_eq!(manual.approval.unwrap().method, "plan-candidate");
    }
}
