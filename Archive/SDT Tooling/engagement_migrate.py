#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
One-shot migration script for the Engagement Letter template.

Pure rename — swaps old variable names to new naming convention.
No co-agent SDTs, no conditionals, no structural changes.
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils

# Old tag → new variable name
RENAMES = {
	"lily:Financial POA Agent Full Name": "Primary FPOA Agent Full Name",
	"lily:Financial POA Agent Phone": "Primary FPOA Agent Phone",
	"lily:Financial POA Alternate Agent Full Name": "Secondary FPOA Agent Full Name",
	"lily:Financial POA Alternate Agent Phone": "Secondary FPOA Agent Phone",
	"lily:Financial POA Third Agent Full Name": "Tertiary FPOA Agent Full Name",
	"lily:Financial POA Third Agent Phone": "Tertiary FPOA Agent Phone",
	"lily:Healthcare POA Agent Full Name": "Primary HPOA Agent Full Name",
	"lily:Healthcare POA Agent Phone": "Primary HPOA Agent Phone",
	"lily:Healthcare POA Alternate Agent Full Name": "Secondary HPOA Agent Full Name",
	"lily:Healthcare POA Alternate Agent Phone": "Secondary HPOA Agent Phone",
	"lily:Healthcare POA Third Agent Full Name": "Tertiary HPOA Agent Full Name",
	"lily:Healthcare POA Third Agent Phone": "Tertiary HPOA Agent Phone",
	"lily:Personal Representative Full Name": "Primary Personal Representative Full Name",
	"lily:Alternate Personal Representative Full Name": "Secondary Personal Representative Full Name",
	"lily:Third Personal Representative Full Name": "Tertiary Personal Representative Full Name",
	"lily:Trustee Full Name": "Primary Trustee Full Name",
	"lily:Alternate Trustee Full Name": "Secondary Trustee Full Name",
}


def migrate(docx_path: str) -> None:
	entries = docx_utils.read_docx_entries(docx_path)
	next_id = docx_utils.find_max_id_across_parts(entries) + 1
	total = 0

	for i, (name, content) in enumerate(entries):
		if not docx_utils.is_variable_part(name):
			continue

		xml = content.decode("utf-8")
		modified = False

		for old_tag, new_name in RENAMES.items():
			new_tag = f"lily:{new_name}"
			# Replace ALL occurrences of this tag in the XML part
			while True:
				tag_str = f'w:tag w:val="{old_tag}"'
				pos = xml.find(tag_str)
				if pos < 0:
					break

				# Find enclosing SDT
				sdt_start = xml.rfind("<w:sdt>", 0, pos)
				sdt_end = xml.index("</w:sdt>", pos) + len("</w:sdt>")
				old_sdt = xml[sdt_start:sdt_end]

				# Extract rPr from content
				rpr_match = re.search(
					r'<w:sdtContent>.*?(<w:rPr>.*?</w:rPr>)',
					old_sdt, re.DOTALL,
				)
				rpr = rpr_match.group(1) if rpr_match else ""

				new_sdt = docx_utils.make_sdt_xml(
					next_id, new_name, new_name, False, rpr,
				)
				next_id += 1

				xml = xml[:sdt_start] + new_sdt + xml[sdt_end:]
				modified = True
				total += 1

		if modified:
			entries[i] = (name, xml.encode("utf-8"))

	docx_utils.write_docx(docx_path, entries)

	variables = docx_utils.extract_variables(docx_path)
	print(f"Renamed {total} SDTs. Variables ({len(variables)} total):")
	for v in variables:
		print(f"  [REPL] {v.display_name}")


if __name__ == "__main__":
	if len(sys.argv) != 2:
		print(f"Usage: {sys.argv[0]} <template.docx>")
		sys.exit(1)
	migrate(sys.argv[1])
