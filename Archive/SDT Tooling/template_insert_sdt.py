#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Insert a Lily SDT content control into a DOCX template.

Replaces selected text with a properly-formed SDT, handling:
- Unique ID generation (scans all XML parts)
- Run properties preservation
- Multiple occurrence disambiguation
- Split variable normalization

Usage:
	python template_insert_sdt.py <template.docx> <search_text> <variable_name> [options]

Options:
	--occurrence INDEX    Replace only the Nth occurrence (0-based)
	--replace-all         Replace all occurrences
	--conditional         Mark as a conditional variable (lily-cond: prefix)
	--dry-run             Show what would change without writing
	--json                Output result as JSON
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


def insert_sdt(
	docx_path: str,
	search_text: str,
	variable_name: str,
	occurrence_index: int | None = None,
	replace_all: bool = False,
	is_conditional: bool = False,
	dry_run: bool = False,
) -> dict:
	"""
	Insert an SDT at occurrences of search_text in the template.

	Matches the Rust insert_template_variable() implementation:
	1. Read all ZIP entries
	2. Find max ID across all XML parts
	3. For each variable-bearing XML part:
	   a. Normalize split variables
	   b. Build paragraph text maps
	   c. Find text occurrences
	   d. Replace text with {Variable Name} placeholder
	   e. Convert placeholder to SDT
	4. Write back to DOCX

	Returns a report dict with results.
	"""
	entries = docx_utils.read_docx_entries(docx_path)
	placeholder = f"{{{variable_name}}}"

	# Find max ID across all XML parts
	next_id = docx_utils.find_max_id_across_parts(entries) + 1

	total_replacements = 0
	occurrences_found = []
	modified_parts = []

	for i, (name, content) in enumerate(entries):
		if not docx_utils.is_variable_part(name):
			continue

		xml_str = content.decode("utf-8")
		normalized = docx_utils.normalize_split_variables(xml_str)
		maps = docx_utils.build_paragraph_text_maps(normalized)
		matches = docx_utils.find_text_in_paragraphs(maps, search_text)

		if not matches:
			continue

		# Record occurrences for reporting
		for idx, (pi, offset) in enumerate(matches):
			para = maps[pi]
			end = offset + len(search_text)
			ctx_start = max(0, offset - 20)
			ctx_end = min(len(para.flat_text), end + 20)
			context = ""
			if ctx_start > 0:
				context += "..."
			context += para.flat_text[ctx_start:ctx_end]
			if ctx_end < len(para.flat_text):
				context += "..."
			occurrences_found.append({
				"index": idx,
				"part": name,
				"paragraph": para.paragraph_number,
				"context": context,
			})

		if len(matches) > 1 and not replace_all and occurrence_index is None:
			return {
				"success": False,
				"error": f'Found {len(matches)} occurrences of "{search_text}". '
					f"Specify --occurrence INDEX or use --replace-all.",
				"occurrences": occurrences_found,
			}

		# Determine which matches to replace
		if replace_all:
			to_replace = matches
		elif occurrence_index is not None:
			if occurrence_index >= len(matches):
				return {
					"success": False,
					"error": f"Occurrence index {occurrence_index} out of range (found {len(matches)})",
					"occurrences": occurrences_found,
				}
			to_replace = [matches[occurrence_index]]
		else:
			to_replace = [matches[0]]

		if dry_run:
			total_replacements += len(to_replace)
			continue

		# Step 1: Replace text with {Variable Name} placeholder
		modified = normalized
		replace_count = len(to_replace)

		for _ in range(replace_count):
			current_maps = docx_utils.build_paragraph_text_maps(modified)
			current_matches = docx_utils.find_text_in_paragraphs(current_maps, search_text)
			if not current_matches:
				break
			# Replace last match first (Rust does this to avoid offset invalidation)
			pi, offset = current_matches[-1]
			modified = docx_utils.replace_text_in_xml(
				modified, current_maps[pi], offset, len(search_text), placeholder,
			)

		# Step 2: Convert placeholder to SDT
		sdt_map = {
			variable_name: (variable_name, variable_name, is_conditional),
		}
		modified, next_id = docx_utils.replace_placeholders_with_sdt(modified, sdt_map, next_id)

		entries[i] = (name, modified.encode("utf-8"))
		total_replacements += replace_count
		modified_parts.append(name)

	if total_replacements == 0:
		return {
			"success": False,
			"error": f'Text "{search_text}" not found in any XML part.',
			"occurrences": [],
		}

	if not dry_run:
		docx_utils.write_docx(docx_path, entries)

	# Return updated variable list
	variables = docx_utils.extract_variables(docx_path) if not dry_run else []

	return {
		"success": True,
		"replacements": total_replacements,
		"modified_parts": modified_parts,
		"dry_run": dry_run,
		"variables": [v.to_dict() for v in variables],
		"occurrences": occurrences_found,
	}


def main():
	parser = argparse.ArgumentParser(
		description="Insert a Lily SDT content control into a DOCX template.",
	)
	parser.add_argument("docx_path", help="Path to the .docx template file")
	parser.add_argument("search_text", help="Text to replace with an SDT")
	parser.add_argument("variable_name", help="Variable name for the SDT (e.g., 'Client Full Name')")
	parser.add_argument("--occurrence", type=int, default=None, help="Replace only the Nth occurrence (0-based)")
	parser.add_argument("--replace-all", action="store_true", help="Replace all occurrences")
	parser.add_argument("--conditional", action="store_true", help="Mark as conditional variable")
	parser.add_argument("--dry-run", action="store_true", help="Show what would change without writing")
	parser.add_argument("--json", action="store_true", help="Output as JSON")
	args = parser.parse_args()

	if not Path(args.docx_path).exists():
		print(f"Error: File not found: {args.docx_path}", file=sys.stderr)
		sys.exit(1)

	result = insert_sdt(
		docx_path=args.docx_path,
		search_text=args.search_text,
		variable_name=args.variable_name,
		occurrence_index=args.occurrence,
		replace_all=args.replace_all,
		is_conditional=args.conditional,
		dry_run=args.dry_run,
	)

	if args.json:
		print(json.dumps(result, indent="\t"))
	else:
		if result["success"]:
			action = "Would replace" if result.get("dry_run") else "Replaced"
			print(f"{action} {result['replacements']} occurrence(s) of search text with SDT '{args.variable_name}'")
			if result.get("modified_parts"):
				print(f"  Modified parts: {', '.join(result['modified_parts'])}")
			if result.get("variables"):
				print(f"\nVariables in template ({len(result['variables'])}):")
				for var in result["variables"]:
					kind = "COND" if var["is_conditional"] else "REPL"
					print(f"  [{kind}] {var['display_name']}")
		else:
			print(f"Error: {result['error']}", file=sys.stderr)
			if result.get("occurrences"):
				print("\nOccurrences found:", file=sys.stderr)
				for occ in result["occurrences"]:
					print(f"  [{occ['index']}] paragraph {occ['paragraph']}: {occ['context']!r}", file=sys.stderr)
			sys.exit(1)


if __name__ == "__main__":
	main()
