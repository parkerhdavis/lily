// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deterministic document plan renderer.
//!
//! A plan is a JSON document of ordered blocks, inline content nodes, clauses,
//! slots, and defined terms. This module resolves the plan against a client
//! `.lily` file and emits a `.docx` by replacing the body of a Word layout
//! skeleton. It makes no model calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::LazyLock;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::lily_file::{self, LilyFile};

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
	/// Rows of cells for a table block.
	#[serde(default)]
	pub rows: Vec<Vec<TableCell>>,
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
}

impl DataView {
	pub fn from_lily(lily: &LilyFile) -> Self {
		let mut view = Self {
			variables: lily.variables.clone(),
			facts: HashMap::new(),
		};
		for (key, value) in &view.variables {
			if value == "true" || value == "false" {
				let fact = value == "true";
				view.facts.insert(key.clone(), fact);
				view.facts.insert(snake_case(key), fact);
			}
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
		view
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

fn render_run(text: &str, marks: &[String]) -> String {
	if text.is_empty() {
		return String::new();
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
	let key = match form.unwrap_or("singular") {
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

fn render_content(
	nodes: &[ContentNode],
	plan: &DocumentPlan,
	data: &DataView,
	numbers: &HashMap<String, String>,
	current_block: Option<&str>,
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let mut output = String::new();
	for node in nodes {
		match node {
			ContentNode::Text { text, marks } => output.push_str(&render_run(text, marks)),
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
				output.push_str(&render_run(&apply_format(value, effective_format), marks));
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
				));
			}
			ContentNode::Ref { target, format } => match numbers.get(target) {
				Some(number) => {
					let template = format.as_deref().unwrap_or("{number}");
					output.push_str(&render_run(&template.replace("{number}", number), &[]));
				}
				None => warnings.push(format!("Cross-reference target not found: {}", target)),
			},
			ContentNode::Number { number } => match current_block.and_then(|id| numbers.get(id)) {
				Some(value) => output.push_str(&render_run(value, &[])),
				None => warnings.push(format!(
					"Number '{}' has no assigned value in block {:?}",
					number, current_block
				)),
			},
			ContentNode::Tab { tab } => {
				if *tab {
					output.push_str("<w:r><w:tab/></w:r>");
				}
			}
			ContentNode::LineBreak { line_break, page } => {
				if *line_break {
					if *page {
						output.push_str("<w:r><w:br w:type=\"page\"/></w:r>");
					} else {
						output.push_str("<w:r><w:br/></w:r>");
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
	warnings: &mut Vec<String>,
) -> Result<String, String> {
	let content = render_content(
		&block.content,
		plan,
		data,
		numbers,
		Some(&block.id),
		warnings,
	)?;
	let properties = if block.style.is_empty() {
		String::new()
	} else {
		format!(
			"<w:pPr><w:pStyle w:val=\"{}\"/></w:pPr>",
			escape_xml(&block.style)
		)
	};
	Ok(format!("<w:p>{}{}</w:p>", properties, content))
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

/// Render the plan's blocks to WordprocessingML body content.
pub fn render_body(
	plan: &DocumentPlan,
	data: &DataView,
	warnings: &mut Vec<String>,
) -> Result<(String, usize), String> {
	let numbers = assign_numbers(plan, data)?;
	let mut body = String::new();
	let mut count = 0;
	for block in &plan.blocks {
		if let Some(condition) = &block.when {
			if !eval_condition(condition, data)? {
				continue;
			}
		}
		match block.kind.as_str() {
			"paragraph" | "heading" => {
				body.push_str(&render_paragraph(block, plan, data, &numbers, warnings)?);
				count += 1;
			}
			"table" => {
				body.push_str(&render_table(block, plan, data, &numbers, warnings)?);
				count += 1;
			}
			"page-break" => {
				body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
				count += 1;
			}
			other => {
				return Err(format!(
					"Unsupported block kind '{}' in block '{}'",
					other, block.id
				))
			}
		}
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
#[tauri::command]
pub fn render_plan_document(docx_path: String, plan_path: String) -> Result<RenderReport, String> {
	let path = Path::new(&docx_path);
	let client_dir = path
		.parent()
		.ok_or_else(|| "Document has no parent directory".to_string())?;
	render_plan(Path::new(&plan_path), client_dir, path, path)
}

/// Render a plan against a client folder and a layout skeleton.
pub fn render_plan(
	plan_path: &Path,
	client_dir: &Path,
	skeleton_path: &Path,
	output_path: &Path,
) -> Result<RenderReport, String> {
	let plan_text = fs::read_to_string(plan_path)
		.map_err(|error| format!("Failed to read plan {}: {}", plan_path.display(), error))?;
	let plan: DocumentPlan = serde_json::from_str(&plan_text)
		.map_err(|error| format!("Failed to parse plan {}: {}", plan_path.display(), error))?;

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

	let mut warnings = Vec::new();
	let (body_xml, blocks_rendered) = render_body(&plan, &view, &mut warnings)?;

	let skeleton_bytes = fs::read(skeleton_path)
		.map_err(|error| format!("Failed to read skeleton {}: {}", skeleton_path.display(), error))?;
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
			let replaced = replace_body(&xml, &body_xml)?;
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
				rows: vec![],
			}],
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
		};
		let mut warnings = Vec::new();
		let (body, count) = render_body(&plan, &view(), &mut warnings).unwrap();
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
				rows: vec![],
			}],
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
		};
		let mut warnings = Vec::new();
		let (body, _) = render_body(&plan, &view(), &mut warnings).unwrap();
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
				rows: vec![],
			}],
			clauses,
			slots: HashMap::new(),
			terms: HashMap::new(),
		};
		let mut warnings = Vec::new();
		let (body, _) = render_body(&plan, &view(), &mut warnings).unwrap();
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
		let (body, _) = render_body(&test_plan(blocks), &view(), &mut warnings).unwrap();
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
		let (body, count) = render_body(&test_plan(blocks), &view(), &mut warnings).unwrap();
		assert_eq!(count, 1);
		assert!(body.contains("<w:tbl>"));
		assert!(body.contains("<w:tr>"));
		assert!(body.contains("Alice"));
		assert!(body.contains("42"));
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
