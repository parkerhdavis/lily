// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read the Carelaw estate-planning questionnaire PDF and turn its AcroForm
//! fields into a reviewable client preview.
//!
//! The PDF is the canonical intake artifact. This module only reads it; the
//! apply step, once it exists, writes the derived `.lily` data after the
//! attorney has reviewed the preview.

use chrono::Datelike;
use crate::lily_file::LilyFile;
use lopdf::{Dictionary, Document, Object};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use uuid::Uuid;

/// The field-map version this module understands.
pub const FIELD_MAP_VERSION: u32 = 1;

/// One AcroForm value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfFieldValue {
	Text(String),
	Check(bool),
}

/// Every field in a questionnaire PDF, keyed by its fully qualified name.
#[derive(Debug, Clone, Default)]
pub struct PdfForm {
	pub fields: BTreeMap<String, PdfFieldValue>,
}

impl PdfForm {
	pub fn text(&self, key: &str) -> &str {
		match self.fields.get(key) {
			Some(PdfFieldValue::Text(value)) => value.trim(),
			_ => "",
		}
	}

	pub fn check(&self, key: &str) -> bool {
		matches!(self.fields.get(key), Some(PdfFieldValue::Check(true)))
	}
}

/// Read every text and checkbox field in a PDF.
pub fn read_pdf_form(path: &Path) -> Result<PdfForm, String> {
	let document = Document::load(path)
		.map_err(|error| format!("Failed to open {}: {}", path.display(), error))?;
	let root = document
		.trailer
		.get_deref(b"Root", &document)
		.map_err(|error| format!("PDF has no document catalog: {}", error))?;
	let root = root
		.as_dict()
		.map_err(|_| "PDF document catalog is not a dictionary".to_string())?;
	let acroform = root
		.get_deref(b"AcroForm", &document)
		.map_err(|_| "PDF has no AcroForm".to_string())?;
	let acroform = acroform
		.as_dict()
		.map_err(|_| "PDF AcroForm is not a dictionary".to_string())?;
	let fields = acroform
		.get_deref(b"Fields", &document)
		.map_err(|_| "PDF AcroForm has no fields".to_string())?;
	let fields = fields
		.as_array()
		.map_err(|_| "PDF AcroForm fields are not an array".to_string())?;

	let mut form = PdfForm::default();
	for field in fields {
		walk_field(&document, field, "", None, &mut form.fields)?;
	}
	Ok(form)
}

fn walk_field(
	document: &Document,
	object: &Object,
	prefix: &str,
	inherited_type: Option<&str>,
	out: &mut BTreeMap<String, PdfFieldValue>,
) -> Result<(), String> {
	let dictionary = dereference(document, object)
		.as_dict()
		.map_err(|_| "PDF field is not a dictionary".to_string())?;
	let name = dictionary
		.get_deref(b"T", document)
		.ok()
		.and_then(|value| value.as_str().ok())
		.map(decode_pdf_string);
	let full_name = match (prefix.is_empty(), name.as_deref()) {
		(true, Some(name)) => name.to_string(),
		(false, Some(name)) => format!("{prefix}.{name}"),
		(_, None) => prefix.to_string(),
	};
	let field_type = dictionary
		.get_deref(b"FT", document)
		.ok()
		.and_then(|value| value.as_name().ok())
		.map(|value| String::from_utf8_lossy(value).to_string())
		.or_else(|| inherited_type.map(str::to_string));

	let owns_type = dictionary.get_deref(b"FT", document).is_ok();
	let is_terminal = name.is_some() || owns_type;
	if let (Some(field_type), true, false) =
		(field_type.as_deref(), is_terminal, full_name.is_empty())
	{
		match field_type {
			"Tx" => {
				let text = field_value(document, dictionary)
					.and_then(|value| object_text(document, value))
					.unwrap_or_default();
				// A widget child without a value must not erase the field's
				// value from the parent dictionary.
				if !text.is_empty() || !out.contains_key(&full_name) {
					out.insert(full_name.clone(), PdfFieldValue::Text(text));
				}
			}
			"Btn" => {
				let checked = field_value(document, dictionary)
					.and_then(|value| checkbox_value(document, value))
					.unwrap_or(false);
				if checked || !out.contains_key(&full_name) {
					out.insert(full_name.clone(), PdfFieldValue::Check(checked));
				}
			}
			_ => {}
		}
	}

	if let Ok(kids) = dictionary.get_deref(b"Kids", document) {
		if let Ok(kids) = kids.as_array() {
			for kid in kids {
				walk_field(document, kid, &full_name, field_type.as_deref(), out)?;
			}
		}
	}
	Ok(())
}

/// A field's value may live on the field dictionary or on a widget child.
fn field_value<'a>(document: &'a Document, dictionary: &'a Dictionary) -> Option<&'a Object> {
	if let Ok(value) = dictionary.get_deref(b"V", document) {
		return Some(value);
	}
	if let Ok(kids) = dictionary.get_deref(b"Kids", document) {
		if let Ok(kids) = kids.as_array() {
			for kid in kids {
				let kid = dereference(document, kid);
				if let Ok(kid) = kid.as_dict() {
					if let Ok(value) = kid.get_deref(b"V", document) {
						return Some(value);
					}
				}
			}
		}
	}
	None
}

fn dereference<'a>(document: &'a Document, object: &'a Object) -> &'a Object {
	match object {
		Object::Reference(id) => document.get_object(*id).unwrap_or(object),
		_ => object,
	}
}

fn object_text(document: &Document, object: &Object) -> Option<String> {
	let object = dereference(document, object);
	match object {
		Object::String(bytes, _) => Some(decode_pdf_string(bytes)),
		Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).to_string()),
		Object::Integer(value) => Some(value.to_string()),
		Object::Real(value) => Some(value.to_string()),
		Object::Boolean(value) => Some(value.to_string()),
		_ => None,
	}
}

fn checkbox_value(document: &Document, object: &Object) -> Option<bool> {
	let value = object_text(document, object)?;
	Some(!matches!(value.as_str(), "Off" | "off" | "False" | "false" | ""))
}

/// PDF text strings are UTF-16BE when they start with a byte-order mark.
fn decode_pdf_string(bytes: &[u8]) -> String {
	if bytes.starts_with(&[0xFE, 0xFF]) {
		let units: Vec<u16> = bytes[2..]
			.chunks_exact(2)
			.map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
			.collect();
		return String::from_utf16_lossy(&units);
	}
	if bytes.starts_with(&[0xFF, 0xFE]) {
		let units: Vec<u16> = bytes[2..]
			.chunks_exact(2)
			.map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
			.collect();
		return String::from_utf16_lossy(&units);
	}
	String::from_utf8_lossy(bytes).to_string()
}

