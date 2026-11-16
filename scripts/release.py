#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Release helpers for Lily.

Usage:
	scripts/release.py version
	scripts/release.py check
	scripts/release.py bump 0.9.0 [--tag] [--dry-run]
	scripts/release.py tag 0.9.0
	scripts/release.py notes [--from v0.9.0] [--to v1.0.0] [--output FILE]
	scripts/release.py publish --tag v1.0.0 --assets-dir dist

The bump command updates every file that carries the version and commits the
change. After the bump merges, the tag command creates the annotated tag on
main. Pushing the tag starts the release workflow; both pushes stay deliberate.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
JSON_FILES = [
	ROOT / "package.json",
	ROOT / "apps/desktop/frontend/package.json",
	ROOT / "apps/desktop/backend/tauri.conf.json",
]
CARGO_TOML = ROOT / "apps/desktop/backend/Cargo.toml"
CARGO_LOCK = ROOT / "apps/desktop/backend/Cargo.lock"
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")


def fail(message: str) -> None:
	print(f"error: {message}", file=sys.stderr)
	raise SystemExit(1)


def git(*args: str) -> str:
	result = subprocess.run(
		["git", *args],
		cwd=ROOT,
		check=False,
		capture_output=True,
		text=True,
	)
	if result.returncode != 0:
		fail(result.stderr.strip() or f"git {' '.join(args)} failed")
	return result.stdout


def read_json_version(path: Path) -> str:
	try:
		value = json.loads(path.read_text(encoding="utf-8"))
	except (OSError, json.JSONDecodeError) as error:
		fail(f"cannot read {path.relative_to(ROOT)}: {error}")
	version = value.get("version")
	if not isinstance(version, str) or not version:
		fail(f"no version in {path.relative_to(ROOT)}")
	return version


def read_cargo_version(path: Path) -> str:
	try:
		text = path.read_text(encoding="utf-8")
	except OSError as error:
		fail(f"cannot read {path.relative_to(ROOT)}: {error}")
	match = re.search(r'(?m)^version = "([^"]+)"', text)
	if not match:
		fail(f"no package version in {path.relative_to(ROOT)}")
	return match.group(1)


def read_lock_version(path: Path) -> str:
	try:
		text = path.read_text(encoding="utf-8")
	except OSError as error:
		fail(f"cannot read {path.relative_to(ROOT)}: {error}")
	match = re.search(r'(?m)^name = "lily"\nversion = "([^"]+)"', text)
	if not match:
		fail(f"no lily package version in {path.relative_to(ROOT)}")
	return match.group(1)


def current_versions() -> list[tuple[str, str]]:
	return [
		("package.json", read_json_version(JSON_FILES[0])),
		("apps/desktop/frontend/package.json", read_json_version(JSON_FILES[1])),
		("apps/desktop/backend/tauri.conf.json", read_json_version(JSON_FILES[2])),
		("apps/desktop/backend/Cargo.toml", read_cargo_version(CARGO_TOML)),
		("apps/desktop/backend/Cargo.lock", read_lock_version(CARGO_LOCK)),
	]


def current_version() -> str:
	return current_versions()[0][1]


def parse_version(value: str) -> tuple[int, int, int]:
	if not VERSION_RE.match(value):
		fail(f"'{value}' is not a version like 1.2.3")
	parts = value.split(".")
	return int(parts[0]), int(parts[1]), int(parts[2])


def replace_once(path: Path, pattern: str, replacement: str) -> None:
	text = path.read_text(encoding="utf-8")
	updated, count = re.subn(pattern, replacement, text, count=1)
	if count != 1:
		fail(f"could not find the version in {path.relative_to(ROOT)}")
	path.write_text(updated, encoding="utf-8")


def command_check(_: argparse.Namespace) -> None:
	versions = current_versions()
	mismatched = False
	for label, value in versions:
		print(f"{label}: {value}")
		if value != versions[0][1]:
			mismatched = True
	if mismatched:
		fail("version files disagree")
	print(f"Version files agree: {versions[0][1]}")


