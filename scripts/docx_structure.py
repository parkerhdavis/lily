# SPDX-License-Identifier: AGPL-3.0-or-later
"""Structural extraction and comparison for rendered Lily documents.

The reviewer needs to see the document as the renderer sees it: paragraphs,
styles, alignment, spacing, and run marks, plus the header and footer text.
Everything here is read-only and deterministic.
"""

from __future__ import annotations

import difflib
import re
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from xml.etree import ElementTree as ET

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
HEADER_FOOTER_RE = re.compile(r"word/(header|footer)\d*\.xml$")


@dataclass
class Run:
	text: str
	bold: bool = False
	italic: bool = False
	underline: bool = False
	caps: bool = False
	small_caps: bool = False
	strike: bool = False
	size: str | None = None
	font: str | None = None
	style: str | None = None


@dataclass
class Paragraph:
	text: str
	style: str | None = None
	align: str | None = None
	indent: dict[str, str] = field(default_factory=dict)
	spacing: dict[str, str] = field(default_factory=dict)
	page_break_before: bool = False
	keep_next: bool = False
	keep_lines: bool = False
	numbering: dict[str, str] = field(default_factory=dict)
	runs: list[Run] = field(default_factory=list)


@dataclass
class Part:
	name: str
	paragraphs: list[Paragraph] = field(default_factory=list)

	@property
	def text(self) -> str:
		return "\n".join(p.text for p in self.paragraphs if p.text.strip())


def attr(element: ET.Element | None, name: str) -> str | None:
	if element is None:
		return None
	return element.get(W + name)


def local_attrs(element: ET.Element | None) -> dict[str, str]:
	if element is None:
		return {}
	return {key.split("}")[-1]: value for key, value in element.attrib.items()}


def parse_part(data: bytes, name: str) -> Part:
	root = ET.fromstring(data)
	part = Part(name=name)
	for p in root.iter(W + "p"):
		text = "".join(t.text or "" for t in p.iter(W + "t")).strip()
		if not text:
			continue
		ppr = p.find(W + "pPr")
		style = attr(ppr.find(W + "pStyle") if ppr is not None else None, "val")
		align = attr(ppr.find(W + "jc") if ppr is not None else None, "val")
		indent = local_attrs(ppr.find(W + "ind") if ppr is not None else None)
		spacing = local_attrs(ppr.find(W + "spacing") if ppr is not None else None)
		numbering: dict[str, str] = {}
		num_pr = ppr.find(W + "numPr") if ppr is not None else None
		if num_pr is not None:
			num_id = num_pr.find(W + "numId")
			ilvl = num_pr.find(W + "ilvl")
			if num_id is not None:
				numbering["numId"] = attr(num_id, "val") or ""
			if ilvl is not None:
				numbering["ilvl"] = attr(ilvl, "val") or ""
		runs: list[Run] = []
		for r in p.iter(W + "r"):
			rtext = "".join(t.text or "" for t in r.iter(W + "t"))
			if not rtext:
				continue
			rpr = r.find(W + "rPr")
			fonts = rpr.find(W + "rFonts") if rpr is not None else None
			size = rpr.find(W + "sz") if rpr is not None else None
			rstyle = rpr.find(W + "rStyle") if rpr is not None else None
			runs.append(
				Run(
					text=rtext,
					bold=rpr is not None and rpr.find(W + "b") is not None,
					italic=rpr is not None and rpr.find(W + "i") is not None,
					underline=rpr is not None and rpr.find(W + "u") is not None,
					caps=rpr is not None and rpr.find(W + "caps") is not None,
					small_caps=rpr is not None and rpr.find(W + "smallCaps") is not None,
					strike=rpr is not None and rpr.find(W + "strike") is not None,
					size=attr(size, "val"),
					font=attr(fonts, "ascii") if fonts is not None else None,
					style=attr(rstyle, "val"),
				)
			)
		part.paragraphs.append(
			Paragraph(
				text=text,
				style=style,
				align=align,
				indent=indent,
				spacing=spacing,
				page_break_before=ppr is not None
				and ppr.find(W + "pageBreakBefore") is not None,
				keep_next=ppr is not None and ppr.find(W + "keepNext") is not None,
				keep_lines=ppr is not None and ppr.find(W + "keepLines") is not None,
				numbering=numbering,
				runs=runs,
			)
		)
	return part


def extract_document(path: Path) -> dict:
	parts: dict[str, Part] = {}
	with zipfile.ZipFile(path) as archive:
		for name in archive.namelist():
			if name == "word/document.xml" or HEADER_FOOTER_RE.match(name):
				parts[name] = parse_part(archive.read(name), name)
	body = parts.get("word/document.xml", Part("word/document.xml"))
	headers = [part for name, part in parts.items() if name.startswith("word/header")]
	footers = [part for name, part in parts.items() if name.startswith("word/footer")]
	return {
		"file": str(path),
		"paragraphs": body.paragraphs,
		"headers": headers,
		"footers": footers,
		"paragraph_count": len(body.paragraphs),
		"word_count": sum(len(p.text.split()) for p in body.paragraphs),
	}


