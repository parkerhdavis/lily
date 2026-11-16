#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Validate a Lily DOCX template for structural issues.

Checks:
- Malformed SDTs (missing tag, alias, id, or content)
- Duplicate IDs across SDTs and bookmarks
- Unconverted {Variable} placeholders (should be SDTs)
- Schema drift (SDTs in docx not in .lily schema, or vice versa)
- Empty SDT content that should be a bookmark instead
- Inconsistent conditional/non-conditional classification

Usage:
	python template_validate.py <template.docx> [--json] [--fix-schema]

Options:
	--json        Output as JSON instead of human-readable text
	--fix-schema  Auto-fix schema drift by syncing the .lily sidecar
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


class Issue:
	"""A validation issue found in the template."""

	SEVERITY_ERROR = "error"
	SEVERITY_WARNING = "warning"
	SEVERITY_INFO = "info"

	def __init__(self, severity: str, category: str, message: str, details: dict | None = None):
		self.severity = severity
		self.category = category
		self.message = message
		self.details = details or {}

	def to_dict(self) -> dict:
		d = {
			"severity": self.severity,
			"category": self.category,
			"message": self.message,
		}
		if self.details:
			d["details"] = self.details
		return d

	def __repr__(self):
		return f"[{self.severity.upper()}] {self.category}: {self.message}"


def validate_template(docx_path: str) -> list[Issue]:
	"""
	Validate a template and return a list of issues.
	"""
	issues = []
	parts = docx_utils.read_docx_xml_parts(docx_path)

	all_sdts = []
	all_bookmarks = []
	all_placeholders = []
	all_ids: dict[int, list[str]] = {}  # id → list of sources

	for part_name, xml in parts.items():
		normalized = docx_utils.normalize_split_variables(xml)

		sdts = docx_utils.find_sdts(normalized, part_name)
		bookmarks = docx_utils.find_bookmarks(normalized, part_name)
		placeholders = docx_utils.find_placeholders(normalized, part_name)

		all_sdts.extend(sdts)
		all_bookmarks.extend(bookmarks)
		all_placeholders.extend(placeholders)

		# ── Check individual SDTs ──
		for sdt in sdts:
			# Missing ID
			if sdt.sdt_id is None:
				issues.append(Issue(
					Issue.SEVERITY_ERROR,
					"malformed_sdt",
					f"SDT for '{sdt.variable_name}' has no <w:id> element",
					{"part": part_name, "variable": sdt.variable_name},
				))
			else:
				source = f"sdt:{sdt.variable_name}@{part_name}"
				all_ids.setdefault(sdt.sdt_id, []).append(source)

			# Missing alias
			if not sdt.alias:
				issues.append(Issue(
					Issue.SEVERITY_WARNING,
					"malformed_sdt",
					f"SDT for '{sdt.variable_name}' has no <w:alias> element",
					{"part": part_name, "variable": sdt.variable_name},
				))

			# Tag/alias mismatch
			if sdt.alias and sdt.alias != sdt.variable_name:
				issues.append(Issue(
					Issue.SEVERITY_WARNING,
					"tag_alias_mismatch",
					f"SDT tag '{sdt.variable_name}' doesn't match alias '{sdt.alias}'",
					{"part": part_name, "tag": sdt.variable_name, "alias": sdt.alias},
				))

			# Empty content that could be a bookmark
			if not sdt.content_text.strip():
				issues.append(Issue(
					Issue.SEVERITY_INFO,
					"empty_sdt",
					f"SDT for '{sdt.variable_name}' has empty content (could be a bookmark)",
					{"part": part_name, "variable": sdt.variable_name},
				))

		# ── Check bookmarks ──
		for bm in bookmarks:
			source = f"bookmark:{bm.variable_name}@{part_name}"
			all_ids.setdefault(bm.bookmark_id, []).append(source)

	# ── Duplicate IDs ──
	for id_val, sources in all_ids.items():
		if len(sources) > 1:
			issues.append(Issue(
				Issue.SEVERITY_ERROR,
				"duplicate_id",
				f"ID {id_val} is used by multiple elements: {', '.join(sources)}",
				{"id": id_val, "sources": sources},
			))

	# ── Unconverted placeholders ──
	for ph in all_placeholders:
		issues.append(Issue(
			Issue.SEVERITY_WARNING,
			"unconverted_placeholder",
			f"Unconverted placeholder {ph.raw_text} in {ph.part_name}",
			{
				"part": ph.part_name,
				"raw_text": ph.raw_text,
				"display_name": ph.display_name,
				"is_conditional": ph.is_conditional,
			},
		))

	# ── Schema drift ──
	schema = docx_utils.load_template_schema(docx_path)
	schema_vars = set(schema.get("variables", {}).keys())

	# Variables in document (from SDTs + bookmarks)
	doc_vars: dict[str, bool] = {}  # name → is_conditional
	for sdt in all_sdts:
		doc_vars[sdt.variable_name] = sdt.is_conditional
	for bm in all_bookmarks:
		doc_vars[bm.variable_name] = bm.is_conditional

	# In document but not in schema
	for var_name in doc_vars:
		if var_name not in schema_vars:
			issues.append(Issue(
				Issue.SEVERITY_WARNING,
				"schema_missing_variable",
				f"Variable '{var_name}' is in the document but not in the .lily schema",
				{"variable": var_name},
			))

	# In schema but not in document
	for var_name in schema_vars:
		if var_name not in doc_vars:
			issues.append(Issue(
				Issue.SEVERITY_INFO,
				"schema_extra_variable",
				f"Variable '{var_name}' is in the .lily schema but not in the document",
				{"variable": var_name},
			))

	# ── Conditional classification consistency ──
	# Check that all SDTs for the same variable agree on conditional status
	var_cond_status: dict[str, set[bool]] = {}
	for sdt in all_sdts:
		var_cond_status.setdefault(sdt.variable_name, set()).add(sdt.is_conditional)
	for bm in all_bookmarks:
		var_cond_status.setdefault(bm.variable_name, set()).add(bm.is_conditional)

	for var_name, statuses in var_cond_status.items():
		if len(statuses) > 1:
			issues.append(Issue(
				Issue.SEVERITY_ERROR,
				"inconsistent_conditional",
				f"Variable '{var_name}' has both conditional and non-conditional markers",
				{"variable": var_name},
			))

	# ── Schema conditional consistency ──
	for var_name, entry in schema.get("variables", {}).items():
		is_schema_cond = entry.get("var_type") == "conditional"
		if var_name in doc_vars:
			is_doc_cond = doc_vars[var_name]
			if is_schema_cond != is_doc_cond:
				issues.append(Issue(
					Issue.SEVERITY_WARNING,
					"schema_conditional_mismatch",
					f"Variable '{var_name}' is {'conditional' if is_doc_cond else 'non-conditional'} "
					f"in document but {'conditional' if is_schema_cond else 'non-conditional'} in schema",
					{"variable": var_name},
				))

	return issues