def command_version(_: argparse.Namespace) -> None:
	print(current_version())


def command_bump(args: argparse.Namespace) -> None:
	current = current_version()
	new = args.version
	if parse_version(new) <= parse_version(current):
		fail(f"new version {new} must be greater than {current}")
	tag = f"v{new}"
	if args.tag and git("tag", "--list", tag).strip():
		fail(f"tag {tag} already exists")
	if not args.dry_run and not args.allow_dirty:
		dirty = git("status", "--porcelain", "--untracked-files=no").strip()
		if dirty:
			fail("working tree has changes; commit or stash them first")

	print(f"Bumping {current} -> {new}")
	targets = [
		(JSON_FILES[0], r'("version"\s*:\s*")[^"]*(")', rf"\g<1>{new}\g<2>"),
		(JSON_FILES[1], r'("version"\s*:\s*")[^"]*(")', rf"\g<1>{new}\g<2>"),
		(JSON_FILES[2], r'("version"\s*:\s*")[^"]*(")', rf"\g<1>{new}\g<2>"),
		(CARGO_TOML, r'(?m)^version = "[^"]*"', f'version = "{new}"'),
		(CARGO_LOCK, r'(?m)(^name = "lily"\nversion = ")[^"]*(")', rf"\g<1>{new}\g<2>"),
	]
	if args.dry_run:
		for path, _, _ in targets:
			print(f"  would update {path.relative_to(ROOT)}")
		print(f"  would commit 'Bump version to {new}'")
		if args.tag:
			print(f"  would tag {tag}")
		return
	for path, pattern, replacement in targets:
		replace_once(path, pattern, replacement)
		print(f"  -> {path.relative_to(ROOT)}")

	git("add", *[str(path.relative_to(ROOT)) for path, _, _ in targets])
	git("commit", "-m", f"Bump version to {new}")
	if args.tag:
		git("tag", "-a", tag, "-m", f"Lily {new}")
	print("")
	print(f"Committed {new}" + (f" and tagged {tag}" if args.tag else ""))
	branch = git("branch", "--show-current").strip() or "HEAD"
	print(f"Next: git push origin {branch}" + (f" {tag}" if args.tag else ""))
	if not args.tag:
		print(f"After the bump merges: make release-tag V={new}")


def repo_url() -> str:
	remote = git("remote", "get-url", "origin").strip()
	if remote.startswith("ssh://"):
		match = re.match(r"ssh://git@([^:/]+)(?::\d+)?/(.+?)(?:\.git)?$", remote)
		if match:
			return f"https://{match.group(1)}/{match.group(2)}"
	if remote.startswith("git@"):
		match = re.match(r"git@([^:]+):(.+?)(?:\.git)?$", remote)
		if match:
			return f"https://{match.group(1)}/{match.group(2)}"
	if remote.startswith("http"):
		return re.sub(r"\.git$", "", remote)
	return remote


def previous_tag(to_ref: str) -> str | None:
	tags = [
		tag
		for tag in git("tag", "--merged", to_ref, "--sort=-v:refname", "--list", "v[0-9]*").splitlines()
		if tag and tag != to_ref
	]
	return tags[0] if tags else None


def link_subject(subject: str, base: str) -> str:
	match = re.search(r"\s*\(#(\d+)\)$", subject)
	if not match:
		return subject
	number = match.group(1)
	subject = subject[: match.start()].strip()
	return f"{subject} ([#{number}]({base}/pulls/{number}))"


