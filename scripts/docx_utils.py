# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Shared utilities for Lily DOCX template tooling.

Provides parity with the Rust backend (apps/desktop/backend/src/docx_ops.rs) for:
- ZIP-based DOCX reading/writing
- XML escaping and entity handling
- SDT/bookmark extraction and ID management
- Split variable normalization
- Conditional variable parsing
- Contact-role dot notation
- Casing rules and display name selection
- Template schema (.lily sidecar) I/O

All operations use stdlib only (zipfile, re, json) — no external deps.
"""

import json
import re
import xml.etree.ElementTree as ET
import zipfile
from io import BytesIO
from pathlib import Path


# ── Constants ───────────────────────────────────────────────────────────────

SDT_TAG_PREFIX = "lily:"
COND_SDT_TAG_PREFIX = "lily-cond:"
BOOKMARK_PREFIX = "lily:"
COND_BOOKMARK_PREFIX = "lily-cond:"

CONTACT_PROPERTIES = [
	"full_name", "first_name", "last_name", "relationship",
	"phone", "email", "address", "city", "state", "zip",
]

# XML parts that contain variable-bearing content
VARIABLE_PARTS = {"word/document.xml"}
VARIABLE_PART_PREFIXES = ("word/header", "word/footer")


# ── XML escaping ────────────────────────────────────────────────────────────

def escape_xml_text(text: str) -> str:
	"""Escape text for XML content. Order matters: & first to avoid double-escaping."""
	return (
		text
		.replace("&", "&amp;")
		.replace("<", "&lt;")
		.replace(">", "&gt;")
		.replace('"', "&quot;")
		.replace("'", "&apos;")
	)


def unescape_xml_text(text: str) -> str:
	"""Unescape XML entities back to plain text."""
	return (
		text
		.replace("&amp;", "&")
		.replace("&lt;", "<")
		.replace("&gt;", ">")
		.replace("&quot;", '"')
		.replace("&apos;", "'")
	)


# ── DOCX ZIP operations ────────────────────────────────────────────────────

def is_variable_part(name: str) -> bool:
	"""Check if a ZIP entry name is one of the XML parts that can contain variables."""
	if name in VARIABLE_PARTS:
		return True
	return any(name.startswith(prefix) for prefix in VARIABLE_PART_PREFIXES)


def read_docx_entries(docx_path: str) -> list[tuple[str, bytes]]:
	"""
	Read all entries from a DOCX (ZIP) file.
	Returns a list of (entry_name, content_bytes) tuples.
	"""
	entries = []
	with zipfile.ZipFile(docx_path, "r") as zf:
		for name in zf.namelist():
			entries.append((name, zf.read(name)))
	return entries


def read_docx_xml_parts(docx_path: str) -> dict[str, str]:
	"""
	Read variable-bearing XML parts from a DOCX file.
	Returns a dict mapping part names to their XML content as strings.
	"""
	parts = {}
	with zipfile.ZipFile(docx_path, "r") as zf:
		for name in zf.namelist():
			if is_variable_part(name):
				parts[name] = zf.read(name).decode("utf-8")
	return parts


def write_docx(docx_path: str, entries: list[tuple[str, bytes]]) -> None:
	"""
	Write entries back to a DOCX (ZIP) file.
	entries is a list of (entry_name, content_bytes) tuples.
	"""
	buf = BytesIO()
	with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
		for name, content in entries:
			zf.writestr(name, content)
	Path(docx_path).write_bytes(buf.getvalue())


# ── Empty conditional paragraph pruning ────────────────────────────────────

_PARAGRAPH_RE = re.compile(r"<w:p\b[^>]*>.*?</w:p>", re.DOTALL)
_COND_BOOKMARK_RE = re.compile(r'<w:bookmarkStart\s+w:id="\d+"\s+w:name="lily-cond:[^"]*"\s*/>')
_T_TEXT_RE = re.compile(r"<w:t(?: [^>]*)?>([^<]*)</w:t>")
_VISIBLE_BLOCK_RE = re.compile(
	r"<w:(?:tab|br|drawing|pict|object|fldChar|sym|ptab|noBreakHyphen|softHyphen)\b"
)


def prune_empty_conditional_paragraphs(xml: str) -> str:
	"""
	Remove `<w:p>` elements that contain a `lily-cond:` bookmark and have
	no remaining visible content.

	Mirrors `prune_empty_conditional_paragraphs` in `docx_ops.rs`. Used as a
	post-pass after conditional resolution: when a `lily-cond:` SDT resolves
	to an empty string it becomes a zero-width bookmark, and if the rest of
	the paragraph is empty (no non-whitespace text, no tabs/breaks/drawings)
	the paragraph itself is dropped so list counters stay tight.
	"""
	out_parts: list[str] = []
	last_end = 0
	for m in _PARAGRAPH_RE.finditer(xml):
		para = m.group(0)
		if not _COND_BOOKMARK_RE.search(para):
			continue
		if _VISIBLE_BLOCK_RE.search(para):
			continue
		if any(t.group(1).strip() for t in _T_TEXT_RE.finditer(para)):
			continue
		out_parts.append(xml[last_end : m.start()])
		last_end = m.end()
	if last_end == 0:
		return xml
	out_parts.append(xml[last_end:])
	return "".join(out_parts)


# ── ID management ───────────────────────────────────────────────────────────

_BOOKMARK_ID_RE = re.compile(r'<w:bookmarkStart\s+w:id="(\d+)"')
_SDT_ID_RE = re.compile(r'<w:id\s+w:val="(\d+)"')


def find_max_id(xml: str) -> int:
	"""
	Find the maximum ID used by bookmarks and SDT content controls in XML.
	Returns 0 if no IDs exist. ID space is shared between bookmarks and SDTs.
	"""
	bookmark_ids = [int(m.group(1)) for m in _BOOKMARK_ID_RE.finditer(xml)]
	sdt_ids = [int(m.group(1)) for m in _SDT_ID_RE.finditer(xml)]
	all_ids = bookmark_ids + sdt_ids
	return max(all_ids) if all_ids else 0


def find_max_id_across_parts(entries: list[tuple[str, bytes]]) -> int:
	"""Find the maximum ID across all variable-bearing XML parts."""
	max_id = 0
	for name, content in entries:
		if is_variable_part(name):
			xml_str = content.decode("utf-8", errors="replace")
			max_id = max(max_id, find_max_id(xml_str))
	return max_id


# ── Quote normalization ─────────────────────────────────────────────────────

def normalize_quotes(s: str) -> str:
	"""Normalize smart/curly quotes to straight ASCII quotes."""
	s = s.replace("\u201c", '"').replace("\u201d", '"')
	s = s.replace("\u2018", "'").replace("\u2019", "'")
	return s


# ── Conditional variable parsing ────────────────────────────────────────────

def find_unescaped_quote(s: str) -> int | None:
	"""Find the first unescaped double-quote in s. Returns index or None."""
	i = 0
	while i < len(s):
		ch = s[i]
		if ch == "\\":
			i += 2  # skip escaped character
			continue
		if ch == '"':
			return i
		i += 1
	return None


def is_conditional_variable(raw_content: str) -> bool:
	"""Check if a variable's raw content (between {}) is a conditional."""
	return "??" in raw_content


