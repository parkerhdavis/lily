// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless fill harness for the current Lily engine.
//!
//! This binary runs the same `extract_variables` → `resolve_contact_variables`
//! → `replace_variables_v2` path the Tauri app uses, but without a window. It
//! exists so the golden corpus can be regenerated and compared outside the UI.
//!
//! Usage:
//!   lily-fill --client-dir <dir> --docx <filename> --templates-dir <dir> \
//!       --template-rel-path <path> [--output <path>] [--json]

// The binary pulls in the app's modules but exercises only the fill path, so
// most of their functions are intentionally unused here.
#![allow(dead_code)]

#[path = "../docx_ops.rs"]
mod docx_ops;

#[path = "../lily_file.rs"]
mod lily_file;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
	client_dir: String,
	docx: String,
	templates_dir: String,
	template_rel_path: String,
	output: Option<String>,
	json: bool,
}

fn usage() -> &'static str {
	"Usage: lily-fill --client-dir <dir> --docx <filename> \
	 --templates-dir <dir> --template-rel-path <path> \
	 [--output <path>] [--json]"
}

fn value_after(args: &[String], index: usize, flag: &str) -> Result<String, String> {
	args.get(index + 1)
		.cloned()
		.ok_or_else(|| format!("{} needs a value", flag))
}

fn parse_args() -> Result<Args, String> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let mut parsed = Args {
		client_dir: String::new(),
		docx: String::new(),
		templates_dir: String::new(),
		template_rel_path: String::new(),
		output: None,
		json: false,
	};

	let mut i = 0;
	while i < args.len() {
		match args[i].as_str() {
			"--client-dir" => {
				parsed.client_dir = value_after(&args, i, "--client-dir")?;
				i += 2;
			}
			"--docx" => {
				parsed.docx = value_after(&args, i, "--docx")?;
				i += 2;
			}
			"--templates-dir" => {
				parsed.templates_dir = value_after(&args, i, "--templates-dir")?;
				i += 2;
			}
			"--template-rel-path" => {
				parsed.template_rel_path = value_after(&args, i, "--template-rel-path")?;
				i += 2;
			}
			"--output" => {
				parsed.output = Some(value_after(&args, i, "--output")?);
				i += 2;
			}
			"--json" => {
				parsed.json = true;
				i += 1;
			}
			"--help" | "-h" => return Err(usage().to_string()),
			other => return Err(format!("Unknown argument: {}\n{}", other, usage())),
		}
	}

	if parsed.client_dir.is_empty()
		|| parsed.docx.is_empty()
		|| parsed.templates_dir.is_empty()
		|| parsed.template_rel_path.is_empty()
	{
		return Err(usage().to_string());
	}

	Ok(parsed)
}

fn run(args: &Args) -> Result<serde_json::Value, String> {
	let client_dir = PathBuf::from(&args.client_dir);
	let docx_path = client_dir.join(&args.docx);

	if !client_dir.is_dir() {
		return Err(format!("Client directory not found: {}", client_dir.display()));
	}
	if !docx_path.is_file() {
		return Err(format!("Document not found: {}", docx_path.display()));
	}

	// Populate derived variables exactly as the app does before a fill. This
	// writes the resolved values back into the client `.lily`, which is why the
	// caller works on a scratch copy of the fixture.
	let warnings = lily_file::resolve_contact_variables(
		client_dir.to_string_lossy().to_string(),
	)?;

	let lily = lily_file::read_lily_file(&client_dir.to_string_lossy())?;
	let variables = docx_ops::extract_variables(docx_path.to_string_lossy().to_string())?;
	let schema = docx_ops::load_template_schema(
		args.templates_dir.clone(),
		args.template_rel_path.clone(),
	)?;

	// Mirror the frontend's `buildVariableValues`: every extracted variable
	// gets an entry, conditionals default to "false", then schema defaults fill
	// anything still empty. A complete map is what keeps an unfilled variable
	// from being left as raw placeholder text on disk.
	let mut values: HashMap<String, String> = HashMap::new();
	for variable in &variables {
		let default_value = if variable.is_conditional { "false" } else { "" };
		let value = lily
			.variables
			.get(&variable.display_name)
			.cloned()
			.unwrap_or_else(|| default_value.to_string());
		values.insert(variable.display_name.clone(), value);
	}
	for variable in &variables {
		let is_empty = values
			.get(&variable.display_name)
			.map(|value| value.is_empty())
			.unwrap_or(true);
		if is_empty {
			if let Some(entry) = schema.variables.get(&variable.display_name) {
				if let Some(default_value) = &entry.default {
					values.insert(variable.display_name.clone(), default_value.clone());
				}
			}
		}
	}

	let mut conditional_schema: HashMap<String, Vec<docx_ops::ConditionalDef>> = HashMap::new();
	for (name, entry) in &schema.variables {
		if !entry.conditions.is_empty() {
			conditional_schema.insert(name.clone(), entry.conditions.clone());
		} else if let Some(condition) = &entry.condition {
			conditional_schema.insert(name.clone(), vec![condition.clone()]);
		}
	}

	docx_ops::replace_variables_v2(
		docx_path.to_string_lossy().to_string(),
		values.clone(),
		conditional_schema,
	)?;

	let output_path = args
		.output
		.as_ref()
		.map(PathBuf::from)
		.unwrap_or_else(|| docx_path.clone());
	if output_path != docx_path {
		if let Some(parent) = output_path.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("Failed to create output directory: {}", e))?;
		}
		std::fs::copy(&docx_path, &output_path)
			.map_err(|e| format!("Failed to copy filled document: {}", e))?;
	}

	Ok(serde_json::json!({
		"client_dir": client_dir.to_string_lossy(),
		"document": docx_path.to_string_lossy(),
		"output": output_path.to_string_lossy(),
		"template_rel_path": args.template_rel_path,
		"variables": variables.iter().map(|v| v.display_name.clone()).collect::<Vec<_>>(),
		"values": values,
		"warnings": warnings,
	}))
}

fn main() -> ExitCode {
	let args = match parse_args() {
		Ok(args) => args,
		Err(message) => {
			eprintln!("{}", message);
			return ExitCode::from(2);
		}
	};

	match run(&args) {
		Ok(report) => {
			if args.json {
				println!(
					"{}",
					serde_json::to_string_pretty(&report).unwrap_or_default()
				);
			} else {
				let output = report
					.get("output")
					.and_then(|value| value.as_str())
					.unwrap_or("(unknown)");
				println!("Filled {}", output);
			}
			ExitCode::SUCCESS
		}
		Err(message) => {
			eprintln!("lily-fill: {}", message);
			ExitCode::FAILURE
		}
	}
}