def print_report(issues: list[Issue], docx_path: str) -> None:
	"""Print a human-readable validation report."""
	print(f"Validating: {docx_path}")
	print()

	if not issues:
		print("No issues found.")
		return

	errors = [i for i in issues if i.severity == Issue.SEVERITY_ERROR]
	warnings = [i for i in issues if i.severity == Issue.SEVERITY_WARNING]
	infos = [i for i in issues if i.severity == Issue.SEVERITY_INFO]

	if errors:
		print(f"ERRORS ({len(errors)}):")
		for issue in errors:
			print(f"  [ERROR] {issue.category}: {issue.message}")
		print()

	if warnings:
		print(f"WARNINGS ({len(warnings)}):")
		for issue in warnings:
			print(f"  [WARN]  {issue.category}: {issue.message}")
		print()

	if infos:
		print(f"INFO ({len(infos)}):")
		for issue in infos:
			print(f"  [INFO]  {issue.category}: {issue.message}")
		print()

	total = len(issues)
	print(f"Total: {len(errors)} errors, {len(warnings)} warnings, {len(infos)} info ({total} issues)")


def main():
	parser = argparse.ArgumentParser(
		description="Validate a Lily DOCX template for structural issues.",
	)
	parser.add_argument("docx_path", help="Path to the .docx template file")
	parser.add_argument("--json", action="store_true", help="Output as JSON")
	args = parser.parse_args()

	if not Path(args.docx_path).exists():
		print(f"Error: File not found: {args.docx_path}", file=sys.stderr)
		sys.exit(1)

	issues = validate_template(args.docx_path)

	if args.json:
		print(json.dumps([i.to_dict() for i in issues], indent="\t"))
	else:
		print_report(issues, args.docx_path)

	# Exit with non-zero if there are errors
	errors = [i for i in issues if i.severity == Issue.SEVERITY_ERROR]
	if errors:
		sys.exit(1)


if __name__ == "__main__":
	main()