def parse_conditional_variable(raw_content: str) -> tuple[str, str, str] | None:
	"""
	Parse conditional variable syntax into (label, true_text, false_text).

	Expected syntax:
	  Client Is Single ?? "single text" :: "couple text"
	  → ("Client Is Single", "single text", "couple text")

	Returns None if parsing fails.
	"""
	raw_content = normalize_quotes(raw_content)
	parts = raw_content.split("??", 1)
	if len(parts) != 2:
		return None

	label = parts[0].strip()
	if not label:
		return None

	rest = parts[1].strip()
	if not rest.startswith('"'):
		return None

	# Find closing quote for true-text
	after_open = rest[1:]
	close_idx = find_unescaped_quote(after_open)
	if close_idx is None:
		return None
	true_text = after_open[:close_idx].replace('\\"', '"')

	# After closing quote, look for ::
	remainder = after_open[close_idx + 1:].strip()
	false_text = ""
	if remainder.startswith("::"):
		after_sep = remainder[2:].strip()
		if after_sep.startswith('"'):
			inner = after_sep[1:]
			end = find_unescaped_quote(inner)
			if end is not None:
				false_text = inner[:end].replace('\\"', '"')

	return (label, true_text, false_text)


def extract_nested_variables(text: str) -> list[str]:
	"""
	Extract all {Variable Name} references from text.
	Only returns simple (non-nested) variables — content must not contain '{'.
	"""
	result = []
	i = 0
	while i < len(text):
		if text[i] == "{":
			inner = []
			i += 1
			found_close = False
			while i < len(text):
				if text[i] == "}":
					found_close = True
					i += 1
					break
				inner.append(text[i])
				i += 1
			inner_str = "".join(inner)
			if found_close and inner_str and "{" not in inner_str:
				result.append(inner_str.strip())
		else:
			i += 1
	return result


# ── Contact-role dot notation ───────────────────────────────────────────────

def parse_contact_role_ref(text: str) -> tuple[str, str] | None:
	"""
	Parse "Role.property" contact-role dot notation.
	Returns (role, property) if valid, None otherwise.
	"""
	dot_pos = text.rfind(".")
	if dot_pos < 0:
		return None
	role = text[:dot_pos].strip()
	prop = text[dot_pos + 1:].strip()
	if not role or not prop:
		return None
	prop_lower = prop.lower()
	if prop_lower in CONTACT_PROPERTIES:
		return (role, prop_lower)
	return None


def property_to_title(prop: str) -> str:
	"""Convert a contact property key like "full_name" to "Full Name"."""
	return " ".join(
		word[0].upper() + word[1:] if word else ""
		for word in prop.split("_")
	)


def contact_role_to_flat_name(role: str, prop: str) -> str:
	"""Convert Role + property to flat name: "Healthcare POA Agent" + "full_name" → "Healthcare POA Agent Full Name"."""
	return f"{role} {property_to_title(prop)}"


# ── Casing rules ────────────────────────────────────────────────────────────

def is_title_case(s: str) -> bool:
	"""Check if string is title case (first letter of each word uppercase, rest lowercase)."""
	for word in s.split():
		chars = list(word)
		if not chars:
			continue
		if not chars[0].isupper():
			return False
		if any(c.isupper() for c in chars[1:]):
			return False
	return True


def to_title_case(s: str) -> str:
	"""
	Convert to title case, capitalizing first letter of each word (by whitespace)
	and each hyphenated segment within a word.
	"""
	words = s.split()
	result_words = []
	for word in words:
		parts = word.split("-")
		titled_parts = []
		for part in parts:
			if not part:
				titled_parts.append("")
				continue
			titled_parts.append(part[0].upper() + part[1:].lower())
		result_words.append("-".join(titled_parts))
	return " ".join(result_words)


def pick_display_name(variants: list[str]) -> str:
	"""
	Choose the best display name from case variants.
	Priority: title-case > mixed-case > title-case-from-all-caps > first variant.
	"""
	# Prefer a title-case variant
	for v in variants:
		if is_title_case(v):
			return v
	# Prefer a mixed-case variant
	for v in variants:
		has_upper = any(c.isupper() for c in v)
		has_lower = any(c.islower() for c in v)
		if has_upper and has_lower:
			return v
	# If first variant is all-caps, generate title case
	first = variants[0]
	alpha = [c for c in first if c.isalpha()]
	if alpha and all(c.isupper() for c in alpha):
		return to_title_case(first)
	return first


