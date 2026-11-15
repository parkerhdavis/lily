// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deterministic document plan renderer.
//!
//! A plan is a JSON document of ordered blocks, inline content nodes, clauses,
//! slots, and defined terms. This module resolves the plan against a client
//! `.lily` file and emits a `.docx` by replacing the body of a Word layout
//! skeleton. It makes no model calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::history::VersionDiffLine;
use crate::lily_file::{self, Contact, LilyFile, RoleAssignment};

static SDT_RE: LazyLock<regex::Regex> =
	LazyLock::new(|| regex::Regex::new(r"(?s)<w:sdt>.*?</w:sdt>").expect("valid sdt regex"));
static TAG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r#"<w:tag w:val="lily:([^"]+)"/>"#).expect("valid tag regex")
});
static T_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)(<w:t(?:\s[^>]*)?>)(.*?)(</w:t>)").expect("valid text regex")
});

fn default_lily_type() -> String {
	"document-plan".to_string()
}

fn default_version() -> u32 {
	1
}

fn default_block_kind() -> String {
	"paragraph".to_string()
}

/// A whole document plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentPlan {
	#[serde(default = "default_lily_type")]
	pub lily_type: String,
	pub id: String,
	#[serde(default = "default_version")]
	pub version: u32,
	#[serde(default)]
	pub title: String,
	#[serde(default)]
	pub layout: String,
	#[serde(default)]
	pub blocks: Vec<Block>,
	#[serde(default)]
	pub clauses: HashMap<String, Clause>,
	#[serde(default)]
	pub slots: HashMap<String, Slot>,
	#[serde(default)]
	pub terms: HashMap<String, Term>,
	/// List-group definitions keyed by the `list` name used on blocks.
	#[serde(default)]
	pub lists: HashMap<String, ListGroup>,
	/// Compiler metadata, including the source template path.
	#[serde(default)]
	pub metadata: Option<Value>,
	/// Directory the plan was read from, used to resolve excerpt paths.
	#[serde(skip)]
	pub base_dir: PathBuf,
}

/// One ordered unit of the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
	pub id: String,
	#[serde(default = "default_block_kind")]
	pub kind: String,
	#[serde(default)]
	pub style: String,
	#[serde(default)]
	pub content: Vec<ContentNode>,
	#[serde(default)]
	pub when: Option<Value>,
	/// Numbering scheme for this block: "article", "section", or "subsection".
	#[serde(default)]
	pub numbering: Option<String>,
	/// Index of the template paragraph this block came from, used to inherit
	/// the source paragraph's direct formatting.
	#[serde(default)]
	pub source_paragraph: Option<usize>,
	/// The list group this block belongs to, when its trailing separator is
	/// chosen from the enabled items rather than written into the content.
	#[serde(default)]
	pub list: Option<String>,
	/// Keep this paragraph on the same page as the next one.
	#[serde(default)]
	pub keep_with_next: bool,
	/// Start this paragraph on a new page.
	#[serde(default)]
	pub page_break_before: bool,
	/// Keep this paragraph's lines on one page.
	#[serde(default)]
	pub keep_together: bool,
	/// Path to a compiled excerpt plan, relative to this plan. Used by
	/// `kind: "excerpt"` blocks.
	#[serde(default)]
	pub path: Option<String>,
	/// Rows of cells for a table block.
	#[serde(default)]
	pub rows: Vec<Vec<TableCell>>,
}

/// Separators for a group of blocks joined into one list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListGroup {
	#[serde(default)]
	pub separator: String,
	#[serde(default)]
	pub last_separator: String,
	#[serde(default)]
	pub terminator: String,
}

/// One cell in a table block. The content is inline; a cell with multiple
/// paragraphs can use explicit line breaks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableCell {
	#[serde(default)]
	pub content: Vec<ContentNode>,
	#[serde(default)]
	pub style: String,
}

/// Direct paragraph and first-run formatting inherited from a source paragraph.
#[derive(Debug, Clone, Default)]
pub struct SourceFormatting {
	pub ppr: String,
	pub rpr: String,
}

static SOURCE_PARAGRAPH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)<w:p(?:\s[^>]*)?>.*?</w:p>").expect("valid source paragraph regex")
});
static SOURCE_PPR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)<w:pPr>.*?</w:pPr>").expect("valid paragraph properties regex")
});
static SOURCE_RUN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)<w:r(?:\s[^>]*)?>.*?</w:r>").expect("valid run regex")
});
static SOURCE_RPR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)<w:rPr>.*?</w:rPr>").expect("valid run properties regex")
});
static NUM_PR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)<w:numPr>.*?</w:numPr>").expect("valid numbering properties regex")
});
static PPR_AT_START_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)^<w:pPr(?:\s[^>]*)?>.*?</w:pPr>|^<w:pPr(?:\s[^>]*)?/>")
		.expect("valid paragraph properties regex")
});
static RUN_AT_START_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)^<w:r(?:\s[^>]*)?>.*?</w:r>").expect("valid run regex")
});
static RPR_AT_START_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)^<w:rPr(?:\s[^>]*)?>.*?</w:rPr>|^<w:rPr(?:\s[^>]*)?/>")
		.expect("valid run properties regex")
});
static SDT_AT_START_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)^<w:sdt>.*?</w:sdt>").expect("valid inline sdt regex")
});
static SDT_CONTENT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(r"(?s)<w:sdtContent>(.*?)</w:sdtContent>")
		.expect("valid sdt content regex")
});
static RUN_PIECE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(
		r#"(?s)^<w:t(?:\s[^>]*)?>.*?</w:t>|^<w:t(?:\s[^>]*)?/>|^<w:tab\s*/>|^<w:br\s+w:type="page"\s*/>|^<w:br\s*/>|^<w:drawing(?:\s[^>]*)?>.*?</w:drawing>|^<w:pict(?:\s[^>]*)?>.*?</w:pict>|^<w:object(?:\s[^>]*)?>.*?</w:object>|^<w:[A-Za-z0-9]+(?:\s[^>]*)?/>|^<w:[A-Za-z0-9]+(?:\s[^>]*)?>.*?</w:[A-Za-z0-9]+>"#,
	)
	.expect("valid run piece regex")
});

/// Extract the direct paragraph properties and first-run properties from a
/// document body, one entry per paragraph, in document order.
pub fn extract_source_formatting(document_xml: &str) -> Vec<SourceFormatting> {
	SOURCE_PARAGRAPH_RE
		.find_iter(document_xml)
		.map(|source| {
			let paragraph = source.as_str();
			let ppr = SOURCE_PPR_RE
				.find(paragraph)
				.map(|matched| matched.as_str().to_string())
				.unwrap_or_default();
			let rpr = SOURCE_RUN_RE
				.find(paragraph)
				.and_then(|run| SOURCE_RPR_RE.find(run.as_str()))
				.map(|matched| matched.as_str().to_string())
				.unwrap_or_default();
			SourceFormatting { ppr, rpr }
		})
		.collect()
}

/// Inline content inside a block or clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentNode {
	Text {
		text: String,
		#[serde(default)]
		marks: Vec<String>,
	},
	Slot {
		slot: String,
		#[serde(default)]
		format: Option<String>,
		#[serde(default)]
		marks: Vec<String>,
	},
	Role {
		role: String,
		#[serde(default)]
		when: Option<Value>,
		#[serde(default)]
		property: Option<String>,
		#[serde(default)]
		intro: Option<String>,
		#[serde(default)]
		shared_intro: Option<String>,
		#[serde(default)]
		successor: Option<String>,
		#[serde(default)]
		shared_successor: Option<String>,
		#[serde(default)]
		conjunction: Option<String>,
		#[serde(default)]
		separator: Option<String>,
		#[serde(default)]
		oxford: Option<bool>,
		#[serde(default)]
		marks: Vec<String>,
	},
	Clause {
		clause: String,
		#[serde(default)]
		when: Option<Value>,
	},
	Term {
		term: String,
		#[serde(default)]
		form: Option<String>,
		#[serde(default)]
		number: Option<String>,
	},
	Ref {
		#[serde(rename = "ref")]
		target: String,
		#[serde(default)]
		format: Option<String>,
	},
	Number {
		number: String,
	},
	Tab {
		tab: bool,
	},
	LineBreak {
		#[serde(rename = "line_break")]
		line_break: bool,
		#[serde(default)]
		page: bool,
	},
}

/// A reusable piece of legal text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clause {
	pub id: String,
	#[serde(default = "default_version")]
	pub version: u32,
	#[serde(default)]
	pub title: String,
	pub body: Vec<ContentNode>,
	#[serde(default)]
	pub when: Option<Value>,
}

/// A declared client-data binding.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Slot {
	#[serde(default)]
	pub label: String,
	#[serde(rename = "type", default)]
	pub slot_type: String,
	#[serde(default)]
	pub required: bool,
	#[serde(default)]
	pub format: Option<String>,
	#[serde(default)]
	pub source: Option<Value>,
}

/// A defined legal term with grammatical forms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Term {
	pub id: String,
	#[serde(default)]
	pub forms: HashMap<String, String>,
	#[serde(default)]
	pub number: Option<Value>,
}

/// Client data plus the boolean facts the conditions evaluate against.
#[derive(Debug, Clone, Default)]
pub struct DataView {
	pub variables: HashMap<String, String>,
	pub facts: HashMap<String, bool>,
	pub lists: HashMap<String, Vec<String>>,
	pub contacts: HashMap<String, Contact>,
	pub roles: HashMap<String, Vec<RoleAssignment>>,
}

/// Convert one legacy single-contact or contact-list binding into an
/// assignment. Manual ("Other") values are read from the client variables.
fn binding_assignment(
	binding: &lily_file::ContactBinding,
	variables: &HashMap<String, String>,
) -> Option<RoleAssignment> {
	if let Some(ids) = &binding.contact_ids {
		if !ids.is_empty() {
			return Some(RoleAssignment {
				contact_ids: ids.clone(),
				values: HashMap::new(),
			});
		}
	}
	if let Some(id) = &binding.contact_id {
		if id != "__none__" && !id.is_empty() {
			return Some(RoleAssignment {
				contact_ids: vec![id.clone()],
				values: HashMap::new(),
			});
		}
		return None;
	}
	if !binding.variable_mappings.is_empty() {
		let mut values = HashMap::new();
		for (variable, property) in &binding.variable_mappings {
			if let Some(value) = variables.get(variable) {
				if !value.is_empty() {
					values.insert(property.clone(), value.clone());
				}
			}
		}
		if !values.is_empty() {
			return Some(RoleAssignment {
				contact_ids: Vec::new(),
				values,
			});
		}
	}
	None
}

impl DataView {
	pub fn from_lily(lily: &LilyFile) -> Self {
		let mut view = Self {
			variables: lily.variables.clone(),
			facts: HashMap::new(),
			lists: HashMap::new(),
			contacts: lily
				.contacts
				.iter()
				.map(|contact| (contact.id.clone(), contact.clone()))
				.collect(),
			roles: HashMap::new(),
		};
		for (role, binding) in &lily.contact_bindings {
			let assignments = match &binding.assignments {
				Some(assignments) => assignments.clone(),
				None => binding_assignment(binding, &lily.variables)
					.into_iter()
					.collect(),
			};
			if assignments.iter().any(|assignment| {
				!assignment.contact_ids.is_empty() || !assignment.values.is_empty()
			}) {
				view.roles.insert(role.clone(), assignments);
			}
		}
		// Older client files bind the trustee tiers to separate roles. Treat
		// them as the ordered list for the merged Successor Trustee role.
		if !view.roles.contains_key("Successor Trustee") {
			let mut assignments = Vec::new();
			for role in ["Primary Trustee", "Secondary Trustee", "Tertiary Trustee"] {
				if let Some(binding) = lily.contact_bindings.get(role) {
					if let Some(assignment) = binding_assignment(binding, &lily.variables) {
						assignments.push(assignment);
					}
				}
			}
			if !assignments.is_empty() {
				view.roles.insert("Successor Trustee".to_string(), assignments);
			}
		}
		// The personal-representative tiers and their co-representatives are
		// bound as separate roles. Fold each tier into one shared assignment
		// for the ordered Personal Representative role.
		if !view.roles.contains_key("Personal Representative") {
			let mut assignments = Vec::new();
			for (primary, co) in [
				(
					"Primary Personal Representative",
					"Primary Co-Personal Representative",
				),
				(
					"Secondary Personal Representative",
					"Secondary Co-Personal Representative",
				),
				(
					"Tertiary Personal Representative",
					"Tertiary Co-Personal Representative",
				),
			] {
				let Some(binding) = lily.contact_bindings.get(primary) else {
					break;
				};
				let Some(mut assignment) = binding_assignment(binding, &lily.variables) else {
					break;
				};
				if let Some(co_binding) = lily.contact_bindings.get(co) {
					if let Some(co_assignment) =
						binding_assignment(co_binding, &lily.variables)
					{
						assignment.contact_ids.extend(co_assignment.contact_ids);
						for (key, value) in co_assignment.values {
							assignment.values.entry(key).or_insert(value);
						}
					}
				}
				if assignment.contact_ids.is_empty() && assignment.values.is_empty() {
					break;
				}
				assignments.push(assignment);
			}
			if !assignments.is_empty() {
				view.roles
					.insert("Personal Representative".to_string(), assignments);
			}
		}
		// A deed designates its beneficiaries together, not in succession.
		// Fold the Will's primary and secondary beneficiary roles into one
		// shared assignment when the client has no deed-specific binding.
		if !view.roles.contains_key("Deed Beneficiary") {
			let mut contact_ids = Vec::new();
			for role in ["Primary Beneficiary", "Secondary Beneficiary"] {
				if let Some(binding) = lily.contact_bindings.get(role) {
					if let Some(assignment) = binding_assignment(binding, &lily.variables) {
						contact_ids.extend(assignment.contact_ids);
					}
				}
			}
			if !contact_ids.is_empty() {
				view.roles.insert(
					"Deed Beneficiary".to_string(),
					vec![RoleAssignment {
						contact_ids,
						values: HashMap::new(),
					}],
				);
			}
		}
		view.lists.insert(
			"required_documents".to_string(),
			lily.required_documents
				.iter()
				.map(|document| document.template_rel_path.clone())
				.collect(),
		);
		let variable_facts: Vec<(String, bool)> = view
			.variables
			.iter()
			.filter(|(_, value)| value.as_str() == "true" || value.as_str() == "false")
			.map(|(key, value)| (key.clone(), value == "true"))
			.collect();
		for (key, fact) in variable_facts {
			view.facts.insert(key.clone(), fact);
			view.facts.insert(snake_case(&key), fact);
		}
		if let Some(value) = view.variables.get("Has Spouse") {
			view.facts.insert("has_spouse".to_string(), value == "true");
		}
		if let Some(value) = view.variables.get("Client has Children") {
			view.facts
				.insert("has_children".to_string(), value == "true");
		}
		if let Some(value) = view.variables.get("Has Minor Children") {
			view.facts
				.insert("has_minor_children".to_string(), value == "true");
		}
		let children_count = lily
			.contacts
			.iter()
			.filter(|contact| contact.relationship.eq_ignore_ascii_case("Child"))
			.count();
		view.variables
			.insert("children_count".to_string(), children_count.to_string());

		let has_spouse = view
			.variables
			.get("Has Spouse")
			.map(|value| value == "true")
			.unwrap_or(false);
		let spouse_name = view
			.variables
			.get("Client Spouse Name")
			.cloned()
			.unwrap_or_default();
		let (term, possessive, verb, have, past, each) = if has_spouse {
			("Settlors", "Settlors\u{2019}", "are", "have", "were", "each Settlor")
		} else {
			("Settlor", "Settlor\u{2019}s", "is", "has", "was", "Settlor")
		};
		view.variables
			.insert("Settlor Term".to_string(), term.to_string());
		view.variables
			.insert("Settlor Possessive".to_string(), possessive.to_string());
		view.variables
			.insert("Settlor Verb".to_string(), verb.to_string());
		view.variables
			.insert("Settlor Have".to_string(), have.to_string());
		view.variables
			.insert("Settlor Verb Past".to_string(), past.to_string());
		view.variables
			.insert("Settlor Each".to_string(), each.to_string());
		view.variables.insert(
			"And Spouse Name".to_string(),
			if has_spouse {
				format!(" and {}", spouse_name)
			} else {
				String::new()
			},
		);
		view
	}

