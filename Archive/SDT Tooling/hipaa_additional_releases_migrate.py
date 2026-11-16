#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
One-shot migration for the HIPAA template: add an "Additional HIPAA Releases"
list element.

Inserts a new numbered list item immediately after the Tertiary HPOA Agent
item, holding a conditional SDT (lily-cond:Has Additional HIPAA Releases). The
new paragraph is a structural clone of the Tertiary HPOA Agent list item, so it
inherits the same list numbering (numId=3), bold run properties, and
indentation. Its schema true_template ("{ADDITIONAL HIPAA RELEASES};") resolves
to the bold, capitalized, semicolon-separated list of additional release
contacts with a trailing semicolon; the false branch is "" so the line
self-prunes when no additional releases are selected.

The matching schema entries (Additional HIPAA Releases + Has Additional HIPAA
Releases) live in the template-library sidecar (Estate Planning Templates.lily).

Usage:
	python hipaa_additional_releases_migrate.py <HIPAA Template.docx> [--dry-run]
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils

OLD_TAG = "lily-cond:Has Tertiary HPOA Agent"
OLD_LABEL = "Has Tertiary HPOA Agent"
NEW_LABEL = "Has Additional HIPAA Releases"
NEW_TAG = f"lily-cond:{NEW_LABEL}"


def build_new_paragraph(tertiary_p: str, new_id: int) -> str:
	"""Clone the Tertiary list-item paragraph as the Additional HIPAA Releases
	conditional, with a fresh SDT id and the new tag/alias/placeholder text."""
	p = tertiary_p
	p = p.replace(
		f'<w:tag w:val="{OLD_TAG}"/>',
		f'<w:tag w:val="{NEW_TAG}"/>',
	)
	p = p.replace(
		f'<w:alias w:val="{OLD_LABEL}"/>',
		f'<w:alias w:val="{NEW_LABEL}"/>',
	)
	p = p.replace(
		f'<w:t xml:space="preserve">{OLD_LABEL}</w:t>',
		f'<w:t xml:space="preserve">{NEW_LABEL}</w:t>',
	)
	# Give the cloned SDT a fresh, unique id (the clone has exactly one SDT).
	p = re.sub(r'<w:id w:val="\d+"/>', f'<w:id w:val="{new_id}"/>', p, count=1)
	return p


def migrate(docx_path: str, dry_run: bool = False) -> dict:
	entries = docx_utils.read_docx_entries(docx_path)
	new_id = docx_utils.find_max_id_across_parts(entries) + 1

	out_entries: list[tuple[str, bytes]] = []
	changed = False
	for name, data in entries:
		if name != "word/document.xml":
			out_entries.append((name, data))
			continue
		xml = data.decode("utf-8")
		if NEW_TAG in xml:
			raise SystemExit(f"'{NEW_LABEL}' SDT already present — nothing to do.")
		tag_pos = xml.find(f'w:tag w:val="{OLD_TAG}"')
		if tag_pos < 0:
			raise SystemExit(f"Could not find the Tertiary HPOA Agent SDT ({OLD_TAG}).")
		# Expand to the enclosing <w:p>...</w:p>.
		p_open = list(re.finditer(r"<w:p\b[^>]*>", xml[:tag_pos]))[-1].start()
		p_close = xml.index("</w:p>", tag_pos) + len("</w:p>")
		new_p = build_new_paragraph(xml[p_open:p_close], new_id)
		xml = xml[:p_close] + new_p + xml[p_close:]
		out_entries.append((name, xml.encode("utf-8")))
		changed = True

	if not changed:
		raise SystemExit("word/document.xml not found in docx.")
	if not dry_run:
		docx_utils.write_docx(docx_path, out_entries)
	return {"new_id": new_id, "new_tag": NEW_TAG}


if __name__ == "__main__":
	positional = [a for a in sys.argv[1:] if not a.startswith("--")]
	dry = "--dry-run" in sys.argv
	if len(positional) != 1:
		print(__doc__)
		sys.exit(2)
	result = migrate(positional[0], dry_run=dry)
	prefix = "[dry-run] " if dry else ""
	print(f"{prefix}Inserted {result['new_tag']} as SDT id={result['new_id']}")