def apply_casing(value: str, original_var_name: str) -> str:
	"""
	Apply the casing pattern of original_var_name to value.
	ALL CAPS → uppercase, all lower → lowercase, otherwise → as-is.
	"""
	alpha = [c for c in original_var_name if c.isalpha()]
	if not alpha:
		return value
	if all(c.isupper() for c in alpha):
		return value.upper()
	if all(c.islower() for c in alpha):
		return value.lower()
	return value


# ── Brace scanning (for nested conditionals) ───────────────────────────────

def scan_brace_content(text: str, start: int) -> tuple[str, int] | None:
	"""
	Starting just after an opening '{' at position `start` in `text`,
	consume characters tracking brace depth until the matching '}'.
	Returns (content, end_pos) where end_pos is just past the closing '}'.
	Returns None if no matching '}' is found.
	"""
	depth = 1
	content = []
	i = start
	while i < len(text):
		c = text[i]
		if c == "{":
			depth += 1
			content.append(c)
		elif c == "}":
			depth -= 1
			if depth == 0:
				return ("".join(content), i + 1)
			content.append(c)
		else:
			content.append(c)
		i += 1
	return None


# ── Split variable normalization ────────────────────────────────────────────

_T_ELEMENT_RE = re.compile(r'<w:t(?: [^>]*)?>([^<]*)</w:t>')


def normalize_split_variables(xml: str) -> str:
	"""
	Pre-process Word XML to merge variable placeholders that Word has split
	across multiple <w:r> runs.

	Matches Rust implementation: finds <w:t> elements with unbalanced opening
	braces, scans forward through subsequent <w:t> elements (within the same
	paragraph) to collect text until braces balance, then merges text into the
	first <w:t> and blanks subsequent ones.
	"""
	result = xml
	search_from = 0

	while True:
		remaining = result[search_from:]
		t_match = _T_ELEMENT_RE.search(remaining)
		if t_match is None:
			break

		abs_start = search_from + t_match.start()
		abs_end = search_from + t_match.end()
		text_content = t_match.group(1)

		# Check brace depth
		brace_depth = 0
		for ch in text_content:
			if ch == "{":
				brace_depth += 1
			elif ch == "}":
				brace_depth -= 1

		if brace_depth <= 0 or "{" not in text_content:
			search_from = abs_end
			continue

		# Unbalanced '{' — scan forward through subsequent <w:t> elements
		merged_text = text_content
		scan_pos = abs_end
		last_consumed_end = abs_end
		found_close = False

		while scan_pos < len(result):
			scan_remaining = result[scan_pos:]
			next_t = _T_ELEMENT_RE.search(scan_remaining)
			if next_t is None:
				break

			next_abs_start = scan_pos + next_t.start()
			next_abs_end = scan_pos + next_t.end()

			# Stop at paragraph boundary
			between = result[last_consumed_end:next_abs_start]
			if "</w:p>" in between:
				break

			next_text = next_t.group(1)
			merged_text += next_text
			last_consumed_end = next_abs_end

			for ch in next_text:
				if ch == "{":
					brace_depth += 1
				elif ch == "}":
					brace_depth -= 1

			if brace_depth <= 0:
				found_close = True
				break

			scan_pos = next_abs_end

		if not found_close:
			search_from = abs_end
			continue

		# Verify merged text contains a valid {Variable} pattern
		has_valid_var = False
		i = 0
		while i < len(merged_text):
			if merged_text[i] == "{":
				scanned = scan_brace_content(merged_text, i + 1)
				if scanned is not None:
					has_valid_var = True
					break
			i += 1

		if not has_valid_var:
			search_from = abs_end
			continue

		# Build replacement: merged text in first <w:t>, blank subsequent ones
		full_tag = result[abs_start:abs_end]
		tag_end_pos = full_tag.index(">") + 1
		opening_tag = full_tag[:tag_end_pos]

		if "xml:space" not in opening_tag:
			opening_tag = opening_tag.replace("<w:t>", '<w:t xml:space="preserve">')

		new_first_t = f"{opening_tag}{merged_text}</w:t>"

		# Blank intermediate <w:t> elements
		intermediate = result[abs_end:last_consumed_end]
		intermediate = _T_ELEMENT_RE.sub(
			lambda m: m.group(0)[:m.group(0).index(">") + 1] + "</w:t>",
			intermediate,
		)

		new_xml = result[:abs_start] + new_first_t + intermediate + result[last_consumed_end:]
		next_search = abs_start + len(new_first_t)
		result = new_xml
		search_from = next_search

	return result


# ── Variable extraction ─────────────────────────────────────────────────────

# Regex patterns for SDT and bookmark extraction
_SDT_TAG_RE = re.compile(r'<w:tag\s+w:val="((?:lily:|lily-cond:)([^"]*))"[^/]*/>')
_BOOKMARK_NAME_RE = re.compile(r'<w:bookmarkStart\s+[^>]*w:name="((?:lily:|lily-cond:)([^"]*))"')
_T_TEXT_RE = re.compile(r'<w:t(?:\s[^>]*)?>([^<]*)</w:t>')

