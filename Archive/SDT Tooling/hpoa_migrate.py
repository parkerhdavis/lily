#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
One-shot migration script for the HPOA template.

Replaces old variable SDTs in Section 1.1 with new renamed variables
and adds co-agent helper variable SDTs. Only modifies the specific
paragraph containing the appointment text — all other paragraphs
are left untouched.

Also replaces old conditional SDTs (Has Healthcare POA Alternate Agent,
Has Healthcare POA Third Agent) with renamed versions.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


def migrate(docx_path: str) -> None:
	entries = docx_utils.read_docx_entries(docx_path)
	next_id = docx_utils.find_max_id_across_parts(entries) + 1

	for i, (name, content) in enumerate(entries):
		if name != "word/document.xml":
			continue

		xml = content.decode("utf-8")

		# ── Step 1: Replace the Healthcare POA Agent Full Name SDT ──────
		# Find the SDT by its tag and replace with:
		#   [SDT: Primary HPOA Agent Full Name][SDT: Primary HPOA Co-Agent And Name]
		# Preserve the run properties from the original SDT content.

		old_name_tag = 'w:tag w:val="lily:Healthcare POA Agent Full Name"'
		if old_name_tag not in xml:
			print("ERROR: Could not find Healthcare POA Agent Full Name SDT")
			return

		# Extract the rPr from the original SDT content
		name_sdt_start = xml.index(old_name_tag)
		# Find the enclosing <w:sdt> ... </w:sdt>
		sdt_open = xml.rfind("<w:sdt>", 0, name_sdt_start)
		sdt_close = xml.index("</w:sdt>", name_sdt_start) + len("</w:sdt>")
		old_name_sdt = xml[sdt_open:sdt_close]

		# Extract rPr from the SDT content runs
		import re
		rpr_match = re.search(r'<w:sdtContent>.*?(<w:rPr>.*?</w:rPr>)', old_name_sdt, re.DOTALL)
		name_rpr = rpr_match.group(1) if rpr_match else ""

		# Build replacement: two SDTs side by side
		new_name_sdt = docx_utils.make_sdt_xml(
			next_id, "Primary HPOA Agent Full Name",
			"Primary HPOA Agent Full Name", False, name_rpr,
		)
		next_id += 1
		new_coname_sdt = docx_utils.make_sdt_xml(
			next_id, "Primary HPOA Co-Agent And Name",
			"Primary HPOA Co-Agent And Name", False, name_rpr,
		)
		next_id += 1

		xml = xml[:sdt_open] + new_name_sdt + new_coname_sdt + xml[sdt_close:]

		# ── Step 2: Replace the Healthcare POA Agent Phone SDT ──────────
		old_phone_tag = 'w:tag w:val="lily:Healthcare POA Agent Phone"'
		if old_phone_tag not in xml:
			print("ERROR: Could not find Healthcare POA Agent Phone SDT")
			return

		phone_sdt_start = xml.index(old_phone_tag)
		sdt_open = xml.rfind("<w:sdt>", 0, phone_sdt_start)
		sdt_close = xml.index("</w:sdt>", phone_sdt_start) + len("</w:sdt>")
		old_phone_sdt = xml[sdt_open:sdt_close]

		rpr_match = re.search(r'<w:sdtContent>.*?(<w:rPr>.*?</w:rPr>)', old_phone_sdt, re.DOTALL)
		phone_rpr = rpr_match.group(1) if rpr_match else ""

		new_phone_sdt = docx_utils.make_sdt_xml(
			next_id, "Primary HPOA Agent Phone",
			"Primary HPOA Agent Phone", False, phone_rpr,
		)
		next_id += 1
		new_cophone_sdt = docx_utils.make_sdt_xml(
			next_id, "Primary HPOA Co-Agent And Phone",
			"Primary HPOA Co-Agent And Phone", False, phone_rpr,
		)
		next_id += 1

		xml = xml[:sdt_open] + new_phone_sdt + new_cophone_sdt + xml[sdt_close:]

		# ── Step 3: Replace "Healthcare Representative." with Title SDT ─
		# Only in the Section 1.1 appointment paragraph. We find it by
		# locating the text between the phone SDT and the conditional SDTs.
		# The text is: ", to serve as my Healthcare Representative. "
		# We replace "Healthcare Representative." with the Title SDT.

		# Find the text run containing "Healthcare Representative." that is
		# in the same paragraph as our new SDTs (Section 1.1).
		# The new phone SDT tag will be nearby.
		new_phone_tag = 'w:tag w:val="lily:Primary HPOA Co-Agent And Phone"'
		phone_pos = xml.index(new_phone_tag)
		# Find the next </w:sdt> after this
		after_phone_sdt = xml.index("</w:sdt>", phone_pos) + len("</w:sdt>")

		# Now find "Healthcare Representative." in the text AFTER the phone SDT
		# but before the next conditional SDT
		search_region_start = after_phone_sdt
		search_region_end = xml.index("lily-cond:", after_phone_sdt)
		search_region = xml[search_region_start:search_region_end]

		# Find the w:t element containing "Healthcare Representative."
		hr_text = "Healthcare Representative."
		hr_escaped = docx_utils.escape_xml_text(hr_text)
		hr_pos_in_region = search_region.find(hr_escaped)
		if hr_pos_in_region < 0:
			print("ERROR: Could not find 'Healthcare Representative.' in Section 1.1")
			return

		# Find the exact position in the full XML
		hr_pos = search_region_start + hr_pos_in_region
		# We need to find the enclosing <w:t> element and split it
		# The text around it is: ", to serve as my Healthcare Representative. "
		# We want: ", to serve as my " + [SDT: Title] + " "

		# Find the enclosing <w:r> element
		run_start = xml.rfind("<w:r>", 0, hr_pos)
		if run_start < 0:
			run_start = xml.rfind("<w:r ", 0, hr_pos)
		run_end = xml.index("</w:r>", hr_pos) + len("</w:r>")
		old_run = xml[run_start:run_end]

		# Extract rPr from this run
		rpr_match = re.search(r'(<w:rPr>.*?</w:rPr>)', old_run, re.DOTALL)
		title_rpr = rpr_match.group(1) if rpr_match else ""

		# Extract the full text content
		t_match = re.search(r'<w:t[^>]*>(.*?)</w:t>', old_run, re.DOTALL)
		if not t_match:
			print("ERROR: Could not parse w:t in Healthcare Representative run")
			return
		full_text = t_match.group(1)

		# Split around "Healthcare Representative."
		before_hr = full_text[:full_text.index(hr_escaped)]
		after_hr = full_text[full_text.index(hr_escaped) + len(hr_escaped):]

		# Build replacement: [run: before text] [SDT: Title] [run: after text]
		parts = []
		if before_hr:
			parts.append(
				f'<w:r>{title_rpr}<w:t xml:space="preserve">'
				f'{before_hr}</w:t></w:r>'
			)

		title_sdt = docx_utils.make_sdt_xml(
			next_id, "Primary HPOA Agent Title",
			"Primary HPOA Agent Title", False, title_rpr,
		)
		next_id += 1
		parts.append(title_sdt)

		if after_hr:
			parts.append(
				f'<w:r>{title_rpr}<w:t xml:space="preserve">'
				f'{after_hr}</w:t></w:r>'
			)

		replacement = "".join(parts)
		xml = xml[:run_start] + replacement + xml[run_end:]

		# ── Step 4: Replace conditional SDTs ────────────────────────────
		# Has Healthcare POA Alternate Agent → Has Secondary HPOA Agent
		old_cond1_tag = 'lily-cond:Has Healthcare POA Alternate Agent'
		new_cond1_tag = 'lily-cond:Has Secondary HPOA Agent'
		new_cond1_alias = 'Has Secondary HPOA Agent'

		if old_cond1_tag in xml:
			# Find and replace the SDT
			cond1_pos = xml.index(old_cond1_tag)
			sdt_open = xml.rfind("<w:sdt>", 0, cond1_pos)
			sdt_close = xml.index("</w:sdt>", cond1_pos) + len("</w:sdt>")
			old_cond1_sdt = xml[sdt_open:sdt_close]

			# Extract rPr
			rpr_match = re.search(r'<w:sdtContent>.*?(<w:rPr>.*?</w:rPr>)', old_cond1_sdt, re.DOTALL)
			cond1_rpr = rpr_match.group(1) if rpr_match else ""

			new_cond1_sdt = docx_utils.make_sdt_xml(
				next_id, new_cond1_alias,
				new_cond1_alias, True, cond1_rpr,
			)
			next_id += 1
			xml = xml[:sdt_open] + new_cond1_sdt + xml[sdt_close:]

		# Has Healthcare POA Third Agent → Has Tertiary HPOA Agent
		old_cond2_tag = 'lily-cond:Has Healthcare POA Third Agent'
		new_cond2_alias = 'Has Tertiary HPOA Agent'

		if old_cond2_tag in xml:
			cond2_pos = xml.index(old_cond2_tag)
			sdt_open = xml.rfind("<w:sdt>", 0, cond2_pos)
			sdt_close = xml.index("</w:sdt>", cond2_pos) + len("</w:sdt>")
			old_cond2_sdt = xml[sdt_open:sdt_close]

			rpr_match = re.search(r'<w:sdtContent>.*?(<w:rPr>.*?</w:rPr>)', old_cond2_sdt, re.DOTALL)
			cond2_rpr = rpr_match.group(1) if rpr_match else ""

			new_cond2_sdt = docx_utils.make_sdt_xml(
				next_id, new_cond2_alias,
				new_cond2_alias, True, cond2_rpr,
			)
			next_id += 1
			xml = xml[:sdt_open] + new_cond2_sdt + xml[sdt_close:]

		entries[i] = (name, xml.encode("utf-8"))
		break

	docx_utils.write_docx(docx_path, entries)

	# Verify
	variables = docx_utils.extract_variables(docx_path)
	print("Migration complete. Variables:")
	for v in variables:
		prefix = "[COND]" if v.is_conditional else "[REPL]"
		print(f"  {prefix} {v.display_name}")


if __name__ == "__main__":
	if len(sys.argv) != 2:
		print(f"Usage: {sys.argv[0]} <template.docx>")
		sys.exit(1)
	migrate(sys.argv[1])