def command_tag(args: argparse.Namespace) -> None:
	version = (args.version or current_version()).removeprefix("v")
	if parse_version(version) != parse_version(current_version()):
		fail(
			f"version files say {current_version()}, not {version}; "
			"merge the matching bump first"
		)
	branch = git("branch", "--show-current").strip()
	if branch != "main":
		fail(f"tag releases from main after the bump merges, not from '{branch}'")
	head = git("rev-parse", "HEAD").strip()
	remote = git("rev-parse", "origin/main").strip()
	if head != remote:
		fail("main is not at origin/main; pull before tagging")
	tag = f"v{version}"
	if git("tag", "--list", tag).strip():
		fail(f"tag {tag} already exists")
	git("tag", "-a", tag, "-m", f"Lily {version}")
	print(f"Tagged {tag}")
	print(f"Next: git push origin {tag}")


def command_notes(args: argparse.Namespace) -> None:
	to_ref = args.to or "HEAD"
	from_ref = args.from_tag or previous_tag(to_ref)
	base = args.repo_url or repo_url()
	print("## What's Changed")
	print("")
	if from_ref:
		range_spec = f"{from_ref}..{to_ref}"
	else:
		range_spec = to_ref
	log = git("log", "--no-merges", "--reverse", "--format=%h%x00%s", range_spec)
	commits = 0
	for line in log.splitlines():
		if "\0" not in line:
			continue
		sha, subject = line.split("\0", 1)
		if subject.startswith("Bump version to ") or subject.lower() == "version bump":
			continue
		print(f"- {link_subject(subject, base)} ([{sha}]({base}/commit/{sha}))")
		commits += 1
	if commits == 0:
		print("- No changes recorded.")
	print("")
	if from_ref:
		print(f"**Full Changelog**: {base}/compare/{from_ref}...{to_ref}")


def http_request(
	method: str,
	url: str,
	token: str,
	body: bytes | None = None,
	content_type: str | None = None,
) -> tuple[int, bytes]:
	headers = {"Authorization": f"token {token}"}
	if content_type:
		headers["Content-Type"] = content_type
	request = urllib.request.Request(url, data=body, headers=headers, method=method)
	try:
		with urllib.request.urlopen(request, timeout=120) as response:
			return response.status, response.read()
	except urllib.error.HTTPError as error:
		return error.code, error.read()
	except urllib.error.URLError as error:
		fail(f"request to {url} failed: {error.reason}")


def json_request(method: str, url: str, token: str, payload: dict | None = None) -> tuple[int, dict]:
	body = json.dumps(payload).encode("utf-8") if payload is not None else None
	status, raw = http_request(method, url, token, body, "application/json" if body else None)
	try:
		data = json.loads(raw.decode("utf-8")) if raw else {}
	except json.JSONDecodeError:
		data = {}
	return status, data


def multipart_body(path: Path) -> tuple[bytes, str]:
	boundary = f"----lily{os.urandom(12).hex()}"
	name = path.name.replace('"', "")
	head = (
		f"--{boundary}\r\n"
		f'Content-Disposition: form-data; name="attachment"; filename="{name}"\r\n'
		"Content-Type: application/octet-stream\r\n\r\n"
	).encode("utf-8")
	tail = f"\r\n--{boundary}--\r\n".encode("utf-8")
	return head + path.read_bytes() + tail, f"multipart/form-data; boundary={boundary}"