# For extracting SDTs structurally
_SDT_FULL_RE = re.compile(r'(?s)<w:sdt>(.*?)</w:sdt>')
_SDT_TAG_VAL_RE = re.compile(r'<w:tag\s+w:val="([^"]*)"[^/]*/>')
_SDT_CONTENT_RE = re.compile(r'(?s)<w:sdtContent>(.*?)</w:sdtContent>')
_SDT_ALIAS_RE = re.compile(r'<w:alias\s+w:val="([^"]*)"[^/]*/>')
_RPR_RE = re.compile(r'(?s)<w:rPr>(.*?)</w:rPr>')


class VariableInfo:
	"""Info about a single logical variable, matching the Rust VariableInfo struct."""

	def __init__(self, display_name: str, variants: list[str], is_conditional: bool):
		self.display_name = display_name
		self.variants = variants
		self.is_conditional = is_conditional

	def __repr__(self):
		kind = "conditional" if self.is_conditional else "replacement"
		return f"VariableInfo({self.display_name!r}, {kind}, variants={self.variants})"

	def to_dict(self) -> dict:
		return {
			"display_name": self.display_name,
			"variants": self.variants,
			"is_conditional": self.is_conditional,
		}


def find_all_variables(xml: str) -> list[VariableInfo]:
	"""
	Single-pass extraction of all variables from Word XML, in document order.
	Finds {Placeholder} patterns in <w:t> text, Lily SDT tags, and Lily bookmarks.
	Matches the Rust find_all_variables() implementation.
	"""
	keys_in_order: list[str] = []
	groups: dict[str, list[str]] = {}
	conditional_keys: set[str] = set()

	def _register(display_name: str, is_cond: bool):
		key = display_name.lower()
		if is_cond:
			conditional_keys.add(key)
		if key not in groups:
			keys_in_order.append(key)
		variants = groups.setdefault(key, [])
		if display_name not in variants:
			variants.append(display_name)

	def _register_nested_from_branch(branch_text: str):
		for nested in extract_nested_variables(branch_text):
			ref = parse_contact_role_ref(nested)
			if ref:
				nkey_name = contact_role_to_flat_name(ref[0], ref[1])
			else:
				nkey_name = nested
			nkey = nkey_name.lower()
			if nkey not in groups:
				keys_in_order.append(nkey)
			nvariants = groups.setdefault(nkey, [])
			if nested not in nvariants:
				nvariants.append(nested)

	# We use regex-based scanning instead of an XML parser for parity with Rust,
	# which uses a mix of event-based XML parsing and regex.
	# Process the XML sequentially, looking for SDT tags, bookmarks, and text content.

	# 1. Find all SDT tags and bookmarks with their positions
	events: list[tuple[int, str, str, bool]] = []  # (position, type, value, is_cond)

	for m in _SDT_TAG_RE.finditer(xml):
		full_val = m.group(1)
		name = m.group(2)
		is_cond = full_val.startswith(COND_SDT_TAG_PREFIX)
		events.append((m.start(), "sdt", name, is_cond))

	for m in _BOOKMARK_NAME_RE.finditer(xml):
		full_val = m.group(1)
		name = m.group(2)
		is_cond = full_val.startswith(COND_SDT_TAG_PREFIX)
		events.append((m.start(), "bookmark", name, is_cond))

	# 2. Find text content and scan for {Variable} patterns
	for m in _T_TEXT_RE.finditer(xml):
		# Skip <w:t> elements inside SDTs — those contain resolved values, not placeholders
		# Check by looking for <w:sdtContent> context
		text = m.group(1)
		pos = m.start()

		# Scan for {Variable} patterns with brace-depth tracking
		i = 0
		while i < len(text):
			if text[i] == "{":
				scanned = scan_brace_content(text, i + 1)
				if scanned is not None:
					var_name, end_pos = scanned
					if var_name:
						trimmed = var_name.strip()
						if is_conditional_variable(trimmed):
							parsed = parse_conditional_variable(trimmed)
							if parsed:
								label, true_text, false_text = parsed
								events.append((pos + i, "cond_placeholder", trimmed, True))
								# Register nested variables from branches
								for branch in [true_text, false_text]:
									for nested in extract_nested_variables(branch):
										ref = parse_contact_role_ref(nested)
										if ref:
											nested_name = contact_role_to_flat_name(ref[0], ref[1])
										else:
											nested_name = nested
										events.append((pos + i + 1, "nested", nested_name, False))
										# Also add the raw form
										events.append((pos + i + 1, "nested_raw", nested, False))
						elif "{" not in trimmed:
							events.append((pos + i, "placeholder", trimmed, False))
					i = end_pos
				else:
					i += 1
			else:
				i += 1

	# Sort by position to get document order
	events.sort(key=lambda e: e[0])

	# 3. Process events in order
	for _pos, event_type, value, is_cond in events:
		if event_type in ("sdt", "bookmark"):
			_register(value, is_cond)
		elif event_type == "cond_placeholder":
			parsed = parse_conditional_variable(value)
			if parsed:
				label, true_text, false_text = parsed
				_register(value, True)
				_register_nested_from_branch(true_text)
				_register_nested_from_branch(false_text)
		elif event_type == "placeholder":
			ref = parse_contact_role_ref(value)
			if ref:
				flat_name = contact_role_to_flat_name(ref[0], ref[1])
				key = flat_name.lower()
			else:
				key = value.lower()
			if key not in groups:
				keys_in_order.append(key)
			variants = groups.setdefault(key, [])
			if value not in variants:
				variants.append(value)
		elif event_type == "nested":
			key = value.lower()
			if key not in groups:
				keys_in_order.append(key)
			groups.setdefault(key, [])
		elif event_type == "nested_raw":
			key_for_raw = value.lower()
			ref = parse_contact_role_ref(value)
			if ref:
				key_for_raw = contact_role_to_flat_name(ref[0], ref[1]).lower()
			variants = groups.setdefault(key_for_raw, [])
			if value not in variants:
				variants.append(value)

	# 4. Build result
	result = []
	for key in keys_in_order:
		variants = groups.get(key)
		if not variants:
			continue
		is_cond = key in conditional_keys
		if is_cond:
			# For conditional variables, derive display name from label
			raw = variants[0]
			parsed = parse_conditional_variable(raw)
			if parsed:
				display_name = parsed[0]
			else:
				display_name = pick_display_name(variants)
		else:
			# Check for contact-role dot notation
			role_ref = None
			for v in variants:
				role_ref = parse_contact_role_ref(v)
				if role_ref:
					break
			if role_ref:
				display_name = contact_role_to_flat_name(role_ref[0], role_ref[1])
			else:
				display_name = pick_display_name(variants)

		result.append(VariableInfo(display_name, variants, is_cond))

	return result


