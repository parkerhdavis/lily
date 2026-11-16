#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
One-shot migration for the HIPAA template:

1. Bolds the body Client Full Name reference (id=2), the Primary HPOA Agent
   name (id=7), the Primary HPOA Co-Agent (id=8), the two conditional agent
   SDTs (id=9, id=10), and the trailing `, ` / `;` punctuation runs.
2. Drops <w:isLgl/> from abstractNum 3/4/5 ilvl=0 in numbering.xml so the
   list renders with the declared lowerLetter format ((a), (b), (c)) instead
   of being forced to decimal by legal-style.
3. Reassigns the conditional Secondary/Tertiary HPOA agent paragraphs from
   numId=4/numId=5 to numId=3 so the whole bullet list shares one counter.

Title client name SDT (id=1) is already bold, and the signature SDT (id=6)
intentionally stays non-bold to match the typed-name-under-signature-line
convention.
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import docx_utils


SDTS_TO_BOLD = ["2", "7", "8", "9", "10"]


def bold_sdt_by_id(xml: str, sdt_id: str) -> str:
	"""Find the SDT whose <w:id w:val="N"/> matches and flip false-bold to true-bold inside its sdtContent."""
	id_str = f'<w:id w:val="{sdt_id}"/>'
	pos = xml.find(id_str)
	if pos < 0:
		print(f"  WARNING: SDT id={sdt_id} not found")
		return xml
	sdt_start = xml.rfind("<w:sdt>", 0, pos)
	sdt_end = xml.index("</w:sdt>", pos) + len("</w:sdt>")
	sdt = xml[sdt_start:sdt_end]
	new_sdt = sdt.replace('<w:b w:val="false"/>', '<w:b/>').replace('<w:bCs w:val="false"/>', '<w:bCs/>')
	return xml[:sdt_start] + new_sdt + xml[sdt_end:]


def bold_run_after_sdt(xml: str, sdt_id: str, expected_text: str) -> str:
	"""Find the <w:r> immediately after the SDT with sdt_id and bold it if its <w:t> matches expected_text."""
	id_str = f'<w:id w:val="{sdt_id}"/>'
	pos = xml.find(id_str)
	if pos < 0:
		print(f"  WARNING: SDT id={sdt_id} not found for trailing-run bolding")
		return xml
	sdt_end = xml.index("</w:sdt>", pos) + len("</w:sdt>")
	# The next element should be <w:r>...
	tail = xml[sdt_end:]
	m = re.match(r"\s*<w:r>(.*?)</w:r>", tail, re.DOTALL)
	if not m:
		print(f"  WARNING: no <w:r> immediately after SDT id={sdt_id}")
		return xml
	run_inner = m.group(1)
	# Confirm the run contains the expected text
	t_match = re.search(r"<w:t(?:\s[^>]*)?>([^<]*)</w:t>", run_inner)
	if not t_match or t_match.group(1) != expected_text:
		print(f"  WARNING: run text after SDT id={sdt_id} does not match {expected_text!r} (found {t_match.group(1) if t_match else None!r})")
		return xml
	full_run = m.group(0)
	new_run = full_run.replace('<w:b w:val="false"/>', '<w:b/>').replace('<w:bCs w:val="false"/>', '<w:bCs/>')
	if new_run == full_run:
		print(f"  NOTE: run after SDT id={sdt_id} had no false-bold to flip")
		return xml
	run_start = sdt_end + m.start()
	run_end = sdt_end + m.end()
	return xml[:run_start] + new_run + xml[run_end:]


def repoint_numid(xml: str, old_numid: str, new_numid: str) -> str:
	"""Replace numId=old → numId=new everywhere it appears (only used for orphan numIds 4 & 5)."""
	src = f'<w:numId w:val="{old_numid}"/>'
	dst = f'<w:numId w:val="{new_numid}"/>'
	count = xml.count(src)
	if count == 0:
		print(f"  WARNING: numId={old_numid} not found")
		return xml
	if count > 1:
		print(f"  WARNING: numId={old_numid} appears {count} times — expected 1")
	return xml.replace(src, dst)


def drop_islgl_from_abstract_num(xml: str, abstract_num_id: str) -> str:
	"""Remove <w:isLgl/> from ilvl=0 of the given abstractNum."""
	pattern = re.compile(
		r'(<w:abstractNum\s+w:abstractNumId="' + re.escape(abstract_num_id) + r'"[^>]*>\s*<w:lvl\s+w:ilvl="0">)(.*?)(</w:lvl>)',
		re.DOTALL,
	)
	m = pattern.search(xml)
	if not m:
		print(f"  WARNING: abstractNum {abstract_num_id} ilvl=0 not found")
		return xml
	inner = m.group(2)
	new_inner = re.sub(r"<w:isLgl\s*/>\s*", "", inner)
	if new_inner == inner:
		print(f"  NOTE: abstractNum {abstract_num_id} ilvl=0 had no <w:isLgl/>")
		return xml
	return xml[:m.start()] + m.group(1) + new_inner + m.group(3) + xml[m.end():]


def migrate(docx_path: str) -> None:
	entries = docx_utils.read_docx_entries(docx_path)
	new_entries: list[tuple[str, bytes]] = []
	for name, content in entries:
		if name == "word/document.xml":
			xml = content.decode("utf-8")
			print(f"\n[document.xml] bolding SDTs {SDTS_TO_BOLD}")
			for sdt_id in SDTS_TO_BOLD:
				xml = bold_sdt_by_id(xml, sdt_id)
			print("[document.xml] bolding `, ` run after Client Full Name (id=2)")
			xml = bold_run_after_sdt(xml, "2", ", ")
			print("[document.xml] bolding `;` run after Primary HPOA Co-Agent (id=8)")
			xml = bold_run_after_sdt(xml, "8", ";")
			print("[document.xml] repointing numId=4 → numId=3 (Has Secondary HPOA Agent paragraph)")
			xml = repoint_numid(xml, "4", "3")
			print("[document.xml] repointing numId=5 → numId=3 (Has Tertiary HPOA Agent paragraph)")
			xml = repoint_numid(xml, "5", "3")
			content = xml.encode("utf-8")
		elif name == "word/numbering.xml":
			xml = content.decode("utf-8")
			print("\n[numbering.xml] removing <w:isLgl/> from abstractNum 3/4/5 ilvl=0")
			for ab in ("3", "4", "5"):
				xml = drop_islgl_from_abstract_num(xml, ab)
			content = xml.encode("utf-8")
		new_entries.append((name, content))
	docx_utils.write_docx(docx_path, new_entries)
	print(f"\nWrote {docx_path}")


if __name__ == "__main__":
	if len(sys.argv) != 2:
		print(f"usage: {sys.argv[0]} <template.docx>", file=sys.stderr)
		sys.exit(2)
	migrate(sys.argv[1])
