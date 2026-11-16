#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""
Regenerate and check the golden corpus for the current Lily fill engine.

The corpus lives in the private Carelaw repository because its documents carry
the firm's legal text. This script is the harness: it reads a manifest, sets up
a scratch client folder per scenario, runs the headless `lily-fill` binary, and
writes a normalized text snapshot that can be diffed across engine changes.

Usage:
	python3 scripts/run_corpus.py --corpus ~/Carelaw/carelaw-lily/Tests --update
	python3 scripts/run_corpus.py --corpus ~/Carelaw/carelaw-lily/Tests --check

Options:
	--corpus PATH    Corpus root containing manifest.json (required)
	--binary PATH    lily-fill binary (default: target/debug/lily-fill)
	--work-dir PATH  Scratch directory (default: .working/corpus-run)
	--update         Write snapshots
	--check          Compare snapshots and exit non-zero on differences
	--only NAME      Run one scenario by name (repeatable)
	--list           List scenario names and exit
	--json           Print a JSON report instead of human-readable text
"""

import argparse
import difflib
import json
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path
import xml.etree.ElementTree as ET

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
REPO_ROOT = Path(__file__).resolve().parent.parent


def load_manifest(corpus_root: Path) -> dict:
	path = corpus_root / "manifest.json"
	if not path.is_file():
		raise SystemExit(f"Manifest not found: {path}")
	return json.loads(path.read_text())


def scenario_dir(corpus_root: Path, scenario: dict) -> Path:
	return corpus_root / scenario["client"]


def snapshot_path(corpus_root: Path, scenario: dict) -> Path:
	return corpus_root / scenario["snapshot"]


def extract_document_text(docx_path: Path) -> tuple[list[str], dict]:
	with zipfile.ZipFile(docx_path) as archive:
		xml = archive.read("word/document.xml")
	root = ET.fromstring(xml)

	lines: list[str] = []
	styles: set[str] = set()
	paragraph_count = 0
	for paragraph in root.iter(W + "p"):
		text = "".join(node.text or "" for node in paragraph.iter(W + "t"))
		text = re.sub(r"\s+", " ", text).strip()
		style = paragraph.find(f"{W}pPr/{W}pStyle")
		if style is not None:
			value = style.get(f"{W}val")
			if value:
				styles.add(value)
		if text:
			paragraph_count += 1
			lines.append(text)

	table_count = sum(1 for _ in root.iter(W + "tbl"))
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


def run_scenario(
	binary: Path,
	corpus_root: Path,
	templates_dir: Path,
	work_root: Path,
	scenario: dict,
) -> tuple[str, str]:
	name = scenario["name"]
	work_dir = work_root / name
	if work_dir.exists():
		shutil.rmtree(work_dir)
	work_dir.mkdir(parents=True)

	client_source = scenario_dir(corpus_root, scenario)
	if not client_source.is_file():
		raise SystemExit(f"Client fixture not found: {client_source}")
	docx_name = scenario["docx"]
	shutil.copy(client_source, work_dir / client_source.name)
	shutil.copy(templates_dir / scenario["template"], work_dir / docx_name)

	output = work_dir / "filled.docx"
	command = [
		str(binary),
		"--client-dir",
		str(work_dir),
		"--docx",
		docx_name,
		"--templates-dir",
		str(templates_dir),
		"--template-rel-path",
		scenario["template"],
		"--output",
		str(output),
		"--json",
	]
	result = subprocess.run(command, capture_output=True, text=True)
	if result.returncode != 0:
		raise SystemExit(
			f"lily-fill failed for {name}:\n{result.stderr.strip() or result.stdout.strip()}"
		)

	lines, meta = extract_document_text(output)
	return render_snapshot(name, scenario["template"], lines, meta), str(work_dir)


def main() -> int:
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument("--corpus", required=True, help="Corpus root containing manifest.json")
	parser.add_argument(
		"--binary",
		default=str(REPO_ROOT / "target" / "debug" / "lily-fill"),
		help="Path to the lily-fill binary",
	)
	parser.add_argument("--work-dir", default=None, help="Scratch directory for runs")
	parser.add_argument("--update", action="store_true", help="Write snapshots")
	parser.add_argument("--check", action="store_true", help="Compare snapshots")
	parser.add_argument("--only", action="append", default=[], help="Scenario name filter")
	parser.add_argument("--list", action="store_true", help="List scenarios")
	parser.add_argument("--json", action="store_true", help="JSON report")
	args = parser.parse_args()

	if not args.update and not args.check and not args.list:
		parser.error("choose --update, --check, or --list")

	corpus_root = Path(args.corpus).expanduser().resolve()
	manifest = load_manifest(corpus_root)
	templates_dir = (corpus_root / manifest["templates_dir"]).resolve()
	binary = Path(args.binary).expanduser().resolve()
	if not binary.is_file():
		raise SystemExit(
			f"lily-fill binary not found: {binary}\n"
			"Build it with: cargo build --bin lily-fill (from apps/desktop/backend)"
		)

	scenarios = manifest["scenarios"]
	if args.only:
		selected = set(args.only)
		scenarios = [scenario for scenario in scenarios if scenario["name"] in selected]
		missing = selected - {scenario["name"] for scenario in scenarios}
		if missing:
			raise SystemExit(f"Unknown scenario(s): {', '.join(sorted(missing))}")

	if args.list:
		for scenario in scenarios:
			print(scenario["name"])
		return 0

	work_root = (
		Path(args.work_dir).expanduser().resolve()
		if args.work_dir
		else REPO_ROOT / ".working" / "corpus-run"
	)
	work_root.mkdir(parents=True, exist_ok=True)

	results = []
	has_diff = False
	for scenario in scenarios:
		snapshot, work_dir = run_scenario(
			binary, corpus_root, templates_dir, work_root, scenario
		)
		target = snapshot_path(corpus_root, scenario)
		entry = {
			"name": scenario["name"],
			"work_dir": work_dir,
			"snapshot": str(target),
			"status": "written" if args.update else "unchanged",
		}

		if args.update:
			target.parent.mkdir(parents=True, exist_ok=True)
			target.write_text(snapshot)
		elif args.check:
			if not target.is_file():
				entry["status"] = "missing"
				has_diff = True
			else:
				expected = target.read_text()
				if expected != snapshot:
					entry["status"] = "diff"
					has_diff = True
					entry["diff"] = "".join(
						difflib.unified_diff(
							expected.splitlines(keepends=True),
							snapshot.splitlines(keepends=True),
							fromfile=f"{target.name} (snapshot)",
							tofile=f"{target.name} (current)",
						)
					)
		results.append(entry)

	if args.json:
		print(json.dumps({"results": results, "has_diff": has_diff}, indent=2))
	else:
		for entry in results:
			print(f"{entry['status']:>9}  {entry['name']}")
			if entry.get("diff"):
				print(entry["diff"])
		if args.update:
			print(f"\nWrote {len(results)} snapshot(s).")
		elif has_diff:
			print("\nCorpus differs from the snapshots.")
		else:
			print("\nCorpus matches the snapshots.")

	return 1 if has_diff else 0


if __name__ == "__main__":
	sys.exit(main())
