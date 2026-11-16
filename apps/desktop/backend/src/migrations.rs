// SPDX-License-Identifier: AGPL-3.0-or-later
//! Versioned `.lily` client-file migrations.
//!
//! The on-disk shape is versioned by `lily_version`. Additive fields may rely
//! on `#[serde(default)]`; anything that changes meaning, moves data, or
//! backfills records gets a step here. Each step takes the raw JSON so a file
//! is upgraded before it is deserialized into the current `LilyFile` shape.
//!
//! Migration is forward-only and refuses a file written by a newer schema. The
//! caller backs the original file up before writing the migrated version.

use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// Current client-file schema version.
pub const CURRENT_VERSION: u32 = 5;

/// The version transitions applied to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
	pub from: u32,
	pub to: u32,
	/// The source version of each applied step, in order.
	pub steps: Vec<u32>,
}

/// Read `lily_version` from a raw client file.
pub fn schema_version(value: &Value) -> Result<u32, String> {
	let version = value
		.get("lily_version")
		.and_then(Value::as_u64)
		.ok_or_else(|| "client file has no numeric lily_version".to_string())?;
	u32::try_from(version).map_err(|_| format!("client schema version {version} is out of range"))
}

/// Upgrade a raw client file to [`CURRENT_VERSION`].
pub fn migrate(value: &mut Value) -> Result<MigrationReport, String> {
	let from = schema_version(value)?;
	if from > CURRENT_VERSION {
		return Err(format!(
			"client file uses schema {from}, but this build supports up to {CURRENT_VERSION}; update Lily before opening it"
		));
	}
	if from < 2 {
		return Err(format!(
			"client file uses unsupported schema {from}; the legacy .lily.json sidecar is migrated separately"
		));
	}
	let mut version = from;
	let mut steps = Vec::new();
	while version < CURRENT_VERSION {
		match version {
			2 => v2_to_v3(value)?,
			3 => v3_to_v4(value)?,
			4 => v4_to_v5(value)?,
			other => return Err(format!("no migration from client schema {other}")),
		}
		steps.push(version);
		version += 1;
		set_version(value, version)?;
	}
	Ok(MigrationReport {
		from,
		to: version,
		steps,
	})
}

fn set_version(value: &mut Value, version: u32) -> Result<(), String> {
	let object = value
		.as_object_mut()
		.ok_or_else(|| "client file is not a JSON object".to_string())?;
	object.insert("lily_version".to_string(), json!(version));
	Ok(())
}

fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>, String> {
	value
		.as_object_mut()
		.ok_or_else(|| "client file is not a JSON object".to_string())
}

/// v2 → v3 added contacts and contact bindings.
fn v2_to_v3(value: &mut Value) -> Result<(), String> {
	let object = object_mut(value)?;
	object
		.entry("contacts")
		.or_insert_with(|| Value::Array(Vec::new()));
	object
		.entry("contact_bindings")
		.or_insert_with(|| Value::Object(Map::new()));
	Ok(())
}

/// v3 → v4 added the questionnaire identity.
fn v3_to_v4(value: &mut Value) -> Result<(), String> {
	let object = object_mut(value)?;
	object
		.entry("questionnaire_id")
		.or_insert(Value::Null);
	object
		.entry("questionnaire_version")
		.or_insert(Value::Null);
	Ok(())
}

/// v4 → v5 added the required-document spine. Backfill one entry per existing
/// document so a migrated client has a usable selection list immediately.
fn v4_to_v5(value: &mut Value) -> Result<(), String> {
	let object = object_mut(value)?;
	let documents = object
		.get("documents")
		.and_then(Value::as_object)
		.cloned()
		.unwrap_or_default();
	let mut required: Vec<Value> = object
		.get("required_documents")
		.and_then(Value::as_array)
		.cloned()
		.unwrap_or_default();

	let mut covered: BTreeSet<(String, String)> = BTreeSet::new();
	let mut used_ids: BTreeSet<String> = BTreeSet::new();
	for entry in &required {
		let Some(entry) = entry.as_object() else {
			continue;
		};
		if let Some(id) = entry.get("id").and_then(Value::as_str) {
			used_ids.insert(id.to_string());
		}
		if let (Some(template), Some(filename)) = (
			entry.get("template_rel_path").and_then(Value::as_str),
			entry.get("document_filename").and_then(Value::as_str),
		) {
			covered.insert((template.to_string(), filename.to_string()));
		}
	}

	let mut filenames: Vec<&String> = documents.keys().collect();
	filenames.sort();
	for filename in filenames {
		let Some(meta) = documents.get(filename).and_then(Value::as_object) else {
			continue;
		};
		let Some(template) = meta.get("template_rel_path").and_then(Value::as_str) else {
			continue;
		};
		if covered.contains(&(template.to_string(), filename.clone())) {
			continue;
		}
		let id = next_id(&used_ids);
		used_ids.insert(id.clone());
		required.push(json!({
			"id": id,
			"template_rel_path": template,
			"status": "drafting",
			"document_filename": filename,
			"notes": ""
		}));
	}
	object.insert("required_documents".to_string(), Value::Array(required));
	Ok(())
}

fn next_id(used: &BTreeSet<String>) -> String {
	let mut index = 0;
	loop {
		let candidate = format!("migrated-{index}");
		if !used.contains(&candidate) {
			return candidate;
		}
		index += 1;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn fixture(name: &str) -> Value {
		let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
			.join("tests/fixtures/lily")
			.join(name);
		let text = std::fs::read_to_string(&path).expect("fixture");
		serde_json::from_str(&text).expect("fixture json")
	}

	#[test]
	fn each_historical_version_reaches_current() {
		for (name, expected_steps) in [
			("v2-client.lily", 3),
			("v3-client.lily", 2),
			("v4-client.lily", 1),
		] {
			let mut value = fixture(name);
			let before = value["variables"]["Client Full Name"].clone();
			let report = migrate(&mut value).expect(name);
			assert_eq!(report.from + expected_steps, CURRENT_VERSION);
			assert_eq!(report.to, CURRENT_VERSION);
			assert_eq!(value["lily_version"], CURRENT_VERSION);
			assert_eq!(value["variables"]["Client Full Name"], before);
		}
	}

	#[test]
	fn migration_is_idempotent_at_current() {
		let mut value = fixture("v5-client.lily");
		let report = migrate(&mut value).expect("current");
		assert_eq!(report.from, CURRENT_VERSION);
		assert!(report.steps.is_empty());
	}

	#[test]
	fn v4_backfills_required_documents_from_documents() {
		let mut value = fixture("v4-client.lily");
		migrate(&mut value).expect("migrate");
		let required = value["required_documents"].as_array().expect("required");
		assert_eq!(required.len(), 1);
		assert_eq!(required[0]["document_filename"], "Will - JANE DOE.docx");
		assert_eq!(required[0]["status"], "drafting");
		assert_eq!(required[0]["id"], "migrated-0");
	}

	#[test]
	fn newer_schema_is_refused() {
		let mut value = json!({"lily_version": CURRENT_VERSION + 1, "variables": {}, "documents": {}});
		let error = migrate(&mut value).expect_err("newer schema");
		assert!(error.contains("update Lily"));
		assert_eq!(value["lily_version"], CURRENT_VERSION + 1);
	}

	#[test]
	fn missing_version_is_refused() {
		let mut value = json!({"variables": {}, "documents": {}});
		let error = migrate(&mut value).expect_err("missing version");
		assert!(error.contains("lily_version"));
	}
}