def command_publish(args: argparse.Namespace) -> None:
	token = os.environ.get(args.token_env) or os.environ.get("GITHUB_TOKEN", "")
	if not token and not args.dry_run:
		fail(f"{args.token_env} is not set")
	assets = sorted(path for path in Path(args.assets_dir).rglob("*") if path.is_file())
	if not assets:
		fail(f"no assets found in {args.assets_dir}")
	if args.dry_run:
		print(f"Would publish {args.tag} to {args.api}/repos/{args.repo}")
		for asset in assets:
			print(f"  {asset.relative_to(args.assets_dir)}")
		return

	release_url = f"{args.api}/repos/{args.repo}/releases"
	status, release = json_request(
		"POST",
		release_url,
		token,
		{
			"tag_name": args.tag,
			"name": args.title,
			"body": Path(args.notes).read_text(encoding="utf-8") if args.notes else "",
			"draft": False,
			"prerelease": args.prerelease,
		},
	)
	if status in (409, 422):
		status, release = json_request(
			"GET",
			f"{release_url}/tags/{urllib.parse.quote(args.tag, safe='')}",
			token,
		)
	if status not in (200, 201):
		fail(f"could not create release {args.tag}: HTTP {status}")
	release_id = release.get("id")
	if not release_id:
		fail("the release response has no id")
	print(f"Release {args.tag} is {release.get('html_url', release_id)}")

	status, existing = json_request(
		"GET", f"{release_url}/{release_id}/assets", token
	)
	existing_names = {
		asset.get("name") for asset in existing if isinstance(asset, dict)
	} if status == 200 and isinstance(existing, list) else set()
	for asset in assets:
		if asset.name in existing_names:
			print(f"  {asset.name}: already attached")
			continue
		body, content_type = multipart_body(asset)
		upload_url = (
			f"{release_url}/{release_id}/assets?"
			f"name={urllib.parse.quote(asset.name, safe='')}"
		)
		status, raw = http_request("POST", upload_url, token, body, content_type)
		if status in (200, 201):
			print(f"  {asset.name}: attached")
		elif status == 409:
			print(f"  {asset.name}: already attached")
		else:
			fail(f"upload of {asset.name} failed: HTTP {status} {raw[:200]!r}")


def build_parser() -> argparse.ArgumentParser:
	parser = argparse.ArgumentParser(description="Bump, tag, note, and publish Lily releases")
	sub = parser.add_subparsers(dest="command", required=True)

	version = sub.add_parser("version", help="Print the current version")
	version.set_defaults(func=command_version)

	check = sub.add_parser("check", help="Verify every version file agrees")
	check.set_defaults(func=command_check)

	bump = sub.add_parser("bump", help="Update version files and commit the bump")
	bump.add_argument("version")
	bump.add_argument("--tag", action="store_true", help="Also create the local tag")
	bump.add_argument("--allow-dirty", action="store_true", help="Allow a dirty working tree")
	bump.add_argument("--dry-run", action="store_true", help="Print the changes without writing")
	bump.set_defaults(func=command_bump)

	tag = sub.add_parser("tag", help="Tag the merged version on main")
	tag.add_argument("version", nargs="?", help="Expected version (default: current)")
	tag.set_defaults(func=command_tag)

	notes = sub.add_parser("notes", help="Draft release notes from the commit range")
	notes.add_argument("--from", dest="from_tag", help="Previous tag (default: newest merged tag)")
	notes.add_argument("--to", help="Tag or commit to describe (default: HEAD)")
	notes.add_argument("--output", help="Write the notes to this file instead of stdout")
	notes.add_argument("--repo-url", help="Repository URL for links")
	notes.set_defaults(func=command_notes)

	publish = sub.add_parser("publish", help="Create a Forgejo release and attach assets")
	publish.add_argument("--api", required=True, help="API base, e.g. https://host/api/v1")
	publish.add_argument("--repo", required=True, help="owner/repo")
	publish.add_argument("--tag", required=True)
	publish.add_argument("--title", required=True)
	publish.add_argument("--notes", help="Markdown file for the release body")
	publish.add_argument("--assets-dir", required=True)
	publish.add_argument("--prerelease", action="store_true")
	publish.add_argument("--token-env", default="FORGEJO_TOKEN")
	publish.add_argument("--dry-run", action="store_true")
	publish.set_defaults(func=command_publish)

	return parser


def main() -> None:
	parser = build_parser()
	args = parser.parse_args()
	if args.command == "notes" and args.output:
		# Capture the printed notes and write them to the requested file.
		import io
		from contextlib import redirect_stdout

		buffer = io.StringIO()
		with redirect_stdout(buffer):
			args.func(args)
		Path(args.output).write_text(buffer.getvalue(), encoding="utf-8")
		return
	args.func(args)


if __name__ == "__main__":
	main()