def normalized_key(text: str) -> str:
	return re.sub(r"[^a-z0-9]+", " ", text.lower()).strip()


def run_signature(run: Run) -> str:
	marks = "".join(
		letter
		for letter, on in (
			("B", run.bold),
			("I", run.italic),
			("U", run.underline),
			("C", run.caps),
			("S", run.small_caps),
			("X", run.strike),
		)
		if on
	)
	return f"{marks or '-'}:{run.size or '-'}:{run.font or '-'}:{run.style or '-'}"


def paragraph_signature(paragraph: Paragraph) -> dict:
	return {
		"style": paragraph.style,
		"align": paragraph.align,
		"indent": paragraph.indent,
		"spacing": paragraph.spacing,
		"page_break_before": paragraph.page_break_before,
		"keep_next": paragraph.keep_next,
		"keep_lines": paragraph.keep_lines,
		"numbering": paragraph.numbering,
		"runs": [run_signature(run) for run in paragraph.runs],
	}


def signature_differences(reference: Paragraph, candidate: Paragraph) -> list[str]:
	left = paragraph_signature(reference)
	right = paragraph_signature(candidate)
	differences = []
	for key in left:
		if left[key] != right[key]:
			differences.append(key)
	return differences


def compare_by_similarity(
	reference: dict,
	candidate: dict,
	threshold: float = 0.86,
	limit: int = 100,
) -> dict:
	ref_paragraphs = reference["paragraphs"]
	cand_paragraphs = candidate["paragraphs"]
	ref_keys = [normalized_key(p.text) for p in ref_paragraphs]
	cand_keys = [normalized_key(p.text) for p in cand_paragraphs]
	used: set[int] = set()
	matches: list[tuple[int, int, float]] = []
	for i, key in enumerate(ref_keys):
		if not key:
			continue
		best: tuple[float, int] | None = None
		for j, other in enumerate(cand_keys):
			if j in used or not other:
				continue
			ratio = difflib.SequenceMatcher(None, key, other).ratio()
			if ratio >= threshold and (best is None or ratio > best[0]):
				best = (ratio, j)
		if best is not None:
			used.add(best[1])
			matches.append((i, best[1], best[0]))
	deltas = []
	exact_differences = 0
	for i, j, ratio in matches:
		differences = signature_differences(ref_paragraphs[i], cand_paragraphs[j])
		if differences:
			exact = ref_keys[i] == cand_keys[j]
			if exact:
				exact_differences += 1
			deltas.append(
				{
					"reference_ordinal": i,
					"candidate_ordinal": j,
					"ratio": round(ratio, 3),
					"exact_text": exact,
					"reference": ref_paragraphs[i].text[:240],
					"candidate": cand_paragraphs[j].text[:240],
					"differences": differences,
				}
			)
	matched_ref = {i for i, _, _ in matches}
	matched_cand = {j for _, j, _ in matches}
	return {
		"reference_paragraphs": len(ref_paragraphs),
		"candidate_paragraphs": len(cand_paragraphs),
		"matched": len(matches),
		"formatting_differences": len(deltas),
		"exact_text_formatting_differences": exact_differences,
		"deltas": deltas[:limit],
		"deltas_truncated": max(0, len(deltas) - limit),
		"unmatched_reference": [
			p.text[:160]
			for i, p in enumerate(ref_paragraphs)
			if i not in matched_ref and p.text.strip()
		][:12],
		"unmatched_candidate": [
			p.text[:160]
			for j, p in enumerate(cand_paragraphs)
			if j not in matched_cand and p.text.strip()
		][:12],
	}


def compare_by_ordinal(template: dict, candidate: dict, limit: int = 100) -> dict:
	left = template["paragraphs"]
	right = candidate["paragraphs"]
	deltas = []
	for index in range(min(len(left), len(right))):
		if not left[index].text.strip() or not right[index].text.strip():
			continue
		differences = signature_differences(left[index], right[index])
		if differences:
			deltas.append(
				{
					"ordinal": index,
					"template": left[index].text[:200],
					"candidate": right[index].text[:200],
					"differences": differences,
				}
			)
	return {
		"template_paragraphs": len(left),
		"candidate_paragraphs": len(right),
		"formatting_differences": len(deltas),
		"deltas": deltas[:limit],
		"deltas_truncated": max(0, len(deltas) - limit),
	}


def part_texts(parts: list[Part]) -> list[dict]:
	return [{"part": part.name, "text": part.text} for part in parts if part.text.strip()]


def sample_text_flags(parts: list[Part]) -> list[dict]:
	flags = []
	for part in parts:
		for pattern in (r"\bJOHN\s+DOE\b", r"\bJANE\s+DOE\b", r"\{[^{}]+\}"):
			for match in re.finditer(pattern, part.text, re.IGNORECASE):
				flags.append(
					{
						"part": part.name,
						"match": match.group(0),
						"text": part.text[:240],
					}
				)
	return flags
