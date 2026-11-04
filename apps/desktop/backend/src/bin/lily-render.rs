// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless plan renderer CLI.
//!
//! Usage:
//!   lily-render --plan <plan.json> --client-dir <dir> --skeleton <docx> \
//!       --output <docx> [--json]

// The binary pulls in the app's modules but exercises only the renderer.
#![allow(dead_code)]

#[path = "../docx_ops.rs"]
mod docx_ops;

#[path = "../lily_file.rs"]
mod lily_file;

#[path = "../plan.rs"]
mod plan;

use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
	plan: PathBuf,
	client_dir: PathBuf,
	skeleton: PathBuf,
	output: PathBuf,
	json: bool,
}

fn usage() -> &'static str {
	"Usage: lily-render --plan <plan.json> --client-dir <dir> \
	 --skeleton <docx> --output <docx> [--json]"
}

fn parse_args() -> Result<Args, String> {
	let arguments: Vec<String> = std::env::args().skip(1).collect();
	let mut plan = None;
	let mut client_dir = None;
	let mut skeleton = None;
	let mut output = None;
	let mut json = false;

	let mut index = 0;
	while index < arguments.len() {
		let flag = arguments[index].as_str();
		match flag {
			"--plan" | "--client-dir" | "--skeleton" | "--output" => {
				let value = arguments
					.get(index + 1)
					.cloned()
					.ok_or_else(|| format!("{} needs a value", flag))?;
				match flag {
					"--plan" => plan = Some(PathBuf::from(value)),
					"--client-dir" => client_dir = Some(PathBuf::from(value)),
					"--skeleton" => skeleton = Some(PathBuf::from(value)),
					"--output" => output = Some(PathBuf::from(value)),
					_ => unreachable!(),
				}
				index += 2;
			}
			"--json" => {
				json = true;
				index += 1;
			}
			"--help" | "-h" => return Err(usage().to_string()),
			other => return Err(format!("Unknown argument: {}\n{}", other, usage())),
		}
	}

	Ok(Args {
		plan: plan.ok_or_else(usage)?,
		client_dir: client_dir.ok_or_else(usage)?,
		skeleton: skeleton.ok_or_else(usage)?,
		output: output.ok_or_else(usage)?,
		json,
	})
}

fn main() -> ExitCode {
	let args = match parse_args() {
		Ok(args) => args,
		Err(message) => {
			eprintln!("{}", message);
			return ExitCode::from(2);
		}
	};

	match plan::render_plan(&args.plan, &args.client_dir, &args.skeleton, &args.output) {
		Ok(report) => {
			if args.json {
				println!(
					"{}",
					serde_json::to_string_pretty(&report).unwrap_or_default()
				);
			} else {
				println!(
					"Rendered {} ({} blocks)",
					report.output, report.blocks_rendered
				);
				for warning in &report.warnings {
					eprintln!("warning: {}", warning);
				}
			}
			ExitCode::SUCCESS
		}
		Err(message) => {
			eprintln!("lily-render: {}", message);
			ExitCode::FAILURE
		}
	}
}
