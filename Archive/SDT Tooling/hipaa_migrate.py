#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
One-shot migration script for the HIPAA template.

Renames Healthcare POA variables to HPOA naming and adds co-agent
helper variable SDTs. The HIPAA template lists agent names (each
on its own line with semicolons), so co-agent SDTs are inserted
next to each name SDT.
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


def replace_sdt_by_tag(xml: str, old_tag: str, new_sdts_xml: str) -> str:
	"""Find an SDT by its exact tag value and replace it with new XML."""
	tag_str = f'w:tag w:val="{old_tag}"'
	pos = xml.find(tag_str)
	if pos < 0:
		print(f"  WARNING: tag '{old_tag}' not found")
		return xml
	sdt_start = xml.rfind("<w:sdt>", 0, pos)
	sdt_end = xml.index("</w:sdt>", pos) + len("</w:sdt>")
	return xml[:sdt_start] + new_sdts_xml + xml[sdt_end:]


def extract_rpr(xml: str, tag: str) -> str:
	"""Extract the rPr from an SDT's content runs."""
	pos = xml.find(f'w:tag w:val="{tag}"')
	if pos < 0:
		return ""
	sdt_start = xml.rfind("<w:sdt>", 0, pos)
	sdt_end = xml.index("</w:sdt>", pos) + len("</w:sdt>")
	sdt = xml[sdt_start:sdt_end]
	match = re.search(r'<w:sdtContent>.*?(<w:rPr>.*?</w:rPr>)', sdt, re.DOTALL)
	return match.group(1) if match else ""


def migrate(docx_path: str) -> None:
	entries = docx_utils.read_docx_entries(docx_path)
	next_id = docx_utils.find_max_id_across_parts(entries) + 1

	for i, (name, content) in enumerate(entries):
		if name != "word/document.xml":
			continue

		xml = content.decode("utf-8")

		# ── Step 1: Healthcare POA Agent Full Name → Primary HPOA Agent Full Name + Co-Agent And Name
		# The HIPAA lists names, so co-agent name appears next to the primary name.
		rpr = extract_rpr(xml, "lily:Healthcare POA Agent Full Name")
		# Keep caps on the name SDT. Co-agent SDT should NOT have caps
		# (value is pre-uppercased with lowercase "and").
		rpr_no_caps = re.sub(r'<w:caps/>', '', rpr)

		new_xml = docx_utils.make_sdt_xml(next_id, "Primary HPOA Agent Full Name", "Primary HPOA Agent Full Name", False, rpr)
		next_id += 1
		new_xml += docx_utils.make_sdt_xml(next_id, "Primary HPOA Co-Agent And Name", "Primary HPOA Co-Agent And Name", False, rpr_no_caps)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily:Healthcare POA Agent Full Name", new_xml)
		print("  Replaced Healthcare POA Agent Full Name → Primary HPOA Agent Full Name + Co-Agent And Name")

		# ── Step 2: Replace conditional SDTs
		rpr = extract_rpr(xml, "lily-cond:Has Healthcare POA Alternate Agent")
		new_xml = docx_utils.make_sdt_xml(next_id, "Has Secondary HPOA Agent", "Has Secondary HPOA Agent", True, rpr)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily-cond:Has Healthcare POA Alternate Agent", new_xml)
		print("  Replaced Has Healthcare POA Alternate Agent → Has Secondary HPOA Agent")

		rpr = extract_rpr(xml, "lily-cond:Has Healthcare POA Third Agent")
		new_xml = docx_utils.make_sdt_xml(next_id, "Has Tertiary HPOA Agent", "Has Tertiary HPOA Agent", True, rpr)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily-cond:Has Healthcare POA Third Agent", new_xml)
		print("  Replaced Has Healthcare POA Third Agent → Has Tertiary HPOA Agent")

		entries[i] = (name, xml.encode("utf-8"))
		break

	docx_utils.write_docx(docx_path, entries)

	variables = docx_utils.extract_variables(docx_path)
	print("\nMigration complete. Variables:")
	for v in variables:
		prefix = "[COND]" if v.is_conditional else "[REPL]"
		print(f"  {prefix} {v.display_name}")


if __name__ == "__main__":
	if len(sys.argv) != 2:
		print(f"Usage: {sys.argv[0]} <template.docx>")
		sys.exit(1)
	migrate(sys.argv[1])