/// Preview a filled questionnaire without writing any client data.
#[tauri::command]
pub fn preview_pdf_questionnaire(pdf_path: String) -> Result<QuestionnairePreview, String> {
	preview_questionnaire(Path::new(&pdf_path))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn questionnaire_path() -> std::path::PathBuf {
		std::path::Path::new("/home/parker/Carelaw/carelaw-lily/Templates/00 - Questionnaire")
			.join("Estate Planning Questionnaire.pdf")
	}

	fn demo_path() -> std::path::PathBuf {
		std::path::Path::new("/home/parker/Carelaw/carelaw-lily/Templates/00 - Questionnaire")
			.join(".working/sample.pdf")
	}

	#[test]
	fn reads_a_small_form_fixture() {
		let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
			.join("tests/fixtures/pdf/simple-form.pdf");
		let form = read_pdf_form(&path).expect("fixture");
		assert_eq!(form.text("a.client.first_name"), "Alexandria");
		assert!(form.check("about.together"));
	}

	fn synthetic_preview() -> QuestionnairePreview {
		QuestionnairePreview {
			field_map_version: FIELD_MAP_VERSION,
			field_count: 2,
			sha256: "a".repeat(64),
			clients: vec![
				PreviewClient {
					key: "a".to_string(),
					first_name: "Alexandria".to_string(),
					last_name: "Morgan".to_string(),
					..PreviewClient::default()
				},
				PreviewClient {
					key: "b".to_string(),
					first_name: "Jordan".to_string(),
					last_name: "Morgan".to_string(),
					..PreviewClient::default()
				},
			],
			people: vec![PreviewPerson {
				number: 1,
				full_name: "Jordan Morgan".to_string(),
				first_name: "Jordan".to_string(),
				last_name: "Morgan".to_string(),
				relationship: "Spouse".to_string(),
				..PreviewPerson::default()
			}],
			roles: BTreeMap::from([
				(
					"a.fpoa".to_string(),
					PreviewRole {
						key: "a.fpoa".to_string(),
						label: "A Financial Power of Attorney".to_string(),
						assignments: vec![PreviewAssignment {
							number: Some(1),
							name: "Jordan Morgan".to_string(),
						}],
					},
				),
				(
					"a.hpoa".to_string(),
					PreviewRole {
						key: "a.hpoa".to_string(),
						label: "A Health Care Power of Attorney".to_string(),
						assignments: vec![PreviewAssignment {
							number: Some(1),
							name: "Jordan Morgan".to_string(),
						}],
					},
				),
				(
					"a.pr".to_string(),
					PreviewRole {
						key: "a.pr".to_string(),
						label: "A Personal Representative".to_string(),
						assignments: vec![PreviewAssignment {
							number: Some(1),
							name: "Jordan Morgan".to_string(),
						}],
					},
				),
				(
					"b.fpoa".to_string(),
					PreviewRole {
						key: "b.fpoa".to_string(),
						label: "B Financial Power of Attorney".to_string(),
						assignments: vec![PreviewAssignment {
							number: Some(1),
							name: "Alexandria Morgan".to_string(),
						}],
					},
				),
			]),
			household: PreviewHousehold {
				trust_name: "The Morgan Family Trust".to_string(),
				..PreviewHousehold::default()
			},
			..QuestionnairePreview::default()
		}
	}

	#[test]
	fn merge_preserves_contact_ids_and_existing_document_state() {
		let preview = synthetic_preview();
		let mut lily = build_client_lily(&preview, "Questionnaire.pdf");
		let jordan = lily
			.contacts
			.iter()
			.find(|contact| contact.full_name == "Jordan Morgan")
			.expect("jordan")
			.id
			.clone();
		lily.required_documents[0].status = crate::lily_file::DocumentStatus::Reviewing;
		lily.required_documents[0].document_filename = Some("Drafted.docx".to_string());
		lily.required_documents[0].id = "existing-document".to_string();
		let before = lily.required_documents.len();
		let counts = merge_questionnaire(&mut lily, &preview);
		assert_eq!(counts.documents_added, 0);
		assert_eq!(lily.required_documents.len(), before);
		assert_eq!(lily.required_documents[0].id, "existing-document");
		assert_eq!(
			lily.required_documents[0].status,
			crate::lily_file::DocumentStatus::Reviewing
		);
		assert_eq!(
			lily.required_documents[0].document_filename.as_deref(),
			Some("Drafted.docx")
		);
		assert!(lily
			.contacts
			.iter()
			.any(|contact| contact.id == jordan && contact.full_name == "Jordan Morgan"));
	}

	#[test]
	fn builds_a_joint_client_from_a_synthetic_preview() {
		let mut preview = synthetic_preview();
		preview.required_document_count =
			required_documents(&preview, true, plan_includes_trust(&preview)).len();
		let lily = build_client_lily(&preview, "Questionnaire.pdf");
		assert_eq!(lily.variables["Client Full Name"], "Alexandria Morgan");
		assert_eq!(lily.co_client.as_ref().expect("co-client").first_name, "Jordan");
		assert_eq!(lily.contact_bindings["FPOA Agent"].assignments.as_ref().unwrap().len(), 1);
		assert!(lily.client_roles["b"].contains_key("FPOA Agent"));
		assert_eq!(lily.required_documents.len(), 10);
		assert_eq!(preview.required_document_count, 10);
		assert!(lily
			.required_documents
			.iter()
			.filter(|document| document.client_key.as_deref() == Some("b"))
			.all(|document| document.draft_ready));
	}

	#[test]
	fn maps_the_demo_questionnaire() {
		let path = demo_path();
		if !path.is_file() {
			return;
		}
		let preview = preview_questionnaire(&path).expect("preview");
		assert_eq!(preview.field_map_version, FIELD_MAP_VERSION);
		assert!(preview.field_count > 300);
		assert_eq!(preview.clients.len(), 2);
		assert_eq!(preview.clients[0].first_name, "Alexandria");
		assert_eq!(preview.clients[1].last_name, "Morgan");
		assert_eq!(preview.people.len(), 5);
		assert_eq!(preview.people[0].full_name, "Jordan Lee Morgan");
		assert_eq!(preview.roles["a.fpoa"].assignments.len(), 3);
		assert_eq!(preview.roles["a.fpoa"].assignments[0].name, "Jordan Lee Morgan");
		assert_eq!(
			preview.household.trust_name,
			"The Alexandria and Jordan Morgan Family Trust"
		);
		assert_eq!(preview.household.children.len(), 2);
		assert!(preview.household.children[0].in_trust);
		assert_eq!(preview.estate.real_property.len(), 1);
		assert_eq!(preview.estate.accounts.len(), 2);
		assert!(preview.final_arrangements["a"].cremation);
		assert_eq!(preview.sign.completed_by, "Alexandria Rae Morgan");
		assert!(preview.unknown_fields.is_empty(), "{:?}", preview.unknown_fields);
		assert!(preview.missing_fields.is_empty(), "{:?}", preview.missing_fields);
		assert!(preview
			.warnings
			.iter()
			.any(|warning| warning.contains("B Financial Power of Attorney tier 1")));
	}

	#[test]
	fn builds_a_joint_client_from_the_demo_questionnaire() {
		let path = demo_path();
		if !path.is_file() {
			return;
		}
		let preview = preview_questionnaire(&path).expect("preview");
		assert_eq!(
			preview.suggested_folder_name,
			"Morgan, Alexandria and Morgan, Jordan"
		);
		let lily = build_client_lily(&preview, "Questionnaire.pdf");
		assert_eq!(lily.variables["Client Full Name"], "Alexandria Morgan");
		assert_eq!(lily.co_client.as_ref().expect("co-client").first_name, "Jordan");
		assert_eq!(lily.contacts.len(), 6);
		assert!(lily
			.contacts
			.iter()
			.any(|contact| contact.relationship == "Child" && contact.is_minor));
		assert!(lily.contact_bindings.contains_key("FPOA Agent"));
		assert!(lily.client_roles["b"].contains_key("FPOA Agent"));
		assert_eq!(lily.variables["Plan Includes Trust"], "true");
		assert_eq!(
			lily.variables["Pour-Over Trust Name"],
			"The Alexandria and Jordan Morgan Family Trust"
		);
		assert!(lily.questionnaire_pdf.is_some());
		assert_eq!(lily.required_documents.len(), 12);
		assert!(lily
			.required_documents
			.iter()
			.filter(|document| document.client_key.as_deref() == Some("b"))
			.all(|document| document.draft_ready));
		assert!(lily
			.required_documents
			.iter()
			.all(|document| document.draft_ready));
	}

	#[test]
	fn creates_a_client_folder_from_the_demo_questionnaire() {
		let path = demo_path();
		if !path.is_file() {
			return;
		}
		let parent = std::env::temp_dir().join(format!("lily-pdf-client-{}", Uuid::new_v4()));
		std::fs::create_dir_all(&parent).expect("parent");
		let created = create_client_from_questionnaire(
			parent.to_string_lossy().to_string(),
			path.to_string_lossy().to_string(),
			None,
		)
		.expect("create client");
		assert_eq!(created.folder_name, "Morgan, Alexandria and Morgan, Jordan");
		assert_eq!(created.required_document_count, 12);
		let directory = Path::new(&created.directory);
		assert!(directory.join("Questionnaire.pdf").is_file());
		let lily = crate::lily_file::read_lily_file(&created.directory).expect("lily");
		assert_eq!(lily.lily_version, crate::migrations::CURRENT_VERSION);
		assert_eq!(lily.variables["Has Spouse"], "true");
		assert_eq!(lily.variables["Client Spouse Name"], "Jordan Lee Morgan");
		assert_eq!(lily.variables["Client has Children"], "true");
		assert_eq!(lily.variables["Has Minor Children"], "true");
		assert!(lily.questionnaire_pdf.as_ref().expect("pdf record").sha256.len() == 64);
		std::fs::remove_dir_all(&parent).ok();
	}

	#[test]
	fn drafts_and_renders_the_second_clients_gpoa() {
		let demo = demo_path();
		let templates = std::path::Path::new(
			"/home/parker/Carelaw/carelaw-lily/Templates",
		);
		if !demo.is_file() || !templates.is_dir() {
			return;
		}
		let parent = std::env::temp_dir().join(format!("lily-b-gpoa-{}", Uuid::new_v4()));
		std::fs::create_dir_all(&parent).expect("parent");
		let created = create_client_from_questionnaire(
			parent.to_string_lossy().to_string(),
			demo.to_string_lossy().to_string(),
			None,
		)
		.expect("create client");
		let lily = crate::lily_file::read_lily_file(&created.directory).expect("lily");
		let requirement = lily
			.required_documents
			.iter()
			.find(|document| {
				document.client_key.as_deref() == Some("b")
					&& document.template_rel_path.contains("GPOA Template")
			})
			.expect("B GPOA");
		let filename = crate::lily_file::draft_required_document(
			created.directory.clone(),
			requirement.id.clone(),
			templates.to_string_lossy().to_string(),
		)
		.expect("draft");
		let document = std::path::Path::new(&created.directory).join(&filename);
		let plan = templates
			.join("02 - Power of Attorney and Medical Templates/Plans/GPOA.plan.json");
		let skeleton = templates
			.join("02 - Power of Attorney and Medical Templates/GPOA Template.docx");
		crate::plan::render_plan_document(
			document.to_string_lossy().to_string(),
			plan.to_string_lossy().to_string(),
			Some(skeleton.to_string_lossy().to_string()),
		)
		.expect("render");
		let text = crate::docx_ops::document_lines(&document)
			.expect("lines")
			.join(" ");
		assert!(text.contains("JORDAN MORGAN"), "{text}");
		assert!(text.contains("ALEXANDRIA RAE MORGAN"), "{text}");
		std::fs::remove_dir_all(&parent).ok();
	}

	#[test]
	fn reads_the_carelaw_questionnaire() {
		let path = questionnaire_path();
		if !path.is_file() {
			return;
		}
		let form = read_pdf_form(&path).expect("form");
		assert!(
			form.fields.len() > 300,
			"expected the full questionnaire, got {} fields",
			form.fields.len()
		);
		assert_eq!(form.text("a.client.first_name"), "");
		assert!(form.fields.contains_key("about.together"));
	}
}