	/// Resolve a role into ordered groups of names. A group with more than one
	/// name is a shared assignment; each group is a successor tier.
	pub fn role_names(&self, role: &str, property: &str) -> Vec<Vec<String>> {
		self.roles
			.get(role)
			.map(|assignments| {
				assignments
					.iter()
					.filter_map(|assignment| {
						let names: Vec<String> = assignment
							.contact_ids
							.iter()
							.filter_map(|id| self.contacts.get(id))
							.map(|contact| lily_file::contact_property(contact, property))
							.filter(|name| !name.is_empty())
							.collect();
						if !names.is_empty() {
							return Some(names);
						}
						assignment
							.values
							.get(property)
							.filter(|value| !value.is_empty())
							.map(|value| vec![value.clone()])
					})
					.collect()
			})
			.unwrap_or_default()
	}

	pub fn get(&self, name: &str) -> Option<&str> {
		self.variables.get(name).map(String::as_str)
	}

	pub fn get_case_insensitive(&self, name: &str) -> Option<&str> {
		self.variables
			.iter()
			.find(|(key, _)| key.eq_ignore_ascii_case(name))
			.map(|(_, value)| value.as_str())
	}

	pub fn get_bool(&self, name: &str) -> bool {
		if let Some(value) = self.facts.get(name) {
			return *value;
		}
		self.variables
			.get(name)
			.map(|value| value == "true")
			.unwrap_or(false)
	}

	pub fn get_list(&self, name: &str) -> &[String] {
		self.lists.get(name).map(Vec::as_slice).unwrap_or(&[])
	}

	pub fn exists(&self, name: &str) -> bool {
		self.facts.contains_key(name)
			|| self
				.variables
				.get(name)
				.map(|value| !value.is_empty())
				.unwrap_or(false)
	}
}

fn snake_case(name: &str) -> String {
	name.chars()
		.map(|character| {
			if character.is_whitespace() {
				'_'
			} else {
				character.to_ascii_lowercase()
			}
		})
		.collect()
}

fn matches_value(name: &str, expected: &Value, data: &DataView) -> bool {
	match expected {
		Value::Bool(value) => data.get_bool(name) == *value,
		Value::String(value) => data
			.get(name)
			.map(|actual| actual == value)
			.unwrap_or(false),
		Value::Number(value) => {
			let expected = value.to_string();
			data.get(name)
				.map(|actual| actual == expected.as_str())
				.unwrap_or(false)
		}
		_ => false,
	}
}

/// Evaluate a plan condition. The language is deliberately small: `eq`, `neq`,
/// `and`, `or`, `not`, and `exists`, with a bare boolean accepted as itself.
pub fn eval_condition(condition: &Value, data: &DataView) -> Result<bool, String> {
	match condition {
		Value::Bool(value) => Ok(*value),
		Value::Object(map) => {
			if let Some(pair) = map.get("eq") {
				let values = pair
					.as_array()
					.ok_or_else(|| "eq expects [name, value]".to_string())?;
				if values.len() != 2 {
					return Err("eq expects exactly two values".to_string());
				}
				let name = values[0]
					.as_str()
					.ok_or_else(|| "eq name must be a string".to_string())?;
				Ok(matches_value(name, &values[1], data))
			} else if let Some(pair) = map.get("neq") {
				let values = pair
					.as_array()
					.ok_or_else(|| "neq expects [name, value]".to_string())?;
				if values.len() != 2 {
					return Err("neq expects exactly two values".to_string());
				}
				let name = values[0]
					.as_str()
					.ok_or_else(|| "neq name must be a string".to_string())?;
				Ok(!matches_value(name, &values[1], data))
			} else if let Some(pair) = map.get("includes") {
				let values = pair
					.as_array()
					.ok_or_else(|| "includes expects [name, value]".to_string())?;
				if values.len() != 2 {
					return Err("includes expects exactly two values".to_string());
				}
				let name = values[0]
					.as_str()
					.ok_or_else(|| "includes name must be a string".to_string())?;
				let needle = values[1]
					.as_str()
					.ok_or_else(|| "includes value must be a string".to_string())?;
				Ok(data.get_list(name).iter().any(|value| value == needle))
			} else if let Some(children) = map.get("and") {
				let children = children
					.as_array()
					.ok_or_else(|| "and expects an array".to_string())?;
				for child in children {
					if !eval_condition(child, data)? {
						return Ok(false);
					}
				}
				Ok(true)
			} else if let Some(children) = map.get("or") {
				let children = children
					.as_array()
					.ok_or_else(|| "or expects an array".to_string())?;
				for child in children {
					if eval_condition(child, data)? {
						return Ok(true);
					}
				}
				Ok(false)
			} else if let Some(child) = map.get("not") {
				Ok(!eval_condition(child, data)?)
			} else if let Some(name) = map.get("exists") {
				let name = name
					.as_str()
					.ok_or_else(|| "exists expects a name".to_string())?;
				Ok(data.exists(name))
			} else {
				Err(format!("Unknown condition: {}", condition))
			}
		}
		_ => Err(format!("Unknown condition: {}", condition)),
	}
}

pub fn apply_format(value: &str, format: Option<&str>) -> String {
	match format.unwrap_or("as-is") {
		"upper" => value.to_uppercase(),
		"lower" => value.to_lowercase(),
		"title" => title_case(value),
		_ => value.to_string(),
	}
}

fn title_case(value: &str) -> String {
	value
		.split_whitespace()
		.map(|word| {
			let mut characters = word.chars();
			match characters.next() {
				Some(first) => format!("{}{}", first.to_uppercase(), characters.as_str().to_lowercase()),
				None => String::new(),
			}
		})
		.collect::<Vec<_>>()
		.join(" ")
}

fn infer_format(name: &str) -> Option<&'static str> {
	let letters: String = name.chars().filter(|character| character.is_alphabetic()).collect();
	if letters.is_empty() {
		return None;
	}
	if letters.chars().all(|character| character.is_uppercase()) {
		Some("upper")
	} else if letters.chars().all(|character| character.is_lowercase()) {
		Some("lower")
	} else {
		None
	}
}

fn escape_xml(text: &str) -> String {
	text.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&apos;")
}

fn render_run(text: &str, marks: &[String], run_properties: Option<&str>) -> String {
	if text.is_empty() {
		return String::new();
	}
	if let Some(properties) = run_properties {
		if !properties.is_empty() {
			return format!(
				"<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
				properties,
				escape_xml(text)
			);
		}
	}
	let mut properties = String::new();
	for mark in marks {
		match mark.as_str() {
			"bold" => properties.push_str("<w:b/>"),
			"italic" => properties.push_str("<w:i/>"),
			"underline" => properties.push_str("<w:u w:val=\"single\"/>"),
			"small_caps" => properties.push_str("<w:smallCaps/>"),
			"caps" => properties.push_str("<w:caps/>"),
			_ => {}
		}
	}
	if properties.is_empty() {
		format!(
			"<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>",
			escape_xml(text)
		)
	} else {
		format!(
			"<w:r><w:rPr>{}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
			properties,
			escape_xml(text)
		)
	}
}

fn resolve_term(
	term: &Term,
	form: Option<&str>,
	number: Option<&str>,
	data: &DataView,
) -> String {
	let plural = match number {
		Some("plural") => true,
		Some("singular") => false,
		Some("auto") | None => term
			.number
			.as_ref()
			.map(|condition| eval_condition(condition, data).unwrap_or(false))
			.unwrap_or(false),
		_ => false,
	};
	let key = match form.unwrap_or(if plural { "plural" } else { "singular" }) {
		"plural" => "plural",
		"singular" => "singular",
		"possessive" => {
			if plural {
				"possessive_plural"
			} else {
				"possessive_singular"
			}
		}
		"possessive_plural" => "possessive_plural",
		"possessive_singular" => "possessive_singular",
		_ => {
			if plural {
				"plural"
			} else {
				"singular"
			}
		}
	};
	term.forms
		.get(key)
		.cloned()
		.unwrap_or_else(|| term.id.clone())
}

fn to_roman(mut value: i32) -> String {
	if value <= 0 {
		return value.to_string();
	}
	const VALUES: &[(i32, &str)] = &[
		(1000, "M"),
		(900, "CM"),
		(500, "D"),
		(400, "CD"),
		(100, "C"),
		(90, "XC"),
		(50, "L"),
		(40, "XL"),
		(10, "X"),
		(9, "IX"),
		(5, "V"),
		(4, "IV"),
		(1, "I"),
	];
	let mut output = String::new();
	for (number, symbol) in VALUES {
		while value >= *number {
			output.push_str(symbol);
			value -= *number;
		}
	}
	output
}

fn assign_numbers(
	plan: &DocumentPlan,
	data: &DataView,
) -> Result<HashMap<String, String>, String> {
	let mut numbers = HashMap::new();
	let mut article = 0;
	let mut section = 0;
	let mut subsection = 0;
	for block in &plan.blocks {
		if let Some(condition) = &block.when {
			if !eval_condition(condition, data)? {
				continue;
			}
		}
		match block.numbering.as_deref() {
			None | Some("") => {}
			Some("article") => {
				article += 1;
				section = 0;
				subsection = 0;
				numbers.insert(block.id.clone(), to_roman(article));
			}
			Some("section") => {
				section += 1;
				subsection = 0;
				let label = if article > 0 {
					format!("{}.{}", article, section)
				} else {
					section.to_string()
				};
				numbers.insert(block.id.clone(), label);
			}
			Some("section_local") => {
				section += 1;
				subsection = 0;
				numbers.insert(block.id.clone(), section.to_string());
			}
			Some("subsection") => {
				subsection += 1;
				let label = if article > 0 && section > 0 {
					format!("{}.{}.{}", article, section, subsection)
				} else {
					subsection.to_string()
				};
				numbers.insert(block.id.clone(), label);
			}
			Some(other) => {
				return Err(format!(
					"Unknown numbering '{}' in block '{}'",
					other, block.id
				))
			}
		}
	}
	Ok(numbers)
}

/// Join names for one assignment: "A", "A and B", or "A, B, and C".
fn join_names(names: &[String], conjunction: &str, separator: &str, oxford: bool) -> String {
	let conjunction = format!(" {} ", conjunction.trim());
	match names {
		[] => String::new(),
		[name] => name.clone(),
		[first, second] => format!("{}{}{}", first, conjunction, second),
		many => {
			let head = many[..many.len() - 1].join(separator);
			let last = many.last().expect("nonempty");
			if oxford {
				format!("{}{}{}{}", head, separator, conjunction.trim_start(), last)
			} else {
				format!("{}{}{}", head, conjunction, last)
			}
		}
	}
}

/// Resolve an ordered role into text. The plan supplies the legal wording for
/// the first assignment, a shared first assignment, and each successor tier.
struct RoleWording<'a> {
	property: &'a str,
	intro: Option<&'a str>,
	shared_intro: Option<&'a str>,
	successor: Option<&'a str>,
	shared_successor: Option<&'a str>,
	conjunction: &'a str,
	separator: &'a str,
	oxford: bool,
}

fn render_role(
	role: &str,
	wording: RoleWording<'_>,
	data: &DataView,
	warnings: &mut Vec<String>,
) -> String {
	let RoleWording {
		property,
		intro,
		shared_intro,
		successor,
		shared_successor,
		conjunction,
		separator,
		oxford,
	} = wording;
	let groups = data.role_names(role, property);
	if groups.is_empty() {
		warnings.push(format!("No assignment for role: {}", role));
		return String::new();
	}
	let mut output = String::new();
	let mut previous: Option<String> = None;
	let mut previous_names: Vec<String> = Vec::new();
	for (index, names) in groups.iter().enumerate() {
		let joined = join_names(names, conjunction, separator, oxford);
		let upper_names: Vec<String> = names.iter().map(|name| name.to_uppercase()).collect();
		let joined_upper = join_names(&upper_names, conjunction, separator, oxford);
		let previous_upper = if previous_names.is_empty() {
			String::new()
		} else {
			let upper: Vec<String> = previous_names
				.iter()
				.map(|name| name.to_uppercase())
				.collect();
			join_names(&upper, conjunction, separator, oxford)
		};
		let template = if index == 0 {
			if names.len() > 1 {
				shared_intro.or(intro)
			} else {
				intro
			}
		} else if names.len() > 1 {
			shared_successor.or(successor)
		} else {
			successor
		};
		let previous_text = previous.as_deref().unwrap_or("");
		let previous_verb = if previous_names.len() > 1 { "are" } else { "is" };
		let previous_predecease = if previous_names.len() > 1 {
			"predecease"
		} else {
			"predeceases"
		};
		match template {
			Some(template) => {
				let text = template
					.replace("{names}", &joined)
					.replace("{names_upper}", &joined_upper)
					.replace("{previous}", previous_text)
					.replace("{previous_upper}", &previous_upper)
					.replace("{previous_verb}", previous_verb)
					.replace("{previous_predecease}", previous_predecease)
					.replace("{role}", role);
				output.push_str(&text);
			}
			None => {
				if index > 0 && !output.ends_with(' ') {
					output.push(' ');
				}
				output.push_str(&joined);
			}
		}
		previous = Some(joined);
		previous_names = names.clone();
	}
	output
}

fn render_content(
	nodes: &[ContentNode],
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	current_block: Option<&str>,
	formatting: Option<&SourceFormatting>,
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let mut output = String::new();
	for node in nodes {
		match node {
			ContentNode::Text { text, marks } => output.push_str(&render_run(
				text,
				marks,
				formatting.map(|source| source.rpr.as_str()),
			)),
			ContentNode::Slot { slot, format, marks } => {
				let value = data
					.get(slot)
					.or_else(|| data.get_case_insensitive(slot))
					.unwrap_or("");
				if value.is_empty() {
					let required = plan
						.slots
						.get(slot)
						.or_else(|| {
							plan.slots
								.iter()
								.find(|(key, _)| key.eq_ignore_ascii_case(slot))
								.map(|(_, entry)| entry)
						})
						.map(|entry| entry.required)
						.unwrap_or(false);
					if required {
						warnings.push(format!("Missing required slot: {}", slot));
					}
				}
				let effective_format = format.as_deref().or_else(|| infer_format(slot));
				output.push_str(&render_run(
					&apply_format(value, effective_format),
					marks,
					formatting.map(|source| source.rpr.as_str()),
				));
			}
			ContentNode::Role {
				role,
				when,
				property,
				intro,
				shared_intro,
				successor,
				shared_successor,
				conjunction,
				separator,
				oxford,
				marks,
			} => {
				let enabled = match when {
					Some(condition) => eval_condition(condition, data)?,
					None => true,
				};
				if !enabled {
					continue;
				}
				let text = render_role(
					role,
					RoleWording {
						property: property.as_deref().unwrap_or("full_name"),
						intro: intro.as_deref(),
						shared_intro: shared_intro.as_deref(),
						successor: successor.as_deref(),
						shared_successor: shared_successor.as_deref(),
						conjunction: conjunction.as_deref().unwrap_or("and"),
						separator: separator.as_deref().unwrap_or(", "),
						oxford: oxford.unwrap_or(true),
					},
					data,
					warnings,
				);
				output.push_str(&render_run(
					&text,
					marks,
					formatting.map(|source| source.rpr.as_str()),
				));
			}
			ContentNode::Clause { clause, when } => {
				let definition = plan
					.clauses
					.get(clause)
					.ok_or_else(|| format!("Unknown clause: {}", clause))?;
				let use_condition = match when {
					Some(condition) => eval_condition(condition, data)?,
					None => true,
				};
				let clause_condition = match &definition.when {
					Some(condition) => eval_condition(condition, data)?,
					None => true,
				};
				if use_condition && clause_condition {
					output.push_str(&render_content(
						&definition.body,
						plan,
						data,
						numbers,
						current_block,
						formatting,
						warnings,
					)?);
				}
			}
			ContentNode::Term { term, form, number } => {
				let definition = plan
					.terms
					.get(term)
					.ok_or_else(|| format!("Unknown term: {}", term))?;
				output.push_str(&render_run(
					&resolve_term(
						definition,
						form.as_deref(),
						number.as_deref(),
						data,
					),
					&[],
					formatting.map(|source| source.rpr.as_str()),
				));
			}
			ContentNode::Ref { target, format } => match numbers.get(target) {
				Some(number) => {
					let template = format.as_deref().unwrap_or("{number}");
					output.push_str(&render_run(
						&template.replace("{number}", number),
						&[],
						formatting.map(|source| source.rpr.as_str()),
					));
				}
				None => warnings.push(format!("Cross-reference target not found: {}", target)),
			},
			ContentNode::Number { number } => match current_block.and_then(|id| numbers.get(id)) {
				Some(value) => output.push_str(&render_run(
					value,
					&[],
					formatting.map(|source| source.rpr.as_str()),
				)),
				None => warnings.push(format!(
					"Number '{}' has no assigned value in block {:?}",
					number, current_block
				)),
			},
			ContentNode::Tab { tab } => {
				if *tab {
					let properties = formatting
						.map(|source| source.rpr.as_str())
						.unwrap_or("");
					if properties.is_empty() {
						output.push_str("<w:r><w:tab/></w:r>");
					} else {
						output.push_str(&format!("<w:r>{}<w:tab/></w:r>", properties));
					}
				}
			}
			ContentNode::LineBreak { line_break, page } => {
				if *line_break {
					let properties = formatting
						.map(|source| source.rpr.as_str())
						.unwrap_or("");
					if *page {
						output.push_str(&format!("<w:r>{}<w:br w:type=\"page\"/></w:r>", properties));
					} else {
						output.push_str(&format!("<w:r>{}<w:br/></w:r>", properties));
					}
				}
			}
		}
	}
	Ok(output)
}

