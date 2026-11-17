// SPDX-License-Identifier: AGPL-3.0-or-later
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use zip::read::ZipArchive;

use tracing::{error, info};

use crate::lily_file;

// ─── Cached regexes ─────────────────────────────────────────────────────────
//
// These patterns are reused across many hot functions. Compiling a regex is
// non-trivial (Aho–Corasick / NFA construction), so we build each one once
// at first use and share it. Only patterns with no runtime interpolation
// live here; dynamic regexes built with format!() stay inline.

/// `<w:t [attrs]>text</w:t>` — one capture group for the text content.
static T_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:t(?: [^>]*)?>([^<]*)</w:t>"#).expect("invalid regex"));

/// Whole `<w:p ...>...</w:p>` paragraph.
static PARAGRAPH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<w:p(?:\s[^>]*)?>.*?</w:p>").expect("invalid regex"));

/// `<w:pgMar ... />` with the four margin values captured (top, right, bottom, left).
static PG_MARGIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<w:pgMar[^>]*w:top="(\d+)"[^>]*w:right="(\d+)"[^>]*w:bottom="(\d+)"[^>]*w:left="(\d+)"[^>]*/>"#,
    )
    .expect("invalid regex")
});

// ─── Types ──────────────────────────────────────────────────────────────────

/// Info about a single logical variable extracted from the document.
/// Variables that differ only in case (e.g., "CLIENT NAME" vs "Client Name")
/// are grouped into one VariableInfo.
#[derive(Debug, Serialize)]
pub struct VariableInfo {
    /// Display name shown in the UI (the title-cased or first-seen variant).
    /// For conditional variables, this is the label portion before the `??`.
    pub display_name: String,
    /// All distinct casings found in the document for this variable.
    pub variants: Vec<String>,
    /// Whether this is a conditional (ternary) variable.
    /// Conditional variables use the syntax `{Label ?? true_text :: false_text}`
    /// and display as a checkbox in the UI.
    pub is_conditional: bool,
}

// ─── Template schema types ──────────────────────────────────────────────────

/// Defines the true/false branch logic for a conditional variable.
/// Branch templates may contain `{VarName}` references that are resolved
/// at replacement time — including contact-role dot notation like
/// `{Healthcare POA Agent.full_name}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalDef {
    /// The variable whose value controls which branch is shown.
    /// Typically the same as the conditional variable's own name
    /// (e.g., "Has Healthcare POA Alternate Agent").
    pub controlling_variable: String,
    /// Template text for the true branch, with `{VarName}` references.
    pub true_template: String,
    /// Template text for the false branch (often empty).
    pub false_template: String,
}

/// Schema definition for a single variable in a template.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VariableSchemaEntry {
    /// The type of this variable: "text", "date", "currency", or "conditional".
    #[serde(default = "default_var_type")]
    pub var_type: String,
    /// Default value if the user hasn't provided one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Help text shown to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Date format string (for date variables, e.g., "MM/DD/YYYY").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
    /// Whether this field is required.
    #[serde(default)]
    pub required: bool,
    /// Conditional branch logic (only for `var_type: "conditional"`).
    /// Single-definition conditionals use `condition`; multi-occurrence
    /// conditionals (same variable controls different text in different
    /// places) use `conditions` with one entry per document occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<ConditionalDef>,
    /// Multiple conditional definitions for a single variable that appears
    /// in multiple places with different branch text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<ConditionalDef>,
    /// Contact role this variable auto-fills from (e.g., "Healthcare POA Agent").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_role: Option<String>,
    /// Contact property this variable maps to (e.g., "full_name").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_property: Option<String>,
}

fn default_var_type() -> String {
    "text".to_string()
}

/// Schema file for a template — stored as a .lily sidecar file alongside the template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableSchema {
    /// Identifies this as a template schema file.
    pub lily_type: String,
    /// The template filename this schema applies to.
    pub template_filename: String,
    /// Which fill engine the template uses: "sdt" (the current engine) or
    /// "plan" (the document-plan renderer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Path to the document plan, relative to the templates directory, when
    /// `engine` is "plan".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<String>,
    /// Map from variable display_name to its schema definition.
    #[serde(default)]
    pub variables: HashMap<String, VariableSchemaEntry>,
}

impl Default for VariableSchema {
    fn default() -> Self {
        Self {
            lily_type: "template-schema".to_string(),
            template_filename: String::new(),
            engine: None,
            plan_path: None,
            variables: HashMap::new(),
        }
    }
}

// ─── Tauri commands ─────────────────────────────────────────────────────────

/// Copy a template .docx file to the working directory.
/// Records the template provenance in the directory's sidecar file.
/// Returns the path to the new copy.
#[tauri::command]
pub fn copy_template(
    template_path: String,
    dest_dir: String,
    filename: String,
    template_rel_path: String,
    client_key: Option<String>,
) -> Result<String, String> {
    info!(%template_path, %filename, "Copying template");
    let src = Path::new(&template_path);
    if !src.exists() {
        error!(%template_path, "Template file not found");
        return Err(format!("Template file not found: {}", template_path));
    }

    let dest = Path::new(&dest_dir).join(&filename);
    fs::copy(src, &dest).map_err(|e| format!("Failed to copy template: {}", e))?;

    // Record template provenance in the .lily file
    lily_file::record_document(&dest_dir, &filename, &template_rel_path, client_key.as_deref())?;

    Ok(dest.to_string_lossy().to_string())
}

/// Rename a document file on disk and update its sidecar entry.
/// Returns the new full path.
#[tauri::command]
pub fn rename_document(docx_path: String, new_filename: String) -> Result<String, String> {
    let path = Path::new(&docx_path);
    let parent = path
        .parent()
        .ok_or_else(|| "Cannot determine parent directory".to_string())?;
    let old_filename = path
        .file_name()
        .ok_or_else(|| "Cannot determine filename".to_string())?
        .to_string_lossy()
        .to_string();

    // Ensure the new filename ends with .docx
    let new_filename = if new_filename.ends_with(".docx") {
        new_filename
    } else {
        format!("{}.docx", new_filename)
    };

    let new_path = parent.join(&new_filename);

    if new_path.exists() {
        return Err(format!("A file named '{}' already exists", new_filename));
    }

    fs::rename(&docx_path, &new_path).map_err(|e| format!("Failed to rename document: {}", e))?;

    // Update the .lily file entry (best-effort)
    let working_dir = parent.to_string_lossy().to_string();
    if let Err(e) = lily_file::rename_document_entry(&working_dir, &old_filename, &new_filename) {
        eprintln!("Warning: failed to update .lily file after rename: {}", e);
    }

    Ok(new_path.to_string_lossy().to_string())
}

/// Extract the non-empty paragraph text from a document, in order. Used by
/// the version diff; tables contribute their paragraph text.
pub fn document_lines(docx_path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(docx_path)
        .map_err(|error| format!("Failed to read {}: {}", docx_path.display(), error))?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Failed to open docx as zip: {}", error))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|error| format!("Failed to read document.xml: {}", error))?
        .read_to_string(&mut xml)
        .map_err(|error| format!("Failed to read document.xml: {}", error))?;
    let mut lines = Vec::new();
    for paragraph in PARAGRAPH_RE.find_iter(&xml) {
        let mut text = String::new();
        for capture in T_RE.captures_iter(paragraph.as_str()) {
            text.push_str(&unescape_text(&capture[1]));
        }
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    Ok(lines)
}

fn unescape_text(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Return the plan slots a template exposes as variables. The template
/// library is the source; the document itself no longer carries SDTs.
#[tauri::command]
pub fn load_template_variables(
    templates_dir: String,
    template_rel_path: String,
) -> Result<Vec<VariableInfo>, String> {
    let schema = load_template_schema(templates_dir, template_rel_path)?;
    if schema.engine.as_deref() != Some("plan") {
        return Err("Template is not plan-driven".to_string());
    }
    let mut variables: Vec<VariableInfo> = schema
        .variables
        .iter()
        .map(|(name, entry)| VariableInfo {
            display_name: name.clone(),
            variants: vec![name.clone()],
            is_conditional: entry.var_type == "conditional"
                || entry.condition.is_some()
                || !entry.conditions.is_empty(),
        })
        .collect();
    variables.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
    });
    Ok(variables)
}

/// Get an HTML representation of the .docx for preview purposes.
/// This extracts the text content with paragraph structure, preserving
/// variable placeholders for highlighting in the frontend.
#[tauri::command]
pub fn get_document_html(docx_path: String) -> Result<String, String> {
    let parts = read_docx_parts(&docx_path)?;
    let xml_content = normalize_split_variables(&parts.document);
    let numbering_map = parts
        .numbering
        .as_deref()
        .map(parse_numbering_xml)
        .unwrap_or_default();
    let style_map = parts
        .styles
        .as_deref()
        .map(parse_styles_xml)
        .unwrap_or_default();
    let rels_map = parts
        .rels
        .as_deref()
        .map(parse_rels_xml)
        .unwrap_or_default();
    let html = xml_to_preview_html(&xml_content, &numbering_map, &style_map, &rels_map);
    Ok(html)
}

// ─── Template authoring commands ────────────────────────────────────────────

// ─── Template editing session ───────────────────────────────────────────────

// ─── Template schema commands ───────────────────────────────────────────────

/// A template library file that stores schemas for all templates in one place.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemplateLibrary {
    lily_type: String,
    templates: HashMap<String, TemplateLibraryEntry>,
}

/// An entry in the template library for a single template.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemplateLibraryEntry {
    /// Which fill engine the template uses. Missing means the SDT engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    engine: Option<String>,
    /// Path to the document plan, relative to the templates directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan_path: Option<String>,
    variables: HashMap<String, VariableSchemaEntry>,
}