def extract_variables(docx_path: str) -> list[VariableInfo]:
	"""
	Extract all variables from a DOCX file.
	Reads all variable-bearing XML parts, normalizes split variables, then extracts.
	"""
	parts = read_docx_xml_parts(docx_path)
	all_vars: list[VariableInfo] = []
	seen_keys: dict[str, int] = {}

	for _part_name, xml in parts.items():
		normalized = normalize_split_variables(xml)
		part_vars = find_all_variables(normalized)

		for var in part_vars:
			key = var.display_name.lower()
			if key in seen_keys:
				# Merge variants into existing entry
				idx = seen_keys[key]
				existing = all_vars[idx]
				for v in var.variants:
					if v not in existing.variants:
						existing.variants.append(v)
				if var.is_conditional:
					existing.is_conditional = True
			else:
				seen_keys[key] = len(all_vars)
				all_vars.append(var)

	return all_vars


# ── SDT structural extraction ──────────────────────────────────────────────

class SDTInfo:
	"""Detailed info about a single SDT content control in the document."""

	def __init__(
		self,
		tag: str,
		alias: str,
		sdt_id: int | None,
		prefix: str,
		variable_name: str,
		is_conditional: bool,
		content_text: str,
		rpr_xml: str,
		full_xml: str,
		part_name: str,
		position: int,
	):
		self.tag = tag
		self.alias = alias
		self.sdt_id = sdt_id
		self.prefix = prefix
		self.variable_name = variable_name
		self.is_conditional = is_conditional
		self.content_text = content_text
		self.rpr_xml = rpr_xml
		self.full_xml = full_xml
		self.part_name = part_name
		self.position = position

	def to_dict(self) -> dict:
		return {
			"tag": self.tag,
			"alias": self.alias,
			"sdt_id": self.sdt_id,
			"prefix": self.prefix,
			"variable_name": self.variable_name,
			"is_conditional": self.is_conditional,
			"content_text": self.content_text,
			"part_name": self.part_name,
			"position": self.position,
		}


class BookmarkInfo:
	"""Info about a Lily bookmark (placeholder for an empty SDT value)."""

	def __init__(
		self,
		name: str,
		bookmark_id: int,
		prefix: str,
		variable_name: str,
		is_conditional: bool,
		part_name: str,
		position: int,
	):
		self.name = name
		self.bookmark_id = bookmark_id
		self.prefix = prefix
		self.variable_name = variable_name
		self.is_conditional = is_conditional
		self.part_name = part_name
		self.position = position

	def to_dict(self) -> dict:
		return {
			"name": self.name,
			"bookmark_id": self.bookmark_id,
			"prefix": self.prefix,
			"variable_name": self.variable_name,
			"is_conditional": self.is_conditional,
			"part_name": self.part_name,
			"position": self.position,
		}


class PlaceholderInfo:
	"""Info about a {Variable} text placeholder (unconverted to SDT)."""

	def __init__(
		self,
		raw_text: str,
		display_name: str,
		is_conditional: bool,
		part_name: str,
		position: int,
	):
		self.raw_text = raw_text
		self.display_name = display_name
		self.is_conditional = is_conditional
		self.part_name = part_name
		self.position = position

	def to_dict(self) -> dict:
		return {
			"raw_text": self.raw_text,
			"display_name": self.display_name,
			"is_conditional": self.is_conditional,
			"part_name": self.part_name,
			"position": self.position,
		}


_BOOKMARK_FULL_RE = re.compile(
	r'<w:bookmarkStart\s+w:id="(\d+)"\s+w:name="((?:lily:|lily-cond:)([^"]*))"[^/]*/>'
)


def find_sdts(xml: str, part_name: str = "word/document.xml") -> list[SDTInfo]:
	"""Find all Lily SDT content controls in an XML string."""
	sdts = []
	for m in _SDT_FULL_RE.finditer(xml):
		inner = m.group(1)
		full_xml = m.group(0)
		pos = m.start()

		# Extract tag value
		tag_m = _SDT_TAG_VAL_RE.search(inner)
		if not tag_m:
			continue
		tag_val = tag_m.group(1)

		# Check for Lily prefix
		if tag_val.startswith(COND_SDT_TAG_PREFIX):
			prefix = COND_SDT_TAG_PREFIX
			var_name = tag_val[len(COND_SDT_TAG_PREFIX):]
			is_cond = True
		elif tag_val.startswith(SDT_TAG_PREFIX):
			prefix = SDT_TAG_PREFIX
			var_name = tag_val[len(SDT_TAG_PREFIX):]
			is_cond = False
		else:
			continue

		# Extract alias
		alias_m = _SDT_ALIAS_RE.search(inner)
		alias = alias_m.group(1) if alias_m else var_name

		# Extract ID
		id_m = _SDT_ID_RE.search(inner)
		sdt_id = int(id_m.group(1)) if id_m else None

		# Extract content text
		content_text = ""
		content_m = _SDT_CONTENT_RE.search(full_xml)
		if content_m:
			# Get text from all <w:t> elements within content
			for t_m in _T_TEXT_RE.finditer(content_m.group(1)):
				content_text += unescape_xml_text(t_m.group(1))

		# Extract run properties
		rpr_xml = ""
		rpr_m = _RPR_RE.search(inner)
		if rpr_m:
			rpr_xml = rpr_m.group(0)
		elif content_m:
			rpr_m = _RPR_RE.search(content_m.group(1))
			if rpr_m:
				rpr_xml = rpr_m.group(0)

		sdts.append(SDTInfo(
			tag=tag_val,
			alias=alias,
			sdt_id=sdt_id,
			prefix=prefix,
			variable_name=var_name,
			is_conditional=is_cond,
			content_text=content_text,
			rpr_xml=rpr_xml,
			full_xml=full_xml,
			part_name=part_name,
			position=pos,
		))

	return sdts


