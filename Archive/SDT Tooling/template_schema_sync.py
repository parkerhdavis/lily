#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Synchronize a .lily template schema sidecar with the DOCX template's
actual SDT inventory.

Operations:
- Generate a new schema from scratch if none exists
- Update an existing schema: add missing variables, flag removed ones
- Preserve existing metadata (help text, required, defaults) on update

Usage:
	python template_schema_sync.py <template.docx> [options]

Options:
	--dry-run      Show what would change without writing
	--json         Output result as JSON
	--overwrite    Overwrite existing schema entries (default: preserve existing metadata)
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


def sync_schema(
	docx_path: str,
	dry_run: bool = False,
	overwrite: bool = False,
) -> dict:
	"""
	Synchronize the .lily schema sidecar with the template's SDT inventory.

	Returns a report dict with:
	- added: variables added to schema
	- removed: variables in schema but not in document (flagged, not deleted)
	- updated: variables with changed type (conditional ↔ non-conditional)
	- unchanged: variables that already match
	- schema: the resulting schema
	"""
	parts = docx_utils.read_docx_xml_parts(docx_path)

	# Collect all SDTs and bookmarks across all parts
	all_sdts = []
	all_bookmarks = []

	for part_name, xml in parts.items():
		normalized = docx_utils.normalize_split_variables(xml)
		all_sdts.extend(docx_utils.find_sdts(normalized, part_name))
		all_bookmarks.extend(docx_utils.find_bookmarks(normalized, part_name))

	# Build document variable inventory
	doc_vars: dict[str, dict] = {}  # variable_name → info

	for sdt in all_sdts:
		name = sdt.variable_name
		if name not in doc_vars:
			doc_vars[name] = {
				"is_conditional": sdt.is_conditional,
				"occurrences": 0,
			}
		doc_vars[name]["occurrences"] += 1
		if sdt.is_conditional:
			doc_vars[name]["is_conditional"] = True

	for bm in all_bookmarks:
		name = bm.variable_name
		if name not in doc_vars:
			doc_vars[name] = {
				"is_conditional": bm.is_conditional,
				"occurrences": 0,
			}
		doc_vars[name]["occurrences"] += 1
		if bm.is_conditional:
			doc_vars[name]["is_conditional"] = True

	# Load existing schema
	existing_schema = docx_utils.load_template_schema(docx_path)
	existing_vars = existing_schema.get("variables", {})

	# Build new schema
	template_filename = Path(docx_path).name
	new_schema = {
		"lily_type": "template-schema",
		"template_filename": template_filename,
		"variables": {},
	}

	added = []
	updated = []
	unchanged = []
	removed = []

	# Process document variables
	for var_name, info in doc_vars.items():
		is_cond = info["is_conditional"]
		var_type = "conditional" if is_cond else "text"

		if var_name in existing_vars and not overwrite:
			existing = existing_vars[var_name]
			existing_type = existing.get("var_type", "text")

			# Check if conditional status changed
			if (existing_type == "conditional") != is_cond:
				entry = dict(existing)
				entry["var_type"] = var_type
				new_schema["variables"][var_name] = entry
				updated.append({
					"variable": var_name,
					"old_type": existing_type,
					"new_type": var_type,
				})
			else:
				# Preserve existing entry as-is
				new_schema["variables"][var_name] = dict(existing)
				unchanged.append(var_name)
		else:
			# New variable or overwrite mode
			entry = {"var_type": var_type}

			# Check for contact-role dot notation
			ref = docx_utils.parse_contact_role_ref(var_name)
			if ref:
				role, prop = ref
				entry["contact_role"] = role
				entry["contact_property"] = prop

			# If overwrite and existing entry has metadata, we still start fresh
			# but preserve conditional defs if they exist
			if var_name in existing_vars:
				old = existing_vars[var_name]
				if is_cond:
					if old.get("condition"):
						entry["condition"] = old["condition"]
					if old.get("conditions"):
						entry["conditions"] = old["conditions"]

			new_schema["variables"][var_name] = entry
			if var_name in existing_vars:
				updated.append({
					"variable": var_name,
					"reason": "overwrite",
				})
			else:
				added.append(var_name)

	# Check for variables in schema but not in document
	for var_name in existing_vars:
		if var_name not in doc_vars:
			removed.append(var_name)
			# Don't include removed vars in new schema

	if not dry_run:
		schema_path = docx_utils.save_template_schema(docx_path, new_schema)
	else:
		schema_path = str(docx_utils.schema_path_for_template(docx_path))

	return {
		"success": True,
		"schema_path": str(schema_path),
		"dry_run": dry_run,
		"added": added,
		"updated": updated,
		"removed": removed,
		"unchanged": unchanged,
		"schema": new_schema,
	}


def main():
	parser = argparse.ArgumentParser(
		description="Synchronize .lily schema sidecar with template SDT inventory.",
	)
	parser.add_argument("docx_path", help="Path to the .docx template file")
	parser.add_argument("--dry-run", action="store_true", help="Show what would change without writing")
	parser.add_argument("--json", action="store_true", help="Output as JSON")
	parser.add_argument("--overwrite", action="store_true", help="Overwrite existing schema entries")
	args = parser.parse_args()

	if not Path(args.docx_path).exists():
		print(f"Error: File not found: {args.docx_path}", file=sys.stderr)
		sys.exit(1)

	result = sync_schema(
		docx_path=args.docx_path,
		dry_run=args.dry_run,
		overwrite=args.overwrite,
	)

	if args.json:
		print(json.dumps(result, indent="\t"))
	else:
		action = "Would write" if result["dry_run"] else "Wrote"
		print(f"{action} schema to: {result['schema_path']}")
		print()

		if result["added"]:
			print(f"Added ({len(result['added'])}):")
			for name in result["added"]:
				print(f"  + {name}")

		if result["updated"]:
			print(f"Updated ({len(result['updated'])}):")
			for entry in result["updated"]:
				if "old_type" in entry:
					print(f"  ~ {entry['variable']}: {entry['old_type']} → {entry['new_type']}")
				else:
					print(f"  ~ {entry['variable']}: {entry.get('reason', 'updated')}")

		if result["removed"]:
			print(f"Removed from schema ({len(result['removed'])}):")
			for name in result["removed"]:
				print(f"  - {name}")

		if result["unchanged"]:
			print(f"Unchanged: {len(result['unchanged'])} variable(s)")

		total = len(result["added"]) + len(result["updated"]) + len(result["removed"])
		if total == 0:
			print("Schema is already in sync.")


if __name__ == "__main__":
	main()