/// Find the template library .lily file in the templates directory.
/// Scans for .lily files at the root with `lily_type: "template-library"`.
/// Returns the first one found, or None.
fn find_template_library(templates_dir: &str) -> Option<PathBuf> {
    let dir = Path::new(templates_dir);
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "lily").unwrap_or(false) && path.is_file() {
            if let Ok(content) = fs::read_to_string(&path) {
                // Quick check for lily_type before full parse
                if content.contains("\"template-library\"") {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Load the full template library. Returns an empty library if none found.
fn load_library(templates_dir: &str) -> TemplateLibrary {
    if let Some(path) = find_template_library(templates_dir) {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(lib) = serde_json::from_str::<TemplateLibrary>(&content) {
                return lib;
            }
        }
    }
    TemplateLibrary {
        lily_type: "template-library".to_string(),
        templates: HashMap::new(),
    }
}

/// Legacy: derive the per-template sidecar path (for migration/fallback).
fn legacy_schema_path(templates_dir: &str, template_rel_path: &str) -> PathBuf {
    let template_path = Path::new(templates_dir).join(template_rel_path);
    let stem = template_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = template_path.parent().unwrap_or(Path::new(templates_dir));
    parent.join(format!("{}.lily", stem))
}

/// Load the variable schema for a template. Reads from the template library
/// file, falling back to a legacy per-template sidecar if present.
/// Returns a default (empty) schema if neither exists.
#[tauri::command]
pub fn load_template_schema(
    templates_dir: String,
    template_rel_path: String,
) -> Result<VariableSchema, String> {
    let template_filename = Path::new(&template_rel_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Try the library file first
    let library = load_library(&templates_dir);
    if let Some(entry) = library.templates.get(&template_rel_path) {
        return Ok(VariableSchema {
            lily_type: "template-schema".to_string(),
            template_filename,
            engine: entry.engine.clone(),
            plan_path: entry.plan_path.clone(),
            variables: entry.variables.clone(),
        });
    }

    // Fall back to legacy per-template sidecar
    let legacy_path = legacy_schema_path(&templates_dir, &template_rel_path);
    if legacy_path.exists() {
        let content = fs::read_to_string(&legacy_path)
            .map_err(|e| format!("Failed to read schema: {}", e))?;
        let schema: VariableSchema = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse schema: {}", e))?;
        return Ok(schema);
    }

    Ok(VariableSchema {
        template_filename,
        ..Default::default()
    })
}

// ─── Migration ─────────────────────────────────────────────────────────────

// ─── Internal helpers ───────────────────────────────────────────────────────

/// Pre-process Word XML to merge variable placeholders that are split across
/// multiple `<w:r>` runs. Word often breaks `{Variable Name}` into separate
/// runs like `<w:t>{</w:t>...<w:t>Variable </w:t>...<w:t>Name}</w:t>`.
/// This function detects such splits and consolidates the text into a single
/// run so that downstream processing (highlight, replace) can see the full
/// `{Variable Name}` string in one `<w:t>` element.
///
/// The approach: find `<w:t ...>...{` where the `{` doesn't have a matching `}`
/// within the same `<w:t>`. Then scan forward to find the `}` in a subsequent
/// `<w:t>`, and merge all intermediate text into the first `<w:t>`.
fn normalize_split_variables(xml: &str) -> String {
    // This regex matches a <w:t> element that contains an unmatched opening brace.
    // We use a loop-based approach: find each `<w:t` element, extract text,
    // check for split variables, and merge if needed.

    // First, collect all <w:r>...</w:r> runs to work with structured data
    // Actually, let's use a simpler direct string approach:
    // Find each occurrence of `{` inside <w:t> text that doesn't have a matching `}`
    // in the same text node, then merge forward.

    let t_open_re: &Regex = &T_RE;

    let mut result = xml.to_string();
    let mut search_from = 0;

    loop {
        // Find the next <w:t> element from our search position
        let remaining = &result[search_from..];
        let Some(t_match) = t_open_re.find(remaining) else {
            break;
        };

        let abs_start = search_from + t_match.start();
        let abs_end = search_from + t_match.end();
        let full_tag = &result[abs_start..abs_end];

        // Extract the text content between <w:t...> and </w:t>.
        // `t_match` succeeded on the same regex + slice, so captures must also.
        let text_content = &t_open_re
            .captures(remaining)
            .expect("t_open_re.find succeeded on this slice, so captures must too")[1];

        // Check if this text has unbalanced braces — more `{` than `}` means
        // part of a variable extends into subsequent runs. Use depth tracking
        // to support nested variables (e.g., conditionals containing {Var}).
        let mut brace_depth: i32 = 0;
        for ch in text_content.chars() {
            if ch == '{' {
                brace_depth += 1;
            } else if ch == '}' {
                brace_depth -= 1;
            }
        }

        if brace_depth <= 0 || !text_content.contains('{') {
            // All braces balanced or net-negative — no split variable here
            search_from = abs_end;
            continue;
        }

        // We have unbalanced `{`. Collect text from subsequent <w:t> elements
        // until the brace depth returns to zero (outermost `}` is found).
        let mut merged_text = text_content.to_string();
        let mut scan_pos = abs_end;
        let mut last_consumed_end = abs_end;
        let mut found_close = false;

        while scan_pos < result.len() {
            let scan_remaining = &result[scan_pos..];
            let Some(next_t) = t_open_re.find(scan_remaining) else {
                break;
            };

            let next_abs_start = scan_pos + next_t.start();
            let next_abs_end = scan_pos + next_t.end();

            // Only merge within the same paragraph — stop at </w:p>
            let between = &result[last_consumed_end..next_abs_start];
            if between.contains("</w:p>") {
                break;
            }

            // `next_t` succeeded on the same regex + slice, so captures must also.
            let next_text = &t_open_re
                .captures(scan_remaining)
                .expect("t_open_re.find succeeded on this slice, so captures must too")[1];
            merged_text.push_str(next_text);
            last_consumed_end = next_abs_end;

            // Update brace depth with the new text
            for ch in next_text.chars() {
                if ch == '{' {
                    brace_depth += 1;
                } else if ch == '}' {
                    brace_depth -= 1;
                }
            }

            if brace_depth <= 0 {
                found_close = true;
                break;
            }

            scan_pos = next_abs_end;
        }

        if !found_close {
            // Couldn't find balancing `}`, skip
            search_from = abs_end;
            continue;
        }

        // Verify the merged text actually contains a valid {Variable} pattern
        // (uses depth-tracking to support nested variables in conditionals)
        let has_valid_var = {
            let mut found = false;
            let mut chars = merged_text.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '{' && scan_brace_content(&mut chars).is_some() {
                    found = true;
                    break;
                }
            }
            found
        };

        if !has_valid_var {
            search_from = abs_end;
            continue;
        }

        // Build the replacement: keep the first <w:t>'s opening tag but with
        // the merged text, and remove the consumed subsequent <w:t> elements
        // by replacing the entire range from the first <w:t> to the last
        // consumed </w:t> end.

        // Extract the opening tag of the first <w:t> element (e.g., `<w:t xml:space="preserve">`)
        let opening_tag_end = full_tag
            .find('>')
            .expect("t_open_re match always contains the opening tag's '>'")
            + 1;
        let opening_tag = &full_tag[..opening_tag_end];

        // Build new element with merged text and xml:space="preserve"
        let new_opening = if opening_tag.contains("xml:space") {
            opening_tag.to_string()
        } else {
            opening_tag.replace("<w:t>", "<w:t xml:space=\"preserve\">")
        };

        // We need to replace the range from the start of the first <w:t> element
        // through the end of the last consumed element. But we can't just cut
        // the intermediate XML — we need to keep the <w:r> wrapper around our
        // merged text. The simplest correct approach: replace the text in the
        // first <w:t>, and blank out the text in subsequent consumed <w:t> elements.

        // Strategy: replace the entire span [abs_start..last_consumed_end] with:
        // 1. The first run's <w:r>...<w:t>MERGED_TEXT</w:t>...</w:r>
        // 2. The intermediate runs with their text emptied

        // Actually, the cleanest approach: just replace the text content in the
        // first <w:t> with the full merged text, and empty the intermediate <w:t> elements.
        //
        // The merged text was extracted via regex from valid XML text content, so
        // it is already properly XML-encoded (entities like &amp; &lt; &gt; are
        // preserved as-is). Do NOT re-escape — that would double-encode entities
        // (e.g., &quot; → &amp;quot;).
        let new_first_t = format!("{}{}</w:t>", new_opening, merged_text);

        // Replace the first <w:t>...</w:t>
        let mut new_xml = String::new();
        new_xml.push_str(&result[..abs_start]);
        new_xml.push_str(&new_first_t);

        // Now blank out all intermediate <w:t> elements between abs_end and last_consumed_end
        let mut intermediate = result[abs_end..last_consumed_end].to_string();
        intermediate = t_open_re
            .replace_all(&intermediate, |caps: &regex::Captures| {
                // Keep the tag but empty the text
                let full = caps
                    .get(0)
                    .expect("regex capture group 0 is always present")
                    .as_str();
                let tag_end = full
                    .find('>')
                    .expect("t_open_re match always contains the opening tag's '>'")
                    + 1;
                format!("{}</w:t>", &full[..tag_end])
            })
            .to_string();
        new_xml.push_str(&intermediate);

        new_xml.push_str(&result[last_consumed_end..]);

        let next_search = abs_start + new_first_t.len();
        result = new_xml;
        search_from = next_search;
    }

    result
}

/// Parts extracted from a .docx ZIP for preview rendering.
struct DocxParts {
    document: String,
    numbering: Option<String>,
    styles: Option<String>,
    rels: Option<String>,
}

/// Read document.xml, numbering.xml, and styles.xml from a .docx ZIP.
fn read_docx_parts(docx_path: &str) -> Result<DocxParts, String> {
    let file_bytes = fs::read(docx_path).map_err(|e| format!("Failed to read docx: {}", e))?;
    let cursor = Cursor::new(file_bytes);
    let mut archive =
        ZipArchive::new(cursor).map_err(|e| format!("Failed to open docx as zip: {}", e))?;

    let document = {
        let mut entry = archive
            .by_name("word/document.xml")
            .map_err(|e| format!("Failed to find document.xml: {}", e))?;
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .map_err(|e| format!("Failed to read document.xml: {}", e))?;
        content
    };

    let numbering = match archive.by_name("word/numbering.xml") {
        Ok(mut entry) => {
            let mut content = String::new();
            entry.read_to_string(&mut content).ok();
            Some(content)
        }
        Err(_) => None,
    };

    let styles = match archive.by_name("word/styles.xml") {
        Ok(mut entry) => {
            let mut content = String::new();
            entry.read_to_string(&mut content).ok();
            Some(content)
        }
        Err(_) => None,
    };

    let rels = match archive.by_name("word/_rels/document.xml.rels") {
        Ok(mut entry) => {
            let mut content = String::new();
            entry.read_to_string(&mut content).ok();
            Some(content)
        }
        Err(_) => None,
    };

    Ok(DocxParts {
        document,
        numbering,
        styles,
        rels,
    })
}

// ─── Numbering / list support ───────────────────────────────────────────

/// Represents a single level within an abstract numbering definition.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct NumberingLevel {
    /// The number format: "decimal", "lowerLetter", "upperLetter",
    /// "lowerRoman", "upperRoman", "bullet", "none", etc.
    num_fmt: String,
    /// The level text pattern, e.g. "%1.", "%1.%2.", etc.
    lvl_text: String,
    /// Left indent in twips (1/20 of a point, 1440 = 1 inch).
    indent_left_twips: Option<i32>,
    /// Hanging indent in twips.
    indent_hanging_twips: Option<i32>,
}

/// Map from (numId, ilvl) to NumberingLevel.
type NumberingMap = HashMap<(String, String), NumberingLevel>;

/// Counters for active numbering sequences, keyed by (numId, ilvl).
type NumberingCounters = HashMap<(String, String), i32>;

/// Parse word/numbering.xml to extract numbering definitions.
///
/// Word numbering has two layers:
/// - `<w:abstractNum>` defines the format for each level
/// - `<w:num>` maps a numId to an abstractNumId
///
/// We flatten these into a direct (numId, ilvl) -> NumberingLevel map.
fn parse_numbering_xml(xml: &str) -> NumberingMap {
    let mut abstract_levels: HashMap<(String, String), NumberingLevel> = HashMap::new();
    let mut num_to_abstract: HashMap<String, String> = HashMap::new();

    // State for parsing
    let mut in_abstract_num = false;
    let mut current_abstract_id = String::new();
    let mut in_lvl = false;
    let mut current_lvl_ilvl = String::new();
    let mut current_num_fmt = String::new();
    let mut current_lvl_text = String::new();
    let mut current_indent_left: Option<i32> = None;
    let mut current_indent_hanging: Option<i32> = None;
    let mut in_num = false;
    let mut current_num_id = String::new();
    let mut in_ppr = false;

    let reader = xml::reader::EventReader::from_str(xml);
    for event in reader {
        match event {
            Ok(xml::reader::XmlEvent::StartElement {
                name, attributes, ..
            }) => match name.local_name.as_str() {
                "abstractNum" => {
                    in_abstract_num = true;
                    current_abstract_id = attributes
                        .iter()
                        .find(|a| a.name.local_name == "abstractNumId")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                }
                "lvl" if in_abstract_num => {
                    in_lvl = true;
                    current_lvl_ilvl = attributes
                        .iter()
                        .find(|a| a.name.local_name == "ilvl")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                    current_num_fmt = String::new();
                    current_lvl_text = String::new();
                    current_indent_left = None;
                    current_indent_hanging = None;
                    in_ppr = false;
                }
                "numFmt" if in_lvl => {
                    current_num_fmt = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                }
                "lvlText" if in_lvl => {
                    current_lvl_text = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                }
                "pPr" if in_lvl => {
                    in_ppr = true;
                }
                "ind" if in_lvl && in_ppr => {
                    for attr in &attributes {
                        match attr.name.local_name.as_str() {
                            "left" => current_indent_left = attr.value.parse().ok(),
                            "hanging" => current_indent_hanging = attr.value.parse().ok(),
                            _ => {}
                        }
                    }
                }
                "num" if !in_abstract_num => {
                    in_num = true;
                    current_num_id = attributes
                        .iter()
                        .find(|a| a.name.local_name == "numId")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                }
                "abstractNumId" if in_num => {
                    let abs_id = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                    num_to_abstract.insert(current_num_id.clone(), abs_id);
                }
                _ => {}
            },
            Ok(xml::reader::XmlEvent::EndElement { name, .. }) => match name.local_name.as_str() {
                "abstractNum" => {
                    in_abstract_num = false;
                }
                "lvl" if in_abstract_num => {
                    abstract_levels.insert(
                        (current_abstract_id.clone(), current_lvl_ilvl.clone()),
                        NumberingLevel {
                            num_fmt: current_num_fmt.clone(),
                            lvl_text: current_lvl_text.clone(),
                            indent_left_twips: current_indent_left,
                            indent_hanging_twips: current_indent_hanging,
                        },
                    );
                    in_lvl = false;
                    in_ppr = false;
                }
                "pPr" if in_lvl => {
                    in_ppr = false;
                }
                "num" => {
                    in_num = false;
                }
                _ => {}
            },
            _ => {}
        }
    }

    // Flatten: map (numId, ilvl) -> NumberingLevel via the num->abstractNum indirection
    let mut result = NumberingMap::new();
    for (num_id, abstract_id) in &num_to_abstract {
        // Collect all levels for this abstract num
        for ((abs_id, ilvl), level) in &abstract_levels {
            if abs_id == abstract_id {
                result.insert((num_id.clone(), ilvl.clone()), level.clone());
            }
        }
    }

    result
}

/// Format a number counter for a given numFmt.
fn format_number(counter: i32, num_fmt: &str) -> String {
    match num_fmt {
        "decimal" => counter.to_string(),
        "lowerLetter" => {
            if (1..=26).contains(&counter) {
                ((b'a' + (counter - 1) as u8) as char).to_string()
            } else {
                counter.to_string()
            }
        }
        "upperLetter" => {
            if (1..=26).contains(&counter) {
                ((b'A' + (counter - 1) as u8) as char).to_string()
            } else {
                counter.to_string()
            }
        }
        "lowerRoman" => to_roman(counter).to_lowercase(),
        "upperRoman" => to_roman(counter),
        _ => counter.to_string(),
    }
}

/// Convert an integer to Roman numerals (uppercase).
fn to_roman(mut n: i32) -> String {
    if n <= 0 || n > 3999 {
        return n.to_string();
    }
    let table = [
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
    let mut result = String::new();
    for (value, symbol) in table {
        while n >= value {
            result.push_str(symbol);
            n -= value;
        }
    }
    result
}

/// Build the displayed list label from a numbering level and counters.
/// Replaces `%1`, `%2`, etc. in the lvl_text pattern with actual counter values.
fn build_list_label(level: &NumberingLevel, num_id: &str, counters: &NumberingCounters) -> String {
    let mut label = level.lvl_text.clone();
    // Replace %1 through %9 with the counter for the corresponding level
    for lvl_idx in 0..9 {
        let placeholder = format!("%{}", lvl_idx + 1);
        if label.contains(&placeholder) {
            let ilvl_str = lvl_idx.to_string();
            let counter = counters
                .get(&(num_id.to_string(), ilvl_str))
                .copied()
                .unwrap_or(1);
            let formatted = format_number(counter, &level.num_fmt);
            label = label.replace(&placeholder, &formatted);
        }
    }
    label
}

// ─── Style support ──────────────────────────────────────────────────────

/// Resolved properties from a style definition.
#[derive(Debug, Clone, Default)]
struct StyleProps {
    /// Paragraph alignment (start/center/right/both/distribute).
    alignment: Option<String>,
    /// Bold.
    bold: Option<bool>,
    /// Italic.
    italic: Option<bool>,
    /// Underline.
    underline: Option<bool>,
    /// Font size in half-points.
    font_size_half_pts: Option<i32>,
    /// All caps.
    all_caps: Option<bool>,
    /// Small caps.
    small_caps: Option<bool>,
    /// Font family (from rFonts ascii/hAnsi).
    font_family: Option<String>,
    /// Left indent in twips.
    indent_left_twips: Option<i32>,
    /// Right indent in twips.
    indent_right_twips: Option<i32>,
    /// First-line indent in twips (negative = hanging).
    indent_first_line_twips: Option<i32>,
    /// Spacing before in twips.
    spacing_before_twips: Option<i32>,
    /// Spacing after in twips.
    spacing_after_twips: Option<i32>,
    /// Line spacing in 240ths of a line.
    line_spacing_240ths: Option<i32>,
    /// Heading outline level (0-based: 0 = Heading 1, 1 = Heading 2, etc.).
    outline_level: Option<u8>,
    /// The parent style ID, if any.
    based_on: Option<String>,
}

/// Map from style ID (e.g., "Heading1") to its properties.
type StyleMap = HashMap<String, StyleProps>;

/// Map from relationship ID (e.g., "rId5") to target URL.
type RelationshipMap = HashMap<String, String>;

/// Parse word/_rels/document.xml.rels to extract hyperlink targets.
fn parse_rels_xml(xml: &str) -> RelationshipMap {
    let mut rels = RelationshipMap::new();
    let reader = xml::reader::EventReader::from_str(xml);
    for event in reader {
        if let Ok(xml::reader::XmlEvent::StartElement {
            name, attributes, ..
        }) = event
        {
            if name.local_name == "Relationship" {
                let id = attributes
                    .iter()
                    .find(|a| a.name.local_name == "Id")
                    .map(|a| a.value.clone());
                let target = attributes
                    .iter()
                    .find(|a| a.name.local_name == "Target")
                    .map(|a| a.value.clone());
                let rel_type = attributes
                    .iter()
                    .find(|a| a.name.local_name == "Type")
                    .map(|a| a.value.clone())
                    .unwrap_or_default();
                // Only include hyperlink relationships
                if rel_type.contains("hyperlink") {
                    if let (Some(id), Some(target)) = (id, target) {
                        rels.insert(id, target);
                    }
                }
            }
        }
    }
    rels
}

/// Parse word/styles.xml to extract style definitions.
fn parse_styles_xml(xml: &str) -> StyleMap {
    let mut styles = StyleMap::new();

    let mut in_style = false;
    let mut current_style_id = String::new();
    let mut current_props = StyleProps::default();
    let mut in_ppr = false;
    let mut in_rpr = false;

    let reader = xml::reader::EventReader::from_str(xml);
    for event in reader {
        match event {
            Ok(xml::reader::XmlEvent::StartElement {
                name, attributes, ..
            }) => match name.local_name.as_str() {
                "style" => {
                    in_style = true;
                    current_style_id = attributes
                        .iter()
                        .find(|a| a.name.local_name == "styleId")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                    current_props = StyleProps::default();
                }
                "basedOn" if in_style => {
                    current_props.based_on = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .map(|a| a.value.clone());
                }
                "pPr" if in_style => {
                    in_ppr = true;
                }
                "rPr" if in_style => {
                    in_rpr = true;
                }
                "jc" if in_ppr => {
                    current_props.alignment = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .map(|a| a.value.clone());
                }
                "ind" if in_ppr => {
                    for attr in &attributes {
                        match attr.name.local_name.as_str() {
                            "left" => {
                                current_props.indent_left_twips = attr.value.parse().ok();
                            }
                            "right" => {
                                current_props.indent_right_twips = attr.value.parse().ok();
                            }
                            "firstLine" => {
                                current_props.indent_first_line_twips = attr.value.parse().ok();
                            }
                            "hanging" => {
                                current_props.indent_first_line_twips =
                                    attr.value.parse::<i32>().ok().map(|v| -v);
                            }
                            _ => {}
                        }
                    }
                }
                "spacing" if in_ppr => {
                    for attr in &attributes {
                        match attr.name.local_name.as_str() {
                            "before" => {
                                current_props.spacing_before_twips = attr.value.parse().ok();
                            }
                            "after" => {
                                current_props.spacing_after_twips = attr.value.parse().ok();
                            }
                            "line" => {
                                current_props.line_spacing_240ths = attr.value.parse().ok();
                            }
                            _ => {}
                        }
                    }
                }
                "outlineLvl" if in_ppr => {
                    current_props.outline_level = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .and_then(|a| a.value.parse().ok());
                }
                "b" if in_rpr => {
                    let disabled = attributes.iter().any(|a| {
                        a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                    });
                    current_props.bold = Some(!disabled);
                }
                "i" if in_rpr => {
                    let disabled = attributes.iter().any(|a| {
                        a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                    });
                    current_props.italic = Some(!disabled);
                }
                "u" if in_rpr => {
                    let is_none = attributes
                        .iter()
                        .any(|a| a.name.local_name == "val" && a.value == "none");
                    current_props.underline = Some(!is_none);
                }
                "sz" if in_rpr => {
                    current_props.font_size_half_pts = attributes
                        .iter()
                        .find(|a| a.name.local_name == "val")
                        .and_then(|a| a.value.parse().ok());
                }
                "caps" if in_rpr => {
                    let disabled = attributes.iter().any(|a| {
                        a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                    });
                    current_props.all_caps = Some(!disabled);
                }
                "smallCaps" if in_rpr => {
                    let disabled = attributes.iter().any(|a| {
                        a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                    });
                    current_props.small_caps = Some(!disabled);
                }
                "rFonts" if in_rpr => {
                    // Prefer ascii, fall back to hAnsi, then cs
                    let font = attributes
                        .iter()
                        .find(|a| a.name.local_name == "ascii")
                        .or_else(|| attributes.iter().find(|a| a.name.local_name == "hAnsi"))
                        .or_else(|| attributes.iter().find(|a| a.name.local_name == "cs"))
                        .map(|a| a.value.clone());
                    if let Some(f) = font {
                        current_props.font_family = Some(f);
                    }
                }
                _ => {}
            },
            Ok(xml::reader::XmlEvent::EndElement { name, .. }) => match name.local_name.as_str() {
                "style" => {
                    if !current_style_id.is_empty() {
                        styles.insert(current_style_id.clone(), current_props.clone());
                    }
                    in_style = false;
                    in_ppr = false;
                    in_rpr = false;
                }
                "pPr" if in_style => {
                    in_ppr = false;
                }
                "rPr" if in_style => {
                    in_rpr = false;
                }
                _ => {}
            },
            _ => {}
        }
    }

    styles
}

/// Resolve a style by following the basedOn chain. Later (more specific)
/// properties override earlier (parent) ones.
fn resolve_style(style_id: &str, style_map: &StyleMap) -> StyleProps {
    let mut chain = Vec::new();
    let mut current = Some(style_id.to_string());
    let mut visited = std::collections::HashSet::new();

    while let Some(id) = current {
        if visited.contains(&id) {
            break; // Prevent infinite loops
        }
        visited.insert(id.clone());
        if let Some(props) = style_map.get(&id) {
            chain.push(props.clone());
            current = props.based_on.clone();
        } else {
            break;
        }
    }

    // Merge from parent to child (chain is child-first, so reverse)
    chain.reverse();
    let mut result = StyleProps::default();
    for props in chain {
        if props.alignment.is_some() {
            result.alignment = props.alignment;
        }
        if props.bold.is_some() {
            result.bold = props.bold;
        }
        if props.italic.is_some() {
            result.italic = props.italic;
        }
        if props.underline.is_some() {
            result.underline = props.underline;
        }
        if props.font_size_half_pts.is_some() {
            result.font_size_half_pts = props.font_size_half_pts;
        }
        if props.all_caps.is_some() {
            result.all_caps = props.all_caps;
        }
        if props.small_caps.is_some() {
            result.small_caps = props.small_caps;
        }
        if props.indent_left_twips.is_some() {
            result.indent_left_twips = props.indent_left_twips;
        }
        if props.indent_right_twips.is_some() {
            result.indent_right_twips = props.indent_right_twips;
        }
        if props.indent_first_line_twips.is_some() {
            result.indent_first_line_twips = props.indent_first_line_twips;
        }
        if props.spacing_before_twips.is_some() {
            result.spacing_before_twips = props.spacing_before_twips;
        }
        if props.spacing_after_twips.is_some() {
            result.spacing_after_twips = props.spacing_after_twips;
        }
        if props.line_spacing_240ths.is_some() {
            result.line_spacing_240ths = props.line_spacing_240ths;
        }
        if props.font_family.is_some() {
            result.font_family = props.font_family;
        }
        if props.outline_level.is_some() {
            result.outline_level = props.outline_level;
        }
    }
    result
}

/// Convert twips to a CSS pt value (1 twip = 1/20 pt).
fn twips_to_pt(twips: i32) -> f64 {
    twips as f64 / 20.0
}

/// Known contact property keys that can appear in `{Role.property}` syntax.
const CONTACT_PROPERTIES: &[&str] = &[
    "full_name",
    "first_name",
    "last_name",
    "relationship",
    "phone",
    "email",
    "address",
    "city",
    "state",
    "zip",
];

/// Try to parse `"Role.property"` contact-role dot notation.
/// Returns `Some((role, property))` if the text contains a dot followed by a
/// known contact property key (case-insensitive).
fn parse_contact_role_ref(text: &str) -> Option<(String, String)> {
    let dot_pos = text.rfind('.')?;
    let role = text[..dot_pos].trim();
    let property = text[dot_pos + 1..].trim();
    if role.is_empty() || property.is_empty() {
        return None;
    }
    let prop_lower = property.to_lowercase();
    if CONTACT_PROPERTIES.contains(&prop_lower.as_str()) {
        Some((role.to_string(), prop_lower))
    } else {
        None
    }
}

/// Convert a contact property key like `"full_name"` to title-case words: `"Full Name"`.
fn property_to_title(property: &str) -> String {
    property
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    format!("{}{}", upper, chars.collect::<String>())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert `"Role.property"` dot notation to a flat canonical variable name.
/// e.g., `"Healthcare POA Agent.full_name"` → `"Healthcare POA Agent Full Name"`.
fn contact_role_to_flat_name(role: &str, property: &str) -> String {
    format!("{} {}", role, property_to_title(property))
}

/// Consume characters after an opening `{` from a peekable char iterator,
/// tracking brace depth to handle nested `{...}` blocks (e.g., replacement
/// variables inside conditional text branches). Returns the full content
/// between the outermost braces, or `None` if the closing `}` is never found.
///
/// After a successful call the iterator is positioned just past the closing `}`.
fn scan_brace_content(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let mut depth: u32 = 1;
    let mut content = String::new();
    for c in chars.by_ref() {
        if c == '{' {
            depth += 1;
            content.push(c);
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(content);
            }
            content.push(c);
        } else {
            content.push(c);
        }
    }
    None // Unbalanced — never found the outermost closing brace
}

/// Check whether a variable's raw content (between `{}`) is a conditional
/// variable. Conditional variables contain `??` as the ternary operator.
fn is_conditional_variable(raw_content: &str) -> bool {
    raw_content.contains("??")
}

/// Normalize smart / curly quotes to plain ASCII double quotes.
///
/// Word's "AutoFormat as you type" automatically converts straight quotes
/// (`"`) to left/right curly quotes (\u{201C} / \u{201D}).  This helper
/// ensures the conditional parser accepts both forms.
fn normalize_quotes(s: &str) -> String {
    s.replace(['\u{201C}', '\u{201D}'], "\"")
        .replace(['\u{2018}', '\u{2019}'], "'")
}

/// Find the first unescaped double-quote in `s`.
/// A quote preceded by a backslash (`\"`) is treated as escaped and skipped.
fn find_unescaped_quote(s: &str) -> Option<usize> {
    let mut chars = s.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '\\' {
            chars.next(); // skip escaped character
        } else if ch == '"' {
            return Some(i);
        }
    }
    None
}

/// Parse a conditional variable's raw content into (label, true_text, false_text).
///
/// Expected syntax:
///   `Client Is Single ?? "single text" :: "couple text"`
///   → `("Client Is Single", "single text", "couple text")`
///
/// Both branch texts must be wrapped in double quotes (straight or smart).
/// Content inside the quotes is preserved exactly (including whitespace).
/// The `::` separator between true and false branches is required.
fn parse_conditional_variable(raw_content: &str) -> Option<(String, String, String)> {
    // Normalize curly/smart quotes to straight quotes so the parser works
    // regardless of whether the user typed in Word (which auto-converts).
    let raw_content = normalize_quotes(raw_content);
    let (label, rest) = raw_content.split_once("??")?;
    let label = label.trim().to_string();
    if label.is_empty() {
        return None;
    }

    let rest = rest.trim();

    if !rest.starts_with('"') {
        return None;
    }

    // Find the closing quote for the true-text (skip escaped quotes)
    let after_open = &rest[1..];
    let close_idx = find_unescaped_quote(after_open)?;
    let true_text = after_open[..close_idx].replace("\\\"", "\"");

    // After the closing quote, look for ::
    let remainder = after_open[close_idx + 1..].trim();
    let false_text = if let Some(after_sep) = remainder.strip_prefix("::") {
        let after_sep = after_sep.trim();
        if let Some(inner) = after_sep.strip_prefix('"') {
            if let Some(end) = find_unescaped_quote(inner) {
                inner[..end].replace("\\\"", "\"")
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    Some((label, true_text, false_text))
}

const SDT_TAG_PREFIX: &str = "lily:";
const COND_SDT_TAG_PREFIX: &str = "lily-cond:";
const BOOKMARK_PREFIX: &str = "lily:";
const COND_BOOKMARK_PREFIX: &str = "lily-cond:";

/// Convert Word XML to an HTML preview with high-fidelity rendering.
/// Preserves paragraph structure, formatting, indentation, alignment,
/// tables, lists, tabs, line breaks, and highlights variables.
///
/// Handles two kinds of variables:
/// 1. Fresh `{Placeholder}` text — wrapped in highlight spans with
///    `data-variable` (lowercase canonical) and `data-original-case`.
/// 2. Lily SDT content controls — the text inside `<w:sdtContent>` is wrapped
///    in a highlight span using the `lily:` tag value as the variable name.
fn xml_to_preview_html(xml: &str, numbering_map: &NumberingMap, style_map: &StyleMap, rels_map: &RelationshipMap) -> String {
    // Extract page margins from <w:sectPr><w:pgMar> for the preview wrapper
    let margin_re: &Regex = &PG_MARGIN_RE;
    let page_margins = margin_re.captures(xml).map(|caps| {
        let top = caps[1].parse::<i32>().unwrap_or(1440);
        let right = caps[2].parse::<i32>().unwrap_or(1440);
        let bottom = caps[3].parse::<i32>().unwrap_or(1440);
        let left = caps[4].parse::<i32>().unwrap_or(1440);
        format!(
            "padding:{:.1}pt {:.1}pt {:.1}pt {:.1}pt",
            twips_to_pt(top), twips_to_pt(right),
            twips_to_pt(bottom), twips_to_pt(left)
        )
    });

    let mut html = if let Some(margins) = page_margins {
        format!("<div class=\"document-preview\" style=\"{}\">", margins)
    } else {
        String::from("<div class=\"document-preview\">")
    };
    let mut current_para = String::new();

    // Hyperlink state
    let mut in_hyperlink = false;
    let mut hyperlink_url: Option<String> = None;

    // Run-level formatting state
    let mut in_t = false;
    let mut in_bold = false;
    let mut in_italic = false;
    let mut in_underline = false;
    let mut in_strikethrough = false;
    let mut in_superscript = false;
    let mut in_subscript = false;
    let mut in_all_caps = false;
    let mut in_small_caps = false;
    let mut font_size_half_pts: Option<i32> = None;
    let mut font_color: Option<String> = None;
    let mut highlight_color: Option<String> = None;
    let mut font_family: Option<String> = None;

    // Run properties parsing state
    let mut in_rpr = false;
    let mut pending_bold = false;
    let mut pending_italic = false;
    let mut pending_underline = false;
    let mut pending_strikethrough = false;
    let mut pending_superscript = false;
    let mut pending_subscript = false;
    let mut pending_all_caps = false;
    let mut pending_small_caps = false;
    let mut pending_font_size: Option<i32> = None;
    let mut pending_font_color: Option<String> = None;
    let mut pending_highlight_color: Option<String> = None;
    let mut pending_font_family: Option<String> = None;

    // Paragraph properties
    let mut in_ppr = false;
    let mut para_alignment: Option<String> = None;
    let mut para_indent_left_twips: Option<i32> = None;
    let mut para_indent_right_twips: Option<i32> = None;
    let mut para_indent_first_line_twips: Option<i32> = None;
    let mut para_spacing_before: Option<i32> = None;
    let mut para_spacing_after: Option<i32> = None;
    let mut para_line_spacing: Option<i32> = None;
    let mut para_style_id: Option<String> = None;
    let mut current_para_style = StyleProps::default();
    let mut para_page_break_before = false;
    // Paragraph-level run defaults (from <w:pPr><w:rPr>) — these override
    // the style and serve as defaults for runs that don't specify their own.
    let mut para_rpr_bold: Option<bool> = None;
    let mut para_rpr_italic: Option<bool> = None;
    let mut para_rpr_underline: Option<bool> = None;
    let mut para_rpr_font_size: Option<i32> = None;
    let mut para_rpr_font_family: Option<String> = None;
    let mut para_rpr_all_caps: Option<bool> = None;
    let mut para_rpr_small_caps: Option<bool> = None;
    let mut in_para_rpr = false;
    let mut para_num_id: Option<String> = None;
    let mut para_ilvl: Option<String> = None;

    // SDT (structured document tag) state
    let mut in_sdt = false;
    let mut in_sdt_pr = false;
    let mut sdt_var_name: Option<String> = None;
    let mut sdt_id: Option<String> = None;
    let mut in_sdt_content = false;

    // Paragraph index counter for position-aware HTML output
    let mut para_idx: usize = 0;

    // Table state
    let mut in_table = false;
    let mut in_tr = false;
    let mut in_tc = false;
    let mut tc_paras: Vec<String> = Vec::new();

    // Numbering counters — tracks the current count for each (numId, ilvl)
    let mut num_counters: NumberingCounters = HashMap::new();
    // Track the last numId we saw, to reset counters when sequences change
    let mut last_num_id: Option<String> = None;

    // Whether we're inside a pPr's numPr element
    let mut in_num_pr = false;

    let reader = xml::reader::EventReader::from_str(xml);
    for event in reader {
        match event {
            Ok(xml::reader::XmlEvent::StartElement {
                name, attributes, ..
            }) => {
                match name.local_name.as_str() {
                    // ─── Table elements ──────────────────────────────
                    "tbl" => {
                        in_table = true;
                        html.push_str("<table class=\"preview-table\">");
                    }
                    "tr" if in_table => {
                        in_tr = true;
                        html.push_str("<tr>");
                    }
                    "tc" if in_tr => {
                        in_tc = true;
                        tc_paras.clear();
                    }
                    // ─── Hyperlink ─────────────────────────────────────
                    "hyperlink" => {
                        in_hyperlink = true;
                        hyperlink_url = attributes
                            .iter()
                            .find(|a| a.name.local_name == "id")
                            .and_then(|a| rels_map.get(&a.value).cloned());
                        if let Some(ref url) = hyperlink_url {
                            current_para.push_str(&format!(
                                "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\" class=\"link link-primary\">",
                                escape_html(url)
                            ));
                        }
                    }
                    // ─── Paragraph ───────────────────────────────────
                    "p" => {
                        current_para.clear();
                        in_bold = false;
                        in_italic = false;
                        in_underline = false;
                        in_strikethrough = false;
                        in_superscript = false;
                        in_subscript = false;
                        in_all_caps = false;
                        in_small_caps = false;
                        font_size_half_pts = None;
                        font_color = None;
                        highlight_color = None;
                        // Reset paragraph properties
                        para_alignment = None;
                        para_indent_left_twips = None;
                        para_indent_right_twips = None;
                        para_indent_first_line_twips = None;
                        para_spacing_before = None;
                        para_spacing_after = None;
                        para_line_spacing = None;
                        para_style_id = None;
                        current_para_style = StyleProps::default();
                        para_rpr_bold = None;
                        para_rpr_italic = None;
                        para_rpr_underline = None;
                        para_rpr_font_size = None;
                        para_rpr_font_family = None;
                        para_rpr_all_caps = None;
                        para_rpr_small_caps = None;
                        para_num_id = None;
                        para_ilvl = None;
                        para_page_break_before = false;
                    }
                    // ─── Paragraph properties ────────────────────────
                    "pPr" => {
                        in_ppr = true;
                    }
                    "pStyle" if in_ppr => {
                        para_style_id = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .map(|a| a.value.clone());
                        // Resolve the style immediately so paragraph-level
                        // rPr (which comes later inside pPr) can inherit
                        // style properties like bold and italic.
                        if let Some(ref id) = para_style_id {
                            current_para_style = resolve_style(id, style_map);
                        }
                    }
                    "jc" if in_ppr && !in_num_pr => {
                        para_alignment = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .map(|a| a.value.clone());
                    }
                    "pageBreakBefore" if in_ppr => {
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        para_page_break_before = !disabled;
                    }
                    "ind" if in_ppr && !in_num_pr => {
                        for attr in &attributes {
                            match attr.name.local_name.as_str() {
                                "left" | "start" => {
                                    para_indent_left_twips = attr.value.parse().ok();
                                }
                                "right" | "end" => {
                                    para_indent_right_twips = attr.value.parse().ok();
                                }
                                "firstLine" => {
                                    para_indent_first_line_twips = attr.value.parse().ok();
                                }
                                "hanging" => {
                                    para_indent_first_line_twips =
                                        attr.value.parse::<i32>().ok().map(|v| -v);
                                }
                                _ => {}
                            }
                        }
                    }
                    "spacing" if in_ppr => {
                        for attr in &attributes {
                            match attr.name.local_name.as_str() {
                                "before" => {
                                    para_spacing_before = attr.value.parse().ok();
                                }
                                "after" => {
                                    para_spacing_after = attr.value.parse().ok();
                                }
                                "line" => {
                                    para_line_spacing = attr.value.parse().ok();
                                }
                                _ => {}
                            }
                        }
                    }
                    "numPr" if in_ppr => {
                        in_num_pr = true;
                    }
                    "ilvl" if in_num_pr => {
                        para_ilvl = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .map(|a| a.value.clone());
                    }
                    "numId" if in_num_pr => {
                        para_num_id = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .map(|a| a.value.clone());
                    }
                    // ─── Lily bookmarks (empty variable placeholders) ─
                    "bookmarkStart" => {
                        for attr in &attributes {
                            if attr.name.local_name == "name" {
                                let var_name = attr.value.strip_prefix(COND_BOOKMARK_PREFIX)
                                    .or_else(|| attr.value.strip_prefix(BOOKMARK_PREFIX));
                                if let Some(var_name) = var_name {
                                    let canonical = var_name.to_lowercase();
                                    current_para.push_str(&format!(
                                        "<span class=\"variable-bookmark\" data-variable=\"{}\" data-original-case=\"{}\"></span>",
                                        escape_html(&canonical), escape_html(var_name)
                                    ));
                                }
                            }
                        }
                    }
                    // ─── SDT (structured document tag) ───────────────
                    "sdt" => {
                        in_sdt = true;
                        sdt_var_name = None;
                        sdt_id = None;
                    }
                    "sdtPr" if in_sdt => {
                        in_sdt_pr = true;
                    }
                    "id" if in_sdt_pr => {
                        for attr in &attributes {
                            if attr.name.local_name == "val" {
                                sdt_id = Some(attr.value.clone());
                            }
                        }
                    }
                    "tag" if in_sdt_pr => {
                        for attr in &attributes {
                            if attr.name.local_name == "val" {
                                if let Some(dn) = attr.value.strip_prefix(COND_SDT_TAG_PREFIX) {
                                    sdt_var_name = Some(dn.to_string());
                                } else if let Some(dn) = attr.value.strip_prefix(SDT_TAG_PREFIX) {
                                    sdt_var_name = Some(dn.to_string());
                                }
                            }
                        }
                    }
                    "sdtContent" if in_sdt => {
                        in_sdt_content = true;
                    }
                    // ─── Run start — reset to effective paragraph defaults ─
                    "r" if !in_rpr => {
                        // Cascade: style → paragraph rPr. If the run has its
                        // own <w:rPr>, it will override these immediately.
                        in_bold = para_rpr_bold
                            .or(current_para_style.bold)
                            .unwrap_or(false);
                        in_italic = para_rpr_italic
                            .or(current_para_style.italic)
                            .unwrap_or(false);
                        in_underline = para_rpr_underline
                            .or(current_para_style.underline)
                            .unwrap_or(false);
                        in_strikethrough = false;
                        in_superscript = false;
                        in_subscript = false;
                        in_all_caps = para_rpr_all_caps
                            .or(current_para_style.all_caps)
                            .unwrap_or(false);
                        in_small_caps = para_rpr_small_caps
                            .or(current_para_style.small_caps)
                            .unwrap_or(false);
                        font_size_half_pts = para_rpr_font_size
                            .or(current_para_style.font_size_half_pts);
                        font_color = None;
                        highlight_color = None;
                        font_family = para_rpr_font_family.clone()
                            .or(current_para_style.font_family.clone());
                    }
                    // ─── Run properties ──────────────────────────────
                    "rPr" => {
                        in_rpr = true;
                        if in_ppr {
                            // This is <w:pPr><w:rPr> — paragraph-level run
                            // defaults. Capture them separately; they override
                            // style and serve as defaults for runs.
                            in_para_rpr = true;
                        }
                        // Start with the effective paragraph defaults:
                        // style → paragraph rPr override → (then run overrides)
                        pending_bold = para_rpr_bold
                            .or(current_para_style.bold)
                            .unwrap_or(false);
                        pending_italic = para_rpr_italic
                            .or(current_para_style.italic)
                            .unwrap_or(false);
                        pending_underline = para_rpr_underline
                            .or(current_para_style.underline)
                            .unwrap_or(false);
                        pending_strikethrough = false;
                        pending_superscript = false;
                        pending_subscript = false;
                        pending_all_caps = para_rpr_all_caps
                            .or(current_para_style.all_caps)
                            .unwrap_or(false);
                        pending_small_caps = para_rpr_small_caps
                            .or(current_para_style.small_caps)
                            .unwrap_or(false);
                        pending_font_size = para_rpr_font_size
                            .or(current_para_style.font_size_half_pts);
                        pending_font_color = None;
                        pending_highlight_color = None;
                        pending_font_family = para_rpr_font_family.clone()
                            .or(current_para_style.font_family.clone());
                    }
                    "b" if in_rpr => {
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        pending_bold = !disabled;
                    }
                    "i" if in_rpr => {
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        pending_italic = !disabled;
                    }
                    "u" if in_rpr => {
                        let is_none = attributes
                            .iter()
                            .any(|a| a.name.local_name == "val" && a.value == "none");
                        pending_underline = !is_none;
                    }
                    "strike" if in_rpr => {
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        pending_strikethrough = !disabled;
                    }
                    "dstrike" if in_rpr => {
                        // Treat double-strikethrough as regular strikethrough for preview
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        pending_strikethrough = !disabled;
                    }
                    "vertAlign" if in_rpr => {
                        let val = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .map(|a| a.value.as_str());
                        match val {
                            Some("superscript") => {
                                pending_superscript = true;
                                pending_subscript = false;
                            }
                            Some("subscript") => {
                                pending_subscript = true;
                                pending_superscript = false;
                            }
                            _ => {
                                pending_superscript = false;
                                pending_subscript = false;
                            }
                        }
                    }
                    "caps" if in_rpr => {
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        pending_all_caps = !disabled;
                    }
                    "smallCaps" if in_rpr => {
                        let disabled = attributes.iter().any(|a| {
                            a.name.local_name == "val" && (a.value == "false" || a.value == "0")
                        });
                        pending_small_caps = !disabled;
                    }
                    "sz" if in_rpr => {
                        pending_font_size = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .and_then(|a| a.value.parse().ok());
                    }
                    "color" if in_rpr => {
                        pending_font_color = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .filter(|a| a.value != "auto")
                            .map(|a| a.value.clone());
                    }
                    "highlight" if in_rpr => {
                        pending_highlight_color = attributes
                            .iter()
                            .find(|a| a.name.local_name == "val")
                            .filter(|a| a.value != "none")
                            .map(|a| word_highlight_to_css(&a.value));
                    }
                    "rFonts" if in_rpr => {
                        // Prefer ascii font, fall back to hAnsi, then cs
                        pending_font_family = attributes
                            .iter()
                            .find(|a| a.name.local_name == "ascii")
                            .or_else(|| attributes.iter().find(|a| a.name.local_name == "hAnsi"))
                            .or_else(|| attributes.iter().find(|a| a.name.local_name == "cs"))
                            .map(|a| a.value.clone());
                    }
                    // ─── Run start ───────────────────────────────────
                    "r" => {
                        // Reset formatting for new run (will be set by rPr if present)
                    }
                    // ─── Text content ────────────────────────────────
                    "t" => {
                        in_t = true;
                    }
                    // ─── Tab character ───────────────────────────────
                    "tab" => {
                        current_para.push_str("<span class=\"preview-tab\"></span>");
                    }
                    // ─── Line break ──────────────────────────────────
                    "br" => {
                        let br_type = attributes
                            .iter()
                            .find(|a| a.name.local_name == "type")
                            .map(|a| a.value.as_str());
                        match br_type {
                            Some("page") => {
                                current_para.push_str("<span class=\"preview-page-break\"></span>");
                            }
                            _ => {
                                current_para.push_str("<br>");
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(xml::reader::XmlEvent::EndElement { name, .. }) => match name.local_name.as_str() {
                // ─── Hyperlink end ──────────────────────────────
                "hyperlink" => {
                    if in_hyperlink && hyperlink_url.is_some() {
                        current_para.push_str("</a>");
                    }
                    in_hyperlink = false;
                    hyperlink_url = None;
                }
                // ─── Table end elements ──────────────────────────
                "tbl" => {
                    in_table = false;
                    html.push_str("</table>");
                }
                "tr" => {
                    in_tr = false;
                    html.push_str("</tr>");
                }
                "tc" => {
                    // Emit the cell with all its paragraphs
                    html.push_str("<td>");
                    for (i, para) in tc_paras.iter().enumerate() {
                        if i > 0 {
                            html.push_str("<br>");
                        }
                        html.push_str(para);
                    }
                    html.push_str("</td>");
                    in_tc = false;
                }
                // ─── SDT end elements ────────────────────────────
                "sdt" => {
                    in_sdt = false;
                    in_sdt_content = false;
                    sdt_var_name = None;
                }
                "sdtPr" => {
                    in_sdt_pr = false;
                }
                "sdtContent" => {
                    in_sdt_content = false;
                }
                // ─── Run properties end ──────────────────────────
                "rPr" => {
                    in_rpr = false;
                    if in_para_rpr {
                        // Store paragraph-level run defaults
                        para_rpr_bold = Some(pending_bold);
                        para_rpr_italic = Some(pending_italic);
                        para_rpr_underline = Some(pending_underline);
                        para_rpr_font_size = pending_font_size;
                        para_rpr_font_family = pending_font_family.clone();
                        para_rpr_all_caps = Some(pending_all_caps);
                        para_rpr_small_caps = Some(pending_small_caps);
                        in_para_rpr = false;
                    } else {
                        // Apply to current run
                        in_bold = pending_bold;
                        in_italic = pending_italic;
                        in_underline = pending_underline;
                        in_strikethrough = pending_strikethrough;
                        in_superscript = pending_superscript;
                        in_subscript = pending_subscript;
                        in_all_caps = pending_all_caps;
                        in_small_caps = pending_small_caps;
                        font_size_half_pts = pending_font_size;
                        font_color = pending_font_color.clone();
                        highlight_color = pending_highlight_color.clone();
                        font_family = pending_font_family.clone();
                    }
                }
                "t" => {
                    in_t = false;
                }
                "pPr" => {
                    in_ppr = false;
                    in_num_pr = false;
                    // Resolve style now so run formatting can inherit
                    // style-level bold/italic/font when the run doesn't
                    // specify its own.
                    current_para_style = para_style_id
                        .as_deref()
                        .map(|id| resolve_style(id, style_map))
                        .unwrap_or_default();
                }
                "numPr" => {
                    in_num_pr = false;
                }
                // ─── Paragraph end ───────────────────────────────
                "p" => {
                    // Resolve style-inherited properties
                    let style_props = para_style_id
                        .as_deref()
                        .map(|id| resolve_style(id, style_map))
                        .unwrap_or_default();

                    // Merge: explicit paragraph properties override style-inherited ones
                    let alignment = para_alignment
                        .as_deref()
                        .or(style_props.alignment.as_deref());
                    let indent_left = para_indent_left_twips.or(style_props.indent_left_twips);
                    let indent_right = para_indent_right_twips.or(style_props.indent_right_twips);
                    let indent_first =
                        para_indent_first_line_twips.or(style_props.indent_first_line_twips);
                    let spacing_before = para_spacing_before.or(style_props.spacing_before_twips);
                    let spacing_after = para_spacing_after.or(style_props.spacing_after_twips);
                    let line_spacing = para_line_spacing.or(style_props.line_spacing_240ths);

                    // Handle list numbering
                    let list_label = if let (Some(ref num_id), Some(ref ilvl)) =
                        (&para_num_id, &para_ilvl)
                    {
                        // numId "0" means "no list" (Word uses it to explicitly remove numbering)
                        if num_id != "0" {
                            if let Some(level) = numbering_map.get(&(num_id.clone(), ilvl.clone()))
                            {
                                // Reset counters for deeper levels when a shallower
                                // level is encountered, and when the numbering
                                // sequence changes
                                if last_num_id.as_ref() != Some(num_id) {
                                    // New numbering sequence
                                    last_num_id = Some(num_id.clone());
                                }

                                let ilvl_int: i32 = ilvl.parse().unwrap_or(0);

                                // Reset deeper level counters
                                for deeper in (ilvl_int + 1)..9 {
                                    num_counters.remove(&(num_id.clone(), deeper.to_string()));
                                }

                                // Increment counter for this level
                                let counter = num_counters
                                    .entry((num_id.clone(), ilvl.clone()))
                                    .or_insert(0);
                                *counter += 1;

                                if level.num_fmt == "bullet" {
                                    Some("\u{2022}".to_string())
                                } else if level.num_fmt == "none" {
                                    None
                                } else {
                                    Some(build_list_label(level, num_id, &num_counters))
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Build inline style
                    let mut styles = Vec::new();

                    // Indentation — for list items, use the numbering definition's
                    // indent if the paragraph doesn't specify its own
                    let effective_indent_left = if indent_left.is_some() {
                        indent_left
                    } else if let (Some(ref num_id), Some(ref ilvl)) = (&para_num_id, &para_ilvl) {
                        numbering_map
                            .get(&(num_id.clone(), ilvl.clone()))
                            .and_then(|l| l.indent_left_twips)
                    } else {
                        None
                    };

                    if let Some(left) = effective_indent_left {
                        if left != 0 {
                            styles.push(format!("padding-left:{:.1}pt", twips_to_pt(left)));
                        }
                    }
                    if let Some(right) = indent_right {
                        if right != 0 {
                            styles.push(format!("padding-right:{:.1}pt", twips_to_pt(right)));
                        }
                    }
                    if let Some(first) = indent_first {
                        if first != 0 {
                            styles.push(format!("text-indent:{:.1}pt", twips_to_pt(first)));
                        }
                    }

                    // Alignment
                    match alignment {
                        Some("center") => styles.push("text-align:center".to_string()),
                        Some("right") | Some("end") => {
                            styles.push("text-align:right".to_string());
                        }
                        Some("both") | Some("distribute") => {
                            styles.push("text-align:justify".to_string());
                        }
                        _ => {}
                    }

                    // Spacing
                    if let Some(before) = spacing_before {
                        if before != 0 {
                            styles.push(format!("margin-top:{:.1}pt", twips_to_pt(before)));
                        }
                    }
                    if let Some(after) = spacing_after {
                        // Use the explicit value even if 0, to override prose defaults
                        styles.push(format!("margin-bottom:{:.1}pt", twips_to_pt(after)));
                    }
                    if let Some(line) = line_spacing {
                        // Word line spacing is in 240ths of a line (240 = single, 480 = double)
                        let ratio = line as f64 / 240.0;
                        if (ratio - 1.0).abs() > 0.05 {
                            styles.push(format!("line-height:{:.2}", ratio));
                        }
                    }

                    // Font size from style (paragraph-level; runs override individually)
                    let style_font_size = style_props.font_size_half_pts;

                    // Build the opening tag
                    let style_attr = if styles.is_empty() {
                        String::new()
                    } else {
                        format!(" style=\"{}\"", styles.join(";"))
                    };

                    // Determine the HTML tag — use heading elements for heading styles
                    let heading_level = para_style_id
                        .as_deref()
                        .map(|id| detect_heading_level(id, style_props.outline_level))
                        .unwrap_or(None);

                    let tag = match heading_level {
                        Some(1) => "h1",
                        Some(2) => "h2",
                        Some(3) => "h3",
                        Some(4) => "h4",
                        Some(5) => "h5",
                        Some(6) => "h6",
                        _ => "p",
                    };

                    // Emit page break before this paragraph if requested
                    if para_page_break_before {
                        html.push_str("<span class=\"preview-page-break\"></span>");
                    }

                    // Prepend list label if this is a list item
                    let mut para_content = String::new();
                    if let Some(ref label) = list_label {
                        para_content.push_str(&format!(
                            "<span class=\"preview-list-label\">{}\u{00a0}</span>",
                            escape_html(label)
                        ));
                    }

                    // For paragraph-level font size we wrap content if needed
                    if let Some(sz) = style_font_size {
                        let pt = sz as f64 / 2.0;
                        // Only add font-size if it differs significantly from the
                        // default prose-sm size (~14px ≈ 10.5pt)
                        if !(9.0..=13.0).contains(&pt) {
                            para_content = format!(
                                "<span style=\"font-size:{:.1}pt\">{}</span>",
                                pt, current_para
                            );
                        } else {
                            para_content.push_str(&current_para);
                        }
                    } else {
                        para_content.push_str(&current_para);
                    }

                    let para_idx_attr = format!(" data-para-idx=\"{}\"", para_idx);
                    para_idx += 1;

                    if in_tc {
                        // Inside a table cell: collect paragraphs for later emission
                        if para_content.is_empty() {
                            tc_paras.push("&nbsp;".to_string());
                        } else {
                            tc_paras.push(para_content);
                        }
                    } else if para_content.is_empty() && list_label.is_none() {
                        html.push_str(&format!("<{}{}{}>", tag, para_idx_attr, style_attr));
                        html.push_str("&nbsp;");
                        html.push_str(&format!("</{}>", tag));
                    } else {
                        html.push_str(&format!("<{}{}{}>", tag, para_idx_attr, style_attr));
                        html.push_str(&para_content);
                        html.push_str(&format!("</{}>", tag));
                    }
                }
                _ => {}
            },
            Ok(xml::reader::XmlEvent::Characters(text)) => {
                if in_t {
                    // If we're inside a Lily SDT content, wrap in a variable span.
                    // The span only gets data-variable and data-original-case;
                    // conditional logic is handled by the frontend using
                    // definitions from the .lily file.
                    let highlighted = if in_sdt_content {
                        let escaped = escape_html(&text);
                        if let Some(ref var_name) = sdt_var_name {
                            let canonical = var_name.to_lowercase();
                            let id_attr = match &sdt_id {
                                Some(id) => format!(" data-sdt-id=\"{}\"", escape_html(id)),
                                None => String::new(),
                            };
                            format!(
                                "<span class=\"variable-highlight filled\" data-variable=\"{}\" data-original-case=\"{}\"{}>{}</span>",
                                escape_html(&canonical), escape_html(var_name), id_attr, escaped
                            )
                        } else {
                            escaped
                        }
                    } else {
                        // Pass raw text — highlight_variables handles HTML
                        // escaping internally so that variable delimiters
                        // (e.g. `"` in conditionals) are parsed before encoding.
                        highlight_variables(&text)
                    };

                    // Apply run-level formatting
                    let mut styled = highlighted;

                    // Apply text transforms (caps/smallcaps) before wrapping in tags
                    if in_all_caps {
                        // All caps: uppercase the text content, but leave HTML tags alone
                        styled = uppercase_text_content(&styled);
                    } else if in_small_caps {
                        styled =
                            format!("<span style=\"font-variant:small-caps\">{}</span>", styled);
                    }

                    if in_bold {
                        styled = format!("<strong>{}</strong>", styled);
                    }
                    if in_italic {
                        styled = format!("<em>{}</em>", styled);
                    }
                    if in_underline {
                        styled = format!("<u>{}</u>", styled);
                    }
                    if in_strikethrough {
                        styled = format!("<s>{}</s>", styled);
                    }
                    if in_superscript {
                        styled = format!("<sup>{}</sup>", styled);
                    }
                    if in_subscript {
                        styled = format!("<sub>{}</sub>", styled);
                    }

                    // Build inline style for font size, color, highlight, font family
                    let mut run_styles = Vec::new();
                    if let Some(ref ff) = font_family {
                        run_styles.push(format!("font-family:'{}'", ff));
                    }
                    if let Some(sz) = font_size_half_pts {
                        let pt = sz as f64 / 2.0;
                        run_styles.push(format!("font-size:{:.1}pt", pt));
                    }
                    if let Some(ref color) = font_color {
                        run_styles.push(format!("color:#{}", color));
                    }
                    if let Some(ref bg) = highlight_color {
                        run_styles.push(format!("background-color:{}", bg));
                    }
                    if !run_styles.is_empty() {
                        styled =
                            format!("<span style=\"{}\">{}</span>", run_styles.join(";"), styled);
                    }

                    current_para.push_str(&styled);
                }
            }
            _ => {}
        }
    }

    html.push_str("</div>");
    html
}

/// Detect heading level from a Word style ID.
/// Returns Some(1..6) for heading styles, None otherwise.
fn detect_heading_level(style_id: &str, outline_level: Option<u8>) -> Option<u8> {
    // Check outlineLvl from the resolved style (most reliable — works even
    // when styleId is numeric like "951" instead of "Heading2")
    if let Some(lvl) = outline_level {
        let heading = lvl + 1; // outlineLvl is 0-based
        if (1..=6).contains(&heading) {
            return Some(heading);
        }
    }
    // Fallback: check for common human-readable style ID prefixes
    let lower = style_id.to_lowercase();
    if let Some(rest) = lower.strip_prefix("heading") {
        rest.trim()
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=6).contains(n))
    } else if let Some(rest) = lower.strip_prefix("titre") {
        rest.trim()
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=6).contains(n))
    } else {
        None
    }
}

/// Convert a Word highlight color name to a CSS color value.
fn word_highlight_to_css(color: &str) -> String {
    match color {
        "yellow" => "#ffff00".to_string(),
        "green" => "#00ff00".to_string(),
        "cyan" => "#00ffff".to_string(),
        "magenta" => "#ff00ff".to_string(),
        "blue" => "#0000ff".to_string(),
        "red" => "#ff0000".to_string(),
        "darkBlue" => "#000080".to_string(),
        "darkCyan" => "#008080".to_string(),
        "darkGreen" => "#008000".to_string(),
        "darkMagenta" => "#800080".to_string(),
        "darkRed" => "#800000".to_string(),
        "darkYellow" => "#808000".to_string(),
        "darkGray" | "darkGrey" => "#808080".to_string(),
        "lightGray" | "lightGrey" => "#c0c0c0".to_string(),
        "black" => "#000000".to_string(),
        "white" => "#ffffff".to_string(),
        _ => format!("#{}", color), // Treat as hex if unknown
    }
}

/// Uppercase only the text content of an HTML string, leaving tags intact.
fn uppercase_text_content(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            result.push(c);
        } else if c == '>' {
            in_tag = false;
            result.push(c);
        } else if in_tag {
            result.push(c);
        } else {
            for uc in c.to_uppercase() {
                result.push(uc);
            }
        }
    }
    result
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Wrap {Variable} placeholders in highlight spans.
/// Uses lowercase `data-variable` for canonical matching and
/// `data-original-case` for the original casing seen in the document.
///
/// Conditional variables (`{Label ?? true :: false}`) get extra
/// `data-conditional`, `data-true-text`, and `data-false-text` attributes
/// so the frontend can toggle between the two text branches.
///
/// Expects **raw** (unescaped) text — the function handles HTML escaping
/// internally so that variable delimiters like `"` are parsed correctly
/// before any encoding is applied.
fn highlight_variables(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            if let Some(var_content) = scan_brace_content(&mut chars) {
                if var_content.is_empty() {
                    result.push_str("{}");
                    continue;
                }
                let trimmed = var_content.trim();
                if is_conditional_variable(trimmed) {
                    if let Some((label, true_text, false_text)) =
                        parse_conditional_variable(trimmed)
                    {
                        let canonical = label.to_lowercase();
                        result.push_str(&format!(
                            "<span class=\"variable-highlight\" data-variable=\"{}\" data-original-case=\"{}\" data-conditional=\"true\" data-true-text=\"{}\" data-false-text=\"{}\">{{{}}}</span>",
                            escape_html(&canonical),
                            escape_html(&label),
                            escape_html(&true_text),
                            escape_html(&false_text),
                            escape_html(&var_content)
                        ));
                    } else {
                        // Malformed conditional — treat as plain text
                        result.push('{');
                        result.push_str(&escape_html(&var_content));
                        result.push('}');
                    }
                } else if !trimmed.contains('{') {
                    // Simple replacement variable (may use contact-role dot notation)
                    let canonical = if let Some((role, property)) =
                        parse_contact_role_ref(trimmed)
                    {
                        contact_role_to_flat_name(&role, &property).to_lowercase()
                    } else {
                        trimmed.to_lowercase()
                    };
                    result.push_str(&format!(
                        "<span class=\"variable-highlight\" data-variable=\"{}\" data-original-case=\"{}\">{{{}}}</span>",
                        escape_html(&canonical), escape_html(&var_content), escape_html(&var_content)
                    ));
                } else {
                    // Contains nested braces but isn't a conditional — emit as-is
                    result.push('{');
                    result.push_str(&escape_html(&var_content));
                    result.push('}');
                }
            } else {
                // Unbalanced brace — emit the `{` literally
                result.push('{');
            }
        } else {
            // Escape HTML special characters for non-variable text
            match c {
                '&' => result.push_str("&amp;"),
                '<' => result.push_str("&lt;"),
                '>' => result.push_str("&gt;"),
                _ => result.push(c),
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_variables_canonical_key() {
        let result = highlight_variables("{CLIENT NAME}");
        assert!(result.contains("data-variable=\"client name\""));
        assert!(result.contains("data-original-case=\"CLIENT NAME\""));
    }

    #[test]
    fn test_normalize_split_variables_brace_in_separate_run() {
        // { and } are in separate runs from the variable name
        let xml = r#"<w:r><w:rPr><w:b/></w:rPr><w:t>{</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>Agent #1 Full Name</w:t></w:r><w:r><w:rPr></w:rPr><w:t>}</w:t></w:r>"#;
        let normalized = normalize_split_variables(xml);
        assert!(
            normalized.contains("{Agent #1 Full Name}"),
            "Expected merged variable, got: {}",
            normalized
        );
    }

    #[test]
    fn test_normalize_split_variables_multi_split() {
        // Variable split across 3+ runs: { | Client | First Name }
        let xml = r#"<w:r><w:t>{</w:t></w:r><w:r><w:t>Client </w:t></w:r><w:r><w:t>First Name}</w:t></w:r>"#;
        let normalized = normalize_split_variables(xml);
        assert!(
            normalized.contains("{Client First Name}"),
            "Expected merged variable, got: {}",
            normalized
        );
    }

    #[test]
    fn test_normalize_preserves_non_split_variables() {
        // Variable already in a single run should be untouched
        let xml = r#"<w:r><w:t>{Client Name}</w:t></w:r>"#;
        let normalized = normalize_split_variables(xml);
        assert!(normalized.contains("{Client Name}"));
    }

    #[test]
    fn test_normalize_then_highlight() {
        // After normalization, highlight_variables should wrap the merged variable
        let xml = r#"<w:r><w:t>{</w:t></w:r><w:r><w:t>Test Var}</w:t></w:r>"#;
        let normalized = normalize_split_variables(xml);
        // The merged text should now contain the complete {Test Var}
        assert!(normalized.contains("{Test Var}"));
        // And highlight_variables should produce a proper span
        let highlighted = highlight_variables("{Test Var}");
        assert!(highlighted.contains("data-variable=\"test var\""));
    }

    #[test]
    fn test_sdt_preview_html() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        // Use a namespace-declared root so xml-rs can parse w: prefixed elements
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:sdt><w:sdtPr><w:tag w:val="lily:Client Name"/></w:sdtPr><w:sdtContent><w:r><w:t>Jane Doe</w:t></w:r></w:sdtContent></w:sdt></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("variable-highlight"),
            "Expected highlight span, got: {}",
            html
        );
        assert!(
            html.contains("data-variable=\"client name\""),
            "Expected canonical key, got: {}",
            html
        );
        assert!(
            html.contains("Jane Doe"),
            "Expected value text, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_paragraph_alignment() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>Centered text</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("text-align:center"),
            "Expected center alignment, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_indentation() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:ind w:left="720"/></w:pPr><w:r><w:t>Indented text</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("padding-left:36.0pt"),
            "Expected left indent, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_tab_character() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Before</w:t></w:r><w:r><w:tab/><w:t>After</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("preview-tab"),
            "Expected tab span, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_line_break() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Line one</w:t><w:br/><w:t>Line two</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(html.contains("<br>"), "Expected line break, got: {}", html);
    }

    #[test]
    fn test_preview_table() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell 1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Cell 2</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("<table"),
            "Expected table element, got: {}",
            html
        );
        assert!(
            html.contains("<td>Cell 1</td>"),
            "Expected cell content, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_strikethrough() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:strike/></w:rPr><w:t>struck</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("<s>struck</s>"),
            "Expected strikethrough, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_superscript() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>th</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("<sup>th</sup>"),
            "Expected superscript, got: {}",
            html
        );
    }

    #[test]
    fn test_preview_font_size() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:sz w:val="36"/></w:rPr><w:t>big</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(
            html.contains("font-size:18.0pt"),
            "Expected 18pt font size (36 half-pts), got: {}",
            html
        );
    }

    // ─── Conditional variable tests ─────────────────────────────────────

    #[test]
    fn test_is_conditional_variable() {
        assert!(is_conditional_variable(
            r#"Client Is Single ?? "single text" :: "couple text""#
        ));
        assert!(is_conditional_variable(r#"Label ?? "true" :: "false""#));
        assert!(is_conditional_variable(
            r#"Label ?? "only true text" :: """#
        ));
        assert!(!is_conditional_variable("Client Name"));
        assert!(!is_conditional_variable("Date"));
        // Single ? should NOT be treated as conditional
        assert!(!is_conditional_variable("What is this?"));
    }

    #[test]
    fn test_parse_conditional_variable() {
        let result =
            parse_conditional_variable(r#"Client Is Single ?? "single text" :: "couple text""#);
        assert_eq!(
            result,
            Some((
                "Client Is Single".to_string(),
                "single text".to_string(),
                "couple text".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_conditional_variable_quoted_empty_false() {
        let result = parse_conditional_variable(r#"Client Is Single ?? "single text" :: """#);
        assert_eq!(
            result,
            Some((
                "Client Is Single".to_string(),
                "single text".to_string(),
                String::new()
            ))
        );
    }

    #[test]
    fn test_parse_conditional_variable_quoted_preserves_spaces() {
        // Trailing space inside quotes should be preserved
        let result =
            parse_conditional_variable(r#"Label ?? "text with trailing space " :: "other text""#);
        assert_eq!(
            result,
            Some((
                "Label".to_string(),
                "text with trailing space ".to_string(),
                "other text".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_conditional_variable_with_special_content() {
        // Colons and :: inside quotes should be preserved as content
        let result = parse_conditional_variable(
            r#"Is Trust ?? "Trust dated: Jan 1, 2020" :: "No trust :: established""#,
        );
        assert_eq!(
            result,
            Some((
                "Is Trust".to_string(),
                "Trust dated: Jan 1, 2020".to_string(),
                "No trust :: established".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_conditional_variable_rejects_unquoted() {
        // Unquoted syntax should no longer parse
        let result = parse_conditional_variable("Label ?? true text :: false text");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_conditional_variable_smart_quotes() {
        // Word's "AutoFormat as you type" converts straight quotes to curly
        // quotes (\u{201C}/\u{201D}).  The parser must accept them.
        let result = parse_conditional_variable(
            "Label ?? \u{201C}true text\u{201D} :: \u{201C}false text\u{201D}",
        );
        assert_eq!(
            result,
            Some((
                "Label".to_string(),
                "true text".to_string(),
                "false text".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_conditional_variable_smart_quotes_with_nested() {
        let result = parse_conditional_variable(
            "Has HPOA #2 ?? \u{201C}If {HPOA #1 Full Name} is unable\u{201D} :: \u{201C}\u{201D}",
        );
        assert_eq!(
            result,
            Some((
                "Has HPOA #2".to_string(),
                "If {HPOA #1 Full Name} is unable".to_string(),
                String::new()
            ))
        );
    }

    #[test]
    fn test_highlight_variables_conditional() {
        let result = highlight_variables(r#"{Client Is Single ?? "single text" :: "couple text"}"#);
        assert!(
            result.contains("data-conditional=\"true\""),
            "Expected conditional attribute, got: {}",
            result
        );
        assert!(
            result.contains("data-variable=\"client is single\""),
            "Expected canonical key from label, got: {}",
            result
        );
        assert!(
            result.contains("data-true-text=\"single text\""),
            "Expected true text, got: {}",
            result
        );
        assert!(
            result.contains("data-false-text=\"couple text\""),
            "Expected false text, got: {}",
            result
        );
    }

    #[test]
    fn test_highlight_variables_conditional_empty_branch() {
        let result = highlight_variables(r#"{Client Is Single ?? "" :: "couple text"}"#);
        assert!(result.contains("data-conditional=\"true\""));
        assert!(result.contains("data-true-text=\"\""));
        assert!(result.contains("data-false-text=\"couple text\""));
    }

    // ─── Nested variable tests ──────────────────────────────────────────

    #[test]
    fn test_highlight_variables_nested_conditional() {
        let result = highlight_variables(
            r#"{Client Is Couple ?? "I am married to {Client Spouse Name}" :: "I am not married"}"#,
        );
        // Should produce a single conditional span for the outer variable
        assert!(
            result.contains("data-conditional=\"true\""),
            "Expected conditional attribute, got: {}",
            result
        );
        assert!(
            result.contains("data-variable=\"client is couple\""),
            "Expected canonical key from label, got: {}",
            result
        );
        // The true text should contain the nested {Client Spouse Name} reference
        assert!(
            result.contains("data-true-text=\"I am married to {Client Spouse Name}\""),
            "Expected true text with nested var, got: {}",
            result
        );
        assert!(
            result.contains("data-false-text=\"I am not married\""),
            "Expected false text, got: {}",
            result
        );
    }

    /// Verify that highlight_variables handles HTML-special characters in
    /// non-variable text (e.g., `&` in "Smith & Jones") correctly.
    #[test]
    fn test_highlight_variables_escapes_html_specials() {
        let result = highlight_variables("Smith & Jones said {Client Name} is <great>");
        assert!(
            result.contains("Smith &amp; Jones"),
            "Expected & to be escaped, got: {}",
            result
        );
        assert!(
            result.contains("&lt;great&gt;"),
            "Expected <> to be escaped, got: {}",
            result
        );
        assert!(
            result.contains("data-variable=\"client name\""),
            "Expected variable span, got: {}",
            result
        );
    }

    /// Verify that straight ASCII quotes in conditional variables are parsed
    /// correctly by highlight_variables (previously broken because escape_html
    /// was called before highlight_variables, converting `"` to `&quot;`).
    #[test]
    fn test_highlight_variables_conditional_straight_quotes() {
        // Straight ASCII quotes — the form generated by programmatic tools
        let result = highlight_variables(
            r#"{Has Alternate ?? "If primary is unable, appoint alternate." :: ""}"#,
        );
        assert!(
            result.contains("data-conditional=\"true\""),
            "Expected conditional with straight quotes, got: {}",
            result
        );
        assert!(
            result.contains("data-variable=\"has alternate\""),
            "Expected canonical key, got: {}",
            result
        );
        assert!(
            result.contains("data-true-text=\"If primary is unable, appoint alternate.\""),
            "Expected true text, got: {}",
            result
        );
    }

    /// Verify that curly/smart quotes in conditional variables are also parsed
    /// (the form generated by Word's AutoFormat).
    #[test]
    fn test_highlight_variables_conditional_curly_quotes() {
        let result = highlight_variables(
            "{Has Alternate ?? \u{201C}If primary is unable, appoint alternate.\u{201D} :: \u{201C}\u{201D}}",
        );
        assert!(
            result.contains("data-conditional=\"true\""),
            "Expected conditional with curly quotes, got: {}",
            result
        );
        assert!(
            result.contains("data-true-text=\"If primary is unable, appoint alternate.\""),
            "Expected true text, got: {}",
            result
        );
    }

    #[test]
    fn test_scan_brace_content_nested() {
        let text = "outer ?? text {inner} :: end}rest";
        let mut chars = text.chars().peekable();
        let content = scan_brace_content(&mut chars);
        // Note: scan_brace_content is called AFTER the opening {, so we simulate
        // by including the content after the first {
        assert_eq!(content, Some("outer ?? text {inner} :: end".to_string()));
    }

    #[test]
    fn test_scan_brace_content_simple() {
        let text = "Client Name}more";
        let mut chars = text.chars().peekable();
        let content = scan_brace_content(&mut chars);
        assert_eq!(content, Some("Client Name".to_string()));
    }

    #[test]
    fn test_scan_brace_content_unbalanced() {
        let text = "no closing brace";
        let mut chars = text.chars().peekable();
        let content = scan_brace_content(&mut chars);
        assert_eq!(content, None);
    }

    #[test]
    fn test_hpoa_highlight() {
        let text = r#"{Has HPOA #2 ?? "If {HPOA #1 Full Name} is unable, I appoint {HPOA #2 Full Name}, reachable at {HPOA #2 Phone}. " :: ""}"#;
        let result = highlight_variables(text);
        assert!(
            result.contains("data-conditional=\"true\""),
            "Expected conditional, got: {}",
            result
        );
        assert!(
            result.contains("data-variable=\"has hpoa #2\""),
            "Expected canonical key, got: {}",
            result
        );
        assert!(
            result.contains("{HPOA #1 Full Name}"),
            "Expected nested var in true-text, got: {}",
            result
        );
        assert!(
            result.contains("{HPOA #2 Full Name}"),
            "Expected nested var in true-text, got: {}",
            result
        );
        assert!(
            result.contains("{HPOA #2 Phone}"),
            "Expected nested var in true-text, got: {}",
            result
        );
    }


    #[test]
    fn test_hpoa_heading_is_bold() {
        let doc_path = "/home/parker/Obsidian/40-69 Projects/42 Carelaw Colorado/Drafting/02-Deliverables/Templates/02 - Power of Attorney and Medical Templates/HPOA Template.docx";
        if !std::path::Path::new(doc_path).exists() { return; }
        let html = get_document_html(doc_path.to_string()).unwrap();

        if let Some(idx) = html.find("Section 1.1") {
            let start = html[..idx].rfind("<h").unwrap_or(idx.saturating_sub(200));
            let end_tag = html[idx..].find("</h2>").unwrap_or(100);
            let heading_html = &html[start..idx + end_tag + 5];
            assert!(
                heading_html.contains("<strong>"),
                "Section 1.1 heading should be bold. HTML: {}", heading_html
            );
        }
    }

    #[test]
    fn test_preview_heading_style() {
        let empty_num = NumberingMap::new();
        let empty_styles = StyleMap::new();
        let empty_rels = RelationshipMap::new();
        let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p></w:body></w:document>"#;
        let html = xml_to_preview_html(xml, &empty_num, &empty_styles, &empty_rels);
        assert!(html.contains("<h1"), "Expected h1 element, got: {}", html);
    }

    // ─── Contact-role dot notation tests ─────────────────────────────────

    #[test]
    fn test_parse_contact_role_ref() {
        let (role, prop) = parse_contact_role_ref("Healthcare POA Agent.full_name").unwrap();
        assert_eq!(role, "Healthcare POA Agent");
        assert_eq!(prop, "full_name");
    }

    #[test]
    fn test_parse_contact_role_ref_unknown_property() {
        assert!(parse_contact_role_ref("Agent.unknown_prop").is_none());
    }

    #[test]
    fn test_parse_contact_role_ref_no_dot() {
        assert!(parse_contact_role_ref("Client Full Name").is_none());
    }

    #[test]
    fn test_property_to_title() {
        assert_eq!(property_to_title("full_name"), "Full Name");
        assert_eq!(property_to_title("phone"), "Phone");
        assert_eq!(property_to_title("first_name"), "First Name");
    }

    #[test]
    fn test_contact_role_to_flat_name() {
        assert_eq!(
            contact_role_to_flat_name("Healthcare POA Agent", "full_name"),
            "Healthcare POA Agent Full Name"
        );
        assert_eq!(
            contact_role_to_flat_name("Financial POA Agent", "phone"),
            "Financial POA Agent Phone"
        );
    }

    #[test]
    fn test_highlight_contact_role_dot_notation() {
        let text = "{Healthcare POA Agent.full_name}";
        let html = highlight_variables(text);
        assert!(
            html.contains("data-variable=\"healthcare poa agent full name\""),
            "Expected flat canonical key in data-variable, got: {}",
            html
        );
    }

}