def find_bookmarks(xml: str, part_name: str = "word/document.xml") -> list[BookmarkInfo]:
	"""Find all Lily bookmarks in an XML string."""
	bookmarks = []
	for m in _BOOKMARK_FULL_RE.finditer(xml):
		bm_id = int(m.group(1))
		full_name = m.group(2)
		var_name = m.group(3)

		if full_name.startswith(COND_BOOKMARK_PREFIX):
			prefix = COND_BOOKMARK_PREFIX
			is_cond = True
		else:
			prefix = BOOKMARK_PREFIX
			is_cond = False

		bookmarks.append(BookmarkInfo(
			name=full_name,
			bookmark_id=bm_id,
			prefix=prefix,
			variable_name=var_name,
			is_conditional=is_cond,
			part_name=part_name,
			position=m.start(),
		))

	return bookmarks


def find_placeholders(xml: str, part_name: str = "word/document.xml") -> list[PlaceholderInfo]:
	"""Find all {Variable} text placeholders in an XML string (not inside SDTs)."""
	# First, mask out SDT content so we don't find placeholders inside SDTs
	masked = _SDT_FULL_RE.sub(lambda m: " " * len(m.group(0)), xml)

	placeholders = []
	for t_m in _T_TEXT_RE.finditer(masked):
		text = t_m.group(1)
		pos = t_m.start()

		i = 0
		while i < len(text):
			if text[i] == "{":
				scanned = scan_brace_content(text, i + 1)
				if scanned is not None:
					var_content, end_pos = scanned
					if var_content and var_content.strip():
						trimmed = var_content.strip()
						is_cond = is_conditional_variable(trimmed)
						if is_cond:
							parsed = parse_conditional_variable(trimmed)
							display = parsed[0] if parsed else trimmed
						else:
							ref = parse_contact_role_ref(trimmed)
							if ref:
								display = contact_role_to_flat_name(ref[0], ref[1])
							else:
								display = trimmed
						placeholders.append(PlaceholderInfo(
							raw_text=f"{{{var_content}}}",
							display_name=display,
							is_conditional=is_cond,
							part_name=part_name,
							position=pos + i,
						))
					i = end_pos
				else:
					i += 1
			else:
				i += 1

	return placeholders


# ── SDT generation ──────────────────────────────────────────────────────────

def make_sdt_xml(
	sdt_id: int,
	variable_name: str,
	value: str,
	is_conditional: bool = False,
	rpr_xml: str = "",
) -> str:
	"""
	Generate the XML for a Lily SDT content control.
	Matches the Rust format exactly.
	"""
	tag_prefix = COND_SDT_TAG_PREFIX if is_conditional else SDT_TAG_PREFIX
	escaped_display = escape_xml_text(variable_name)
	escaped_val = escape_xml_text(value)

	return (
		f'<w:sdt>'
		f'<w:sdtPr>'
		f'<w:id w:val="{sdt_id}"/>'
		f'<w:tag w:val="{tag_prefix}{escaped_display}"/>'
		f'<w:alias w:val="{escaped_display}"/>'
		f'</w:sdtPr>'
		f'<w:sdtContent>'
		f'<w:r>{rpr_xml}<w:t xml:space="preserve">{escaped_val}</w:t></w:r>'
		f'</w:sdtContent>'
		f'</w:sdt>'
	)


def make_bookmark_xml(bookmark_id: int, variable_name: str, is_conditional: bool = False) -> str:
	"""
	Generate the XML for a zero-width Lily bookmark (empty SDT value).
	Matches the Rust format exactly.
	"""
	prefix = COND_BOOKMARK_PREFIX if is_conditional else BOOKMARK_PREFIX
	escaped_name = escape_xml_text(variable_name)
	return (
		f'<w:bookmarkStart w:id="{bookmark_id}" w:name="{prefix}{escaped_name}"/>'
		f'<w:bookmarkEnd w:id="{bookmark_id}"/>'
	)


# ── Placeholder-to-SDT conversion ──────────────────────────────────────────

_RUN_RE = re.compile(r'(?s)<w:r\b[^>]*>.*?</w:r>')
_T_CONTENT_RE = re.compile(r'<w:t(?:\s[^>]*)?>([^<]*)</w:t>')