// ─── Preview ───────────────────────────────────────────────────────────────

/// A reviewable view of a filled questionnaire. Nothing is written until the
/// caller applies it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct QuestionnairePreview {
	pub field_map_version: u32,
	pub field_count: usize,
	pub source_path: String,
	pub source_filename: String,
	pub sha256: String,
	pub suggested_folder_name: String,
	pub clients: Vec<PreviewClient>,
	pub people: Vec<PreviewPerson>,
	pub roles: BTreeMap<String, PreviewRole>,
	pub household: PreviewHousehold,
	pub estate: PreviewEstate,
	pub final_arrangements: BTreeMap<String, PreviewFinal>,
	pub notes: BTreeMap<String, String>,
	pub sign: PreviewSign,
	pub required_document_count: usize,
	pub warnings: Vec<String>,
	pub unknown_fields: Vec<String>,
	pub missing_fields: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewClient {
	pub key: String,
	pub first_name: String,
	pub middle_name: String,
	pub last_name: String,
	pub other_names: String,
	pub preferred_name: String,
	pub date_of_birth: String,
	pub phone: String,
	pub email: String,
	pub address: String,
	pub city: String,
	pub state: String,
	pub zip: String,
	pub county: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewPerson {
	pub number: u32,
	pub full_name: String,
	pub first_name: String,
	pub middle_name: String,
	pub last_name: String,
	pub relationship: String,
	pub other_relationship: String,
	pub date_of_birth: String,
	pub phone: String,
	pub email: String,
	pub address: String,
	pub is_minor: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewRole {
	pub key: String,
	pub label: String,
	pub assignments: Vec<PreviewAssignment>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewAssignment {
	pub number: Option<u32>,
	pub name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewHousehold {
	pub trust_name: String,
	pub trustees: Vec<PreviewAssignment>,
	pub no_minor_children: bool,
	pub guardians: Vec<PreviewAssignment>,
	pub guardian_conditions: String,
	pub children: Vec<PreviewChild>,
	pub tangible_recipient: String,
	pub distribution_summary: String,
	pub distribution_instructions: String,
	pub existing_notes: String,
	pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewChild {
	pub number: u32,
	pub contact: Option<u32>,
	pub name: String,
	pub age: String,
	pub outright: bool,
	pub in_trust: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewEstate {
	pub real_property: Vec<BTreeMap<String, String>>,
	pub accounts: Vec<BTreeMap<String, String>>,
	pub insurance: Vec<BTreeMap<String, String>>,
	pub businesses: Vec<BTreeMap<String, String>>,
	pub liabilities: Vec<BTreeMap<String, String>>,
	pub gifts: Vec<BTreeMap<String, String>>,
	pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewFinal {
	pub burial: bool,
	pub cremation: bool,
	pub details: String,
	pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreviewSign {
	pub completed_by: String,
	pub date: String,
	pub checks: BTreeMap<String, bool>,
	pub send: BTreeMap<String, bool>,
}

/// Read a filled questionnaire and map it to the review shape.
pub fn preview_questionnaire(path: &Path) -> Result<QuestionnairePreview, String> {
	let bytes = std::fs::read(path)
		.map_err(|error| format!("Failed to read {}: {}", path.display(), error))?;
	let form = read_pdf_form(path)?;
	let mut preview = map_form(&form);
	preview.source_path = path.to_string_lossy().to_string();
	preview.source_filename = path
		.file_name()
		.map(|name| name.to_string_lossy().to_string())
		.unwrap_or_default();
	preview.sha256 = crate::provenance::sha256_hex(&bytes);
	preview.suggested_folder_name = suggested_folder_name(&preview);
	Ok(preview)
}

/// Map a parsed form to the review shape.
pub fn map_form(form: &PdfForm) -> QuestionnairePreview {
	let mut preview = QuestionnairePreview {
		field_map_version: FIELD_MAP_VERSION,
		field_count: form.fields.len(),
		..QuestionnairePreview::default()
	};
	let mut consumed = BTreeSet::new();

	for key in ["a", "b"] {
		let mut client = PreviewClient {
			key: key.to_string(),
			..PreviewClient::default()
		};
		client.first_name = read(form, &mut consumed, &format!("{key}.client.first_name"));
		client.middle_name = read(form, &mut consumed, &format!("{key}.client.middle_name"));
		client.last_name = read(form, &mut consumed, &format!("{key}.client.last_name"));
		client.other_names = read(form, &mut consumed, &format!("{key}.client.other_names"));
		client.preferred_name = read(form, &mut consumed, &format!("{key}.client.preferred_name"));
		client.date_of_birth = read(form, &mut consumed, &format!("{key}.client.dob"));
		client.phone = read(form, &mut consumed, &format!("{key}.client.phone"));
		client.email = read(form, &mut consumed, &format!("{key}.client.email"));
		client.address = read(form, &mut consumed, &format!("{key}.client.address"));
		client.city = read(form, &mut consumed, &format!("{key}.client.city"));
		client.state = read(form, &mut consumed, &format!("{key}.client.state"));
		client.zip = read(form, &mut consumed, &format!("{key}.client.zip"));
		client.county = read(form, &mut consumed, &format!("{key}.client.county"));
		if has_client(&client) {
			preview.clients.push(client);
		}
	}

	for number in 1..=16 {
		let prefix = format!("people.{number:02}");
		let full_name = read(form, &mut consumed, &format!("{prefix}.name"));
		let relationship = read(form, &mut consumed, &format!("{prefix}.rel_a"));
		let other_relationship = read(form, &mut consumed, &format!("{prefix}.rel_b"));
		let date_of_birth = read(form, &mut consumed, &format!("{prefix}.dob"));
		let phone = read(form, &mut consumed, &format!("{prefix}.phone"));
		let email = read(form, &mut consumed, &format!("{prefix}.email"));
		let address = read(form, &mut consumed, &format!("{prefix}.address"));
		if full_name.is_empty()
			&& relationship.is_empty()
			&& other_relationship.is_empty()
			&& date_of_birth.is_empty()
			&& phone.is_empty()
			&& email.is_empty()
			&& address.is_empty()
		{
			continue;
		}
		let (first_name, middle_name, last_name) = split_name(&full_name);
		preview.people.push(PreviewPerson {
			number,
			full_name,
			first_name,
			middle_name,
			last_name,
			relationship,
			other_relationship,
			is_minor: is_minor(&date_of_birth),
			date_of_birth,
			phone,
			email,
			address,
		});
	}

	let roster: BTreeMap<u32, String> = preview
		.people
		.iter()
		.map(|person| (person.number, person.full_name.clone()))
		.collect();
	for (key, label, tiers) in [
		("a.fpoa", "A Financial Power of Attorney", 3),
		("a.hpoa", "A Health Care Power of Attorney", 3),
		("a.pr", "A Personal Representative", 3),
		("a.hipaa", "A HIPAA Releases", 4),
		("b.fpoa", "B Financial Power of Attorney", 3),
		("b.hpoa", "B Health Care Power of Attorney", 3),
		("b.pr", "B Personal Representative", 3),
		("b.hipaa", "B HIPAA Releases", 4),
	] {
		if let Some(role) = read_role(
			form,
			&mut consumed,
			key,
			label,
			tiers,
			&roster,
			&mut preview.warnings,
		) {
			preview.roles.insert(key.to_string(), role);
		}
	}
	if let Some(role) = read_role(
		form,
		&mut consumed,
		"household.trustee",
		"Successor Trustee",
		3,
		&roster,
		&mut preview.warnings,
	) {
		preview.household.trustees = role.assignments;
	}
	if let Some(role) = read_role(
		form,
		&mut consumed,
		"household.guardian",
		"Guardian",
		2,
		&roster,
		&mut preview.warnings,
	) {
		preview.household.guardians = role.assignments;
	}

	preview.household.trust_name =
		read(form, &mut consumed, "household.trust_name");
	preview.household.no_minor_children = consume_check(form, &mut consumed, "household.no_minor_children");
	preview.household.guardian_conditions =
		read(form, &mut consumed, "household.guardian.conditions");
	preview.household.tangible_recipient =
		read(form, &mut consumed, "household.tangible.recipient");
	preview.household.distribution_summary =
		read(form, &mut consumed, "household.distribution.summary");
	preview.household.distribution_instructions =
		read(form, &mut consumed, "household.distribution.instructions");
	preview.household.existing_notes =
		read(form, &mut consumed, "household.existing.notes");
	preview.household.notes = read(form, &mut consumed, "household.notes");

	for number in 1..=4 {
		let prefix = format!("household.child.{number:02}");
		let contact = read(form, &mut consumed, &format!("{prefix}.contact"));
		let name = read(form, &mut consumed, &format!("{prefix}.name"));
		let age = read(form, &mut consumed, &format!("{prefix}.age"));
		let outright = consume_check(form, &mut consumed, &format!("{prefix}.outright"));
		let in_trust = consume_check(form, &mut consumed, &format!("{prefix}.in_trust"));
		if name.is_empty() && age.is_empty() && !outright && !in_trust {
			continue;
		}
		preview.household.children.push(PreviewChild {
			number,
			contact: contact.trim().parse::<u32>().ok(),
			name,
			age,
			outright,
			in_trust,
		});
	}

	preview.estate.real_property = read_rows(
		form,
		&mut consumed,
		"estate.real",
		4,
		&["description", "title", "value"],
	);
	preview.estate.accounts = read_rows(
		form,
		&mut consumed,
		"estate.account",
		6,
		&["institution", "value", "beneficiary"],
	);
	preview.estate.insurance = read_rows(
		form,
		&mut consumed,
		"estate.insurance",
		2,
		&["policy", "face", "beneficiary"],
	);
	preview.estate.businesses = read_rows(
		form,
		&mut consumed,
		"estate.business",
		2,
		&["name", "ownership", "value"],
	);
	preview.estate.liabilities = read_rows(
		form,
		&mut consumed,
		"estate.liability",
		4,
		&["creditor", "balance", "notes"],
	);
	preview.estate.gifts = read_rows(
		form,
		&mut consumed,
		"estate.gift",
		2,
		&["description", "value"],
	);
	preview.estate.notes = read(form, &mut consumed, "estate.notes");

	for key in ["a", "b"] {
		let mut final_arrangement = PreviewFinal {
			burial: consume_check(form, &mut consumed, &format!("{key}.final.burial")),
			cremation: consume_check(form, &mut consumed, &format!("{key}.final.cremation")),
			details: read(form, &mut consumed, &format!("{key}.final.details")),
			notes: read(form, &mut consumed, &format!("{key}.notes")),
		};
		if !final_arrangement.burial
			&& !final_arrangement.cremation
			&& final_arrangement.details.is_empty()
			&& final_arrangement.notes.is_empty()
		{
			continue;
		}
		final_arrangement.details = final_arrangement.details.trim().to_string();
		preview
			.final_arrangements
			.insert(key.to_string(), final_arrangement);
	}

	preview.sign.completed_by = read(form, &mut consumed, "sign.completed_by");
	preview.sign.date = read(form, &mut consumed, "sign.date");
	for number in 1..=6 {
		let key = format!("sign.check.{number}");
		if form.fields.contains_key(&key) {
			preview
				.sign
				.checks
				.insert(number.to_string(), consume_check(form, &mut consumed, &key));
		}
	}
	for number in 1..=3 {
		let key = format!("sign.send.{number}");
		if form.fields.contains_key(&key) {
			preview
				.sign
				.send
				.insert(number.to_string(), consume_check(form, &mut consumed, &key));
		}
	}
	if let Some(client) = preview.clients.iter().find(|client| client.key == "a") {
		preview
			.notes
			.insert("a".to_string(), read(form, &mut consumed, "a.notes"));
		if client.first_name.is_empty() && client.last_name.is_empty() {
			preview
				.warnings
				.push("Client A has no first or last name.".to_string());
		}
	}
	if let Some(client) = preview.clients.iter().find(|client| client.key == "b") {
		preview
			.notes
			.insert("b".to_string(), read(form, &mut consumed, "b.notes"));
		if client.first_name.is_empty() && client.last_name.is_empty() {
			preview
				.warnings
				.push("Client B has no first or last name.".to_string());
		}
	}
	let together = consume_check(form, &mut consumed, "about.together");
	if together
		&& !preview.clients.iter().any(|client| client.key == "b")
	{
		preview
			.warnings
			.push("The form says the plan is for a couple, but Client B is blank.".to_string());
	}
	if !together
		&& preview.clients.iter().any(|client| client.key == "b")
	{
		preview
			.warnings
			.push("Client B has answers, but the form is marked as a single plan.".to_string());
	}

	preview.unknown_fields = form
		.fields
		.keys()
		.filter(|key| !consumed.contains(*key))
		.cloned()
		.collect();
	let joint = preview.clients.iter().any(|client| client.key == "b");
	let trust = plan_includes_trust(&preview);
	preview.required_document_count = required_documents(&preview, joint, trust).len();
	preview.missing_fields = ["a.client.first_name", "a.client.last_name", "about.together"]
		.into_iter()
		.filter(|key| !form.fields.contains_key(*key))
		.map(str::to_string)
		.collect();
	preview
}

fn read(form: &PdfForm, consumed: &mut BTreeSet<String>, key: &str) -> String {
	consumed.insert(key.to_string());
	form.text(key).to_string()
}

fn consume_check(form: &PdfForm, consumed: &mut BTreeSet<String>, key: &str) -> bool {
	consumed.insert(key.to_string());
	form.check(key)
}

fn has_client(client: &PreviewClient) -> bool {
	!client.first_name.is_empty()
		|| !client.middle_name.is_empty()
		|| !client.last_name.is_empty()
		|| !client.other_names.is_empty()
		|| !client.preferred_name.is_empty()
		|| !client.date_of_birth.is_empty()
		|| !client.phone.is_empty()
		|| !client.email.is_empty()
		|| !client.address.is_empty()
		|| !client.city.is_empty()
		|| !client.state.is_empty()
		|| !client.zip.is_empty()
		|| !client.county.is_empty()
}

fn read_role(
	form: &PdfForm,
	consumed: &mut BTreeSet<String>,
	prefix: &str,
	label: &str,
	tiers: u32,
	roster: &BTreeMap<u32, String>,
	warnings: &mut Vec<String>,
) -> Option<PreviewRole> {
	let mut role = PreviewRole {
		key: prefix.to_string(),
		label: label.to_string(),
		assignments: Vec::new(),
	};
	for tier in 1..=tiers {
		let contact = read(form, consumed, &format!("{prefix}.{tier}.contact"));
		let name = read(form, consumed, &format!("{prefix}.{tier}.name"));
		if contact.is_empty() && name.is_empty() {
			continue;
		}
		let number = contact.trim().parse::<u32>().ok();
		let mut assignment = PreviewAssignment {
			number,
			name: name.clone(),
		};
		match number {
			Some(number) => match roster.get(&number) {
				Some(roster_name) if !name.is_empty() && !same_name(roster_name, &name) => {
					warnings.push(format!(
						"{label} tier {tier} names {name} but roster number {number} is {roster_name}."
					));
				}
				Some(_) => {}
				None => warnings.push(format!(
					"{label} tier {tier} points at roster number {number}, which is blank."
				)),
			},
			None if !name.is_empty() => {
				warnings.push(format!(
					"{label} tier {tier} names {name} without a roster number."
				));
				if let Some((number, _)) = roster
					.iter()
					.find(|(_, roster_name)| same_name(roster_name, &name))
				{
					assignment.number = Some(*number);
				}
			}
			None => {}
		}
		role.assignments.push(assignment);
	}
	if role.assignments.is_empty() {
		None
	} else {
		Some(role)
	}
}

fn read_rows(
	form: &PdfForm,
	consumed: &mut BTreeSet<String>,
	prefix: &str,
	count: u32,
	fields: &[&str],
) -> Vec<BTreeMap<String, String>> {
	let mut rows = Vec::new();
	for index in 1..=count {
		let mut row = BTreeMap::new();
		for field in fields {
			let key = format!("{prefix}.{index}.{field}");
			let value = read(form, consumed, &key);
			if !value.is_empty() {
				row.insert((*field).to_string(), value);
			}
		}
		if !row.is_empty() {
			rows.push(row);
		}
	}
	rows
}

fn split_name(full_name: &str) -> (String, String, String) {
	let parts: Vec<&str> = full_name.split_whitespace().collect();
	if parts.is_empty() {
		return (String::new(), String::new(), String::new());
	}
	if parts.len() == 1 {
		return (parts[0].to_string(), String::new(), String::new());
	}
	let suffix = matches!(
		parts.last().copied().unwrap_or_default(),
		"Jr." | "Jr" | "Sr." | "Sr" | "II" | "III" | "IV"
	);
	let last_index = if suffix && parts.len() >= 3 {
		parts.len() - 2
	} else {
		parts.len() - 1
	};
	let first = parts[0].to_string();
	let middle = parts[1..last_index].join(" ");
	let mut last = parts[last_index].to_string();
	if suffix {
		last.push(' ');
		last.push_str(parts.last().copied().unwrap_or_default());
	}
	(first, middle, last)
}

fn is_minor(date_of_birth: &str) -> bool {
	let digits: String = date_of_birth.chars().filter(|char| char.is_ascii_digit()).collect();
	if digits.len() != 8 {
		return false;
	}
	let month = digits[0..2].parse::<u32>().ok();
	let day = digits[2..4].parse::<u32>().ok();
	let year = digits[4..8].parse::<i32>().ok();
	let (Some(month), Some(day), Some(year)) = (month, day, year) else {
		return false;
	};
	let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
		return false;
	};
	let today = chrono::Utc::now().date_naive();
	let mut age = today.year() - date.year();
	if (today.month(), today.day()) < (date.month(), date.day()) {
		age -= 1;
	}
	age < 18
}

fn same_name(left: &str, right: &str) -> bool {
	normalize_name(left) == normalize_name(right)
}

fn normalize_name(value: &str) -> String {
	value
		.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ")
		.to_lowercase()
}

// ─── Apply ─────────────────────────────────────────────────────────────────

const ENGAGEMENT_LETTER: &str =
	"01 - Engagement and Cover Letters/Engagement Letter Template.docx";
const GPOA_TEMPLATE: &str =
	"02 - Power of Attorney and Medical Templates/GPOA Template.docx";
const HPOA_TEMPLATE: &str =
	"02 - Power of Attorney and Medical Templates/HPOA Template.docx";
const HIPAA_TEMPLATE: &str =
	"02 - Power of Attorney and Medical Templates/HIPAA Template.docx";
const LAST_WILL_TEMPLATE: &str =
	"03 - Will Templates/Last Will and Testament Template.docx";
const POUR_OVER_TEMPLATE: &str =
	"03 - Will Templates/Pour Over Will Template.docx";
const TRUST_TEMPLATE: &str = "04 - Trust Templates/Trust Agreement Template.docx";

/// What a successful import created.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedQuestionnaireClient {
	pub directory: String,
	pub folder_name: String,
	pub warnings: Vec<String>,
	pub required_document_count: usize,
}

/// Create a client folder and `.lily` from a filled questionnaire.
#[tauri::command]
pub fn create_client_from_questionnaire(
	parent_dir: String,
	pdf_path: String,
	folder_name: Option<String>,
) -> Result<CreatedQuestionnaireClient, String> {
	let preview = preview_questionnaire(Path::new(&pdf_path))?;
	if preview.clients.is_empty() {
		return Err("The questionnaire has no client name.".to_string());
	}
	let folder = folder_name
		.filter(|name| !name.trim().is_empty())
		.map(|name| sanitize_folder_name(&name))
		.unwrap_or_else(|| suggested_folder_name(&preview));
	let parent = Path::new(&parent_dir);
	if !parent.is_dir() {
		return Err(format!("Client library is not a directory: {}", parent_dir));
	}
	let directory = parent.join(&folder);
	if directory.exists() {
		return Err(format!("Client folder already exists: {}", directory.display()));
	}
	std::fs::create_dir(&directory)
		.map_err(|error| format!("Failed to create {}: {}", directory.display(), error))?;

	let result = (|| -> Result<CreatedQuestionnaireClient, String> {
		let pdf_filename = "Questionnaire.pdf";
		std::fs::copy(&pdf_path, directory.join(pdf_filename))
			.map_err(|error| format!("Failed to store the questionnaire PDF: {}", error))?;
		let lily = build_client_lily(&preview, pdf_filename);
		crate::lily_file::write_client_lily(&directory.to_string_lossy(), &lily)?;
		crate::lily_file::resolve_contact_variables(directory.to_string_lossy().to_string())?;
		Ok(CreatedQuestionnaireClient {
			directory: directory.to_string_lossy().to_string(),
			folder_name: folder.clone(),
			warnings: preview.warnings.clone(),
			required_document_count: lily.required_documents.len(),
		})
	})();
	if result.is_err() {
		let _ = std::fs::remove_dir_all(&directory);
	}
	result
}

fn suggested_folder_name(preview: &QuestionnairePreview) -> String {
	let client = |key: &str| preview.clients.iter().find(|client| client.key == key);
	let named = |client: &PreviewClient| {
		format!(
			"{}, {}",
			sanitize_folder_name(&client.last_name),
			sanitize_folder_name(&client.first_name)
		)
	};
	match (client("a"), client("b")) {
		(Some(a), Some(b)) if !b.first_name.is_empty() && !b.last_name.is_empty() => {
			format!("{} and {}", named(a), named(b))
		}
		(Some(a), _) => named(a),
		(None, Some(b)) => named(b),
		(None, None) => "New Client".to_string(),
	}
}

fn sanitize_folder_name(value: &str) -> String {
	let mut result: String = value
		.chars()
		.map(|character| match character {
			'<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '#' | '^' | '[' | ']' => '-',
			control if control.is_control() => '-',
			other => other,
		})
		.collect();
	while result.ends_with(' ') || result.ends_with('.') {
		result.pop();
	}
	if result.trim().is_empty() {
		"New Client".to_string()
	} else {
		result
	}
}

/// Build the `.lily` content for a parsed questionnaire.
pub fn build_client_lily(preview: &QuestionnairePreview, pdf_filename: &str) -> LilyFile {
	let (contacts, roster_ids, name_ids) = build_contacts(preview);
	let mut lily = LilyFile {
		lily_type: Some("client".to_string()),
		workflow_stage: crate::lily_file::WorkflowStage::Engagement,
		..LilyFile::default()
	};

	for (key, value) in client_variables(preview) {
		lily.variables.insert(key, value);
	}
	if preview.clients.iter().any(|client| client.key == "b") {
		let profile = preview
			.clients
			.iter()
			.find(|client| client.key == "b")
			.map(client_profile)
			.unwrap_or_default();
		lily.co_client = Some(profile);
	}
	lily.contacts = contacts;
	lily.questionnaire_pdf = Some(crate::lily_file::QuestionnairePdf {
		filename: pdf_filename.to_string(),
		sha256: preview.sha256.clone(),
		imported_at: chrono::Utc::now(),
		field_map_version: preview.field_map_version,
	});
	lily.estate = Some(crate::lily_file::EstateWorksheet {
		real_property: preview.estate.real_property.clone(),
		accounts: preview.estate.accounts.clone(),
		insurance: preview.estate.insurance.clone(),
		businesses: preview.estate.businesses.clone(),
		liabilities: preview.estate.liabilities.clone(),
		gifts: preview.estate.gifts.clone(),
		notes: preview.estate.notes.clone(),
	});

	let joint = preview.clients.iter().any(|client| client.key == "b");
	let trust = plan_includes_trust(preview);
	let mut bindings = HashMap::new();
	let mut client_roles: BTreeMap<String, BTreeMap<String, crate::lily_file::ContactBinding>> =
		BTreeMap::new();
	for key in ["a", "b"] {
		let Some(role) = preview.roles.get(&format!("{key}.fpoa")) else {
			continue;
		};
		if let Some(binding) = binding_for_role(role, &roster_ids, &name_ids) {
			client_roles
				.entry(key.to_string())
				.or_default()
				.insert("FPOA Agent".to_string(), binding.clone());
			if key == "a" {
				bindings.insert("FPOA Agent".to_string(), binding);
			}
		}
	}
	for key in ["a", "b"] {
		let Some(role) = preview.roles.get(&format!("{key}.hpoa")) else {
			continue;
		};
		if let Some(binding) = binding_for_role(role, &roster_ids, &name_ids) {
			client_roles
				.entry(key.to_string())
				.or_default()
				.insert("HPOA Agent".to_string(), binding.clone());
			if key == "a" {
				bindings.insert("HPOA Agent".to_string(), binding);
			}
		}
	}
	for key in ["a", "b"] {
		let Some(role) = preview.roles.get(&format!("{key}.pr")) else {
			continue;
		};
		if let Some(binding) = binding_for_role(role, &roster_ids, &name_ids) {
			client_roles
				.entry(key.to_string())
				.or_default()
				.insert("Personal Representative".to_string(), binding.clone());
			if key == "a" {
				bindings.insert("Personal Representative".to_string(), binding);
			}
		}
	}
	for key in ["a", "b"] {
		let Some(role) = preview.roles.get(&format!("{key}.hipaa")) else {
			continue;
		};
		if let Some(binding) = binding_for_names(&role.assignments, &roster_ids, &name_ids) {
			client_roles
				.entry(key.to_string())
				.or_default()
				.insert("Additional HIPAA Releases".to_string(), binding);
		}
	}
	if let Some(binding) = binding_for_names(
		&preview.household.trustees,
		&roster_ids,
		&name_ids,
	) {
		bindings.insert("Successor Trustee".to_string(), binding);
	}
	if let Some(binding) = binding_for_names(
		&preview.household.guardians,
		&roster_ids,
		&name_ids,
	) {
		bindings.insert("Guardian".to_string(), binding);
	}
	lily.contact_bindings = bindings;
	lily.client_roles = client_roles;

	if let Some(role) = preview.roles.get("a.hipaa") {
		let names: Vec<&str> = role
			.assignments
			.iter()
			.filter(|assignment| !assignment.name.is_empty())
			.map(|assignment| assignment.name.as_str())
			.collect();
		if !names.is_empty() {
			lily.variables
				.insert("Additional HIPAA Releases".to_string(), names.join("; "));
			lily.variables
				.insert("Has Additional HIPAA Releases".to_string(), "true".to_string());
		}
	}
	if trust {
		lily.variables
			.insert("Plan Includes Trust".to_string(), "true".to_string());
		if !preview.household.trust_name.is_empty() {
			lily.variables.insert(
				"Pour-Over Trust Name".to_string(),
				preview.household.trust_name.clone(),
			);
			lily.variables.insert(
				"Trust Name".to_string(),
				preview.household.trust_name.clone(),
			);
		}
	}
	if !preview.household.distribution_summary.is_empty() {
		lily.variables.insert(
			"Estate Distribution Summary".to_string(),
			preview.household.distribution_summary.clone(),
		);
	}
	if !preview.household.distribution_instructions.is_empty() {
		lily.variables.insert(
			"Distribution Instructions".to_string(),
			preview.household.distribution_instructions.clone(),
		);
	}
	if !preview.household.tangible_recipient.is_empty() {
		lily.variables.insert(
			"Tangible Property Recipient(s)".to_string(),
			preview.household.tangible_recipient.clone(),
		);
	}

	lily.required_documents = required_documents(preview, joint, trust);
	lily
}

fn client_variables(preview: &QuestionnairePreview) -> Vec<(String, String)> {
	let Some(client) = preview.clients.iter().find(|client| client.key == "a") else {
		return Vec::new();
	};
	let full_name = [client.first_name.as_str(), client.last_name.as_str()]
		.iter()
		.filter(|part| !part.is_empty())
		.copied()
		.collect::<Vec<_>>()
		.join(" ");
	vec![
		("Client First Name".to_string(), client.first_name.clone()),
		("Client Middle Name".to_string(), client.middle_name.clone()),
		("Client Last Name".to_string(), client.last_name.clone()),
		("Client Full Name".to_string(), full_name),
		("Client Phone".to_string(), client.phone.clone()),
		("Client Email".to_string(), client.email.clone()),
		("Client Address".to_string(), client.address.clone()),
		("Client City".to_string(), client.city.clone()),
		("Client State".to_string(), client.state.clone()),
		("Client Zip".to_string(), client.zip.clone()),
		("Client County".to_string(), client.county.clone()),
	]
}

fn client_profile(client: &PreviewClient) -> crate::lily_file::ClientProfile {
	crate::lily_file::ClientProfile {
		first_name: client.first_name.clone(),
		middle_name: client.middle_name.clone(),
		last_name: client.last_name.clone(),
		other_names: client.other_names.clone(),
		preferred_name: client.preferred_name.clone(),
		date_of_birth: client.date_of_birth.clone(),
		phone: client.phone.clone(),
		email: client.email.clone(),
		address: client.address.clone(),
		city: client.city.clone(),
		state: client.state.clone(),
		zip: client.zip.clone(),
		county: client.county.clone(),
	}
}

type ContactIndex = (
	Vec<crate::lily_file::Contact>,
	BTreeMap<u32, String>,
	BTreeMap<String, String>,
);

fn build_contacts(preview: &QuestionnairePreview) -> ContactIndex {
	let mut contacts: Vec<crate::lily_file::Contact> = Vec::new();
	let mut roster_ids = BTreeMap::new();
	for person in &preview.people {
		let contact = crate::lily_file::Contact {
			id: Uuid::new_v4().to_string(),
			full_name: person.full_name.clone(),
			first_name: person.first_name.clone(),
			middle_name: person.middle_name.clone(),
			last_name: person.last_name.clone(),
			relationship: normalize_relationship(&person.relationship),
			other_relationship: if person.other_relationship.is_empty() {
				person.relationship.clone()
			} else {
				person.other_relationship.clone()
			},
			phone: person.phone.clone(),
			email: person.email.clone(),
			address: person.address.clone(),
			city: String::new(),
			state: String::new(),
			zip: String::new(),
			is_minor: person.is_minor,
			preferred_name: String::new(),
			other_names: String::new(),
			date_of_birth: person.date_of_birth.clone(),
			roster_number: Some(person.number),
		};
		roster_ids.insert(person.number, contact.id.clone());
		contacts.push(contact);
	}
	let mut client_ids = BTreeMap::new();
	for client in &preview.clients {
		let full_name = [
			client.first_name.as_str(),
			client.middle_name.as_str(),
			client.last_name.as_str(),
		]
		.iter()
		.filter(|part| !part.is_empty())
		.copied()
		.collect::<Vec<_>>()
		.join(" ");
		let existing = contacts.iter_mut().find(|contact| {
			same_name(&contact.full_name, &full_name) && !full_name.is_empty()
		});
		let contact = match existing {
			Some(contact) => contact,
			None => {
				contacts.push(crate::lily_file::Contact {
					id: Uuid::new_v4().to_string(),
					full_name: full_name.clone(),
					first_name: client.first_name.clone(),
					middle_name: client.middle_name.clone(),
					last_name: client.last_name.clone(),
					relationship: if client.key == "b" {
						"Spouse".to_string()
					} else {
						"Client".to_string()
					},
					other_relationship: String::new(),
					phone: client.phone.clone(),
					email: client.email.clone(),
					address: client.address.clone(),
					city: client.city.clone(),
					state: client.state.clone(),
					zip: client.zip.clone(),
					is_minor: false,
					preferred_name: client.preferred_name.clone(),
					other_names: client.other_names.clone(),
					date_of_birth: client.date_of_birth.clone(),
					roster_number: None,
				});
				contacts.last_mut().expect("contact just pushed")
			}
		};
		contact.phone = non_empty(&client.phone, &contact.phone);
		contact.email = non_empty(&client.email, &contact.email);
		contact.address = non_empty(&client.address, &contact.address);
		contact.city = non_empty(&client.city, &contact.city);
		contact.state = non_empty(&client.state, &contact.state);
		contact.zip = non_empty(&client.zip, &contact.zip);
		contact.preferred_name = client.preferred_name.clone();
		contact.other_names = client.other_names.clone();
		contact.date_of_birth = client.date_of_birth.clone();
		if client.key == "b" {
			contact.relationship = "Spouse".to_string();
		}
		client_ids.insert(client.key.clone(), contact.id.clone());
	}
	let mut name_ids = BTreeMap::new();
	for contact in &contacts {
		name_ids.insert(normalize_name(&contact.full_name), contact.id.clone());
	}
	(contacts, roster_ids, name_ids)
}

fn normalize_relationship(value: &str) -> String {
	match value.trim().to_lowercase().as_str() {
		"daughter" | "son" | "child" | "stepdaughter" | "stepson" => "Child".to_string(),
		"spouse" => "Spouse".to_string(),
		"" => String::new(),
		_ => value.trim().to_string(),
	}
}

fn non_empty(value: &str, fallback: &str) -> String {
	if value.trim().is_empty() {
		fallback.to_string()
	} else {
		value.to_string()
	}
}

fn binding_for_role(
	role: &PreviewRole,
	roster: &BTreeMap<u32, String>,
	names: &BTreeMap<String, String>,
) -> Option<crate::lily_file::ContactBinding> {
	let assignments: Vec<crate::lily_file::RoleAssignment> = role
		.assignments
		.iter()
		.filter_map(|assignment| {
			// Prefer the written name when it matches a known contact; a
			// roster number that disagrees with the name is a form error, and
			// the name is the human's actual answer.
			let id = names
				.get(&normalize_name(&assignment.name))
				.cloned()
				.or_else(|| assignment.number.and_then(|number| roster.get(&number).cloned()))?;
			Some(crate::lily_file::RoleAssignment {
				contact_ids: vec![id],
				values: HashMap::new(),
			})
		})
		.collect();
	if assignments.is_empty() {
		return None;
	}
	Some(crate::lily_file::ContactBinding {
		contact_id: assignments
			.first()
			.and_then(|assignment| assignment.contact_ids.first().cloned()),
		variable_mappings: HashMap::new(),
		contact_ids: None,
		assignments: Some(assignments),
	})
}

fn binding_for_names(
	assignments: &[PreviewAssignment],
	roster: &BTreeMap<u32, String>,
	names: &BTreeMap<String, String>,
) -> Option<crate::lily_file::ContactBinding> {
	let ids: Vec<String> = assignments
		.iter()
		.filter_map(|assignment| {
			names
				.get(&normalize_name(&assignment.name))
				.cloned()
				.or_else(|| assignment.number.and_then(|number| roster.get(&number).cloned()))
		})
		.collect();
	if ids.is_empty() {
		return None;
	}
	Some(crate::lily_file::ContactBinding {
		contact_id: ids.first().cloned(),
		variable_mappings: HashMap::new(),
		contact_ids: Some(ids.clone()),
		assignments: Some(vec![crate::lily_file::RoleAssignment {
			contact_ids: ids,
			values: HashMap::new(),
		}]),
	})
}

fn plan_includes_trust(preview: &QuestionnairePreview) -> bool {
	!preview.household.trust_name.trim().is_empty()
		|| !preview.household.trustees.is_empty()
		|| preview
			.household
			.children
			.iter()
			.any(|child| child.in_trust)
}

fn required_documents(
	preview: &QuestionnairePreview,
	joint: bool,
	trust: bool,
) -> Vec<crate::lily_file::RequiredDocument> {
	let mut documents = Vec::new();
	let a = preview.clients.iter().find(|client| client.key == "a");
	let b = preview.clients.iter().find(|client| client.key == "b");
	if let Some(a) = a {
		let name = if joint {
			format!(
				"{} {} and {} {}",
				a.first_name,
				a.last_name,
				b.map(|b| b.first_name.as_str()).unwrap_or_default(),
				b.map(|b| b.last_name.as_str()).unwrap_or_default()
			)
		} else {
			format!("{} {}", a.first_name, a.last_name)
		};
		documents.push(required_document(
			ENGAGEMENT_LETTER,
			"Engagement Letter",
			&name,
			None,
			true,
		));
	}
	for (key, client) in [("a", a), ("b", b)] {
		let Some(client) = client else {
			continue;
		};
		let name = format!("{} {}", client.first_name, client.last_name);
		let ready = true;
		if preview.roles.contains_key(&format!("{key}.fpoa")) {
			documents.push(required_document(
				GPOA_TEMPLATE,
				"GPOA",
				&name,
				Some(key),
				ready,
			));
		}
		if preview.roles.contains_key(&format!("{key}.hpoa")) {
			documents.push(required_document(
				HPOA_TEMPLATE,
				"HPOA",
				&name,
				Some(key),
				ready,
			));
		}
		if preview.roles.contains_key(&format!("{key}.hipaa"))
			|| preview.roles.contains_key(&format!("{key}.hpoa"))
		{
			documents.push(required_document(
				HIPAA_TEMPLATE,
				"HIPAA",
				&name,
				Some(key),
				ready,
			));
		}
		documents.push(required_document(
			LAST_WILL_TEMPLATE,
			"Last Will",
			&name,
			Some(key),
			ready,
		));
		if trust {
			documents.push(required_document(
				POUR_OVER_TEMPLATE,
				"Pour Over Will",
				&name,
				Some(key),
				ready,
			));
		}
	}
	if trust {
		let name = a
			.map(|client| format!("{} {}", client.first_name, client.last_name))
			.unwrap_or_default();
		documents.push(required_document(
			TRUST_TEMPLATE,
			"Trust Agreement",
			&name,
			None,
			true,
		));
	}
	documents
}

fn required_document(
	template: &str,
	stem: &str,
	client_name: &str,
	client_key: Option<&str>,
	draft_ready: bool,
) -> crate::lily_file::RequiredDocument {
	let filename = if client_name.trim().is_empty() {
		format!("{stem}.docx")
	} else {
		format!("{stem} - {}.docx", client_name.trim())
	};
	crate::lily_file::RequiredDocument {
		id: Uuid::new_v4().to_string(),
		template_rel_path: template.to_string(),
		status: crate::lily_file::DocumentStatus::NotStarted,
		document_filename: None,
		proposed_filename: Some(filename),
		notes: if draft_ready {
			String::new()
		} else {
			"Waiting on per-client role assignments.".to_string()
		},
		signed_artifacts: Vec::new(),
		client_key: client_key.map(str::to_string),
		draft_ready,
	}
}

// ─── Re-import ─────────────────────────────────────────────────────────────

/// A preview of what re-importing would change on an existing client.
#[derive(Debug, Clone, Serialize)]
pub struct QuestionnaireUpdateReport {
	pub preview: QuestionnairePreview,
	pub variables_added: Vec<String>,
	pub variables_changed: Vec<String>,
	pub contacts_added: usize,
	pub contacts_updated: usize,
	pub documents_added: usize,
	pub documents_updated: usize,
}

#[derive(Debug, Clone, Default)]
struct UpdateCounts {
	variables_added: Vec<String>,
	variables_changed: Vec<String>,
	contacts_added: usize,
	contacts_updated: usize,
	documents_added: usize,
	documents_updated: usize,
}

/// Preview a re-import without writing anything.
#[tauri::command]
pub fn preview_questionnaire_update(
	working_dir: String,
	pdf_path: String,
) -> Result<QuestionnaireUpdateReport, String> {
	let preview = preview_questionnaire(Path::new(&pdf_path))?;
	let mut lily = crate::lily_file::read_lily_file(&working_dir)?;
	let counts = merge_questionnaire(&mut lily, &preview);
	Ok(QuestionnaireUpdateReport {
		preview,
		variables_added: counts.variables_added,
		variables_changed: counts.variables_changed,
		contacts_added: counts.contacts_added,
		contacts_updated: counts.contacts_updated,
		documents_added: counts.documents_added,
		documents_updated: counts.documents_updated,
	})
}

/// Apply the re-import to an existing client.
#[tauri::command]
pub fn apply_questionnaire_update(
	working_dir: String,
	pdf_path: String,
) -> Result<QuestionnaireUpdateReport, String> {
	let preview = preview_questionnaire(Path::new(&pdf_path))?;
	let mut lily = crate::lily_file::read_lily_file(&working_dir)?;
	let counts = merge_questionnaire(&mut lily, &preview);
	let destination = Path::new(&working_dir).join("Questionnaire.pdf");
	if Path::new(&pdf_path) != destination {
		if destination.exists() {
			let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f");
			let backup = destination.with_file_name(format!(
				"Questionnaire.pdf.bak-{}",
				stamp
			));
			std::fs::copy(&destination, backup).map_err(|error| {
				format!("Failed to back up the previous questionnaire PDF: {}", error)
			})?;
		}
		std::fs::copy(&pdf_path, &destination)
			.map_err(|error| format!("Failed to store the questionnaire PDF: {}", error))?;
	}
	crate::lily_file::write_client_lily(&working_dir, &lily)?;
	crate::lily_file::resolve_contact_variables(working_dir.clone())?;
	Ok(QuestionnaireUpdateReport {
		preview,
		variables_added: counts.variables_added,
		variables_changed: counts.variables_changed,
		contacts_added: counts.contacts_added,
		contacts_updated: counts.contacts_updated,
		documents_added: counts.documents_added,
		documents_updated: counts.documents_updated,
	})
}

const MANAGED_VARIABLES: &[&str] = &[
	"Plan Includes Trust",
	"Pour-Over Trust Name",
	"Trust Name",
	"Estate Distribution Summary",
	"Distribution Instructions",
	"Tangible Property Recipient(s)",
	"Additional HIPAA Releases",
	"Has Additional HIPAA Releases",
];

fn merge_questionnaire(lily: &mut LilyFile, preview: &QuestionnairePreview) -> UpdateCounts {
	let fresh = build_client_lily(preview, "Questionnaire.pdf");
	let mut counts = UpdateCounts::default();

	// Clear the keys this importer owns so a cleared PDF answer disappears
	// instead of leaving the previous value behind.
	for key in MANAGED_VARIABLES {
		lily.variables.remove(*key);
	}

	for (key, value) in &fresh.variables {
		match lily.variables.get(key) {
			None => {
				counts.variables_added.push(key.clone());
				lily.variables.insert(key.clone(), value.clone());
			}
			Some(existing) if existing != value => {
				counts.variables_changed.push(key.clone());
				lily.variables.insert(key.clone(), value.clone());
			}
			Some(_) => {}
		}
	}

	let mut id_map: HashMap<String, String> = HashMap::new();
	for fresh_contact in &fresh.contacts {
		let existing = lily.contacts.iter_mut().find(|contact| {
			same_name(&contact.full_name, &fresh_contact.full_name)
		});
		if let Some(existing) = existing {
			id_map.insert(fresh_contact.id.clone(), existing.id.clone());
			let updated = crate::lily_file::Contact {
				id: existing.id.clone(),
				..fresh_contact.clone()
			};
			if *existing != updated {
				counts.contacts_updated += 1;
				*existing = updated;
			}
		} else {
			id_map.insert(fresh_contact.id.clone(), fresh_contact.id.clone());
			counts.contacts_added += 1;
			lily.contacts.push(fresh_contact.clone());
		}
	}

	lily.contact_bindings = fresh
		.contact_bindings
		.into_iter()
		.map(|(role, binding)| (role, remap_binding(binding, &id_map)))
		.collect();
	lily.client_roles = fresh
		.client_roles
		.into_iter()
		.map(|(client, roles)| {
			(
				client,
				roles
					.into_iter()
					.map(|(role, binding)| (role, remap_binding(binding, &id_map)))
					.collect(),
			)
		})
		.collect();
	lily.co_client = fresh.co_client;
	lily.estate = fresh.estate;
	lily.questionnaire_pdf = fresh.questionnaire_pdf;

	for fresh_document in fresh.required_documents {
		let existing = lily.required_documents.iter_mut().find(|document| {
			document.template_rel_path == fresh_document.template_rel_path
				&& document.client_key == fresh_document.client_key
		});
		if let Some(existing) = existing {
			let mut changed = false;
			if existing.proposed_filename != fresh_document.proposed_filename {
				existing.proposed_filename = fresh_document.proposed_filename;
				changed = true;
			}
			if existing.draft_ready != fresh_document.draft_ready {
				existing.draft_ready = fresh_document.draft_ready;
				changed = true;
			}
			if changed {
				counts.documents_updated += 1;
			}
		} else {
			counts.documents_added += 1;
			lily.required_documents.push(fresh_document);
		}
	}

	counts
}

fn remap_binding(
	mut binding: crate::lily_file::ContactBinding,
	ids: &HashMap<String, String>,
) -> crate::lily_file::ContactBinding {
	if let Some(id) = &binding.contact_id {
		if let Some(mapped) = ids.get(id) {
			binding.contact_id = Some(mapped.clone());
		}
	}
	if let Some(contact_ids) = &mut binding.contact_ids {
		for id in contact_ids {
			if let Some(mapped) = ids.get(id) {
				*id = mapped.clone();
			}
		}
	}
	if let Some(assignments) = &mut binding.assignments {
		for assignment in assignments {
			for id in &mut assignment.contact_ids {
				if let Some(mapped) = ids.get(id) {
					*id = mapped.clone();
				}
			}
		}
	}
	binding
}
