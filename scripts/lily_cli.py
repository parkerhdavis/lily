#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Agent-facing command line over Lily's client, template, and document primitives.

The tool layer is deliberately thin. It reads `.lily` client files and template
libraries, inspects a `.docx` without the UI, extracts normalized document text,
diffs two documents, and forwards to the headless `lily-fill` binary. The plan,
clause, render, and validate commands will extend this CLI when the plan
renderer lands.

Usage:
	scripts/lily client show <client-dir>
	scripts/lily template inspect <template.docx>
	scripts/lily template schema <templates-dir> <relative-path>
	scripts/lily docx text <document.docx>
	scripts/lily docx diff <left.docx> <right.docx>
	scripts/lily fill --client-dir <dir> --docx <name> --templates-dir <dir> \
		--template-rel-path <path> [--output <path>] [--json]
	scripts/lily doctor [--strict]

Every command prints JSON unless the command is explicitly a text diff. Errors
go to stderr with a non-zero exit code so an agent can branch on them.
"""

import argparse
import difflib
import json
import os
import subprocess
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
sys.path.insert(0, str(SCRIPT_DIR))

from run_corpus import extract_document_text, render_snapshot

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"

CLIENT_LILY_TYPES = {"client", None}
NON_CLIENT_LILY_TYPES = {"template-library", "questionnaire", "template-schema"}

PARTS_TO_INSPECT = (
	"word/document.xml",
	"word/header1.xml",
	"word/header2.xml",
	"word/header3.xml",
	"word/footer1.xml",
	"word/footer2.xml",
	"word/footer3.xml",
	"word/footnotes.xml",
	"word/endnotes.xml",
)


def fail(message: str, exit_code: int = 1) -> None:
	print(json.dumps({"error": message}, indent=2), file=sys.stderr)
	raise SystemExit(exit_code)


def read_json(path: Path) -> dict:
	try:
		return json.loads(path.read_text(encoding="utf-8-sig"))
	except FileNotFoundError:
		fail(f"File not found: {path}")
	except json.JSONDecodeError as error:
		fail(f"Invalid JSON in {path}: {error}")


def find_client_lily(target: Path) -> tuple[Path, dict]:
	if target.is_file():
		if target.suffix != ".lily":
			fail(f"Not a .lily file: {target}")
		data = read_json(target)
		if data.get("lily_type") in NON_CLIENT_LILY_TYPES:
			fail(f"Not a client .lily file: {target}")
		return target, data
	if not target.is_dir():
		fail(f"Client directory or .lily file not found: {target}")
	candidates = []
	for path in sorted(target.glob("*.lily")):
		try:
			data = read_json(path)
		except SystemExit:
			continue
		lily_type = data.get("lily_type")
		if lily_type in NON_CLIENT_LILY_TYPES:
			continue
		candidates.append((path, data))
	if not candidates:
		fail(f"No client .lily file found in {target}")
	if len(candidates) > 1:
		names = ", ".join(path.name for path, _ in candidates)
		fail(f"Multiple client .lily files found in {target}: {names}")
	return candidates[0]


def find_template_library(templates_dir: Path) -> Path:
	if not templates_dir.is_dir():
		fail(f"Templates directory not found: {templates_dir}")
	for path in sorted(templates_dir.glob("*.lily")):
		try:
			data = read_json(path)
		except SystemExit:
			continue
		if data.get("lily_type") == "template-library":
			return path
	fail(f"No template-library .lily file found in {templates_dir}")


def sdt_tag(sdt) -> str | None:
	tag = sdt.find(f"{W}sdtPr/{W}tag")
	if tag is None:
		return None
	return tag.get(f"{W}val")


def run_text(run, tags: list[str]) -> str:
	parts: list[str] = []
	for child in run:
		if child.tag == f"{W}t":
			parts.append(child.text or "")
		elif child.tag == f"{W}tab":
			parts.append("\t")
		elif child.tag == f"{W}br":
			parts.append("\n")
		elif child.tag == f"{W}sdt":
			tag = sdt_tag(child)
			if tag:
				parts.append(f"[[{tag}]]")
				tags.append(tag)
			else:
				parts.append("".join(node.text or "" for node in child.iter(f"{W}t")))
		elif child.tag == f"{W}drawing":
			parts.append("[drawing]")
	return "".join(parts)


def paragraph_text(paragraph) -> tuple[str, list[str]]:
	parts: list[str] = []
	tags: list[str] = []
	for child in paragraph:
		if child.tag == f"{W}r":
			parts.append(run_text(child, tags))
		elif child.tag == f"{W}sdt":
			tag = sdt_tag(child)
			if tag:
				parts.append(f"[[{tag}]]")
				tags.append(tag)
			else:
				parts.append("".join(node.text or "" for node in child.iter(f"{W}t")))
		elif child.tag == f"{W}hyperlink":
			parts.append("".join(node.text or "" for node in child.iter(f"{W}t")))
		elif child.tag == f"{W}bookmarkStart":
			name = child.get(f"{W}name", "")
			if name.startswith("lily:"):
				parts.append(f"[[{name}]]")
				tags.append(name)
		elif child.tag == f"{W}pPr":
			continue
		else:
			parts.append("".join(node.text or "" for node in child.iter(f"{W}t")))
	return "".join(parts), tags


def part_base_context(part_name: str) -> str:
	if part_name.startswith("word/header"):
		return "header"
	if part_name.startswith("word/footer"):
		return "footer"
	if part_name.startswith("word/footnotes"):
		return "footnotes"
	if part_name.startswith("word/endnotes"):
		return "endnotes"
	return "document"


def inspect_part(part_name: str, xml: bytes) -> dict:
	root = ET.fromstring(xml)
	paragraphs: list[dict] = []
	sdts: list[dict] = []
	base_context = part_base_context(part_name)

	def walk(element, context: str, paragraph_index: int | None) -> None:
		for child in element:
			next_context = context
			if child.tag == f"{W}tbl":
				next_context = f"{context}/table"
			elif child.tag == f"{W}txbxContent":
				next_context = f"{context}/textbox"

			if child.tag == f"{W}p":
				index = len(paragraphs)
				text, tags = paragraph_text(child)
				style = child.find(f"{W}pPr/{W}pStyle")
				paragraphs.append(
					{
						"index": index,
						"context": context,
						"style": style.get(f"{W}val") if style is not None else "",
						"text": text,
						"tags": tags,
					}
				)
				walk(child, next_context, index)
			elif child.tag == f"{W}sdt":
				tag = sdt_tag(child)
				if tag:
					sdts.append(
						{
							"tag": tag,
							"text": "".join(node.text or "" for node in child.iter(f"{W}t")),
							"paragraph": paragraph_index,
							"context": context,
						}
					)
				walk(child, next_context, paragraph_index)
			else:
				walk(child, next_context, paragraph_index)

	walk(root, base_context, None)
	return {"paragraphs": paragraphs, "sdts": sdts}


def command_client_show(args) -> None:
	directory = Path(args.client_dir).expanduser().resolve()
	path, client = find_client_lily(directory)

	contacts = client.get("contacts", [])
	children = [contact for contact in contacts if contact.get("relationship") == "Child"]
	spouse = next(
		(contact for contact in contacts if contact.get("relationship") == "Spouse"),
		None,
	)
	role_contacts: dict[str, str | None] = {}
	contact_names = {contact.get("id"): contact.get("full_name", "") for contact in contacts}
	for role, binding in client.get("contact_bindings", {}).items():
		contact_id = binding.get("contact_id")
		if contact_id:
			role_contacts[role] = contact_names.get(contact_id)
		else:
			role_contacts[role] = None

	derived = {
		"has_spouse": spouse is not None,
		"spouse": spouse.get("full_name") if spouse else None,
		"children": [child.get("full_name", "") for child in children],
		"has_children": bool(children),
		"has_minor_children": any(child.get("is_minor") for child in children),
		"roles": role_contacts,
	}

	payload = {
		"directory": str(directory),
		"file": path.name,
		"client": client,
		"derived": derived,
	}
	print(json.dumps(payload, indent=2))


def command_template_inspect(args) -> None:
	docx_path = Path(args.template).expanduser().resolve()
	if not docx_path.is_file():
		fail(f"Template not found: {docx_path}")

	with zipfile.ZipFile(docx_path) as archive:
		names = set(archive.namelist())
		parts = {}
		for part_name in PARTS_TO_INSPECT:
			if part_name not in names:
				continue
			parts[part_name] = inspect_part(part_name, archive.read(part_name))

	total_paragraphs = sum(len(part["paragraphs"]) for part in parts.values())
	total_sdts = sum(len(part["sdts"]) for part in parts.values())
	conditional_sdts = sum(
		1 for part in parts.values() for sdt in part["sdts"] if sdt["tag"].startswith("lily-cond:")
	)
	textbox_sdts = sum(
		1 for part in parts.values() for sdt in part["sdts"] if "textbox" in sdt["context"]
	)

	payload = {
		"file": str(docx_path),
		"parts": parts,
		"summary": {
			"parts": len(parts),
			"paragraphs": total_paragraphs,
			"sdts": total_sdts,
			"conditional_sdts": conditional_sdts,
			"textbox_sdts": textbox_sdts,
		},
	}
	print(json.dumps(payload, indent=2))


def parse_conditional_schema(entry: dict) -> dict:
	schema: dict[str, list] = {}
	for name, variable in entry.get("variables", {}).items():
		conditions = variable.get("conditions") or []
		condition = variable.get("condition")
		if conditions:
			schema[name] = conditions
		elif condition:
			schema[name] = [condition]
	return schema


def command_template_schema(args) -> None:
	templates_dir = Path(args.templates_dir).expanduser().resolve()
	library_path = find_template_library(templates_dir)
	library = read_json(library_path)
	entry = library.get("templates", {}).get(args.relative_path)
	if entry is None:
		available = "\n".join(sorted(library.get("templates", {}).keys()))
		fail(f"Template not found in {library_path}:\n{args.relative_path}\n\nAvailable:\n{available}")

	payload = {
		"library": str(library_path),
		"template": args.relative_path,
		"variables": entry.get("variables", {}),
		"conditional_schema": parse_conditional_schema(entry),
	}
	if args.conditions_only:
		payload["variables"] = {
			name: variable
			for name, variable in payload["variables"].items()
			if variable.get("var_type") == "conditional"
		}
		payload["conditional_schema"] = {
			name: definitions
			for name, definitions in payload["conditional_schema"].items()
			if name in payload["variables"]
		}
	print(json.dumps(payload, indent=2))


def command_docx_text(args) -> None:
	docx_path = Path(args.document).expanduser().resolve()
	if not docx_path.is_file():
		fail(f"Document not found: {docx_path}")
	lines, meta = extract_document_text(docx_path)
	if args.json:
		print(json.dumps({"file": str(docx_path), "lines": lines, "meta": meta}, indent=2))
	else:
		print(render_snapshot(docx_path.name, "", lines, meta), end="")


def render_diff_snapshot(lines: list[str], meta: dict) -> str:
	header = [
		f"# Paragraphs: {meta['paragraphs']}",
		f"# Tables: {meta['tables']}",
		f"# Styles: {', '.join(meta['styles'])}",
		"# ---",
	]
	return "\n".join(header + lines) + "\n"


def command_docx_diff(args) -> None:
	left = Path(args.left).expanduser().resolve()
	right = Path(args.right).expanduser().resolve()
	for path in (left, right):
		if not path.is_file():
			fail(f"Document not found: {path}")
	left_lines, left_meta = extract_document_text(left)
	right_lines, right_meta = extract_document_text(right)
	left_snapshot = render_diff_snapshot(left_lines, left_meta)
	right_snapshot = render_diff_snapshot(right_lines, right_meta)
	diff = "".join(
		difflib.unified_diff(
			left_snapshot.splitlines(keepends=True),
			right_snapshot.splitlines(keepends=True),
			fromfile=str(left),
			tofile=str(right),
		)
	)
	if args.json:
		print(json.dumps({"equal": not diff, "diff": diff}, indent=2))
	else:
		print(diff, end="")
	if diff:
		raise SystemExit(1)


def resolve_fill_binary() -> Path:
	override = os.environ.get("LILY_FILL_BIN")
	if override:
		return Path(override).expanduser().resolve()
	return REPO_ROOT / "target" / "debug" / "lily-fill"


def command_fill(forwarded: list[str]) -> None:
	binary = resolve_fill_binary()
	if not binary.is_file():
		fail(
			f"lily-fill not found: {binary}\n"
			"Build it with: cargo build --bin lily-fill (from apps/desktop/backend)"
		)
	result = subprocess.run([str(binary), *forwarded])
	raise SystemExit(result.returncode)


def command_doctor(args) -> None:
	fill_binary = resolve_fill_binary()
	corpus = Path(os.environ.get("LILY_CORPUS", "~/Carelaw/carelaw-lily/Tests")).expanduser()
	templates = Path(
		os.environ.get("LILY_TEMPLATES", "~/Carelaw/carelaw-lily/Templates")
	).expanduser()

	checks = {
		"python": {"version": sys.version.split()[0], "ok": sys.version_info >= (3, 10)},
		"lily_fill": {"path": str(fill_binary), "ok": fill_binary.is_file()},
		"corpus": {
			"path": str(corpus),
			"manifest": str(corpus / "manifest.json"),
			"ok": (corpus / "manifest.json").is_file(),
		},
		"templates": {
			"path": str(templates),
			"ok": (templates / "Estate Planning Templates.lily").is_file(),
		},
	}
	print(json.dumps({"checks": checks}, indent=2))
	if args.strict and not all(check.get("ok") for check in checks.values()):
		raise SystemExit(1)


def build_parser() -> argparse.ArgumentParser:
	parser = argparse.ArgumentParser(
		prog="lily",
		description="Agent-facing CLI over Lily's client, template, and document primitives.",
	)
	subparsers = parser.add_subparsers(dest="command", required=True)

	client = subparsers.add_parser("client", help="Inspect a client folder")
	client_sub = client.add_subparsers(dest="client_command", required=True)
	client_show = client_sub.add_parser("show", help="Print the .lily client and a derived summary")
	client_show.add_argument("client_dir")
	client_show.set_defaults(func=command_client_show)

	template = subparsers.add_parser("template", help="Inspect templates and template schemas")
	template_sub = template.add_subparsers(dest="template_command", required=True)
	template_inspect = template_sub.add_parser(
		"inspect", help="Print every paragraph and SDT, including headers, footers, and text boxes"
	)
	template_inspect.add_argument("template")
	template_inspect.set_defaults(func=command_template_inspect)

	template_schema = template_sub.add_parser(
		"schema", help="Print a template's schema and conditional definitions"
	)
	template_schema.add_argument("templates_dir")
	template_schema.add_argument("relative_path")
	template_schema.add_argument(
		"--conditions-only", action="store_true", help="Print only conditional variables"
	)
	template_schema.set_defaults(func=command_template_schema)

	docx = subparsers.add_parser("docx", help="Read and compare document text")
	docx_sub = docx.add_subparsers(dest="docx_command", required=True)
	docx_text = docx_sub.add_parser("text", help="Print normalized document text and paragraph styles")
	docx_text.add_argument("document")
	docx_text.add_argument("--json", action="store_true", help="Print JSON instead of plain text")
	docx_text.set_defaults(func=command_docx_text)

	docx_diff = docx_sub.add_parser("diff", help="Diff two documents by normalized text and styles")
	docx_diff.add_argument("left")
	docx_diff.add_argument("right")
	docx_diff.add_argument("--json", action="store_true", help="Print JSON instead of a unified diff")
	docx_diff.set_defaults(func=command_docx_diff)

	fill = subparsers.add_parser(
		"fill", help="Forward to the headless lily-fill binary", add_help=False
	)
	fill.add_argument("forwarded", nargs=argparse.REMAINDER)
	fill.set_defaults(func=lambda arguments: command_fill(arguments.forwarded))

	doctor = subparsers.add_parser("doctor", help="Check the tool layer's environment")
	doctor.add_argument("--strict", action="store_true", help="Exit non-zero if a check fails")
	doctor.set_defaults(func=command_doctor)

	return parser


def main() -> None:
	# `fill` forwards every argument verbatim; argparse would reject lily-fill's
	# own options, so intercept it before building the parser.
	if len(sys.argv) > 1 and sys.argv[1] == "fill":
		command_fill(sys.argv[2:])
		return
	parser = build_parser()
	args = parser.parse_args()
	args.func(args)


if __name__ == "__main__":
	main()