def replace_placeholders_with_sdt(
	xml: str,
	replacements: dict[str, tuple[str, str, bool]],
	next_id: int,
) -> tuple[str, int]:
	"""
	Convert {Variable Name} placeholders to SDT content controls.

	replacements: dict mapping variable_name → (display_name, value, is_conditional)
	next_id: the next available ID for SDTs

	Returns (modified_xml, next_id_after).
	Matches Rust replace_placeholders_with_sdt_v2().
	"""
	if not replacements:
		return xml, next_id

	result = []
	last_end = 0
	current_id = next_id

	for run_match in _RUN_RE.finditer(xml):
		run_str = run_match.group(0)

		# Check if this run contains any placeholder
		has_replacement = False
		t_caps = _T_CONTENT_RE.search(run_str)
		if t_caps:
			text = t_caps.group(1)
			for var_name in replacements:
				pattern = f"{{{var_name}}}"
				if pattern in text:
					has_replacement = True
					break

		if not has_replacement:
			result.append(xml[last_end:run_match.end()])
			last_end = run_match.end()
			continue

		result.append(xml[last_end:run_match.start()])

		# Extract run properties
		rpr_m = _RPR_RE.search(run_str)
		rpr = rpr_m.group(0) if rpr_m else ""

		t_caps = _T_CONTENT_RE.search(run_str)
		if t_caps:
			text = t_caps.group(1)
			output_parts = []

			while True:
				# Find earliest placeholder
				best = None
				for var_name, (display_name, value, is_cond) in replacements.items():
					pattern = f"{{{var_name}}}"
					pos = text.find(pattern)
					if pos >= 0:
						if best is None or pos < best[0]:
							best = (pos, var_name, display_name, value, is_cond)

				if best is None:
					if text:
						output_parts.append(
							f'<w:r>{rpr}<w:t xml:space="preserve">{escape_xml_text(text)}</w:t></w:r>'
						)
					break

				pos, var_name, display_name, value, is_cond = best
				pattern = f"{{{var_name}}}"

				before = text[:pos]
				if before:
					output_parts.append(
						f'<w:r>{rpr}<w:t xml:space="preserve">{escape_xml_text(before)}</w:t></w:r>'
					)

				sdt_xml = make_sdt_xml(current_id, display_name, value, is_cond, rpr)
				current_id += 1
				output_parts.append(sdt_xml)

				text = text[pos + len(pattern):]

			result.extend(output_parts)
		else:
			result.append(run_str)

		last_end = run_match.end()

	result.append(xml[last_end:])
	return "".join(result), current_id


# ── Paragraph text mapping (for text search/replace) ────────────────────────

class TextSegment:
	"""Maps flat-text char offsets to XML byte ranges."""
	def __init__(self, xml_content_start: int, xml_content_end: int, text_start: int, text_end: int):
		self.xml_content_start = xml_content_start
		self.xml_content_end = xml_content_end
		self.text_start = text_start
		self.text_end = text_end


class ParagraphTextMap:
	"""A paragraph with its text segments and flat text."""
	def __init__(self, flat_text: str, segments: list[TextSegment], paragraph_number: int):
		self.flat_text = flat_text
		self.segments = segments
		self.paragraph_number = paragraph_number


_P_START_RE = re.compile(r'<w:p[\s>/]')
_P_END_RE = re.compile(r'</w:p>')


def build_paragraph_text_maps(xml: str) -> list[ParagraphTextMap]:
	"""
	Build text-to-XML mapping for all paragraphs.
	Maps flat text positions back to byte ranges in the XML.
	Matches Rust build_paragraph_text_maps().
	"""
	maps = []
	para_num = 0
	search_from = 0

	while True:
		remaining = xml[search_from:]
		p_start = _P_START_RE.search(remaining)
		if p_start is None:
			break

		para_start = search_from + p_start.start()
		para_num += 1
		after_start = para_start + p_start.end() - p_start.start()

		p_end = _P_END_RE.search(xml[after_start:])
		if p_end is None:
			search_from = after_start
			continue

		para_end = after_start + p_end.end()
		para_xml = xml[para_start:para_end]

		flat_text = ""
		segments = []

		for t_match in _T_ELEMENT_RE.finditer(para_xml):
			content = t_match.group(1)
			xml_content_start = para_start + t_match.start(1)
			xml_content_end = para_start + t_match.end(1)
			text_start = len(flat_text)
			decoded = unescape_xml_text(content)
			flat_text += decoded
			text_end = len(flat_text)

			segments.append(TextSegment(
				xml_content_start=xml_content_start,
				xml_content_end=xml_content_end,
				text_start=text_start,
				text_end=text_end,
			))

		if flat_text:
			maps.append(ParagraphTextMap(
				flat_text=flat_text,
				segments=segments,
				paragraph_number=para_num,
			))

		search_from = para_end

	return maps


def find_text_in_paragraphs(
	maps: list[ParagraphTextMap],
	search_text: str,
) -> list[tuple[int, int]]:
	"""
	Find all occurrences of search_text across paragraphs.
	Returns (paragraph_index, char_offset_in_flat_text) pairs.
	"""
	results = []
	for pi, para in enumerate(maps):
		start = 0
		while True:
			pos = para.flat_text.find(search_text, start)
			if pos < 0:
				break
			results.append((pi, pos))
			start = pos + 1
	return results


