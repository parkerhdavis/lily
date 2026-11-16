#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
One-shot migration script for the GPOA template.

Renames Financial POA variables to FPOA naming and adds co-agent
helper variable SDTs. Only modifies the specific SDTs — all
paragraph boundaries and surrounding text are preserved.
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

		# ── Step 1: Financial POA Agent Full Name → Primary FPOA Agent Full Name + Co-Agent And Name
		rpr = extract_rpr(xml, "lily:Financial POA Agent Full Name")
		# The name SDT already has <w:caps/>, keep it.
		# The co-agent SDT should NOT have caps (value is pre-uppercased with lowercase "and")
		rpr_no_caps = re.sub(r'<w:caps/>', '', rpr)

		new_xml = docx_utils.make_sdt_xml(next_id, "Primary FPOA Agent Full Name", "Primary FPOA Agent Full Name", False, rpr)
		next_id += 1
		new_xml += docx_utils.make_sdt_xml(next_id, "Primary FPOA Co-Agent And Name", "Primary FPOA Co-Agent And Name", False, rpr_no_caps)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily:Financial POA Agent Full Name", new_xml)
		print("  Replaced Financial POA Agent Full Name → Primary FPOA Agent Full Name + Co-Agent And Name")

		# ── Step 2: Financial POA Agent Phone → Primary FPOA Agent Phone + Co-Agent And Phone
		rpr = extract_rpr(xml, "lily:Financial POA Agent Phone")

		new_xml = docx_utils.make_sdt_xml(next_id, "Primary FPOA Agent Phone", "Primary FPOA Agent Phone", False, rpr)
		next_id += 1
		new_xml += docx_utils.make_sdt_xml(next_id, "Primary FPOA Co-Agent And Phone", "Primary FPOA Co-Agent And Phone", False, rpr)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily:Financial POA Agent Phone", new_xml)
		print("  Replaced Financial POA Agent Phone → Primary FPOA Agent Phone + Co-Agent And Phone")

		# ── Step 3: Replace "Agent." with Primary FPOA Agent Title SDT
		# Find the text "Agent." in the same paragraph as the new Primary FPOA SDTs,
		# specifically the one that says "as my Agent."
		new_phone_tag = 'lily:Primary FPOA Co-Agent And Phone'
		phone_pos = xml.find(f'w:tag w:val="{new_phone_tag}"')
		after_phone_sdt = xml.index("</w:sdt>", phone_pos) + len("</w:sdt>")

		# Find ", as my Agent." in the text after the phone SDTs
		search_start = after_phone_sdt
		# Look for the next conditional or end of paragraph
		search_end = xml.find("lily-cond:", search_start)
		search_region = xml[search_start:search_end]

		# Find the run containing "Agent."
		agent_text = "Agent."
		agent_escaped = docx_utils.escape_xml_text(agent_text)
		hr_pos_in_region = search_region.find(agent_escaped)
		if hr_pos_in_region < 0:
			print("  ERROR: Could not find 'Agent.' in appointment paragraph")
			return

		hr_pos = search_start + hr_pos_in_region
		run_start = xml.rfind("<w:r>", 0, hr_pos)
		if run_start < 0:
			run_start = xml.rfind("<w:r ", 0, hr_pos)
		run_end = xml.index("</w:r>", hr_pos) + len("</w:r>")
		old_run = xml[run_start:run_end]

		rpr_match = re.search(r'(<w:rPr>.*?</w:rPr>)', old_run, re.DOTALL)
		title_rpr = rpr_match.group(1) if rpr_match else ""

		t_match = re.search(r'<w:t[^>]*>(.*?)</w:t>', old_run, re.DOTALL)
		if not t_match:
			print("  ERROR: Could not parse w:t in Agent. run")
			return
		full_text = t_match.group(1)

		before_agent = full_text[:full_text.index(agent_escaped)]
		after_agent = full_text[full_text.index(agent_escaped) + len(agent_escaped):]

		parts = []
		if before_agent:
			parts.append(f'<w:r>{title_rpr}<w:t xml:space="preserve">{before_agent}</w:t></w:r>')
		title_sdt = docx_utils.make_sdt_xml(next_id, "Primary FPOA Agent Title", "Primary FPOA Agent Title", False, title_rpr)
		next_id += 1
		parts.append(title_sdt)
		if after_agent:
			parts.append(f'<w:r>{title_rpr}<w:t xml:space="preserve">{after_agent}</w:t></w:r>')

		xml = xml[:run_start] + "".join(parts) + xml[run_end:]
		print("  Replaced 'Agent.' with Primary FPOA Agent Title SDT")

		# ── Step 4: Replace conditional SDTs
		rpr = extract_rpr(xml, "lily-cond:Has Financial POA Alternate Agent")
		new_xml = docx_utils.make_sdt_xml(next_id, "Has Secondary FPOA Agent", "Has Secondary FPOA Agent", True, rpr)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily-cond:Has Financial POA Alternate Agent", new_xml)
		print("  Replaced Has Financial POA Alternate Agent → Has Secondary FPOA Agent")

		rpr = extract_rpr(xml, "lily-cond:Has Financial POA Third Agent")
		new_xml = docx_utils.make_sdt_xml(next_id, "Has Tertiary FPOA Agent", "Has Tertiary FPOA Agent", True, rpr)
		next_id += 1
		xml = replace_sdt_by_tag(xml, "lily-cond:Has Financial POA Third Agent", new_xml)
		print("  Replaced Has Financial POA Third Agent → Has Tertiary FPOA Agent")

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