fn render_paragraph(
	block: &Block,
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	sources: &[SourceFormatting],
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let source_index = block.source_paragraph.or_else(|| {
		block
			.id
			.strip_prefix("block-")
			.and_then(|value| value.parse::<usize>().ok())
	});
	let source = source_index.and_then(|index| sources.get(index));
	let content = render_content(
		&block.content,
		plan,
		data,
		numbers,
		Some(&block.id),
		source,
		warnings,
	)?;
	let properties = if let Some(source) = source {
		if !source.ppr.is_empty() {
			if block.numbering.is_some() {
				NUM_PR_RE.replace_all(&source.ppr, "").to_string()
			} else {
				source.ppr.clone()
			}
		} else {
			style_paragraph(block)
		}
	} else {
		style_paragraph(block)
	};
	let properties = apply_layout_hints(&properties, block);
	Ok(format!("<w:p>{}{}</w:p>", properties, content))
}

fn style_paragraph(block: &Block) -> String {
	if block.style.is_empty() {
		String::new()
	} else {
		format!(
			"<w:pPr><w:pStyle w:val=\"{}\"/></w:pPr>",
			escape_xml(&block.style)
		)
	}
}

/// Add the block's layout intents to a paragraph's properties.
fn apply_layout_hints(ppr: &str, block: &Block) -> String {
	let mut hints = String::new();
	if block.keep_with_next && !ppr.contains("<w:keepNext") {
		hints.push_str("<w:keepNext/>");
	}
	if block.page_break_before && !ppr.contains("<w:pageBreakBefore") {
		hints.push_str("<w:pageBreakBefore/>");
	}
	if block.keep_together && !ppr.contains("<w:keepLines") {
		hints.push_str("<w:keepLines/>");
	}
	if hints.is_empty() {
		return ppr.to_string();
	}
	if ppr.is_empty() || ppr == "<w:pPr/>" {
		return format!("<w:pPr>{}</w:pPr>", hints);
	}
	if ppr.ends_with("/>") {
		let prefix = ppr.trim_end_matches("/>");
		return format!("{}>{}</w:pPr>", prefix, hints);
	}
	match ppr.rfind("</w:pPr>") {
		Some(position) => {
			let mut output = ppr.to_string();
			output.insert_str(position, &hints);
			output
		}
		None => format!("<w:pPr>{}</w:pPr>", hints),
	}
}

fn render_table(
	block: &Block,
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let style = if block.style.is_empty() {
		String::new()
	} else {
		format!(
			"<w:tblStyle w:val=\"{}\"/>",
			escape_xml(&block.style)
		)
	};
	let mut rows = String::new();
	for row in &block.rows {
		let mut cells = String::new();
		for cell in row {
			let content = render_content(
				&cell.content,
				plan,
				data,
				numbers,
				Some(&block.id),
				None,
				warnings,
			)?;
			let properties = if cell.style.is_empty() {
				String::new()
			} else {
				format!(
					"<w:pPr><w:pStyle w:val=\"{}\"/></w:pPr>",
					escape_xml(&cell.style)
				)
			};
			cells.push_str(&format!(
				"<w:tc><w:p>{}{}</w:p></w:tc>",
				properties, content
			));
		}
		rows.push_str(&format!("<w:tr>{}</w:tr>", cells));
	}
	Ok(format!(
		"<w:tbl><w:tblPr>{}<w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>{}</w:tbl>",
		style, rows
	))
}

// ---------------------------------------------------------------------------
// In-place editing
//
// A plan block normally corresponds to one paragraph of the lawyer's template.
// Instead of rebuilding that paragraph from the plan, the renderer keeps the
// source paragraph and replaces only its text. Each resolved segment inherits
// the formatting of the source run whose text it replaces, so underlined
// blanks, tabs, drawings, and mixed-run emphasis survive untouched.
// ---------------------------------------------------------------------------

/// One resolved inline piece of a plan.
#[derive(Debug, Clone)]
enum Segment {
	Text { text: String, marks: Vec<String> },
	Tab,
	Break { page: bool },
}

/// One inline piece of a source paragraph.
#[derive(Debug, Clone, PartialEq)]
enum SourceItem {
	Character(char),
	Tab,
	Break { page: bool },
}