def replace_text_in_xml(
	xml: str,
	para: ParagraphTextMap,
	text_offset: int,
	search_len: int,
	replacement: str,
) -> str:
	"""
	Replace text at a specific position in the XML, handling cross-segment spans.
	Matches Rust replace_text_in_xml().
	"""
	text_end = text_offset + search_len

	first_seg = None
	last_seg = None
	for i, seg in enumerate(para.segments):
		if seg.text_end > text_offset and seg.text_start < text_end:
			if first_seg is None:
				first_seg = i
			last_seg = i

	if first_seg is None:
		return xml

	if last_seg is None:
		last_seg = first_seg

	result = xml

	if first_seg == last_seg:
		# Single segment
		seg = para.segments[first_seg]
		seg_text_start = text_offset - seg.text_start
		seg_text_end = text_end - seg.text_start
		original = result[seg.xml_content_start:seg.xml_content_end]
		decoded = unescape_xml_text(original)
		new_decoded = decoded[:seg_text_start] + replacement + decoded[seg_text_end:]
		new_encoded = escape_xml_text(new_decoded)
		result = result[:seg.xml_content_start] + new_encoded + result[seg.xml_content_end:]
	else:
		# Multi-segment — process in reverse to preserve offsets
		for i in range(last_seg, first_seg, -1):
			seg = para.segments[i]
			original = result[seg.xml_content_start:seg.xml_content_end]
			decoded = unescape_xml_text(original)
			if i == last_seg:
				keep_start = text_end - seg.text_start
				remaining = decoded[keep_start:]
			else:
				remaining = ""
			result = result[:seg.xml_content_start] + escape_xml_text(remaining) + result[seg.xml_content_end:]

		# First segment
		seg = para.segments[first_seg]
		seg_text_start = text_offset - seg.text_start
		original = result[seg.xml_content_start:seg.xml_content_end]
		decoded = unescape_xml_text(original)
		new_decoded = decoded[:seg_text_start] + replacement
		result = result[:seg.xml_content_start] + escape_xml_text(new_decoded) + result[seg.xml_content_end:]

	return result


# ── Template schema I/O ─────────────────────────────────────────────────────
#
# Schemas are stored in a centralized template-library .lily file (not per-
# template sidecars).  The library file has lily_type "template-library" and
# a "templates" dict keyed by relative path from the library's directory.
#
# The functions below locate the library by walking up from the template,
# compute the relative key, and read/write the appropriate entry.


def _find_template_library(start: Path) -> Path | None:
	"""Walk up from *start* looking for a .lily file with lily_type 'template-library'."""
	for directory in [start.parent] + list(start.parent.parents):
		for candidate in directory.glob("*.lily"):
			try:
				with open(candidate, "r", encoding="utf-8") as f:
					data = json.load(f)
				if data.get("lily_type") == "template-library":
					return candidate
			except (json.JSONDecodeError, OSError):
				continue
	return None


def _template_key(library_path: Path, template_path: Path) -> str:
	"""Compute the relative key for a template within its library."""
	try:
		return str(template_path.resolve().relative_to(library_path.parent.resolve()))
	except ValueError:
		return template_path.name


def schema_path_for_template(template_path: str) -> Path:
	"""Return the path to the template-library .lily file that contains
	this template's schema.  Falls back to a per-template sidecar path
	if no library is found (for backwards compatibility)."""
	p = Path(template_path).resolve()
	lib = _find_template_library(p)
	return lib if lib else p.with_suffix(".lily")


def load_template_schema(template_path: str) -> dict:
	"""
	Load the schema for a template from the template-library .lily file.
	Returns a default empty schema if no library or entry is found.
	"""
	p = Path(template_path).resolve()
	template_filename = p.name

	lib_path = _find_template_library(p)
	if lib_path is None:
		return {
			"lily_type": "template-schema",
			"template_filename": template_filename,
			"variables": {},
		}

	with open(lib_path, "r", encoding="utf-8") as f:
		library = json.load(f)

	key = _template_key(lib_path, p)
	entry = library.get("templates", {}).get(key, {})

	return {
		"lily_type": "template-schema",
		"template_filename": template_filename,
		"variables": entry.get("variables", {}),
	}


def save_template_schema(template_path: str, schema: dict) -> Path:
	"""
	Save the schema for a template into the template-library .lily file.
	Creates the library entry if it doesn't exist.  Returns the library path.
	"""
	p = Path(template_path).resolve()
	lib_path = _find_template_library(p)

	if lib_path is None:
		# No library found — create one next to the template's parent dir
		lib_path = p.parent.parent / "Templates.lily"
		library = {"lily_type": "template-library", "templates": {}}
	else:
		with open(lib_path, "r", encoding="utf-8") as f:
			library = json.load(f)

	key = _template_key(lib_path, p)
	if "templates" not in library:
		library["templates"] = {}

	library["templates"][key] = {
		"variables": schema.get("variables", {}),
	}

	with open(lib_path, "w", encoding="utf-8") as f:
		json.dump(library, f, indent="\t", ensure_ascii=False)
		f.write("\n")

	return lib_path


# ── Document text and snapshot helpers ──────────────────────────────────────

def extract_document_text(docx_path: Path) -> tuple[list[str], dict]:
	"""Return the normalized paragraph lines and document metadata."""
	entries = read_docx_xml_parts(str(docx_path))
	root = ET.fromstring(entries["word/document.xml"])
	word = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"

	lines: list[str] = []
	styles: set[str] = set()
	paragraph_count = 0
	for paragraph in root.iter(word + "p"):
		text = "".join(node.text or "" for node in paragraph.iter(word + "t"))
		text = re.sub(r"\s+", " ", text).strip()
		style = paragraph.find(f"{word}pPr/{word}pStyle")
		if style is not None:
			value = style.get(f"{word}val")
			if value:
				styles.add(value)
		if text:
			paragraph_count += 1
			lines.append(text)

	table_count = sum(1 for _ in root.iter(word + "tbl"))
	meta = {
		"paragraphs": paragraph_count,
		"tables": table_count,
		"styles": sorted(styles),
	}
	return lines, meta


def render_snapshot(name: str, template_rel: str, lines: list[str], meta: dict) -> str:
	header = [
		f"# Scenario: {name}",
		f"# Template: {template_rel}",
		f"# Paragraphs: {meta['paragraphs']}",
		f"# Tables: {meta['tables']}",
		f"# Styles: {', '.join(meta['styles'])}",
		"# ---",
	]
	return "\n".join(header + lines) + "\n"
