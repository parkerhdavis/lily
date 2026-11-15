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
import copy
import difflib
import json
import os
import shutil
import subprocess
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
sys.path.insert(0, str(SCRIPT_DIR))

from run_corpus import extract_document_text, render_snapshot
import lily_compile

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


def resolve_render_binary() -> Path:
	override = os.environ.get("LILY_RENDER_BIN")
	if override:
		return Path(override).expanduser().resolve()
	return REPO_ROOT / "target" / "debug" / "lily-render"


def command_compile(args) -> None:
	plan, report = lily_compile.compile_template(
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


def audit_variables(client: dict) -> dict:
	variables = dict(client.get("variables", {}))
	contacts = client.get("contacts", [])
	children = [contact for contact in contacts if contact.get("relationship") == "Child"]
	spouse = next(
		(contact for contact in contacts if contact.get("relationship") == "Spouse"),
		None,
	)
	variables["children_count"] = str(len(children))
	variables["Has Spouse"] = "true" if spouse is not None else "false"
	variables["Client has Children"] = "true" if children else "false"
	variables["Has Minor Children"] = (
		"true" if any(contact.get("is_minor") for contact in children) else "false"
	)
	if spouse is not None:
		variables["Client Spouse Name"] = spouse.get("full_name", "")

	has_spouse = str(variables.get("Has Spouse", "")).strip().lower() == "true"
	spouse_name = variables.get("Client Spouse Name", "")
	helpers = (
		("Settlor Term", "Settlors", "Settlor"),
		("Settlor Possessive", "Settlors\u2019", "Settlor\u2019s"),
		("Settlor Verb", "are", "is"),
		("Settlor Have", "have", "has"),
		("Settlor Verb Past", "were", "was"),
		("Settlor Each", "each Settlor", "Settlor"),
	)
	for name, plural, singular in helpers:
		variables[name] = plural if has_spouse else singular
	variables["And Spouse Name"] = (
		f" and {spouse_name}" if has_spouse and spouse_name else ""
	)
	lists = {
		"required_documents": [
			document.get("template_rel_path", "")
			for document in client.get("required_documents", [])
			if document.get("template_rel_path")
		]
	}
	return variables, lists


def lookup_variable(variables: dict, name: str):
	if name in variables:
		return variables[name]
	lowered = name.lower()
	for key, value in variables.items():
		if key.lower() == lowered:
			return value
	return None


def condition_variables(condition) -> set[str]:
	if not isinstance(condition, dict):
		return set()
	names: set[str] = set()
	for key in ("eq", "neq"):
		pair = condition.get(key)
		if isinstance(pair, list) and len(pair) == 2 and isinstance(pair[0], str):
			names.add(pair[0])
	includes = condition.get("includes")
	if isinstance(includes, list) and len(includes) == 2 and isinstance(includes[0], str):
		names.add(includes[0])
	for key in ("and", "or"):
		for child in condition.get(key) or []:
			names |= condition_variables(child)
	if "not" in condition:
		names |= condition_variables(condition["not"])
	if isinstance(condition.get("exists"), str):
		names.add(condition["exists"])
	return names


def evaluate_condition(condition, variables: dict, lists: dict):
	if isinstance(condition, bool):
		return condition
	if not isinstance(condition, dict):
		return None
	if "eq" in condition or "neq" in condition:
		key = "eq" if "eq" in condition else "neq"
		pair = condition.get(key)
		if not isinstance(pair, list) or len(pair) != 2:
			return None
		name, expected = pair
		actual = lookup_variable(variables, name)
		if actual is None:
			return None
		if isinstance(expected, bool):
			result = (str(actual).strip().lower() == "true") == expected
		elif isinstance(expected, (int, float)):
			try:
				result = float(actual) == float(expected)
			except (TypeError, ValueError):
				return None
		else:
			result = str(actual) == str(expected)
		return result if key == "eq" else not result
	if "includes" in condition:
		pair = condition.get("includes")
		if not isinstance(pair, list) or len(pair) != 2:
			return None
		name, needle = pair
		return str(needle) in lists.get(name, [])
	if "and" in condition:
		results = [
			evaluate_condition(child, variables, lists)
			for child in condition.get("and") or []
		]
		if any(result is False for result in results):
			return False
		return None if any(result is None for result in results) else True
	if "or" in condition:
		results = [
			evaluate_condition(child, variables, lists)
			for child in condition.get("or") or []
		]
		if any(result is True for result in results):
			return True
		return None if any(result is None for result in results) else False
	if "not" in condition:
		result = evaluate_condition(condition["not"], variables, lists)
		return None if result is None else not result
	if isinstance(condition.get("exists"), str):
		value = lookup_variable(variables, condition["exists"])
		return bool(value) if value is not None else False
	return None


def expand_excerpts(
	plan: dict,
	base_dir: Path,
	variables: dict,
	lists: dict,
	prefix: str = "",
) -> dict:
	"""Inline excerpt blocks so an audit sees the data gaps inside them."""
	expanded = copy.deepcopy(plan)
	blocks = []
	for block in plan.get("blocks", []):
		block = copy.deepcopy(block)
		block_id = block.get("id", "excerpt")
		if block.get("kind") != "excerpt":
			block["id"] = f"{prefix}{block_id}"
			blocks.append(block)
			continue
		condition = block.get("when")
		if condition is not None and evaluate_condition(condition, variables, lists) is not True:
			continue
		path = block.get("path")
		if not path:
			fail(f"Excerpt block '{block_id}' has no path")
		excerpt_path = (base_dir / path).resolve()
		if not excerpt_path.is_file():
			fail(f"Excerpt not found: {excerpt_path}")
		excerpt = read_json(excerpt_path)
		nested = expand_excerpts(
			excerpt,
			excerpt_path.parent,
			variables,
			lists,
			prefix=f"{prefix}{block_id}:",
		)
		blocks.extend(nested.get("blocks", []))
		clauses = expanded.setdefault("clauses", {})
		for clause_id, clause in nested.get("clauses", {}).items():
			clauses.setdefault(clause_id, clause)
		slots = expanded.setdefault("slots", {})
		for name, slot in nested.get("slots", {}).items():
			slots.setdefault(name, slot)
	expanded["blocks"] = blocks
	return expanded


def collect_plan_references(plan: dict) -> dict:
	slots: list[str] = []
	roles: set[str] = set()
	clause_refs: set[str] = set()
	terms: set[str] = set()
	refs: list[str] = []
	conditions: list[tuple[str, object]] = []

	def visit(node, where: str) -> None:
		if isinstance(node, dict):
			if isinstance(node.get("slot"), str):
				slots.append(node["slot"])
			if isinstance(node.get("role"), str):
				roles.add(node["role"])
			if isinstance(node.get("clause"), str):
				clause_refs.add(node["clause"])
			if isinstance(node.get("term"), str):
				terms.add(node["term"])
			if isinstance(node.get("ref"), str):
				refs.append(node["ref"])
			if node.get("when") is not None:
				conditions.append((where, node["when"]))
			for value in node.values():
				visit(value, where)
		elif isinstance(node, list):
			for value in node:
				visit(value, where)

	for block in plan.get("blocks", []):
		block_id = block.get("id", "?")
		visit(block.get("content", []), block_id)
		if block.get("when") is not None:
			conditions.append((block_id, block["when"]))
	for clause_id, clause in plan.get("clauses", {}).items():
		visit(clause.get("body", []), f"clause:{clause_id}")
		if clause.get("when") is not None:
			conditions.append((f"clause:{clause_id}", clause["when"]))
	return {
		"slots": slots,
		"roles": sorted(roles),
		"clause_refs": sorted(clause_refs),
		"terms": sorted(terms),
		"refs": refs,
		"conditions": conditions,
	}


def computed_slot(name: str) -> bool:
	return (
		name.startswith("Settlor ")
		or name == "And Spouse Name"
		or "Co-" in name
		or name.endswith(" And Name")
		or name.endswith(" And Phone")
		or name.endswith(" Title")
		or name.endswith(" Verb")
	)


def client_role_names(client: dict) -> dict[str, list[str]]:
	contacts = {contact.get("id"): contact for contact in client.get("contacts", [])}
	roles: dict[str, list[str]] = {}
	for role, binding in client.get("contact_bindings", {}).items():
		names: list[str] = []
		assignments = binding.get("assignments") or []
		for assignment in assignments:
			for contact_id in assignment.get("contact_ids", []):
				contact = contacts.get(contact_id)
				if contact and contact.get("full_name"):
					names.append(contact["full_name"])
			for value in assignment.get("values", {}).values():
				if value:
					names.append(value)
		if not assignments:
			contact_ids = binding.get("contact_ids") or []
			for contact_id in contact_ids:
				contact = contacts.get(contact_id)
				if contact and contact.get("full_name"):
					names.append(contact["full_name"])
			contact_id = binding.get("contact_id")
			if contact_id and contact_id != "__none__":
				contact = contacts.get(contact_id)
				if contact and contact.get("full_name"):
					names.append(contact["full_name"])
		if not names:
			for variable in binding.get("variable_mappings", {}):
				value = client.get("variables", {}).get(variable)
				if value:
					names.append(value)
		if names:
			roles[role] = names
	if "Successor Trustee" not in roles:
		legacy_names: list[str] = []
		for role in ["Primary Trustee", "Secondary Trustee", "Tertiary Trustee"]:
			legacy_names.extend(roles.get(role, []))
		if legacy_names:
			roles["Successor Trustee"] = legacy_names

	def fold_tiers(target: str, tiers: list[tuple[str, str | None]]) -> None:
		if target in roles:
			return
		legacy_names: list[str] = []
		for parent, co in tiers:
			legacy_names.extend(roles.get(parent, []))
			if co:
				legacy_names.extend(roles.get(co, []))
		if legacy_names:
			roles[target] = legacy_names

	fold_tiers(
		"Personal Representative",
		[
			("Primary Personal Representative", "Primary Co-Personal Representative"),
			("Secondary Personal Representative", "Secondary Co-Personal Representative"),
			("Tertiary Personal Representative", "Tertiary Co-Personal Representative"),
		],
	)
	fold_tiers("Guardian", [("Primary Guardian", None), ("Secondary Guardian", None)])
	fold_tiers(
		"HPOA Agent",
		[
			("Primary HPOA Agent", "Primary HPOA Co-Agent"),
			("Secondary HPOA Agent", "Secondary HPOA Co-Agent"),
			("Tertiary HPOA Agent", "Tertiary HPOA Co-Agent"),
		],
	)
	fold_tiers(
		"FPOA Agent",
		[
			("Primary FPOA Agent", "Primary FPOA Co-Agent"),
			("Secondary FPOA Agent", "Secondary FPOA Co-Agent"),
			("Tertiary FPOA Agent", "Tertiary FPOA Co-Agent"),
		],
	)
	fold_tiers(
		"Deed Beneficiary",
		[("Primary Beneficiary", None), ("Secondary Beneficiary", None)],
	)
	return roles


def audit_plan(plan: dict, variables: dict, lists: dict, roles: dict[str, list[str]] | None = None) -> dict:
	references = collect_plan_references(plan)
	roles = roles or {}
	missing_roles = [role for role in references["roles"] if not roles.get(role)]
	declared = plan.get("slots", {})
	used = sorted(set(references["slots"]))
	condition_names: set[str] = set()
	for _, condition in references["conditions"]:
		condition_names |= condition_variables(condition)
	empty = []
	for name in used:
		value = lookup_variable(variables, name)
		if value is not None and str(value) != "":
			continue
		declaration = declared.get(name, {})
		empty.append(
			{
				"name": name,
				"required": bool(declaration.get("required", False)),
				"kind": "computed" if computed_slot(name) else "missing",
				"value": "" if value is None else str(value),
			}
		)
	conditions = []
	seen = set()
	for where, condition in references["conditions"]:
		signature = json.dumps(condition, sort_keys=True, ensure_ascii=False)
		if signature in seen:
			continue
		seen.add(signature)
		names = sorted(condition_variables(condition))
		values = {
			name: lists[name] if name in lists else lookup_variable(variables, name)
			for name in names
		}
		conditions.append(
			{
				"where": where,
				"condition": condition,
				"variables": names,
				"values": values,
				"result": evaluate_condition(condition, variables, lists),
			}
		)
	return {
		"plan": {
			"id": plan.get("id"),
			"version": plan.get("version"),
			"blocks": len(plan.get("blocks", [])),
			"clauses": len(plan.get("clauses", {})),
		},
		"slots": {
			"used": used,
			"declared": sorted(declared),
			"undeclared": sorted(set(used) - set(declared)),
			"unused": sorted(set(declared) - set(used) - condition_names),
			"empty": empty,
		},
		"conditions": conditions,
		"terms": references["terms"],
		"refs": sorted(set(references["refs"])),
		"clause_refs": references["clause_refs"],
		"roles": references["roles"],
		"missing_roles": missing_roles,
	}


def command_plan_audit(args) -> None:
	plan_path = Path(args.plan).expanduser().resolve()
	if not plan_path.is_file():
		fail(f"Plan not found: {plan_path}")
	plan = read_json(plan_path)
	client_path, client = find_client_lily(Path(args.client_dir).expanduser().resolve())
	variables, lists = audit_variables(client)
	roles = client_role_names(client)
	plan = expand_excerpts(plan, plan_path.parent, variables, lists)
	report = audit_plan(plan, variables, lists, roles)
	report["client"] = str(client_path)
	if args.json:
		print(json.dumps(report, indent=2))
		return

	meta = report["plan"]
	print(f"Plan {meta['id']} v{meta['version']}: {meta['blocks']} blocks, {meta['clauses']} clauses")
	slots = report["slots"]
	print(f"Slots: {len(slots['used'])} used, {len(slots['declared'])} declared")
	if slots["undeclared"]:
		print("Undeclared slots:")
		for name in slots["undeclared"]:
			print(f"  - {name}")
	if slots["unused"]:
		print("Declared but unused:")
		for name in slots["unused"]:
			print(f"  - {name}")
	if slots["empty"]:
		print("Empty values:")
		for entry in slots["empty"]:
			required = "required" if entry["required"] else "optional"
			print(f"  - {entry['name']} ({required}, {entry['kind']})")
	else:
		print("Empty values: none")
	if report["missing_roles"]:
		print("Unassigned roles:")
		for role in report["missing_roles"]:
			print(f"  - {role}")
	else:
		print("Unassigned roles: none")
	print("Conditions:")
	for entry in report["conditions"]:
		result = {True: "true", False: "false", None: "unknown"}[entry["result"]]
		values = ", ".join(f"{name}={value!r}" for name, value in entry["values"].items())
		print(f"  - {entry['where']}: {entry['condition']} -> {result} ({values})")


def plan_skeleton(plan_path: Path, plan: dict, override) -> Path:
	if override:
		skeleton = Path(override).expanduser().resolve()
	else:
		layout = plan.get("layout")
		if not layout:
			fail("Plan has no layout; pass --skeleton")
		skeleton = (plan_path.parent / layout).resolve()
	if not skeleton.is_file():
		fail(f"Skeleton not found: {skeleton}")
	return skeleton


def render_plan_file(
	plan_path: Path, client_dir: Path, skeleton: Path, output: Path
) -> dict:
	binary = resolve_render_binary()
	if not binary.is_file():
		fail(
			f"lily-render not found: {binary}\n"
			"Build it with: cargo build --bin lily-render (from apps/desktop/backend)"
		)
	command = [
		str(binary),
		"--plan",
		str(plan_path),
		"--client-dir",
		str(client_dir),
		"--skeleton",
		str(skeleton),
		"--output",
		str(output),
		"--json",
	]
	result = subprocess.run(command, capture_output=True, text=True)
	if result.returncode != 0:
		fail(result.stderr.strip() or f"lily-render exited {result.returncode}")
	try:
		return json.loads(result.stdout)
	except json.JSONDecodeError:
		return {"output": str(output), "warnings": []}


def document_diff(left: Path, right: Path) -> str:
	left_lines, left_meta = extract_document_text(left)
	right_lines, right_meta = extract_document_text(right)
	return "".join(
		difflib.unified_diff(
			render_diff_snapshot(left_lines, left_meta).splitlines(keepends=True),
			render_diff_snapshot(right_lines, right_meta).splitlines(keepends=True),
			fromfile=str(left),
			tofile=str(right),
		)
	)


def command_draft(args) -> None:
	plan_path = Path(args.plan).expanduser().resolve()
	if not plan_path.is_file():
		fail(f"Plan not found: {plan_path}")
	plan = read_json(plan_path)
	client_path, client = find_client_lily(Path(args.client_dir).expanduser().resolve())
	variables, lists = audit_variables(client)
	roles = client_role_names(client)
	audit = audit_plan(
		expand_excerpts(plan, plan_path.parent, variables, lists),
		variables,
		lists,
		roles,
	)
	skeleton = plan_skeleton(plan_path, plan, args.skeleton)

	output_dir = Path(args.output_dir).expanduser().resolve()
	output_dir.mkdir(parents=True, exist_ok=True)
	candidate = output_dir / "candidate.docx"
	render = render_plan_file(plan_path, client_path.parent, skeleton, candidate)

	previous = None
	if args.previous:
		previous = Path(args.previous).expanduser().resolve()
		if not previous.is_file():
			fail(f"Previous document not found: {previous}")
	diff = document_diff(previous, candidate) if previous else ""

	audit_path = output_dir / "audit.json"
	render_path = output_dir / "render.json"
	diff_path = output_dir / "diff.txt"
	audit_path.write_text(json.dumps(audit, indent=2) + "\n")
	render_path.write_text(json.dumps(render, indent=2) + "\n")
	diff_path.write_text(diff)

	flags: list[str] = []
	for entry in audit["slots"]["empty"]:
		if entry["required"]:
			flags.append(f"Required slot missing: {entry['name']}")
		elif entry["kind"] == "missing":
			flags.append(f"Optional slot empty: {entry['name']}")
	for condition in audit["conditions"]:
		if condition["result"] is None:
			flags.append(
				"Condition has an unknown value: "
				f"{condition['where']} {json.dumps(condition['condition'], ensure_ascii=False)}"
			)
	for role in audit["missing_roles"]:
		flags.append(f"Role has no assignment: {role}")
	for warning in render.get("warnings", []):
		flags.append(f"Render warning: {warning}")
	changed = sum(
		1
		for line in diff.splitlines()
		if line.startswith(("+", "-")) and not line.startswith(("+++", "---"))
	)
	if previous:
		flags.append(f"{changed} changed line(s) against {previous.name}")

	review_lines = [
		f"# Draft Review — {plan.get('id', 'plan')}",
		"",
		f"- Client: `{client_path}`",
		f"- Plan: `{plan_path}`",
		f"- Candidate: `{candidate}`",
		f"- Previous: `{previous}`" if previous else "- Previous: none",
		"",
		"## Data Gaps",
		"",
	]
	required = [entry for entry in audit["slots"]["empty"] if entry["required"]]
	gaps = [
		entry
		for entry in audit["slots"]["empty"]
		if not entry["required"] and entry["kind"] == "missing"
	]
	for entry in required:
		review_lines.append(f"- Required: {entry['name']} ({entry['kind']})")
	for entry in gaps:
		review_lines.append(f"- Optional: {entry['name']}")
	for role in audit["missing_roles"]:
		review_lines.append(f"- Role: {role} (no assignment)")
	if not required and not gaps and not audit["missing_roles"]:
		review_lines.append("None.")
	computed = [
		entry for entry in audit["slots"]["empty"] if entry["kind"] == "computed"
	]
	review_lines += ["", "## Computed Values", ""]
	if computed:
		for entry in computed:
			review_lines.append(f"- {entry['name']} (verify after render)")
	else:
		review_lines.append("None.")
	review_lines += ["", "## Conditions", ""]
	if audit["conditions"]:
		for condition in audit["conditions"]:
			result = {True: "true", False: "false", None: "unknown"}[condition["result"]]
			review_lines.append(
				f"- `{condition['where']}`: "
				f"`{json.dumps(condition['condition'], ensure_ascii=False)}` -> **{result}**"
			)
	else:
		review_lines.append("None.")
	review_lines += ["", "## Render", ""]
	if render.get("blocks_rendered") is not None:
		review_lines.append(f"- Blocks rendered: {render['blocks_rendered']}")
	if render.get("warnings"):
		for warning in render["warnings"]:
			review_lines.append(f"- Warning: {warning}")
	else:
		review_lines.append("- No warnings.")
	review_lines += ["", "## Diff", ""]
	if previous and diff:
		review_lines += ["```diff", diff.rstrip("\n"), "```"]
	elif previous:
		review_lines.append(f"No textual differences against `{previous}`.")
	else:
		review_lines.append("No previous document supplied.")
	review_lines += ["", "## Flags", ""]
	if flags:
		for flag in flags:
			review_lines.append(f"- {flag}")
	else:
		review_lines.append("None.")
	review_path = output_dir / "review.md"
	review_path.write_text("\n".join(review_lines) + "\n")

	summary = {
		"plan": str(plan_path),
		"client": str(client_path),
		"candidate": str(candidate),
		"review": str(review_path),
		"audit": str(audit_path),
		"render": str(render_path),
		"diff": str(diff_path) if previous else None,
		"flags": flags,
		"render_warnings": render.get("warnings", []),
	}
	if args.json:
		print(json.dumps(summary, indent=2))
	else:
		print(f"Draft review: {review_path}")
		for flag in flags:
			print(f"  - {flag}")


def command_layout(args) -> None:
	docx_path = Path(args.docx).expanduser().resolve()
	if not docx_path.is_file():
		fail(f"Document not found: {docx_path}")
	output_dir = Path(args.output_dir).expanduser().resolve()
	output_dir.mkdir(parents=True, exist_ok=True)
	soffice = shutil.which("soffice") or shutil.which("libreoffice")
	if not soffice:
		fail("LibreOffice (soffice) is not on PATH")
	pdftoppm = shutil.which("pdftoppm")
	if not pdftoppm:
		fail("pdftoppm is not on PATH")
	profile = output_dir / "lo-profile"
	profile.mkdir(exist_ok=True)
	convert = subprocess.run(
		[
			soffice,
			"--headless",
			f"-env:UserInstallation={profile.as_uri()}",
			"--convert-to",
			"pdf",
			"--outdir",
			str(output_dir),
			str(docx_path),
		],
		capture_output=True,
		text=True,
	)
	if convert.returncode != 0:
		fail(convert.stderr.strip() or f"LibreOffice exited {convert.returncode}")
	pdf_path = output_dir / f"{docx_path.stem}.pdf"
	if not pdf_path.is_file():
		fail(f"LibreOffice produced no PDF for {docx_path}")
	for old in output_dir.glob("page-*.png"):
		old.unlink()
	prefix = output_dir / "page"
	render = subprocess.run(
		[pdftoppm, "-png", "-r", str(args.dpi), str(pdf_path), str(prefix)],
		capture_output=True,
		text=True,
	)
	if render.returncode != 0:
		fail(render.stderr.strip() or f"pdftoppm exited {render.returncode}")
	images = sorted(
		output_dir.glob("page-*.png"),
		key=lambda path: int(path.stem.rsplit("-", 1)[-1]),
	)
	report = {
		"docx": str(docx_path),
		"pdf": str(pdf_path),
		"pages": len(images),
		"images": [str(path) for path in images],
	}
	if args.json:
		print(json.dumps(report, indent=2))
	else:
		print(f"Rendered {len(images)} page(s) from {docx_path.name}")
		print(f"PDF: {pdf_path}")
		for image in images:
			print(image)


def command_plan_render(args) -> None:
	binary = resolve_render_binary()
	if not binary.is_file():
		fail(
			f"lily-render not found: {binary}\n"
			"Build it with: cargo build --bin lily-render (from apps/desktop/backend)"
		)

	plan_path = Path(args.plan).expanduser().resolve()
	if not plan_path.is_file():
		fail(f"Plan not found: {plan_path}")
	if args.skeleton:
		skeleton = str(Path(args.skeleton).expanduser().resolve())
	else:
		plan = read_json(plan_path)
		layout = plan.get("layout")
		if not layout:
			fail("Plan has no layout; pass --skeleton")
		skeleton = str((plan_path.parent / layout).resolve())

	command = [
		str(binary),
		"--plan",
		str(plan_path),
		"--client-dir",
		str(Path(args.client_dir).expanduser().resolve()),
		"--skeleton",
		skeleton,
		"--output",
		str(Path(args.output).expanduser().resolve()),
	]
	if args.json:
		command.append("--json")
	result = subprocess.run(command)
	raise SystemExit(result.returncode)


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
	render_binary = resolve_render_binary()
	corpus = Path(os.environ.get("LILY_CORPUS", "~/Carelaw/carelaw-lily/Tests")).expanduser()
	templates = Path(
		os.environ.get("LILY_TEMPLATES", "~/Carelaw/carelaw-lily/Templates")
	).expanduser()

	checks = {
		"python": {"version": sys.version.split()[0], "ok": sys.version_info >= (3, 10)},
		"lily_fill": {"path": str(fill_binary), "ok": fill_binary.is_file()},
		"lily_render": {"path": str(render_binary), "ok": render_binary.is_file()},
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

	plan = subparsers.add_parser("plan", help="Validate and render document plans")
	plan_sub = plan.add_subparsers(dest="plan_command", required=True)
	plan_render = plan_sub.add_parser("render", help="Render a document plan to .docx")
	plan_render.add_argument("--plan", required=True, help="Path to the plan JSON")
	plan_render.add_argument("--client-dir", required=True, help="Client folder containing the .lily file")
	plan_render.add_argument(
		"--skeleton",
		help="Layout skeleton .docx; defaults to the plan's layout path",
	)
	plan_render.add_argument("--output", required=True, help="Output .docx path")
	plan_render.add_argument("--json", action="store_true", help="Print the render report as JSON")
	plan_render.set_defaults(func=command_plan_render)
	plan_audit = plan_sub.add_parser(
		"audit",
		help="Report slot coverage, empty values, and condition results for a client",
	)
	plan_audit.add_argument("--plan", required=True, help="Path to the plan JSON")
	plan_audit.add_argument("--client-dir", required=True, help="Client folder containing the .lily file")
	plan_audit.add_argument("--json", action="store_true", help="Print the audit report as JSON")
	plan_audit.set_defaults(func=command_plan_audit)

	draft = subparsers.add_parser(
		"draft", help="Produce a draft review bundle: audit, candidate, diff, and flags"
	)
	draft.add_argument("--plan", required=True, help="Path to the plan JSON")
	draft.add_argument("--client-dir", required=True, help="Client folder containing the .lily file")
	draft.add_argument(
		"--output-dir", required=True, help="Directory for candidate.docx and the review files"
	)
	draft.add_argument("--previous", help="Existing document to diff against")
	draft.add_argument(
		"--skeleton", help="Layout skeleton .docx; defaults to the plan's layout path"
	)
	draft.add_argument("--json", action="store_true", help="Print the bundle summary as JSON")
	draft.set_defaults(func=command_draft)

	layout = subparsers.add_parser(
		"layout", help="Render a document to PDF pages for layout review"
	)
	layout.add_argument("--docx", required=True, help="Document to render")
	layout.add_argument(
		"--output-dir", required=True, help="Directory for the PDF and page images"
	)
	layout.add_argument("--dpi", type=int, default=80, help="Page image resolution")
	layout.add_argument("--json", action="store_true", help="Print the render report as JSON")
	layout.set_defaults(func=command_layout)

	compile_command = subparsers.add_parser(
		"compile", help="Compile a template into a document plan"
	)
	compile_command.add_argument("--templates-dir", required=True)
	compile_command.add_argument("--template-rel-path", required=True)
	compile_command.add_argument("--output", required=True)
	compile_command.add_argument("--json", action="store_true")
	compile_command.add_argument(
		"--allow-missing-schema",
		action="store_true",
		help="Compile a template that has no template-library entry as literal text",
	)
	compile_command.set_defaults(func=command_compile)

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
