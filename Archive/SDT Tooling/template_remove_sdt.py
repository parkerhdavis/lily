#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Remove a Lily SDT content control from a DOCX template, replacing it
with plain text.

Usage:
	python template_remove_sdt.py <template.docx> <variable_name> <replacement_text> [options]

Options:
	--occurrence INDEX    Remove only the Nth occurrence (0-based)
	--dry-run             Show what would change without writing
	--json                Output result as JSON
"""

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


def remove_sdt(
	docx_path: str,
	variable_name: str,
	replacement_text: str,
	occurrence_index: int | None = None,
	dry_run: bool = False,
) -> dict:
	"""
	Remove SDTs for a variable and replace with plain text.

	Matches the Rust remove_template_variable() implementation:
	1. Read all ZIP entries
	2. For each variable-bearing XML part:
	   a. Find SDTs with matching lily: or lily-cond: tag
	   b. Extract run properties from the SDT
	   c. Replace SDT with a plain <w:r> containing the text
	3. Write back to DOCX

	Returns a report dict with results.
	"""
	entries = docx_utils.read_docx_entries(docx_path)
	escaped_name = docx_utils.escape_xml_text(variable_name)

	# Build regex to match SDTs with this variable name
	sdt_pattern = (
		r'(?s)<w:sdt>(.*?<w:tag\s+w:val="(?:lily:|lily-cond:)'
		+ re.escape(escaped_name)
		+ r'".*?)</w:sdt>'
	)
	sdt_re = re.compile(sdt_pattern)
	rpr_re = re.compile(r'(?s)<w:rPr>(.*?)</w:rPr>')

	total_removed = 0
	total_found = 0
	modified_parts = []

	for i, (name, content) in enumerate(entries):
		if not docx_utils.is_variable_part(name):
			continue

		xml_str = content.decode("utf-8")
		match_count = len(sdt_re.findall(xml_str))

		if match_count == 0:
			continue

		total_found += match_count

		if dry_run:
			total_removed += match_count if occurrence_index is None else min(1, match_count)
			modified_parts.append(name)
			continue

		replace_all_occ = occurrence_index is None
		escaped_replacement = docx_utils.escape_xml_text(replacement_text)
		occurrence = [0]  # mutable counter for closure

		def replace_fn(m):
			should_replace = replace_all_occ or occurrence_index == occurrence[0]
			occurrence[0] += 1
			if not should_replace:
				return m.group(0)

			# Extract run properties from SDT content
			inner = m.group(1)
			rpr_m = rpr_re.search(inner)
			rpr = rpr_m.group(0) if rpr_m else ""

			return f'<w:r>{rpr}<w:t xml:space="preserve">{escaped_replacement}</w:t></w:r>'

		modified = sdt_re.sub(replace_fn, xml_str)
		actual_removed = occurrence[0] if replace_all_occ else min(1, occurrence[0])
		total_removed += actual_removed

		entries[i] = (name, modified.encode("utf-8"))
		modified_parts.append(name)

	if total_found == 0:
		return {
			"success": False,
			"error": f"No SDTs found for variable '{variable_name}'",
		}

	if not dry_run:
		docx_utils.write_docx(docx_path, entries)

	variables = docx_utils.extract_variables(docx_path) if not dry_run else []

	return {
		"success": True,
		"removed": total_removed,
		"found": total_found,
		"modified_parts": modified_parts,
		"dry_run": dry_run,
		"variables": [v.to_dict() for v in variables],
	}


def main():
	parser = argparse.ArgumentParser(
		description="Remove a Lily SDT from a DOCX template and replace with text.",
	)
	parser.add_argument("docx_path", help="Path to the .docx template file")
	parser.add_argument("variable_name", help="Variable name of the SDT to remove")
	parser.add_argument("replacement_text", help="Text to put in place of the SDT")
	parser.add_argument("--occurrence", type=int, default=None, help="Remove only the Nth occurrence (0-based)")
	parser.add_argument("--dry-run", action="store_true", help="Show what would change without writing")
	parser.add_argument("--json", action="store_true", help="Output as JSON")
	args = parser.parse_args()

	if not Path(args.docx_path).exists():
		print(f"Error: File not found: {args.docx_path}", file=sys.stderr)
		sys.exit(1)

	result = remove_sdt(
		docx_path=args.docx_path,
		variable_name=args.variable_name,
		replacement_text=args.replacement_text,
		occurrence_index=args.occurrence,
		dry_run=args.dry_run,
	)

	if args.json:
		print(json.dumps(result, indent="\t"))
	else:
		if result["success"]:
			action = "Would remove" if result.get("dry_run") else "Removed"
			print(f"{action} {result['removed']} SDT(s) for '{args.variable_name}', replaced with '{args.replacement_text}'")
			if result.get("modified_parts"):
				print(f"  Modified parts: {', '.join(result['modified_parts'])}")
			if result.get("variables"):
				print(f"\nRemaining variables ({len(result['variables'])}):")
				for var in result["variables"]:
					kind = "COND" if var["is_conditional"] else "REPL"
					print(f"  [{kind}] {var['display_name']}")
		else:
			print(f"Error: {result['error']}", file=sys.stderr)
			sys.exit(1)


if __name__ == "__main__":
	main()
