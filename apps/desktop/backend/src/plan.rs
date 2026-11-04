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
use std::path::{Path, PathBuf};
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
	/// Compiler metadata, including the source template path.
	#[serde(default)]
	pub metadata: Option<Value>,
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
	let mut groups: Vec<(usize, Vec<String>, Vec<Segment>)> = Vec::new();
	for segment in segments {
		let assigned = match &segment {
			Segment::Text { text, .. } if text.is_empty() => continue,
			Segment::Text { text, .. } => match find_text(&items, cursor, text) {
				Some(position) => {
					cursor = position + text.chars().count();
					run_of[position]
				}
				None => run_at(&run_of, cursor),
			},
			Segment::Tab => {
				match (cursor..items.len()).find(|index| items[*index] == SourceItem::Tab) {
					Some(position) => {
						cursor = position + 1;
						run_of[position]
					}
					None => run_at(&run_of, cursor),
				}
			}
			Segment::Break { page } => {
				match (cursor..items.len()).find(|index| {
					matches!(items[*index], SourceItem::Break { page: source } if source == *page)
				}) {
					Some(position) => {
						cursor = position + 1;
						run_of[position]
					}
					None => run_at(&run_of, cursor),
				}
			}
		};
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
	let mut inner = parsed.ppr.clone();
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

	let numbers = assign_numbers(plan, data)?;
	let mut replacements: HashMap<usize, Option<String>> = HashMap::new();
	let mut insertions: HashMap<usize, Vec<String>> = HashMap::new();
	let mut before_first: Vec<String> = Vec::new();
	let mut last_source: Option<usize> = None;
	let mut count = 0usize;
	let mut resolved = 0usize;

	for block in &plan.blocks {
		let enabled = match &block.when {
			Some(condition) => eval_condition(condition, data)?,
			None => true,
		};
		match block_source_child(block, &paragraphs, children.len()) {
			Some(index) if children[index].trim_start().starts_with("<w:p") => {
				last_source = Some(index);
				resolved += 1;
				if enabled {
					let edited = edit_paragraph(
						&children[index],
						block,
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
				resolved += 1;
				if enabled {
					let rendered =
						render_block(block, plan, data, &numbers, sources, warnings)?;
					replacements.insert(index, Some(rendered));
					count += 1;
				} else {
					replacements.insert(index, None);
				}
			}
			None if enabled => {
				let rendered = render_block(block, plan, data, &numbers, sources, warnings)?;
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
		"page-break" => Ok("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>".to_string()),
		other => Err(format!(
			"Unsupported block kind '{}' in block '{}'",
			other, block.id
		)),
	}
}

/// Render the plan's blocks to WordprocessingML body content.
pub fn render_body(
	plan: &DocumentPlan,
	data: &DataView,
	sources: &[SourceFormatting],
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
		body.push_str(&render_block(block, plan, data, &numbers, sources, warnings)?);
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
				source_paragraph: None,
				rows: vec![],
			}],
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
			metadata: None,
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
				rows: vec![],
			}],
			clauses: HashMap::new(),
			slots: HashMap::new(),
			terms: HashMap::new(),
			metadata: None,
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
				rows: vec![],
			}],
			clauses,
			slots: HashMap::new(),
			terms: HashMap::new(),
			metadata: None,
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
			metadata: None,
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
			rows: vec![],
		};
		let mut warnings = Vec::new();
		let result =
			edit_body(document, &test_plan(vec![block]), &view(), &[], &mut warnings).unwrap();
		assert!(result.is_none(), "unmatched plans must use the rebuild path");
		assert!(warnings.is_empty());
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