#[derive(Debug, Clone)]
struct ParsedRun {
	rpr: String,
	items: Vec<SourceItem>,
	/// Non-text payload preserved without taking part in alignment: drawings,
	/// field characters, and anything else the renderer does not interpret.
	extras: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct ParsedParagraph {
	ppr: String,
	runs: Vec<ParsedRun>,
}

fn paragraph_inner(paragraph_xml: &str) -> &str {
	match paragraph_xml.find('>') {
		Some(position) => &paragraph_xml[position + 1..],
		None => paragraph_xml,
	}
}

fn parse_paragraph(paragraph_xml: &str) -> ParsedParagraph {
	let mut parsed = ParsedParagraph::default();
	let mut cursor = paragraph_inner(paragraph_xml);
	if let Some(matched) = PPR_AT_START_RE.find(cursor) {
		parsed.ppr = matched.as_str().to_string();
		cursor = &cursor[matched.end()..];
	}
	parsed.runs = parse_inline_runs(cursor);
	parsed
}

fn parse_inline_runs(mut cursor: &str) -> Vec<ParsedRun> {
	let mut runs = Vec::new();
	while !cursor.is_empty() {
		if cursor.starts_with("</w:p") || cursor.starts_with("</w:sdtContent") {
			break;
		}
		if let Some(matched) = RUN_AT_START_RE.find(cursor) {
			runs.push(parse_run(matched.as_str()));
			cursor = &cursor[matched.end()..];
			continue;
		}
		if let Some(matched) = SDT_AT_START_RE.find(cursor) {
			if let Some(content) = SDT_CONTENT_RE.captures(matched.as_str()) {
				runs.extend(parse_inline_runs(&content[1]));
			}
			cursor = &cursor[matched.end()..];
			continue;
		}
		match cursor.find('<') {
			Some(0) => match cursor.find('>') {
				Some(position) => cursor = &cursor[position + 1..],
				None => break,
			},
			Some(position) => cursor = &cursor[position..],
			None => break,
		}
	}
	runs
}

fn parse_run(run_xml: &str) -> ParsedRun {
	let mut run = ParsedRun {
		rpr: String::new(),
		items: Vec::new(),
		extras: Vec::new(),
	};
	let mut cursor = paragraph_inner(run_xml);
	if let Some(matched) = RPR_AT_START_RE.find(cursor) {
		run.rpr = matched.as_str().to_string();
		cursor = &cursor[matched.end()..];
	}
	while !cursor.is_empty() {
		if cursor.starts_with("</w:r") {
			break;
		}
		let Some(matched) = RUN_PIECE_RE.find(cursor) else {
			break;
		};
		let piece = matched.as_str();
		if let Some(text) = text_piece(piece) {
			for character in unescape_xml(text).chars() {
				run.items.push(SourceItem::Character(character));
			}
		} else if piece.starts_with("<w:tab") {
			run.items.push(SourceItem::Tab);
		} else if piece.starts_with("<w:br") {
			run.items.push(SourceItem::Break {
				page: piece.contains("w:type=\"page\""),
			});
		} else if !ignorable_piece(piece) {
			run.extras.push(piece.to_string());
		}
		cursor = &cursor[matched.end()..];
	}
	run
}

/// Layout and spell-check hints that can be dropped without changing content.
fn ignorable_piece(piece: &str) -> bool {
	piece.starts_with("<w:lastRenderedPageBreak")
		|| piece.starts_with("<w:proofErr")
		|| piece.starts_with("<w:bookmarkStart")
		|| piece.starts_with("<w:bookmarkEnd")
		|| piece.starts_with("<w:commentRangeStart")
		|| piece.starts_with("<w:commentRangeEnd")
}

fn text_piece(piece: &str) -> Option<&str> {
	if !piece.starts_with("<w:t") {
		return None;
	}
	match piece.as_bytes().get(4) {
		Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') => {}
		_ => return None,
	}
	let open = piece.find('>')?;
	if piece[..=open].ends_with("/>") {
		return Some("");
	}
	let close = piece.rfind("</w:t>")?;
	Some(&piece[open + 1..close])
}

fn unescape_xml(text: &str) -> String {
	let mut output = String::with_capacity(text.len());
	let mut cursor = 0usize;
	while let Some(relative) = text[cursor..].find('&') {
		let start = cursor + relative;
		output.push_str(&text[cursor..start]);
		let Some(end) = text[start..].find(';').map(|value| start + value) else {
			output.push_str(&text[start..]);
			return output;
		};
		let entity = &text[start + 1..end];
		let replacement = match entity {
			"amp" => Some('&'),
			"lt" => Some('<'),
			"gt" => Some('>'),
			"quot" => Some('"'),
			"apos" => Some('\''),
			_ if entity.starts_with("#x") || entity.starts_with("#X") => {
				u32::from_str_radix(&entity[2..], 16)
					.ok()
					.and_then(char::from_u32)
			}
			_ if entity.starts_with('#') => entity[1..]
				.parse::<u32>()
				.ok()
				.and_then(char::from_u32),
			_ => None,
		};
		match replacement {
			Some(character) => output.push(character),
			None => output.push_str(&text[start..=end]),
		}
		cursor = end + 1;
	}
	output.push_str(&text[cursor..]);
	output
}

fn find_text(items: &[SourceItem], from: usize, text: &str) -> Option<usize> {
	let needle: Vec<char> = text.chars().collect();
	if needle.is_empty() || needle.len() > items.len() {
		return None;
	}
	for start in from..=items.len() - needle.len() {
		let matched = needle.iter().enumerate().all(|(offset, character)| {
			matches!(items[start + offset], SourceItem::Character(value) if value == *character)
		});
		if matched {
			return Some(start);
		}
	}
	None
}

fn run_at(run_of: &[usize], position: usize) -> usize {
	if run_of.is_empty() {
		0
	} else if position >= run_of.len() {
		*run_of.last().expect("non-empty run map")
	} else {
		run_of[position]
	}
}

fn segment_marks(segment: &Segment) -> Vec<String> {
	match segment {
		Segment::Text { marks, .. } => marks.clone(),
		_ => Vec::new(),
	}
}

type RunGroups = HashMap<usize, Vec<(Vec<String>, Vec<Segment>)>>;

fn merge_marks(rpr: &str, marks: &[String]) -> String {
	let mut additions = String::new();
	for mark in marks {
		let (needle, xml) = match mark.as_str() {
			"bold" => ("<w:b", "<w:b/>"),
			"italic" => ("<w:i", "<w:i/>"),
			"underline" => ("<w:u", "<w:u w:val=\"single\"/>"),
			"small_caps" => ("<w:smallCaps", "<w:smallCaps/>"),
			"caps" => ("<w:caps", "<w:caps/>"),
			_ => continue,
		};
		if !rpr.contains(needle) && !additions.contains(needle) {
			additions.push_str(xml);
		}
	}
	if additions.is_empty() {
		return rpr.to_string();
	}
	if rpr.is_empty() || rpr == "<w:rPr/>" {
		return format!("<w:rPr>{}</w:rPr>", additions);
	}
	match rpr.rfind("</w:rPr>") {
		Some(position) => {
			let mut merged = rpr.to_string();
			merged.insert_str(position, &additions);
			merged
		}
		None => rpr.to_string(),
	}
}

fn segment_body(segments: &[Segment]) -> String {
	let mut body = String::new();
	for segment in segments {
		match segment {
			Segment::Text { text, .. } => body.push_str(&format!(
				"<w:t xml:space=\"preserve\">{}</w:t>",
				escape_xml(text)
			)),
			Segment::Tab => body.push_str("<w:tab/>"),
			Segment::Break { page } => {
				if *page {
					body.push_str("<w:br w:type=\"page\"/>");
				} else {
					body.push_str("<w:br/>");
				}
			}
		}
	}
	body
}

fn wrap_run(rpr: &str, body: &str) -> String {
	if rpr.is_empty() {
		format!("<w:r>{}</w:r>", body)
	} else {
		format!("<w:r>{}{}</w:r>", rpr, body)
	}
}

fn emit_segment_group(rpr: &str, marks: &[String], segments: &[Segment]) -> String {
	wrap_run(&merge_marks(rpr, marks), &segment_body(segments))
}

/// Resolve a block's content into inline segments. This mirrors the rebuild
/// path's resolution but produces data instead of XML.
fn resolve_segments(
	nodes: &[ContentNode],
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	current_block: Option<&str>,
	warnings: &mut Vec<String>,
) -> Result<Vec<Segment>, String> {
	let mut segments = Vec::new();
	for node in nodes {
		match node {
			ContentNode::Text { text, marks } => segments.push(Segment::Text {
				text: text.clone(),
				marks: marks.clone(),
			}),
			ContentNode::Slot { slot, format, marks } => {
				let value = data
					.get(slot)
					.or_else(|| data.get_case_insensitive(slot))
					.unwrap_or("");
				if value.is_empty() {
					let required = plan
						.slots
						.get(slot)
						.or_else(|| {
							plan.slots
								.iter()
								.find(|(key, _)| key.eq_ignore_ascii_case(slot))
								.map(|(_, entry)| entry)
						})
						.map(|entry| entry.required)
						.unwrap_or(false);
					if required {
						warnings.push(format!("Missing required slot: {}", slot));
					}
				}
				let effective_format = format.as_deref().or_else(|| infer_format(slot));
				segments.push(Segment::Text {
					text: apply_format(value, effective_format),
					marks: marks.clone(),
				});
			}
			ContentNode::Role {
				role,
				when,
				property,
				intro,
				shared_intro,
				successor,
				shared_successor,
				conjunction,
				separator,
				oxford,
				marks,
			} => {
				let enabled = match when {
					Some(condition) => eval_condition(condition, data)?,
					None => true,
				};
				if !enabled {
					continue;
				}
				let text = render_role(
					role,
					RoleWording {
						property: property.as_deref().unwrap_or("full_name"),
						intro: intro.as_deref(),
						shared_intro: shared_intro.as_deref(),
						successor: successor.as_deref(),
						shared_successor: shared_successor.as_deref(),
						conjunction: conjunction.as_deref().unwrap_or("and"),
						separator: separator.as_deref().unwrap_or(", "),
						oxford: oxford.unwrap_or(true),
					},
					data,
					warnings,
				);
				segments.push(Segment::Text {
					text,
					marks: marks.clone(),
				});
			}
			ContentNode::Clause { clause, when } => {
				let definition = plan
					.clauses
					.get(clause)
					.ok_or_else(|| format!("Unknown clause: {}", clause))?;
				let use_condition = match when {
					Some(condition) => eval_condition(condition, data)?,
					None => true,
				};
				let clause_condition = match &definition.when {
					Some(condition) => eval_condition(condition, data)?,
					None => true,
				};
				if use_condition && clause_condition {
					segments.extend(resolve_segments(
						&definition.body,
						plan,
						data,
						numbers,
						current_block,
						warnings,
					)?);
				}
			}
			ContentNode::Term { term, form, number } => {
				let definition = plan
					.terms
					.get(term)
					.ok_or_else(|| format!("Unknown term: {}", term))?;
				segments.push(Segment::Text {
					text: resolve_term(definition, form.as_deref(), number.as_deref(), data),
					marks: Vec::new(),
				});
			}
			ContentNode::Ref { target, format } => {
				if let Some(number) = numbers.get(target) {
					let template = format.as_deref().unwrap_or("{number}");
					segments.push(Segment::Text {
						text: template.replace("{number}", number),
						marks: Vec::new(),
					});
				} else {
					warnings.push(format!("Cross-reference target not found: {}", target));
				}
			}
			ContentNode::Number { number } => {
				if let Some(value) = current_block.and_then(|id| numbers.get(id)) {
					segments.push(Segment::Text {
						text: value.clone(),
						marks: Vec::new(),
					});
				} else {
					warnings.push(format!(
						"Number '{}' has no assigned value in block {:?}",
						number, current_block
					));
				}
			}
			ContentNode::Tab { tab } => {
				if *tab {
					segments.push(Segment::Tab);
				}
			}
			ContentNode::LineBreak { line_break, page } => {
				if *line_break {
					segments.push(Segment::Break { page: *page });
				}
			}
		}
	}
	Ok(segments)
}

/// Rewrite one source paragraph with the plan block's resolved content.
fn edit_paragraph(
	paragraph_xml: &str,
	block: &Block,
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let parsed = parse_paragraph(paragraph_xml);
	let segments = resolve_segments(
		&block.content,
		plan,
		data,
		numbers,
		Some(&block.id),
		warnings,
	)?;

	// Flatten the text-bearing runs so each resolved segment can be matched to
	// the run that supplied the text it replaces.
	let mut items: Vec<SourceItem> = Vec::new();
	let mut run_of: Vec<usize> = Vec::new();
	for (index, run) in parsed.runs.iter().enumerate() {
		for item in &run.items {
			items.push(item.clone());
			run_of.push(index);
		}
	}

	let mut cursor = 0usize;
	let mut assignments: Vec<(usize, Segment)> = Vec::new();
	for segment in &segments {
		match segment {
			Segment::Text { text, .. } if text.is_empty() => {}
			Segment::Text { text, marks } => match find_text(&items, cursor, text) {
				Some(position) => {
					let length = text.chars().count();
					cursor = position + length;
					// A single plan text node can span several source runs with
					// different formatting. Split it at each run boundary so
					// each slice inherits the formatting of the run it covers.
					let mut start = 0usize;
					let mut current = run_of[position];
					for offset in 0..length {
						let run = run_of[position + offset];
						if run != current {
							let piece: String =
								text.chars().skip(start).take(offset - start).collect();
							assignments.push((
								current,
								Segment::Text {
									text: piece,
									marks: marks.clone(),
								},
							));
							start = offset;
							current = run;
						}
					}
					let piece: String = text.chars().skip(start).take(length - start).collect();
					assignments.push((
						current,
						Segment::Text {
							text: piece,
							marks: marks.clone(),
						},
					));
				}
				None => assignments.push((run_at(&run_of, cursor), segment.clone())),
			},
			Segment::Tab => {
				let run = match (cursor..items.len()).find(|index| items[*index] == SourceItem::Tab) {
					Some(position) => {
						cursor = position + 1;
						run_of[position]
					}
					None => run_at(&run_of, cursor),
				};
				assignments.push((run, Segment::Tab));
			}
			Segment::Break { page } => {
				let run = match (cursor..items.len()).find(|index| {
					matches!(items[*index], SourceItem::Break { page: source } if source == *page)
				}) {
					Some(position) => {
						cursor = position + 1;
						run_of[position]
					}
					None => run_at(&run_of, cursor),
				};
				assignments.push((run, Segment::Break { page: *page }));
			}
		}
	}

	let mut groups: Vec<(usize, Vec<String>, Vec<Segment>)> = Vec::new();
	for (assigned, segment) in assignments {
		let marks = segment_marks(&segment);
		if let Some(last) = groups.last_mut() {
			if last.0 == assigned && last.1 == marks {
				last.2.push(segment);
				continue;
			}
		}
		groups.push((assigned, marks, vec![segment]));
	}

	let mut by_run: RunGroups = HashMap::new();
	for (run, marks, segments) in groups {
		by_run.entry(run).or_default().push((marks, segments));
	}

	let (open, self_closing) = match paragraph_xml.find('>') {
		Some(position) => {
			let opening = &paragraph_xml[..=position];
			(opening, opening.ends_with("/>"))
		}
		None => (paragraph_xml, false),
	};
	let open = if self_closing {
		format!("{}>", open.strip_suffix("/>").unwrap_or(open))
	} else {
		open.to_string()
	};
	let mut inner = apply_layout_hints(&parsed.ppr, block);
	if parsed.runs.is_empty() {
		if let Some(groups) = by_run.get(&0) {
			for (marks, segments) in groups {
				inner.push_str(&emit_segment_group("", marks, segments));
			}
		}
		return Ok(format!("{}{}</w:p>", open, inner));
	}
	for (index, run) in parsed.runs.iter().enumerate() {
		let groups = by_run.get(&index);
		if groups.is_none() && run.extras.is_empty() {
			continue;
		}
		if !run.extras.is_empty() {
			inner.push_str(&wrap_run(&run.rpr, &run.extras.join("")));
		}
		if let Some(groups) = groups {
			for (marks, segments) in groups {
				inner.push_str(&emit_segment_group(&run.rpr, marks, segments));
			}
		}
	}
	Ok(format!("{}{}</w:p>", open, inner))
}

/// Split a document into the text before the body, the body's children, and
/// the text from the closing body tag onward.
fn split_body(document_xml: &str) -> Option<(String, String, String)> {
	let start = document_xml.find("<w:body>")? + "<w:body>".len();
	let end = document_xml[start..].find("</w:body>")? + start;
	Some((
		document_xml[..start].to_string(),
		document_xml[start..end].to_string(),
		document_xml[end..].to_string(),
	))
}

fn element_name(opening: &str) -> Option<&str> {
	let inner = opening.strip_prefix('<')?;
	let end = inner.find(|character: char| {
		character.is_whitespace() || character == '>' || character == '/'
	})?;
	Some(&inner[..end])
}

/// Split a body's inner XML into its top-level elements.
fn split_top_level(xml: &str) -> Vec<String> {
	let mut children = Vec::new();
	let mut cursor = 0usize;
	while cursor < xml.len() {
		let Some(relative) = xml[cursor..].find('<') else {
			break;
		};
		let start = cursor + relative;
		if xml[start..].starts_with("<!--") {
			let Some(end) = xml[start..].find("-->") else {
				break;
			};
			let next = start + end + 3;
			children.push(xml[start..next].to_string());
			cursor = next;
			continue;
		}
		if xml[start..].starts_with("<?") {
			let Some(end) = xml[start..].find("?>") else {
				break;
			};
			cursor = start + end + 2;
			continue;
		}
		let Some(open_end_relative) = xml[start..].find('>') else {
			break;
		};
		let open_end = start + open_end_relative;
		let opening = &xml[start..=open_end];
		if opening.ends_with("/>") {
			children.push(opening.to_string());
			cursor = open_end + 1;
			continue;
		}
		let Some(name) = element_name(opening) else {
			cursor = open_end + 1;
			continue;
		};
		let close = format!("</{}>", name);
		let open_probe = format!("<{}", name);
		let mut depth = 1usize;
		let mut scan = open_end + 1;
		let mut end = None;
		while let Some(relative) = xml[scan..].find('<') {
			let position = scan + relative;
			if xml[position..].starts_with(&close) {
				depth -= 1;
				if depth == 0 {
					end = Some(position + close.len());
					break;
				}
				scan = position + close.len();
				continue;
			}
			if xml[position..].starts_with(&open_probe) {
				let boundary = xml.as_bytes().get(position + open_probe.len()).copied();
				if matches!(
					boundary,
					Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/')
				) {
					let Some(tag_end_relative) = xml[position..].find('>') else {
						break;
					};
					let tag_end = position + tag_end_relative;
					if !xml[position..=tag_end].ends_with("/>") {
						depth += 1;
					}
					scan = tag_end + 1;
					continue;
				}
			}
			scan = position + 1;
		}
		match end {
			Some(next) => {
				children.push(xml[start..next].to_string());
				cursor = next;
			}
			None => break,
		}
	}
	children
}

fn block_source_child(block: &Block, paragraphs: &[usize], child_count: usize) -> Option<usize> {
	if let Some(ordinal) = block.source_paragraph {
		return paragraphs.get(ordinal).copied();
	}
	block
		.id
		.strip_prefix("block-")
		.and_then(|value| value.parse::<usize>().ok())
		.filter(|index| *index < child_count)
}

/// Apply a plan to the template body in place. Returns the edited document and
/// the number of blocks emitted, or `None` when the body cannot be edited.
fn edit_body(
	document_xml: &str,
	plan: &DocumentPlan,
	data: &DataView,
	sources: &[SourceFormatting],
	warnings: &mut Vec<String>,
) -> Result<Option<(String, usize)>, String> {
	let Some((prefix, inner, suffix)) = split_body(document_xml) else {
		return Ok(None);
	};
	let children = split_top_level(&inner);
	let paragraphs: Vec<usize> = children
		.iter()
		.enumerate()
		.filter(|(_, child)| child.trim_start().starts_with("<w:p"))
		.map(|(index, _)| index)
		.collect();
	if paragraphs.is_empty() {
		return Ok(None);
	}
	let paragraph_ordinal: HashMap<usize, usize> = paragraphs
		.iter()
		.enumerate()
		.map(|(ordinal, index)| (*index, ordinal))
		.collect();

	let numbers = assign_numbers(plan, data)?;
	let enabled = evaluate_block_conditions(plan, data)?;
	let mut replacements: HashMap<usize, Option<String>> = HashMap::new();
	let mut insertions: HashMap<usize, Vec<String>> = HashMap::new();
	let mut before_first: Vec<String> = Vec::new();
	let mut last_source: Option<usize> = None;
	let mut last_source_ordinal: Option<usize> = None;
	let mut count = 0usize;
	let mut resolved = 0usize;

	for block in &plan.blocks {
		let block_enabled = enabled.get(&block.id).copied().unwrap_or(true);
		match block_source_child(block, &paragraphs, children.len()) {
			Some(index) if children[index].trim_start().starts_with("<w:p") => {
				last_source = Some(index);
				last_source_ordinal = paragraph_ordinal.get(&index).copied();
				resolved += 1;
				if block_enabled {
					let effective = with_list_separator(block, plan, &enabled);
					let edited = edit_paragraph(
						&children[index],
						&effective,
						plan,
						data,
						&numbers,
						warnings,
					)?;
					replacements.insert(index, Some(edited));
					count += 1;
				} else {
					replacements.insert(index, None);
				}
			}
			Some(index) => {
				last_source = Some(index);
				last_source_ordinal = None;
				resolved += 1;
				if block_enabled {
					let rendered =
						render_block(block, plan, data, &numbers, sources, warnings)?;
					replacements.insert(index, Some(rendered));
					count += 1;
				} else {
					replacements.insert(index, None);
				}
			}
			None if block_enabled => {
				let effective = with_list_separator(block, plan, &enabled);
				let mut formatted = effective.clone();
				if formatted.source_paragraph.is_none() {
					formatted.source_paragraph = last_source_ordinal;
				}
				let rendered =
					render_block(&formatted, plan, data, &numbers, sources, warnings)?;
				match last_source {
					Some(index) => insertions.entry(index).or_default().push(rendered),
					None => before_first.push(rendered),
				}
				count += 1;
			}
			None => {}
		}
	}

	// A plan whose blocks do not address the template's paragraphs is an
	// authored-from-scratch plan; rebuild the body instead of layering it.
	if resolved == 0 {
		return Ok(None);
	}

	let mut rebuilt = String::new();
	for rendered in before_first {
		rebuilt.push_str(&rendered);
	}
	for (index, child) in children.iter().enumerate() {
		match replacements.get(&index) {
			Some(Some(rendered)) => rebuilt.push_str(rendered),
			Some(None) => {}
			None => rebuilt.push_str(child),
		}
		if let Some(extra) = insertions.get(&index) {
			for rendered in extra {
				rebuilt.push_str(rendered);
			}
		}
	}
	Ok(Some((format!("{}{}{}", prefix, rebuilt, suffix), count)))
}

fn render_block(
	block: &Block,
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	sources: &[SourceFormatting],
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	match block.kind.as_str() {
		"paragraph" | "heading" => {
			render_paragraph(block, plan, data, numbers, sources, warnings)
		}
		"table" => render_table(block, plan, data, numbers, warnings),
		"excerpt" => render_excerpt(block, plan, data, warnings),
		"page-break" => Ok("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>".to_string()),
		other => Err(format!(
			"Unsupported block kind '{}' in block '{}'",
			other, block.id
		)),
	}
}

fn load_skeleton_sources(skeleton_path: &Path) -> Result<Vec<SourceFormatting>, String> {
	let bytes = fs::read(skeleton_path)
		.map_err(|error| format!("Failed to read skeleton {}: {}", skeleton_path.display(), error))?;
	let mut archive = ZipArchive::new(Cursor::new(bytes))
		.map_err(|error| format!("Failed to open skeleton as a docx: {}", error))?;
	let mut document_xml = String::new();
	archive
		.by_name("word/document.xml")
		.map_err(|error| format!("Failed to read skeleton document.xml: {}", error))?
		.read_to_string(&mut document_xml)
		.map_err(|error| format!("Failed to read skeleton document.xml: {}", error))?;
	Ok(extract_source_formatting(&document_xml))
}

/// Render a compiled excerpt plan and return its body. The excerpt keeps the
/// formatting of its own layout skeleton; it is inserted where the excerpt
/// block sits in the main plan.
fn render_excerpt(
	block: &Block,
	plan: &DocumentPlan,
	data: &DataView,
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let Some(path) = &block.path else {
		return Err(format!("Excerpt block '{}' has no path", block.id));
	};
	let excerpt_path = plan.base_dir.join(path);
	let text = fs::read_to_string(&excerpt_path).map_err(|error| {
		format!(
			"Failed to read excerpt {}: {}",
			excerpt_path.display(),
			error
		)
	})?;
	let mut excerpt: DocumentPlan = serde_json::from_str(&text).map_err(|error| {
		format!(
			"Failed to parse excerpt {}: {}",
			excerpt_path.display(),
			error
		)
	})?;
	excerpt.base_dir = excerpt_path
		.parent()
		.unwrap_or_else(|| Path::new("."))
		.to_path_buf();
	let sources = if excerpt.layout.is_empty() {
		Vec::new()
	} else {
		load_skeleton_sources(&excerpt.base_dir.join(&excerpt.layout))?
	};
	let (body, _) = render_body(&excerpt, data, &sources, warnings)?;
	Ok(body)
}

/// Evaluate every block's condition once, so a list group can see which
/// items survive before any of them are rendered.
fn evaluate_block_conditions(
	plan: &DocumentPlan,
	data: &DataView,
) -> Result<HashMap<String, bool>, String> {
	let mut enabled = HashMap::new();
	for block in &plan.blocks {
		let value = match &block.when {
			Some(condition) => eval_condition(condition, data)?,
			None => true,
		};
		enabled.insert(block.id.clone(), value);
	}
	Ok(enabled)
}

/// Choose the separator that follows a list item. The item before the last
/// enabled item takes the conjunction, and the last item takes the
/// terminator, so an omitted item cannot leave a dangling semicolon.
fn list_separator(
	block: &Block,
	plan: &DocumentPlan,
	enabled: &HashMap<String, bool>,
) -> String {
	let Some(group_name) = block.list.as_deref() else {
		return String::new();
	};
	if !enabled.get(&block.id).copied().unwrap_or(true) {
		return String::new();
	}
	let Some(group) = plan.lists.get(group_name) else {
		return String::new();
	};
	let Some(position) = plan
		.blocks
		.iter()
		.position(|candidate| candidate.id == block.id)
	else {
		return String::new();
	};
	let mut following = 0usize;
	for candidate in plan.blocks.iter().skip(position + 1) {
		if candidate.list.as_deref() == Some(group_name)
			&& enabled.get(&candidate.id).copied().unwrap_or(true)
		{
			following += 1;
		}
	}
	match following {
		0 => group.terminator.clone(),
		1 => group.last_separator.clone(),
		_ => group.separator.clone(),
	}
}

fn with_list_separator(
	block: &Block,
	plan: &DocumentPlan,
	enabled: &HashMap<String, bool>,
) -> Block {
	let separator = list_separator(block, plan, enabled);
	if separator.is_empty() {
		return block.clone();
	}
	let mut effective = block.clone();
	effective.content.push(ContentNode::Text {
		text: separator,
		marks: Vec::new(),
	});
	effective
}

/// Render the plan's blocks to WordprocessingML body content.
pub fn render_body(
	plan: &DocumentPlan,
	data: &DataView,
	sources: &[SourceFormatting],
	warnings: &mut Vec<String>,
) -> Result<(String, usize), String> {
	let numbers = assign_numbers(plan, data)?;
	let enabled = evaluate_block_conditions(plan, data)?;
	let mut body = String::new();
	let mut count = 0;
	for block in &plan.blocks {
		if !enabled.get(&block.id).copied().unwrap_or(true) {
			continue;
		}
		let effective = with_list_separator(block, plan, &enabled);
		body.push_str(&render_block(&effective, plan, data, &numbers, sources, warnings)?);
		count += 1;
	}
	Ok((body, count))
}

/// Replace the body content of a skeleton document, preserving its final
/// section properties so page size, margins, headers, and footers survive.
pub fn replace_body(document_xml: &str, body_xml: &str) -> Result<String, String> {
	let body_start = document_xml
		.find("<w:body>")
		.ok_or_else(|| "Skeleton document has no <w:body>".to_string())?;
	let body_end = document_xml
		.find("</w:body>")
		.ok_or_else(|| "Skeleton document has no </w:body>".to_string())?;
	if body_end < body_start {
		return Err("Skeleton document has a malformed body".to_string());
	}
	let inner_start = body_start + "<w:body>".len();
	let inner = &document_xml[inner_start..body_end];
	let section_start = inner.rfind("<w:sectPr").unwrap_or(inner.len());
	let section = &inner[section_start..];
	Ok(format!(
		"{}<w:body>{}{}</w:body>{}",
		&document_xml[..body_start],
		body_xml,
		section,
		&document_xml[body_end + "</w:body>".len()..]
	))
}

/// Replace simple `lily:` SDTs in headers and footers. Conditional SDTs are
/// left for the plan renderer; the skeleton's headers and footers are fixed
/// layout, not plan content.
pub fn replace_simple_sdts(xml: &str, data: &DataView, warnings: &mut Vec<String>) -> String {
	SDT_RE
		.replace_all(xml, |captures: &regex::Captures| {
			let block = captures.get(0).expect("whole match").as_str();
			let Some(tag_captures) = TAG_RE.captures(block) else {
				return block.to_string();
			};
			let name = &tag_captures[1];
			let value = data.get(name).unwrap_or("");
			if value.is_empty() {
				warnings.push(format!("Missing value for header or footer slot: {}", name));
			}
			if let Some(text_captures) = T_RE.captures(block) {
				let whole = text_captures.get(0).expect("text match");
				let mut result = block.to_string();
				result.replace_range(
					whole.start()..whole.end(),
					&format!(
						"{}{}{}",
						&text_captures[1],
						escape_xml(value),
						&text_captures[3]
					),
				);
				result
			} else {
				block.to_string()
			}
		})
		.to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderReport {
	pub plan_id: String,
	pub plan_version: u32,
	pub output: String,
	pub blocks_rendered: usize,
	pub warnings: Vec<String>,
}

/// Render a plan over the document copy in place. The document is both the
/// layout skeleton and the output, which is safe because `render_plan` reads
/// the skeleton into memory before writing.
#[derive(Debug, Clone, Serialize)]
pub struct AuditSlot {
    pub name: String,
    pub required: bool,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditCondition {
    #[serde(rename = "where")]
    pub location: String,
    pub condition: Value,
    pub variables: HashMap<String, Option<String>>,
    pub result: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanAudit {
    pub plan_id: String,
    pub plan_version: u32,
    pub blocks: usize,
    pub clauses: usize,
    pub used_slots: Vec<String>,
    pub declared_slots: Vec<String>,
    pub missing_slots: Vec<AuditSlot>,
    pub undeclared_slots: Vec<String>,
    pub unused_slots: Vec<String>,
    pub conditions: Vec<AuditCondition>,
    pub terms: Vec<String>,
    pub refs: Vec<String>,
    pub roles: Vec<String>,
    pub missing_roles: Vec<String>,
}

fn condition_names(condition: &Value, names: &mut Vec<String>) {
    let Value::Object(map) = condition else {
        return;
    };
    for key in ["eq", "neq", "includes"] {
        if let Some(Value::Array(pair)) = map.get(key) {
            if let Some(Value::String(name)) = pair.first() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
    }
    for key in ["and", "or"] {
        if let Some(Value::Array(children)) = map.get(key) {
            for child in children {
                condition_names(child, names);
            }
        }
    }
    if let Some(child) = map.get("not") {
        condition_names(child, names);
    }
    if let Some(Value::String(name)) = map.get("exists") {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_audit_nodes(
    plan: &DocumentPlan,
    nodes: &[ContentNode],
    location: &str,
    used_slots: &mut Vec<String>,
    terms: &mut Vec<String>,
    refs: &mut Vec<String>,
    roles: &mut Vec<String>,
    conditions: &mut Vec<(String, Value)>,
    visited_clauses: &mut HashSet<String>,
) {
    for node in nodes {
        match node {
            ContentNode::Slot { slot, .. } => {
                if !used_slots.contains(slot) {
                    used_slots.push(slot.clone());
                }
            }
            ContentNode::Clause { clause, when } => {
                if let Some(condition) = when {
                    conditions.push((location.to_string(), condition.clone()));
                }
                if visited_clauses.insert(clause.clone()) {
                    if let Some(definition) = plan.clauses.get(clause) {
                        if let Some(condition) = &definition.when {
                            conditions.push((format!("clause:{}", clause), condition.clone()));
                        }
                        collect_audit_nodes(
                            plan,
                            &definition.body,
                            &format!("clause:{}", clause),
                            used_slots,
                            terms,
                            refs,
                            roles,
                            conditions,
                            visited_clauses,
                        );
                    }
                }
            }
            ContentNode::Role { role, when, .. } => {
                if let Some(condition) = when {
                    conditions.push((location.to_string(), condition.clone()));
                }
                if !roles.contains(role) {
                    roles.push(role.clone());
                }
            }
            ContentNode::Term { term, .. } => {
                if !terms.contains(term) {
                    terms.push(term.clone());
                }
            }
            ContentNode::Ref { target, .. } => {
                if !refs.contains(target) {
                    refs.push(target.clone());
                }
            }
            _ => {}
        }
    }
}

pub fn audit_plan(plan: &DocumentPlan, data: &DataView) -> PlanAudit {
    let mut used_slots: Vec<String> = Vec::new();
    let mut terms: Vec<String> = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    let mut roles: Vec<String> = Vec::new();
    let mut conditions: Vec<(String, Value)> = Vec::new();
    let mut visited_clauses: HashSet<String> = HashSet::new();
    for block in &plan.blocks {
        if let Some(condition) = &block.when {
            conditions.push((block.id.clone(), condition.clone()));
        }
        collect_audit_nodes(
            plan,
            &block.content,
            &block.id,
            &mut used_slots,
            &mut terms,
            &mut refs,
            &mut roles,
            &mut conditions,
            &mut visited_clauses,
        );
    }
    used_slots.sort();
    used_slots.dedup();
    terms.sort();
    refs.sort();
    roles.sort();
    roles.dedup();
    let missing_roles: Vec<String> = roles
        .iter()
        .filter(|role| data.role_names(role, "full_name").is_empty())
        .cloned()
        .collect();

    let mut declared: Vec<String> = plan.slots.keys().cloned().collect();
    declared.sort();

    let mut missing_slots = Vec::new();
    for name in &used_slots {
        let value = data
            .get(name)
            .or_else(|| data.get_case_insensitive(name))
            .unwrap_or("");
        if value.is_empty() {
            let required = plan
                .slots
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, slot)| slot.required)
                .unwrap_or(false);
            missing_slots.push(AuditSlot {
                name: name.clone(),
                required,
                value: value.to_string(),
            });
        }
    }

    let mut condition_name_list: Vec<String> = Vec::new();
    for (_, condition) in &conditions {
        condition_names(condition, &mut condition_name_list);
    }
    let used_set: HashSet<&str> = used_slots.iter().map(String::as_str).collect();
    let condition_set: HashSet<&str> = condition_name_list.iter().map(String::as_str).collect();
    let undeclared_slots: Vec<String> = used_slots
        .iter()
        .filter(|name| {
            !plan
                .slots
                .keys()
                .any(|key| key.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect();
    let unused_slots: Vec<String> = declared
        .iter()
        .filter(|name| {
            !used_set.contains(name.as_str()) && !condition_set.contains(name.as_str())
        })
        .cloned()
        .collect();

    let mut audit_conditions = Vec::new();
    for (location, condition) in conditions {
        let mut names: Vec<String> = Vec::new();
        condition_names(&condition, &mut names);
        names.sort();
        names.dedup();
        let variables: HashMap<String, Option<String>> = names
            .iter()
            .map(|name| {
                let value = data
                    .get(name)
                    .or_else(|| data.get_case_insensitive(name))
                    .map(str::to_string)
                    .or_else(|| data.facts.get(name).map(|value| value.to_string()))
                    .or_else(|| {
                        let list = data.get_list(name);
                        if list.is_empty() {
                            None
                        } else {
                            Some(list.join("; "))
                        }
                    });
                (name.clone(), value)
            })
            .collect();
        audit_conditions.push(AuditCondition {
            location,
            condition: condition.clone(),
            variables,
            result: eval_condition(&condition, data).ok(),
        });
    }

    PlanAudit {
        plan_id: plan.id.clone(),
        plan_version: plan.version,
        blocks: plan.blocks.len(),
        clauses: plan.clauses.len(),
        used_slots,
        declared_slots: declared,
        missing_slots,
        undeclared_slots,
        unused_slots,
        conditions: audit_conditions,
        terms,
        refs,
        roles,
        missing_roles,
    }
}

/// Read a plan and remember the directory its excerpt paths resolve against.
fn read_plan_file(plan_path: &Path) -> Result<DocumentPlan, String> {
	let plan_text = fs::read_to_string(plan_path)
		.map_err(|error| format!("Failed to read plan {}: {}", plan_path.display(), error))?;
	let mut plan: DocumentPlan = serde_json::from_str(&plan_text)
		.map_err(|error| format!("Failed to parse plan {}: {}", plan_path.display(), error))?;
	plan.base_dir = plan_path
		.parent()
		.unwrap_or_else(|| Path::new("."))
		.to_path_buf();
	Ok(plan)
}

/// Replace excerpt blocks with the blocks of the plan they point to, so an
/// audit reports data gaps inside a selected excerpt. Excerpt block ids are
/// prefixed with the parent block id; clause references keep their own names.
fn inline_excerpts(plan: &DocumentPlan, data: &DataView) -> Result<DocumentPlan, String> {
	let mut expanded = DocumentPlan {
		blocks: Vec::new(),
		..plan.clone()
	};
	for block in &plan.blocks {
		if block.kind != "excerpt" {
			expanded.blocks.push(block.clone());
			continue;
		}
		if let Some(condition) = &block.when {
			if !eval_condition(condition, data)? {
				continue;
			}
		}
		let Some(path) = &block.path else {
			return Err(format!("Excerpt block '{}' has no path", block.id));
		};
		let excerpt_path = plan.base_dir.join(path);
		let excerpt = read_plan_file(&excerpt_path)?;
		let nested = inline_excerpts(&excerpt, data)?;
		for mut child in nested.blocks {
			child.id = format!("{}:{}", block.id, child.id);
			expanded.blocks.push(child);
		}
		for (id, clause) in nested.clauses {
			expanded.clauses.entry(id).or_insert(clause);
		}
		for (name, slot) in nested.slots {
			expanded.slots.entry(name).or_insert(slot);
		}
	}
	Ok(expanded)
}

#[tauri::command]
pub fn audit_plan_document(plan_path: String, client_dir: String) -> Result<PlanAudit, String> {
	let plan = read_plan_file(Path::new(&plan_path))?;
	lily_file::resolve_contact_variables(client_dir.clone())?;
	let lily = lily_file::read_lily_file(&client_dir)?;
	let view = DataView::from_lily(&lily);
	let plan = inline_excerpts(&plan, &view)?;
	Ok(audit_plan(&plan, &view))
}

#[tauri::command]
pub fn render_plan_document(
	docx_path: String,
	plan_path: String,
	skeleton_path: Option<String>,
) -> Result<RenderReport, String> {
	let path = Path::new(&docx_path);
	let client_dir = path
		.parent()
		.ok_or_else(|| "Document has no parent directory".to_string())?;
	let plan_file = Path::new(&plan_path);
	let skeleton = skeleton_path
		.filter(|candidate| Path::new(candidate).is_file())
		.map(PathBuf::from)
		.or_else(|| resolve_plan_skeleton(plan_file))
		.unwrap_or_else(|| path.to_path_buf());
	render_plan(plan_file, client_dir, &skeleton, path)
}

/// Resolve the pristine template a plan was compiled from. The app normally
/// passes the template path directly; this fallback serves callers that only
/// have the plan file.
fn resolve_plan_skeleton(plan_path: &Path) -> Option<PathBuf> {
	let text = fs::read_to_string(plan_path).ok()?;
	let plan: DocumentPlan = serde_json::from_str(&text).ok()?;
	let plan_dir = plan_path.parent()?;
	let mut candidates: Vec<PathBuf> = Vec::new();
	if !plan.layout.is_empty() {
		candidates.push(plan_dir.join(&plan.layout));
		if let Some(templates_root) = plan_dir.parent() {
			candidates.push(templates_root.join(&plan.layout));
		}
	}
	if let Some(metadata) = &plan.metadata {
		if let Some(source) = metadata.get("source_template").and_then(Value::as_str) {
			if let Some(templates_root) = plan_dir.parent() {
				candidates.push(templates_root.join(source));
			}
		}
	}
	candidates.into_iter().find(|candidate| candidate.is_file())
}

/// Render a plan against a client folder and a layout skeleton.
pub fn render_plan(
	plan_path: &Path,
	client_dir: &Path,
	skeleton_path: &Path,
	output_path: &Path,
) -> Result<RenderReport, String> {
	let plan = read_plan_file(plan_path)?;

	// Resolve contact-derived variables exactly as the app does before a fill.
	lily_file::resolve_contact_variables(client_dir.to_string_lossy().to_string())?;
	let lily = lily_file::read_lily_file(&client_dir.to_string_lossy())?;
	let mut view = DataView::from_lily(&lily);
	if let Some(filename) = skeleton_path.file_name().and_then(|name| name.to_str()) {
		if let Some(meta) = lily.documents.get(filename) {
			for (name, value) in &meta.variable_overrides {
				view.variables.insert(name.clone(), value.clone());
			}
			for role_override in meta.role_overrides.values() {
				for (name, value) in &role_override.values {
					view.variables.insert(name.clone(), value.clone());
				}
			}
		}
	}

	let skeleton_bytes = fs::read(skeleton_path)
		.map_err(|error| format!("Failed to read skeleton {}: {}", skeleton_path.display(), error))?;
	let (document_xml, sources) = {
		let mut source_archive = ZipArchive::new(Cursor::new(&skeleton_bytes))
			.map_err(|error| format!("Failed to open skeleton as a docx: {}", error))?;
		let mut document_xml = String::new();
		source_archive
			.by_name("word/document.xml")
			.map_err(|error| format!("Failed to read skeleton document.xml: {}", error))?
			.read_to_string(&mut document_xml)
			.map_err(|error| format!("Failed to read skeleton document.xml: {}", error))?;
		let sources = extract_source_formatting(&document_xml);
		(document_xml, sources)
	};

	let mut warnings = Vec::new();
	let edited = edit_body(&document_xml, &plan, &view, &sources, &mut warnings)?;
	let (body_xml, blocks_rendered, edited_document) = match edited {
		Some((document, count)) => (String::new(), count, Some(document)),
		None => {
			let (body, count) = render_body(&plan, &view, &sources, &mut warnings)?;
			(body, count, None)
		}
	};

	let mut archive = ZipArchive::new(Cursor::new(skeleton_bytes))
		.map_err(|error| format!("Failed to open skeleton as a docx: {}", error))?;

	let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
	for index in 0..archive.len() {
		let mut entry = archive
			.by_index(index)
			.map_err(|error| format!("Failed to read skeleton entry: {}", error))?;
		let name = entry.name().to_string();
		let mut bytes = Vec::new();
		entry
			.read_to_end(&mut bytes)
			.map_err(|error| format!("Failed to read skeleton entry data: {}", error))?;
		entries.push((name, bytes));
	}

	let mut output_entries: Vec<(String, Vec<u8>)> = Vec::new();
	for (name, bytes) in entries {
		if name == "word/document.xml" {
			let xml = String::from_utf8_lossy(&bytes);
			let replaced = match &edited_document {
				Some(document) => document.clone(),
				None => replace_body(&xml, &body_xml)?,
			};
			output_entries.push((name, replaced.into_bytes()));
		} else if name.starts_with("word/header") || name.starts_with("word/footer") {
			let xml = String::from_utf8_lossy(&bytes);
			let replaced = replace_simple_sdts(&xml, &view, &mut warnings);
			output_entries.push((name, replaced.into_bytes()));
		} else {
			output_entries.push((name, bytes));
		}
	}

	let mut output = Cursor::new(Vec::new());
	{
		let mut writer = ZipWriter::new(&mut output);
		let options =
			SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
		for (name, bytes) in &output_entries {
			writer
				.start_file(name.as_str(), options)
				.map_err(|error| format!("Failed to write docx entry: {}", error))?;
			writer
				.write_all(bytes)
				.map_err(|error| format!("Failed to write docx entry data: {}", error))?;
		}
		writer
			.finish()
			.map_err(|error| format!("Failed to finish docx: {}", error))?;
	}

	if let Some(parent) = output_path.parent() {
		fs::create_dir_all(parent)
			.map_err(|error| format!("Failed to create output directory: {}", error))?;
	}
	fs::write(output_path, output.into_inner())
		.map_err(|error| format!("Failed to write rendered document: {}", error))?;

	Ok(RenderReport {
		plan_id: plan.id,
		plan_version: plan.version,
		output: output_path.to_string_lossy().to_string(),
		blocks_rendered,
		warnings,
	})
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanCandidateReport {
	pub candidate_path: String,
	pub candidate_html: String,
	pub diff: Vec<VersionDiffLine>,
	pub plan_id: String,
	pub plan_version: u32,
	pub blocks_rendered: usize,
	pub warnings: Vec<String>,
}

fn candidate_output_path(docx: &Path) -> PathBuf {
	let stem = docx
		.file_stem()
		.and_then(|value| value.to_str())
		.unwrap_or("candidate");
	let unique = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|duration| duration.as_nanos())
		.unwrap_or(0);
	std::env::temp_dir().join(format!("lily-candidate-{}-{}.docx", stem, unique))
}

fn ensure_candidate(path: &Path) -> Result<(), String> {
	let name = path
		.file_name()
		.and_then(|value| value.to_str())
		.unwrap_or("");
	if !name.starts_with("lily-candidate-") {
		return Err(format!("Not a Lily candidate: {}", path.display()));
	}
	Ok(())
}

/// Render the current plan and client data to a temporary document without
/// touching the client's document. The candidate is shown with a diff so the
/// attorney can approve it as the draft.
#[tauri::command]
pub fn render_plan_candidate(
	docx_path: String,
	plan_path: String,
	skeleton_path: String,
) -> Result<PlanCandidateReport, String> {
	let docx = Path::new(&docx_path);
	if !docx.is_file() {
		return Err(format!("Document not found: {}", docx.display()));
	}
	let client_dir = docx
		.parent()
		.ok_or_else(|| "Document has no parent directory".to_string())?;
	let candidate = candidate_output_path(docx);
	let report = render_plan(
		Path::new(&plan_path),
		client_dir,
		Path::new(&skeleton_path),
		&candidate,
	)
	.inspect_err(|_| {
		let _ = fs::remove_file(&candidate);
	})?;
	let candidate_html =
		crate::docx_ops::get_document_html(candidate.to_string_lossy().to_string()).inspect_err(
			|_| {
				let _ = fs::remove_file(&candidate);
			},
		)?;
	let diff = crate::history::diff_document_pair_paths(docx, &candidate).inspect_err(|_| {
		let _ = fs::remove_file(&candidate);
	})?;
	Ok(PlanCandidateReport {
		candidate_path: candidate.to_string_lossy().to_string(),
		candidate_html,
		diff,
		plan_id: report.plan_id,
		plan_version: report.plan_version,
		blocks_rendered: report.blocks_rendered,
		warnings: report.warnings,
	})
}

/// Replace the client's document with an approved candidate. The current
/// document is snapshotted first and the temporary candidate is removed.
#[tauri::command]
pub fn approve_plan_candidate(docx_path: String, candidate_path: String) -> Result<(), String> {
	let docx = Path::new(&docx_path);
	let candidate = Path::new(&candidate_path);
	ensure_candidate(candidate)?;
	if !candidate.is_file() {
		return Err(format!("Candidate not found: {}", candidate.display()));
	}
	crate::history::snapshot_document_path(&docx_path)?;
	let bytes = fs::read(candidate)
		.map_err(|error| format!("Failed to read candidate {}: {}", candidate.display(), error))?;
	fs::write(docx, bytes)
		.map_err(|error| format!("Failed to write document {}: {}", docx.display(), error))?;
	let _ = fs::remove_file(candidate);
	Ok(())
}

#[tauri::command]
pub fn discard_plan_candidate(candidate_path: String) -> Result<(), String> {
	let candidate = Path::new(&candidate_path);
	ensure_candidate(candidate)?;
	if candidate.is_file() {
		fs::remove_file(candidate).map_err(|error| {
			format!(
				"Failed to discard candidate {}: {}",
				candidate.display(),
				error
			)
		})?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn view() -> DataView {
		let mut lily = LilyFile::default();
		lily.variables
			.insert("Client Full Name".to_string(), "Jane A. Doe".to_string());
		lily.variables
			.insert("Has Spouse".to_string(), "true".to_string());
		lily.variables
			.insert("Client has Children".to_string(), "true".to_string());
		lily.variables
			.insert("Primary HPOA Agent Full Name".to_string(), "Mary Smith".to_string());
		DataView::from_lily(&lily)
	}

	fn role_contact(id: &str, name: &str) -> Contact {
		Contact {
			id: id.to_string(),
			full_name: name.to_string(),
			first_name: name.split_whitespace().next().unwrap_or(name).to_string(),
			middle_name: String::new(),
			last_name: name.split_whitespace().last().unwrap_or(name).to_string(),
			relationship: "Friend".to_string(),
			other_relationship: String::new(),
			phone: String::new(),
			email: String::new(),
			address: String::new(),
			city: String::new(),
			state: String::new(),
			zip: String::new(),
			is_minor: false,
		}
	}

	fn role_view() -> DataView {
		let mut lily = LilyFile::default();
		lily.contacts = vec![
			role_contact("a", "Alice Adams"),
			role_contact("b", "Bob Brown"),
			role_contact("c", "Carol Clark"),
		];
		lily.contact_bindings.insert(
			"Successor Trustee".to_string(),
			crate::lily_file::ContactBinding {
				contact_id: None,
				variable_mappings: HashMap::new(),
				contact_ids: None,
				assignments: Some(vec![
					RoleAssignment {
						contact_ids: vec!["a".to_string(), "b".to_string()],
						values: HashMap::new(),
					},
					RoleAssignment {
						contact_ids: vec!["c".to_string()],
						values: HashMap::new(),
					},
				]),
			},
		);
		DataView::from_lily(&lily)
	}

	#[test]
	fn role_assignments_join_shared_groups_and_successors() {
		let view = role_view();
		let mut warnings = Vec::new();
		let text = render_role(
			"Successor Trustee",
			RoleWording {
				property: "full_name",
				intro: Some("Settlors nominate {names} to serve as Trustee. "),
				shared_intro: Some(
					"Settlors nominate {names} to serve as Co-Trustees. ",
				),
				successor: Some(
					"If {previous} shall die, resign, or cease to act, {names} shall serve as Trustee. ",
				),
				shared_successor: None,
				conjunction: "and",
				separator: ", ",
				oxford: true,
			},
			&view,
			&mut warnings,
		);
		assert_eq!(
			text,
			"Settlors nominate Alice Adams and Bob Brown to serve as Co-Trustees. If Alice Adams and Bob Brown shall die, resign, or cease to act, Carol Clark shall serve as Trustee. "
		);
		assert!(warnings.is_empty());
	}

	#[test]
	fn legacy_contact_binding_is_one_role_assignment() {
		let mut lily = LilyFile::default();
		lily.contacts = vec![role_contact("a", "Alice Adams")];
		lily.contact_bindings.insert(
			"Primary Trustee".to_string(),
			crate::lily_file::ContactBinding {
				contact_id: Some("a".to_string()),
				variable_mappings: HashMap::new(),
				contact_ids: None,
				assignments: None,
			},
		);
		let view = DataView::from_lily(&lily);
		assert_eq!(
			view.role_names("Primary Trustee", "full_name"),
			vec![vec!["Alice Adams".to_string()]]
		);
	}

	#[test]
	fn role_shared_successor_and_previous_grammar() {
		let mut lily = LilyFile::default();
		lily.contacts = vec![
			role_contact("a", "Alice Adams"),
			role_contact("b", "Bob Brown"),
			role_contact("c", "Carol Clark"),
		];
		lily.contact_bindings.insert(
			"Personal Representative".to_string(),
			crate::lily_file::ContactBinding {
				contact_id: None,
				variable_mappings: HashMap::new(),
				contact_ids: None,
				assignments: Some(vec![
					RoleAssignment {
						contact_ids: vec!["a".to_string()],
						values: HashMap::new(),
					},
					RoleAssignment {
						contact_ids: vec!["b".to_string(), "c".to_string()],
						values: HashMap::new(),
					},
				]),
			},
		);
		let view = DataView::from_lily(&lily);
		let mut warnings = Vec::new();
		let text = render_role(
			"Personal Representative",
			RoleWording {
				property: "full_name",
				intro: Some("I appoint {names} as Personal Representative. "),
				shared_intro: Some("I appoint {names} as Co-Personal Representatives. "),
				successor: Some("If {previous} is unable to act, {names} shall serve. "),
				shared_successor: Some(
					"If {previous_upper} {previous_predecease} me or {previous_verb} unable to act, {names_upper} shall serve as Co-Personal Representatives. ",
				),
				conjunction: "and",
				separator: ", ",
				oxford: true,
			},
			&view,
			&mut warnings,
		);
		assert_eq!(
			text,
			"I appoint Alice Adams as Personal Representative. If ALICE ADAMS predeceases me or is unable to act, BOB BROWN and CAROL CLARK shall serve as Co-Personal Representatives. "
		);
		assert!(warnings.is_empty());
	}

	#[test]
	fn personal_representative_tiers_fold_into_shared_assignments() {
		let mut lily = LilyFile::default();
		lily.contacts = vec![
			role_contact("a", "Alice Adams"),
			role_contact("b", "Bob Brown"),
			role_contact("c", "Carol Clark"),
		];
		for (role, contact) in [
			("Primary Personal Representative", "a"),
			("Primary Co-Personal Representative", "b"),
			("Secondary Personal Representative", "c"),
		] {
			lily.contact_bindings.insert(
				role.to_string(),
				crate::lily_file::ContactBinding {
					contact_id: Some(contact.to_string()),
					variable_mappings: HashMap::new(),
					contact_ids: None,
					assignments: None,
				},
			);
		}
		let view = DataView::from_lily(&lily);
		assert_eq!(
			view.role_names("Personal Representative", "full_name"),
			vec![
				vec!["Alice Adams".to_string(), "Bob Brown".to_string()],
				vec!["Carol Clark".to_string()]
			]
		);
	}

	#[test]
	fn empty_paragraph_content_is_inserted() {
		let paragraph = "<w:p><w:pPr><w:pStyle w:val=\"Normal\"/></w:pPr></w:p>";
		let block = test_block(
			"block-000",
			"paragraph",
			vec![ContentNode::Slot {
				slot: "Client Full Name".to_string(),
				format: None,
				marks: vec![],
			}],
			None,
		);
		let plan = test_plan(vec![block.clone()]);
		let mut warnings = Vec::new();
		let result = edit_paragraph(
			paragraph,
			&block,
			&plan,
			&view(),
			&HashMap::new(),
			&mut warnings,
		)
		.expect("edited paragraph");
		assert!(result.contains("Jane A. Doe"));
		assert!(result.contains("<w:pStyle w:val=\"Normal\"/>"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn deed_beneficiaries_fold_into_one_shared_assignment() {
		let mut lily = LilyFile::default();
		lily.contacts = vec![
			role_contact("a", "Alice Adams"),
			role_contact("b", "Bob Brown"),
		];
		for (role, contact) in [
			("Primary Beneficiary", "a"),
			("Secondary Beneficiary", "b"),
		] {
			lily.contact_bindings.insert(
				role.to_string(),
				crate::lily_file::ContactBinding {
					contact_id: Some(contact.to_string()),
					variable_mappings: HashMap::new(),
					contact_ids: None,
					assignments: None,
				},
			);
		}
		let view = DataView::from_lily(&lily);
		assert_eq!(
			view.role_names("Deed Beneficiary", "full_name"),
			vec![vec!["Alice Adams".to_string(), "Bob Brown".to_string()]]
		);
	}

	#[test]
	fn missing_role_reports_a_warning() {
		let view = DataView::default();
		let mut warnings = Vec::new();
		let text = render_role(
			"Successor Trustee",
			RoleWording {
				property: "full_name",
				intro: Some("Settlors nominate {names}. "),
				shared_intro: None,
				successor: Some("If {previous} ceases to act, {names} shall serve. "),
				shared_successor: None,
				conjunction: "and",
				separator: ", ",
				oxford: true,
			},
			&view,
			&mut warnings,
		);
		assert!(text.is_empty());
		assert_eq!(warnings, vec!["No assignment for role: Successor Trustee"]);
	}

	#[test]
	fn join_names_uses_the_plan_conjunction() {
		let names = |values: &[&str]| {
			values
				.iter()
				.map(|value| value.to_string())
				.collect::<Vec<_>>()
		};
		assert_eq!(join_names(&names(&["A", "B"]), "or", ", ", true), "A or B");
		assert_eq!(
			join_names(&names(&["A", "B", "C"]), "&", ", ", true),
			"A, B, & C"
		);
		assert_eq!(
			join_names(&names(&["A", "B", "C"]), "and", ", ", false),
			"A, B and C"
		);
	}

	#[test]
	fn eval_condition_reads_boolean_facts() {
		assert!(eval_condition(&json!({"eq": ["has_spouse", true]}), &view()).unwrap());
		assert!(!eval_condition(&json!({"eq": ["has_spouse", false]}), &view()).unwrap());
		assert!(eval_condition(&json!({"eq": ["Has Spouse", "true"]}), &view()).unwrap());
	}

	#[test]
	fn eval_condition_handles_and_or_not() {
		let condition = json!({"and": [{"eq": ["has_spouse", true]}, {"or": [{"eq": ["has_children", true]}, {"eq": ["has_children", false]}]}]});
		assert!(eval_condition(&condition, &view()).unwrap());
		let condition = json!({"not": {"eq": ["has_spouse", true]}});
		assert!(!eval_condition(&condition, &view()).unwrap());
	}

	#[test]
	fn eval_condition_includes_required_document() {
		let mut lily = LilyFile::default();
		lily.required_documents.push(lily_file::RequiredDocument {
			id: "trust".to_string(),
			template_rel_path: "04 - Trust Templates/Trust Agreement Template.docx".to_string(),
			status: lily_file::DocumentStatus::default(),
			document_filename: None,
			notes: String::new(),
		});
		let view = DataView::from_lily(&lily);
		assert!(eval_condition(
			&json!({
				"includes": [
					"required_documents",
					"04 - Trust Templates/Trust Agreement Template.docx"
				]
			}),
			&view
		)
		.unwrap());
		assert!(!eval_condition(
			&json!({
				"includes": [
					"required_documents",
					"03 - Will Templates/Last Will and Testament Template.docx"
				]
			}),
			&view
		)
		.unwrap());
	}

	#[test]
	fn apply_format_upper() {
		assert_eq!(apply_format("Jane A. Doe", Some("upper")), "JANE A. DOE");
		assert_eq!(apply_format("Jane A. Doe", Some("lower")), "jane a. doe");
		assert_eq!(apply_format("Jane A. Doe", None), "Jane A. Doe");
	}

	#[test]
	fn render_paragraph_with_slot() {
		let plan = DocumentPlan {
			lily_type: "document-plan".to_string(),
			id: "test".to_string(),
			version: 1,
			title: String::new(),
			layout: String::new(),
			blocks: vec![Block {
				id: "principal".to_string(),
				kind: "paragraph".to_string(),
				style: "BodyText".to_string(),
				content: vec![ContentNode::Slot {
					slot: "Client Full Name".to_string(),
					format: None,
					marks: vec![],
				}],
				when: None,
				numbering: None,
				source_paragraph: None,
				list: None,
				keep_with_next: false,
				page_break_before: false,
				keep_together: false,
				path: None,
				rows: vec![],
			}],
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
			lists: HashMap::new(),
			metadata: None,
			base_dir: PathBuf::new(),
		};
		let mut warnings = Vec::new();
		let (body, count) = render_body(&plan, &view(), &[], &mut warnings).unwrap();
		assert_eq!(count, 1);
		assert!(body.contains("Jane A. Doe"));
		assert!(body.contains("<w:pStyle w:val=\"BodyText\"/>"));
	}

	#[test]
	fn slot_lookup_is_case_insensitive_and_infers_format() {
		let plan = DocumentPlan {
			lily_type: "document-plan".to_string(),
			id: "test".to_string(),
			version: 1,
			title: String::new(),
			layout: String::new(),
			blocks: vec![Block {
				id: "agent".to_string(),
				kind: "paragraph".to_string(),
				style: String::new(),
				content: vec![ContentNode::Slot {
					slot: "PRIMARY HPOA AGENT FULL NAME".to_string(),
					format: None,
					marks: vec![],
				}],
				when: None,
				numbering: None,
				source_paragraph: None,
				list: None,
				keep_with_next: false,
				page_break_before: false,
				keep_together: false,
				path: None,
				rows: vec![],
			}],
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
			lists: HashMap::new(),
			metadata: None,
			base_dir: PathBuf::new(),
		};
		let mut warnings = Vec::new();
		let (body, _) = render_body(&plan, &view(), &[], &mut warnings).unwrap();
		assert!(body.contains("MARY SMITH"));
	}

	#[test]
	fn render_conditional_clause() {
		let mut clauses = HashMap::new();
		clauses.insert(
			"secondary".to_string(),
			Clause {
				id: "secondary".to_string(),
				version: 1,
				title: String::new(),
				body: vec![ContentNode::Text {
					text: "Second sentence.".to_string(),
					marks: vec![],
				}],
				when: None,
			},
		);
		let plan = DocumentPlan {
			lily_type: "document-plan".to_string(),
			id: "test".to_string(),
			version: 1,
			title: String::new(),
			layout: String::new(),
			blocks: vec![Block {
				id: "body".to_string(),
				kind: "paragraph".to_string(),
				style: String::new(),
				content: vec![ContentNode::Clause {
					clause: "secondary".to_string(),
					when: Some(json!({"eq": ["has_spouse", true]})),
				}],
				when: None,
				numbering: None,
				source_paragraph: None,
				list: None,
				keep_with_next: false,
				page_break_before: false,
				keep_together: false,
				path: None,
				rows: vec![],
			}],
			clauses,
			slots: HashMap::new(),
			terms: HashMap::new(),
			lists: HashMap::new(),
			metadata: None,
			base_dir: PathBuf::new(),
		};
		let mut warnings = Vec::new();
		let (body, _) = render_body(&plan, &view(), &[], &mut warnings).unwrap();
		assert!(body.contains("Second sentence."));
	}

	#[test]
	fn replace_body_preserves_section_properties() {
		let skeleton = "<w:document><w:body><w:p>old</w:p><w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>";
		let result = replace_body(skeleton, "<w:p>new</w:p>").unwrap();
		assert!(result.contains("<w:p>new</w:p>"));
		assert!(result.contains("<w:sectPr><w:pgSz/></w:sectPr>"));
		assert!(!result.contains("old"));
	}

	#[test]
	fn replace_simple_sdt_in_footer() {
		let footer = r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="lily:Client Full Name"/></w:sdtPr><w:sdtContent><w:r><w:t>Client Full Name</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
		let mut warnings = Vec::new();
		let result = replace_simple_sdts(footer, &view(), &mut warnings);
		assert!(result.contains("Jane A. Doe"));
		assert!(!result.contains("<w:t>Client Full Name</w:t>"));
		assert!(result.contains(r#"<w:tag w:val="lily:Client Full Name"/>"#));
		assert!(warnings.is_empty());
	}

	#[test]
	fn term_resolution_uses_number_condition() {
		let term = Term {
			id: "Settlor".to_string(),
			forms: HashMap::from([
				("singular".to_string(), "Settlor".to_string()),
				("plural".to_string(), "Settlors".to_string()),
				("possessive_singular".to_string(), "Settlor's".to_string()),
				("possessive_plural".to_string(), "Settlors'".to_string()),
			]),
			number: Some(json!({"eq": ["has_spouse", true]})),
		};
		assert_eq!(
			resolve_term(&term, Some("possessive"), Some("auto"), &view()),
			"Settlors'"
		);
		assert_eq!(
			resolve_term(&term, Some("singular"), Some("auto"), &view()),
			"Settlor"
		);
		assert_eq!(resolve_term(&term, None, None, &view()), "Settlors");
		assert_eq!(
			resolve_term(&term, None, Some("singular"), &view()),
			"Settlor"
		);
	}

	fn test_block(
		id: &str,
		kind: &str,
		content: Vec<ContentNode>,
		numbering: Option<&str>,
	) -> Block {
		Block {
			id: id.to_string(),
			kind: kind.to_string(),
			style: String::new(),
			content,
			when: None,
			numbering: numbering.map(|value| value.to_string()),
			source_paragraph: None,
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		}
	}

	fn test_plan(blocks: Vec<Block>) -> DocumentPlan {
		DocumentPlan {
			lily_type: "document-plan".to_string(),
			id: "test".to_string(),
			version: 1,
			title: String::new(),
			layout: String::new(),
			blocks,
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
			lists: HashMap::new(),
			metadata: None,
			base_dir: PathBuf::new(),
		}
	}

	fn text_node(value: &str) -> ContentNode {
		ContentNode::Text {
			text: value.to_string(),
			marks: vec![],
		}
	}

	#[test]
	fn numbering_assigns_articles_sections_and_references() {
		let blocks = vec![
			test_block(
				"art-1",
				"heading",
				vec![
					text_node("ARTICLE "),
					ContentNode::Number {
						number: "article".to_string(),
					},
				],
				Some("article"),
			),
			test_block(
				"sec-1-1",
				"heading",
				vec![
					text_node("Section "),
					ContentNode::Number {
						number: "section".to_string(),
					},
				],
				Some("section"),
			),
			test_block(
				"ref-art-2",
				"paragraph",
				vec![ContentNode::Ref {
					target: "art-2".to_string(),
					format: Some("Article {number}".to_string()),
				}],
				None,
			),
			test_block(
				"art-2",
				"heading",
				vec![
					text_node("ARTICLE "),
					ContentNode::Number {
						number: "article".to_string(),
					},
				],
				Some("article"),
			),
			test_block(
				"sec-2-1",
				"heading",
				vec![
					text_node("Section "),
					ContentNode::Number {
						number: "section".to_string(),
					},
				],
				Some("section"),
			),
		];
		let mut warnings = Vec::new();
		let (body, _) = render_body(&test_plan(blocks), &view(), &[], &mut warnings).unwrap();
		assert!(body.contains("<w:t xml:space=\"preserve\">ARTICLE </w:t>"));
		assert!(body.contains("<w:t xml:space=\"preserve\">I</w:t>"));
		assert!(body.contains("<w:t xml:space=\"preserve\">Section </w:t>"));
		assert!(body.contains("<w:t xml:space=\"preserve\">1.1</w:t>"));
		assert!(body.contains("Article II"));
		assert!(body.contains("<w:t xml:space=\"preserve\">II</w:t>"));
		assert!(body.contains("<w:t xml:space=\"preserve\">2.1</w:t>"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn table_blocks_emit_word_tables() {
		let blocks = vec![Block {
			id: "table-1".to_string(),
			kind: "table".to_string(),
			style: "TableGrid".to_string(),
			content: vec![],
			when: None,
			numbering: None,
			source_paragraph: None,
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![
				vec![
					TableCell {
						content: vec![text_node("Name")],
						style: String::new(),
					},
					TableCell {
						content: vec![text_node("Value")],
						style: String::new(),
					},
				],
				vec![
					TableCell {
						content: vec![text_node("Alice")],
						style: String::new(),
					},
					TableCell {
						content: vec![text_node("42")],
						style: String::new(),
					},
				],
			],
		}];
		let mut warnings = Vec::new();
		let (body, count) = render_body(&test_plan(blocks), &view(), &[], &mut warnings).unwrap();
		assert_eq!(count, 1);
		assert!(body.contains("<w:tbl>"));
		assert!(body.contains("<w:tr>"));
		assert!(body.contains("Alice"));
		assert!(body.contains("42"));
	}

	#[test]
	fn settlor_helpers_follow_spouse_fact() {
		let mut lily = LilyFile::default();
		lily.variables
			.insert("Has Spouse".to_string(), "true".to_string());
		lily.variables
			.insert("Client Spouse Name".to_string(), "John B. Doe".to_string());
		let view = DataView::from_lily(&lily);
		assert_eq!(view.get("Settlor Term"), Some("Settlors"));
		assert_eq!(view.get("Settlor Possessive"), Some("Settlors\u{2019}"));
		assert_eq!(view.get("Settlor Verb"), Some("are"));
		assert_eq!(view.get("And Spouse Name"), Some(" and John B. Doe"));

		lily.variables
			.insert("Has Spouse".to_string(), "false".to_string());
		let view = DataView::from_lily(&lily);
		assert_eq!(view.get("Settlor Term"), Some("Settlor"));
		assert_eq!(view.get("Settlor Verb"), Some("is"));
	}

	#[test]
	fn section_local_numbering_resets_per_article() {
		let blocks = vec![
			test_block(
				"art-1",
				"heading",
				vec![
					text_node("ARTICLE "),
					ContentNode::Number {
						number: "article".to_string(),
					},
				],
				Some("article"),
			),
			test_block(
				"sec-1",
				"heading",
				vec![
					text_node("Section "),
					ContentNode::Number {
						number: "section_local".to_string(),
					},
				],
				Some("section_local"),
			),
			test_block(
				"sec-2",
				"heading",
				vec![
					text_node("Section "),
					ContentNode::Number {
						number: "section_local".to_string(),
					},
				],
				Some("section_local"),
			),
			test_block(
				"art-2",
				"heading",
				vec![
					text_node("ARTICLE "),
					ContentNode::Number {
						number: "article".to_string(),
					},
				],
				Some("article"),
			),
			test_block(
				"sec-1b",
				"heading",
				vec![
					text_node("Section "),
					ContentNode::Number {
						number: "section_local".to_string(),
					},
				],
				Some("section_local"),
			),
		];
		let mut warnings = Vec::new();
		let (body, _) = render_body(&test_plan(blocks), &view(), &[], &mut warnings).unwrap();
		assert_eq!(body.matches(">1</w:t>").count(), 2);
		assert_eq!(body.matches(">2</w:t>").count(), 1);
		assert!(warnings.is_empty());
	}

	#[test]
	fn source_paragraph_formatting_is_used() {
		let source = SourceFormatting {
			ppr: "<w:pPr><w:ind w:left=\"720\"/></w:pPr>".to_string(),
			rpr: "<w:rPr><w:rFonts w:ascii=\"Times New Roman\"/><w:sz w:val=\"24\"/></w:rPr>"
				.to_string(),
		};
		let block = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Hello")],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (body, _) =
			render_body(&test_plan(vec![block]), &view(), &[source], &mut warnings).unwrap();
		assert!(body.contains("w:ind w:left=\"720\""));
		assert!(body.contains("w:rFonts"));
		assert!(body.contains("Times New Roman"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_keeps_drawings_and_run_formatting() {
		let document = r#"<w:document><w:body><w:p><w:pPr><w:jc w:val="both"/></w:pPr><w:r><w:rPr/><w:drawing/></w:r><w:r><w:rPr><w:sz w:val="24"/><w:u w:val="single"/></w:rPr><w:tab/><w:tab/></w:r><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>Hello world</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#;
		let block = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![
				ContentNode::Tab { tab: true },
				ContentNode::Tab { tab: true },
				text_node("Hello world"),
			],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (edited, count) =
			edit_body(document, &test_plan(vec![block]), &view(), &[], &mut warnings)
				.unwrap()
				.expect("body is editable");
		assert_eq!(count, 1);
		assert!(edited.contains("<w:drawing/>"), "drawing lost: {edited}");
		assert!(edited.contains("<w:jc w:val=\"both\"/>"));
		let before_text = edited
			.split("<w:t xml:space=\"preserve\">Hello world</w:t>")
			.next()
			.expect("text present");
		let text_run = before_text.rsplit("<w:r>").next().expect("text run");
		assert!(
			!text_run.contains("<w:u "),
			"text inherited the blank's underline: {edited}"
		);
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_replaces_slots_inside_sdts() {
		let document = r#"<w:document><w:body><w:p><w:r><w:t xml:space="preserve">I, </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="lily:Client Full Name"/></w:sdtPr><w:sdtContent><w:r><w:rPr><w:caps/></w:rPr><w:t>Client Full Name</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t>, of Denver.</w:t></w:r></w:p></w:body></w:document>"#;
		let block = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![
				text_node("I, "),
				ContentNode::Slot {
					slot: "Client Full Name".to_string(),
					format: None,
					marks: vec![],
				},
				text_node(", of Denver."),
			],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (edited, _) =
			edit_body(document, &test_plan(vec![block]), &view(), &[], &mut warnings)
				.unwrap()
				.expect("body is editable");
		assert!(edited.contains("Jane A. Doe"));
		assert!(edited.contains("<w:caps/>"));
		assert!(!edited.contains("Client Full Name"));
		assert!(edited.contains(", of Denver."));
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_removes_conditioned_paragraphs() {
		let document = r#"<w:document><w:body><w:p><w:r><w:t>Keep me</w:t></w:r></w:p><w:p><w:r><w:t>Remove me</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#;
		let keep = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Keep me")],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let remove = Block {
			id: "block-001".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Remove me")],
			when: Some(json!({"eq": ["has_spouse", false]})),
			numbering: None,
			source_paragraph: Some(1),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (edited, count) =
			edit_body(document, &test_plan(vec![keep, remove]), &view(), &[], &mut warnings)
				.unwrap()
				.expect("body is editable");
		assert_eq!(count, 1);
		assert!(edited.contains("Keep me"));
		assert!(!edited.contains("Remove me"));
		assert!(edited.contains("<w:sectPr/>"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_inserts_blocks_without_a_source() {
		let document = r#"<w:document><w:body><w:p><w:r><w:t>First</w:t></w:r></w:p></w:body></w:document>"#;
		let first = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("First")],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let extra = Block {
			id: "guardian-note".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Inserted")],
			when: None,
			numbering: None,
			source_paragraph: None,
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (edited, count) =
			edit_body(document, &test_plan(vec![first, extra]), &view(), &[], &mut warnings)
				.unwrap()
				.expect("body is editable");
		assert_eq!(count, 2);
		assert!(edited.contains("First"));
		assert!(edited.contains("Inserted"));
		assert!(edited.find("First").unwrap() < edited.find("Inserted").unwrap());
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_handles_self_closing_paragraphs() {
		let document = r#"<w:document><w:body><w:p w14:paraId="1"/><w:p><w:r><w:t>Text</w:t></w:r></w:p></w:body></w:document>"#;
		let empty = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let text = Block {
			id: "block-001".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Text")],
			when: None,
			numbering: None,
			source_paragraph: Some(1),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (edited, _) =
			edit_body(document, &test_plan(vec![empty, text]), &view(), &[], &mut warnings)
				.unwrap()
				.expect("body is editable");
		assert!(!edited.contains("/></w:p>"), "self-closing paragraph left a stray close: {edited}");
		assert!(edited.contains("w14:paraId=\"1\""));
		assert!(edited.contains("Text"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_falls_back_when_no_block_matches() {
		let document = r#"<w:document><w:body><w:p><w:r><w:t>Template</w:t></w:r></w:p></w:body></w:document>"#;
		let block = Block {
			id: "authored-clause".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Authored")],
			when: None,
			numbering: None,
			source_paragraph: None,
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let result =
			edit_body(document, &test_plan(vec![block]), &view(), &[], &mut warnings).unwrap();
		assert!(result.is_none(), "unmatched plans must use the rebuild path");
		assert!(warnings.is_empty());
	}

	#[test]
	fn in_place_editing_splits_text_across_source_runs() {
		let document = r#"<w:document><w:body><w:p><w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t>Title</w:t></w:r><w:r><w:t>. Body text.</w:t></w:r></w:p></w:body></w:document>"#;
		let block = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("Title. Body text.")],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let (edited, _) =
			edit_body(document, &test_plan(vec![block]), &view(), &[], &mut warnings)
				.unwrap()
				.expect("body is editable");
		let underlined = edited
			.split("<w:u w:val=\"single\"/>")
			.nth(1)
			.expect("underline present");
		let underlined_run = underlined.split("</w:r>").next().expect("run");
		assert!(underlined_run.contains("Title"));
		assert!(!underlined_run.contains("Body"));
		assert!(edited.contains(". Body text."));
		assert!(warnings.is_empty());
	}

	#[test]
	fn inserted_blocks_inherit_the_preceding_paragraph_formatting() {
		let document = r#"<w:document><w:body><w:p><w:pPr><w:jc w:val="both"/></w:pPr><w:r><w:rPr><w:rFonts w:ascii="Times New Roman"/></w:rPr><w:t>First</w:t></w:r></w:p></w:body></w:document>"#;
		let first = Block {
			id: "block-000".to_string(),
			kind: "paragraph".to_string(),
			style: String::new(),
			content: vec![text_node("First")],
			when: None,
			numbering: None,
			source_paragraph: Some(0),
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let inserted = Block {
			id: "note".to_string(),
			kind: "paragraph".to_string(),
			style: "Normal".to_string(),
			content: vec![text_node("Inserted")],
			when: None,
			numbering: None,
			source_paragraph: None,
			list: None,
			keep_with_next: false,
			page_break_before: false,
			keep_together: false,
			path: None,
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let sources = vec![SourceFormatting {
			ppr: "<w:pPr><w:jc w:val=\"both\"/></w:pPr>".to_string(),
			rpr: "<w:rPr><w:rFonts w:ascii=\"Times New Roman\"/></w:rPr>".to_string(),
		}];
		let (edited, _) =
			edit_body(document, &test_plan(vec![first, inserted]), &view(), &sources, &mut warnings)
				.unwrap()
				.expect("body is editable");
		let position = edited.find("Inserted").expect("inserted text");
		let start = edited[..position].rfind("<w:p>").expect("inserted paragraph");
		let paragraph = &edited[start..];
		assert!(paragraph.contains("<w:jc w:val=\"both\"/>"));
		assert!(paragraph.contains("Times New Roman"));
		assert!(!paragraph.contains("w:pStyle"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn list_separators_follow_enabled_items() {
		fn list_block(id: &str, value: &str) -> Block {
			Block {
				id: id.to_string(),
				kind: "paragraph".to_string(),
				style: String::new(),
				content: vec![text_node(value)],
				when: None,
				numbering: None,
				source_paragraph: None,
				list: Some("letters".to_string()),
				keep_with_next: false,
				page_break_before: false,
				keep_together: false,
				path: None,
				rows: vec![],
			}
		}
		fn body_text(body: &str) -> String {
			let pattern = regex::Regex::new(r"(?s)<w:t[^>]*>(.*?)</w:t>").unwrap();
			pattern
				.captures_iter(body)
				.map(|captures| captures[1].to_string())
				.collect::<Vec<_>>()
				.join("")
		}
		fn group() -> ListGroup {
			ListGroup {
				separator: "; ".to_string(),
				last_separator: "; and ".to_string(),
				terminator: ".".to_string(),
			}
		}

		let mut all = test_plan(vec![list_block("a", "A"), list_block("b", "B"), list_block("c", "C")]);
		all.lists.insert("letters".to_string(), group());
		let mut warnings = Vec::new();
		let (body, _) = render_body(&all, &view(), &[], &mut warnings).unwrap();
		assert_eq!(body_text(&body), "A; B; and C.");

		let mut middle_off = test_plan(vec![list_block("a", "A"), list_block("b", "B"), list_block("c", "C")]);
		middle_off.lists.insert("letters".to_string(), group());
		middle_off.blocks[1].when = Some(json!({"eq": ["has_spouse", false]}));
		let (body, _) = render_body(&middle_off, &view(), &[], &mut warnings).unwrap();
		assert_eq!(body_text(&body), "A; and C.");

		let mut single = test_plan(vec![list_block("c", "C")]);
		single.lists.insert("letters".to_string(), group());
		let (body, _) = render_body(&single, &view(), &[], &mut warnings).unwrap();
		assert_eq!(body_text(&body), "C.");
		assert!(warnings.is_empty());
	}

	#[test]
	fn audit_plan_reports_missing_slots_and_conditions() {
		let mut plan = test_plan(vec![
			test_block(
				"present",
				"paragraph",
				vec![ContentNode::Slot {
					slot: "Client Full Name".to_string(),
					format: None,
					marks: vec![],
				}],
				None,
			),
			test_block(
				"absent",
				"paragraph",
				vec![ContentNode::Slot {
					slot: "Missing Slot".to_string(),
					format: None,
					marks: vec![],
				}],
				None,
			),
		]);
		plan.blocks[0].when = Some(json!({"eq": ["has_spouse", true]}));
		let audit = audit_plan(&plan, &view());
		assert_eq!(
			audit.used_slots,
			vec!["Client Full Name".to_string(), "Missing Slot".to_string()]
		);
		assert_eq!(audit.missing_slots.len(), 1);
		assert_eq!(audit.missing_slots[0].name, "Missing Slot");
		assert_eq!(audit.conditions.len(), 1);
		assert_eq!(audit.conditions[0].result, Some(true));
		assert_eq!(
			audit.conditions[0].variables.get("has_spouse"),
			Some(&Some("true".to_string()))
		);
	}

	#[test]
	fn layout_hints_emit_word_properties() {
		let mut block = test_block("spaced", "paragraph", vec![text_node("Text")], None);
		block.keep_with_next = true;
		block.page_break_before = true;
		block.keep_together = true;
		let mut warnings = Vec::new();
		let (body, _) =
			render_body(&test_plan(vec![block]), &view(), &[], &mut warnings).unwrap();
		assert!(body.contains("<w:keepNext/>"));
		assert!(body.contains("<w:pageBreakBefore/>"));
		assert!(body.contains("<w:keepLines/>"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn layout_hints_merge_into_source_properties() {
		let source = SourceFormatting {
			ppr: "<w:pPr><w:jc w:val=\"both\"/></w:pPr>".to_string(),
			rpr: String::new(),
		};
		let mut block = test_block("block-000", "paragraph", vec![text_node("Text")], None);
		block.source_paragraph = Some(0);
		block.keep_with_next = true;
		let mut warnings = Vec::new();
		let (body, _) =
			render_body(&test_plan(vec![block]), &view(), &[source], &mut warnings).unwrap();
		assert!(body.contains("<w:jc w:val=\"both\"/>"));
		assert!(body.contains("<w:keepNext/>"));
		assert!(warnings.is_empty());
	}

	#[test]
	fn excerpt_blocks_render_their_fragment() {
		let unique = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("clock")
			.as_nanos();
		let directory = std::env::temp_dir().join(format!("lily-excerpt-{}", unique));
		std::fs::create_dir_all(&directory).expect("temp directory");
		let excerpt = test_plan(vec![test_block(
			"excerpt-body",
			"paragraph",
			vec![text_node("Excerpt text")],
			None,
		)]);
		std::fs::write(
			directory.join("excerpt.plan.json"),
			serde_json::to_string(&excerpt).expect("excerpt json"),
		)
		.expect("excerpt plan");
		let mut excerpt_block = test_block("excerpt", "excerpt", vec![], None);
		excerpt_block.path = Some("excerpt.plan.json".to_string());
		let mut plan = test_plan(vec![excerpt_block.clone()]);
		plan.base_dir = directory.clone();
		let mut warnings = Vec::new();
		let (body, count) = render_body(&plan, &view(), &[], &mut warnings).unwrap();
		assert_eq!(count, 1);
		assert!(body.contains("Excerpt text"));
		assert!(warnings.is_empty());

		let mut skipped = excerpt_block;
		skipped.when = Some(json!({"eq": ["has_spouse", false]}));
		let mut plan = test_plan(vec![skipped]);
		plan.base_dir = directory.clone();
		let mut warnings = Vec::new();
		let (body, count) = render_body(&plan, &view(), &[], &mut warnings).unwrap();
		assert_eq!(count, 0);
		assert!(!body.contains("Excerpt text"));
		assert!(warnings.is_empty());
		std::fs::remove_dir_all(&directory).ok();
	}

	#[test]
	fn audit_expands_excerpt_blocks() {
		let unique = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("clock")
			.as_nanos();
		let directory = std::env::temp_dir().join(format!("lily-excerpt-audit-{}", unique));
		std::fs::create_dir_all(&directory).expect("temp directory");
		let mut child = test_block(
			"child",
			"paragraph",
			vec![ContentNode::Slot {
				slot: "Excerpt Missing".to_string(),
				format: None,
				marks: vec![],
			}],
			None,
		);
		child.when = Some(json!({"eq": ["has_spouse", true]}));
		let excerpt = test_plan(vec![child]);
		std::fs::write(
			directory.join("excerpt.plan.json"),
			serde_json::to_string(&excerpt).expect("excerpt json"),
		)
		.expect("excerpt plan");
		let mut excerpt_block = test_block("excerpt", "excerpt", vec![], None);
		excerpt_block.path = Some("excerpt.plan.json".to_string());
		let mut plan = test_plan(vec![excerpt_block]);
		plan.base_dir = directory.clone();
		let expanded = inline_excerpts(&plan, &view()).expect("expanded plan");
		assert_eq!(expanded.blocks.len(), 1);
		assert_eq!(expanded.blocks[0].id, "excerpt:child");
		let audit = audit_plan(&expanded, &view());
		assert_eq!(audit.used_slots, vec!["Excerpt Missing".to_string()]);
		assert_eq!(audit.missing_slots.len(), 1);
		assert_eq!(audit.conditions.len(), 1);
		assert_eq!(audit.conditions[0].location, "excerpt:child");

		let mut skipped = test_plan(vec![test_block("excerpt", "excerpt", vec![], None)]);
		skipped.base_dir = directory.clone();
		skipped.blocks[0].path = Some("excerpt.plan.json".to_string());
		skipped.blocks[0].when = Some(json!({"eq": ["has_spouse", false]}));
		let expanded = inline_excerpts(&skipped, &view()).expect("expanded plan");
		assert!(expanded.blocks.is_empty());
		std::fs::remove_dir_all(&directory).ok();
	}

	#[test]
	fn candidate_paths_are_scoped_to_lily_candidates() {
		let candidate = candidate_output_path(Path::new("/tmp/Last Will.docx"));
		let name = candidate
			.file_name()
			.and_then(|value| value.to_str())
			.unwrap_or("");
		assert!(name.starts_with("lily-candidate-"));
		assert!(name.ends_with(".docx"));
		assert!(ensure_candidate(&candidate).is_ok());
		assert!(ensure_candidate(Path::new("/tmp/Last Will.docx")).is_err());
	}

	fn write_minimal_docx(path: &Path, text: &str) {
		let file = fs::File::create(path).expect("docx file");
		let mut writer = ZipWriter::new(file);
		let options =
			SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
		writer
			.start_file("word/document.xml", options)
			.expect("document entry");
		let xml = format!(
			"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p><w:sectPr/></w:body></w:document>",
			text
		);
		writer.write_all(xml.as_bytes()).expect("document xml");
		writer.finish().expect("docx finish");
	}

	#[test]
	fn render_candidate_returns_a_diff_and_a_html_preview() {
		let unique = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("clock")
			.as_nanos();
		let directory = std::env::temp_dir().join(format!("lily-candidate-test-{}", unique));
		let client_dir = directory.join("client");
		fs::create_dir_all(&client_dir).expect("client directory");
		fs::write(
			client_dir.join("Client.lily"),
			r#"{"lily_type":"client","lily_version":4,"variables":{"Client Full Name":"Jane Doe"},"documents":{},"contacts":[],"contact_bindings":{}}"#,
		)
		.expect("client file");
		let skeleton = directory.join("skeleton.docx");
		write_minimal_docx(&skeleton, "Old text");
		let document = client_dir.join("Will.docx");
		fs::copy(&skeleton, &document).expect("document copy");
		let plan_path = directory.join("plan.json");
		fs::write(
			&plan_path,
			r#"{"lily_type":"document-plan","id":"candidate-test","version":1,"layout":"skeleton.docx","blocks":[{"id":"block-000","kind":"paragraph","style":"Normal","content":[{"text":"Hello "},{"slot":"Client Full Name"}]}]}"#,
		)
		.expect("plan file");

		let report = render_plan_candidate(
			document.to_string_lossy().to_string(),
			plan_path.to_string_lossy().to_string(),
			skeleton.to_string_lossy().to_string(),
		)
		.expect("candidate");
		assert_eq!(report.plan_id, "candidate-test");
		assert_eq!(report.blocks_rendered, 1);
		assert!(report.candidate_html.contains("Hello Jane Doe"));
		assert!(report
			.diff
			.iter()
			.any(|line| line.kind == "removed" && line.text == "Old text"));
		assert!(report
			.diff
			.iter()
			.any(|line| line.kind == "added" && line.text == "Hello Jane Doe"));
		discard_plan_candidate(report.candidate_path).expect("discard");
		fs::remove_dir_all(&directory).ok();
	}

	#[test]
	fn roman_numeral_conversion() {
		assert_eq!(to_roman(1), "I");
		assert_eq!(to_roman(4), "IV");
		assert_eq!(to_roman(9), "IX");
		assert_eq!(to_roman(14), "XIV");
		assert_eq!(to_roman(40), "XL");
	}
}
