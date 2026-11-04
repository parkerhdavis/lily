#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Compile a Lily `.docx` template and its template-library schema into a document plan.

The compiler is deliberately conservative. It captures paragraphs, run marks,
tabs, line breaks, simple slots, and conditional true/false clauses, and it
warns about anything it cannot represent. It does not infer defined terms,
tables, or cross-references; those are reviewed and added by an author.

Usage:
	python3 scripts/lily_compile.py --templates-dir <dir> \
		--template-rel-path "<relative/path.docx>" --output <plan.json> [--json]
"""

import argparse
import json
import re
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"

PROPERTY_TITLES = {
	"full_name": "Full Name",
	"phone": "Phone",
	"email": "Email",
	"address": "Address",
	"city": "City",
	"state": "State",
	"zip": "Zip",
}

PLACEHOLDER = re.compile(r"\{([^{}]+)\}")


def fail(message: str, exit_code: int = 1) -> None:
	print(json.dumps({"error": message}, indent=2), file=sys.stderr)
	raise SystemExit(exit_code)


def local_name(element) -> str:
	return element.tag.split("}", 1)[-1]


def read_json(path: Path) -> dict:
	try:
		return json.loads(path.read_text(encoding="utf-8-sig"))
	except FileNotFoundError:
		fail(f"File not found: {path}")
	except json.JSONDecodeError as error:
		fail(f"Invalid JSON in {path}: {error}")


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


def sdt_text(sdt) -> str:
	return "".join(node.text or "" for node in sdt.iter(f"{W}t"))


def sdt_marks(sdt) -> list[str]:
	run = sdt.find(f".//{W}r")
	if run is None:
		return []
	return run_marks(run.find(f"{W}rPr"))


def run_marks(rpr) -> list[str]:
	marks: list[str] = []
	if rpr is None:
		return marks
	for child in rpr:
		name = local_name(child)
		value = child.get(f"{W}val")
		enabled = value not in ("false", "0", "none")
		if name == "b" and enabled:
			marks.append("bold")
		elif name == "i" and enabled:
			marks.append("italic")
		elif name == "u" and enabled:
			marks.append("underline")
		elif name == "caps":
			marks.append("caps")
		elif name == "smallCaps":
			marks.append("small_caps")
	return marks


def with_marks(node: dict, marks: list[str]) -> dict:
	if marks:
		node["marks"] = marks
	return node


def format_for_name(name: str) -> str | None:
	letters = "".join(character for character in name if character.isalpha())
	if not letters:
		return None
	if letters.isupper():
		return "upper"
	if letters.islower():
		return "lower"
	return None


def slugify(value: str) -> str:
	slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
	return slug or "block"


def find_schema_variable(name: str, variables: dict) -> tuple[str, dict | None]:
	if name in variables:
		return name, variables[name]
	for key, entry in variables.items():
		if key.lower() == name.lower():
			return key, entry
	if "." in name:
		role, property_name = name.split(".", 1)
		flat = f"{role} {PROPERTY_TITLES.get(property_name, property_name.replace('_', ' ').title())}"
		return find_schema_variable(flat, variables)
	return name, None


def slot_node(
	name: str,
	marks: list[str],
	variables: dict,
	slots: dict,
	infer_format: bool,
) -> dict:
	canonical, entry = find_schema_variable(name, variables)
	node = {"slot": name}
	if infer_format:
		format_value = format_for_name(name)
		if format_value:
			node["format"] = format_value
	with_marks(node, marks)

	if canonical not in slots:
		slot = {
			"label": canonical,
			"type": entry.get("var_type", "text") if entry else "text",
			"required": bool(entry.get("required", False)) if entry else False,
		}
		if entry:
			if entry.get("default") is not None:
				slot["default"] = entry["default"]
			if entry.get("help"):
				slot["help"] = entry["help"]
			if entry.get("contact_role"):
				slot["source"] = {
					"kind": "contact_role",
					"role": entry["contact_role"],
					"property": entry.get("contact_property", "full_name"),
				}
		slots[canonical] = slot
	return node


def branch_nodes(
	text: str,
	default_marks: list[str],
	variables: dict,
	slots: dict,
	warnings: list[str],
) -> list[dict]:
	if not text:
		return []
	nodes: list[dict] = []
	position = 0
	for match in PLACEHOLDER.finditer(text):
		if match.start() > position:
			nodes.append(with_marks({"text": text[position : match.start()]}, default_marks))
		name = match.group(1)
		if "." in name:
			warnings.append(f"Contact-role placeholder kept as a flat lookup: {name}")
		nodes.append(slot_node(name, default_marks, variables, slots, infer_format=True))
		position = match.end()
	if position < len(text):
		nodes.append(with_marks({"text": text[position:]}, default_marks))
	return nodes


def conditional_nodes(
	sdt,
	name: str,
	marks: list[str],
	variables: dict,
	slots: dict,
	clauses: dict,
	occurrences: dict,
	warnings: list[str],
) -> list[dict]:
	canonical, entry = find_schema_variable(name, variables)
	if entry is None:
		warnings.append(f"Conditional has no schema entry: {name}")
		text = sdt_text(sdt)
		return [with_marks({"text": text}, marks)] if text else []

	definitions = entry.get("conditions") or []
	if not definitions and entry.get("condition"):
		definitions = [entry["condition"]]
	if not definitions:
		warnings.append(f"Conditional has no branch definitions: {name}")
		return []

	index = occurrences.get(canonical, 0)
	occurrences[canonical] = index + 1
	if index >= len(definitions):
		warnings.append(
			f"Conditional occurrence {index + 1} of {name} has no definition; using the first"
		)
		definition = definitions[0]
	else:
		definition = definitions[index]

	controlling = definition.get("controlling_variable") or canonical
	nodes: list[dict] = []
	for branch, expected in (("true", True), ("false", False)):
		text = definition.get(f"{branch}_template", "")
		if not text:
			continue
		clause_id = f"clause-{slugify(canonical)}-{index + 1}-{branch}"
		body = branch_nodes(text, marks, variables, slots, warnings)
		clauses[clause_id] = {
			"id": clause_id,
			"version": 1,
			"title": f"{canonical} ({branch})",
			"body": body,
		}
		nodes.append({"clause": clause_id, "when": {"eq": [controlling, expected]}})
	return nodes


def sdt_nodes(
	sdt,
	variables: dict,
	slots: dict,
	clauses: dict,
	occurrences: dict,
	warnings: list[str],
) -> list[dict]:
	tag = sdt_tag(sdt)
	marks = sdt_marks(sdt)
	if not tag:
		text = sdt_text(sdt)
		return [with_marks({"text": text}, marks)] if text else []
	if tag.startswith("lily-cond:"):
		return conditional_nodes(
			sdt,
			tag[len("lily-cond:") :],
			marks,
			variables,
			slots,
			clauses,
			occurrences,
			warnings,
		)
	if tag.startswith("lily:"):
		return [slot_node(tag[len("lily:") :], marks, variables, slots, infer_format=False)]
	warnings.append(f"SDT has an unknown tag prefix: {tag}")
	text = sdt_text(sdt)
	return [with_marks({"text": text}, marks)] if text else []


def run_nodes(
	run,
	variables: dict,
	slots: dict,
	clauses: dict,
	occurrences: dict,
	warnings: list[str],
) -> list[dict]:
	marks = run_marks(run.find(f"{W}rPr"))
	nodes: list[dict] = []
	for child in run:
		if child.tag == f"{W}t":
			text = child.text or ""
			if text:
				nodes.append(with_marks({"text": text}, marks))
		elif child.tag == f"{W}tab":
			nodes.append({"tab": True})
		elif child.tag == f"{W}br":
			node = {"line_break": True}
			if child.get(f"{W}type") == "page":
				node["page"] = True
			nodes.append(node)
		elif child.tag == f"{W}sdt":
			nodes.extend(
				sdt_nodes(child, variables, slots, clauses, occurrences, warnings)
			)
		elif child.tag == f"{W}drawing":
			warnings.append("Drawing skipped; the renderer does not support images yet")
	return nodes


def paragraph_nodes(
	paragraph,
	variables: dict,
	slots: dict,
	clauses: dict,
	occurrences: dict,
	warnings: list[str],
) -> list[dict]:
	nodes: list[dict] = []
	for child in paragraph:
		if child.tag == f"{W}pPr":
			continue
		if child.tag == f"{W}r":
			nodes.extend(run_nodes(child, variables, slots, clauses, occurrences, warnings))
		elif child.tag == f"{W}sdt":
			nodes.extend(sdt_nodes(child, variables, slots, clauses, occurrences, warnings))
		elif child.tag == f"{W}hyperlink":
			for run in child.findall(f"{W}r"):
				nodes.extend(run_nodes(run, variables, slots, clauses, occurrences, warnings))
		elif child.tag == f"{W}bookmarkStart":
			name = child.get(f"{W}name", "")
			if name.startswith("lily:"):
				nodes.append(
					slot_node(
						name[len("lily:") :],
						[],
						variables,
						slots,
						infer_format=False,
					)
				)
		elif child.tag in (f"{W}ins", f"{W}smartTag"):
			for sub in child:
				if sub.tag == f"{W}r":
					nodes.extend(
						run_nodes(sub, variables, slots, clauses, occurrences, warnings)
					)
				elif sub.tag == f"{W}sdt":
					nodes.extend(
						sdt_nodes(sub, variables, slots, clauses, occurrences, warnings)
					)
		elif child.tag == f"{W}fldSimple":
			text = "".join(node.text or "" for node in child.iter(f"{W}t"))
			if text:
				nodes.append({"text": text})
		elif child.tag == f"{W}tbl":
			warnings.append("Table skipped; the renderer does not support tables yet")
	return nodes


def compile_template(
	templates_dir: Path,
	template_rel_path: str,
	allow_missing_schema: bool = False,
) -> tuple[dict, dict]:
	templates_dir = templates_dir.expanduser().resolve()
	library_path = find_template_library(templates_dir)
	library = read_json(library_path)
	entry = library.get("templates", {}).get(template_rel_path)
	missing_schema = False
	if entry is None:
		if not allow_missing_schema:
			available = "\n".join(sorted(library.get("templates", {}).keys()))
			fail(
				f"Template not found in {library_path}:\n{template_rel_path}\n\nAvailable:\n{available}"
			)
		entry = {"variables": {}}
		missing_schema = True

	template_path = templates_dir / template_rel_path
	if not template_path.is_file():
		fail(f"Template not found: {template_path}")

	with zipfile.ZipFile(template_path) as archive:
		if "word/document.xml" not in archive.namelist():
			fail(f"Template has no word/document.xml: {template_path}")
		xml = archive.read("word/document.xml")

	root = ET.fromstring(xml)
	body = root.find(f"{W}body")
	if body is None:
		fail(f"Template has no document body: {template_path}")

	variables = entry.get("variables", {})
	slots: dict = {}
	clauses: dict = {}
	occurrences: dict = {}
	warnings: list[str] = []
	if missing_schema:
		warnings.append("No template library entry; compiled as literal text.")
	blocks: list[dict] = []
	paragraph_index = 0

	for index, child in enumerate(body):
		if child.tag == f"{W}p":
			style = child.find(f"{W}pPr/{W}pStyle")
			blocks.append(
				{
					"id": f"block-{index:03d}",
					"kind": "paragraph",
					"style": style.get(f"{W}val") if style is not None else "",
					"content": paragraph_nodes(
						child, variables, slots, clauses, occurrences, warnings
					),
					"source_paragraph": paragraph_index,
				}
			)
			paragraph_index += 1
		elif child.tag == f"{W}tbl":
			warnings.append(f"Table at body index {index} skipped")

	for name, count in occurrences.items():
		canonical, schema_entry = find_schema_variable(name, variables)
		if schema_entry is None:
			continue
		definitions = schema_entry.get("conditions") or []
		if not definitions and schema_entry.get("condition"):
			definitions = [schema_entry["condition"]]
		if count < len(definitions):
			warnings.append(
				f"{canonical}: {len(definitions) - count} conditional definition(s) unused"
			)

	plan = {
		"lily_type": "document-plan",
		"id": slugify(template_path.stem),
		"version": 1,
		"title": template_path.stem,
		"layout": template_path.name,
		"blocks": blocks,
		"clauses": clauses,
		"slots": slots,
		"terms": {},
		"metadata": {
			"compiler": "lily-compile",
			"source_template": template_rel_path,
			"warnings": warnings,
		},
	}
	report = {
		"template": str(template_path),
		"blocks": len(blocks),
		"clauses": len(clauses),
		"slots": len(slots),
		"warnings": warnings,
	}
	return plan, report


def main() -> None:
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument("--templates-dir", required=True)
	parser.add_argument("--template-rel-path", required=True)
	parser.add_argument("--output", required=True)
	parser.add_argument("--json", action="store_true")
	parser.add_argument(
		"--allow-missing-schema",
		action="store_true",
		help="Compile a template that has no template-library entry as literal text",
	)
	args = parser.parse_args()

	plan, report = compile_template(
		Path(args.templates_dir),
		args.template_rel_path,
		allow_missing_schema=args.allow_missing_schema,
	)
	output = Path(args.output).expanduser().resolve()
	output.parent.mkdir(parents=True, exist_ok=True)
	output.write_text(json.dumps(plan, indent=2) + "\n")

	if args.json:
		print(json.dumps(report, indent=2))
	else:
		print(f"Compiled {report['template']}")
		print(
			f"  {report['blocks']} blocks, {report['clauses']} clauses, {report['slots']} slots"
		)
		for warning in report["warnings"]:
			print(f"  warning: {warning}")


if __name__ == "__main__":
	main()
