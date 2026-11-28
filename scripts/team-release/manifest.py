#!/usr/bin/env python3
"""Generate, bump, and roll back schema-1 Hex release manifests and dev notes."""

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import sys

ALLOWED_CHANNELS = ("dev", "stable", "beta")
VERSION_ALLOWED_EXTRA = set(".-+_")
MAX_NOTES_ITEMS = 40
MAX_NOTE_TITLE = 200


def is_version_name(text: str) -> bool:
    return (
        1 <= len(text) <= 128
        and not text.startswith(".")
        and all(
            ch.isascii() and (ch.isalnum() or ch in VERSION_ALLOWED_EXTRA)
            for ch in text
        )
    )


def validate_https_url(url: str) -> str:
    if (
        not url.startswith("https://")
        or len(url) <= len("https://")
        or len(url) > 2048
        or any(ch.isspace() or ord(ch) < 32 or ord(ch) == 127 for ch in url)
    ):
        raise ValueError(f"Invalid HTTPS URL: {url!r}")
    return url


def utc_now_rfc3339() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def file_digest_and_size(path: pathlib.Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def build_manifest(
    *,
    channel: str,
    version: str,
    sequence: int,
    zip_path: pathlib.Path,
    dmg_path: pathlib.Path,
    zip_name: str = "Hex.zip",
    dmg_name: str = "Hex.dmg",
    notes_url: str | None = None,
    rollout_percent: int = 100,
    required: bool = False,
    published_at: str | None = None,
) -> dict:
    if channel not in ALLOWED_CHANNELS:
        raise ValueError(f"Unsupported channel {channel!r}")
    if not is_version_name(version):
        raise ValueError(f"Invalid version name {version!r}")
    if sequence < 1:
        raise ValueError("sequence must be >= 1")
    if not (0 <= rollout_percent <= 100):
        raise ValueError("rollout_percent must be between 0 and 100")
    for fname in (zip_name, dmg_name):
        if not is_version_name(fname):
            raise ValueError(f"Invalid artifact file name {fname!r}")

    zip_sha, zip_size = file_digest_and_size(zip_path)
    dmg_sha, dmg_size = file_digest_and_size(dmg_path)

    release: dict = {
        "version": version,
        "rollout_percent": rollout_percent,
        "required": required,
        "targets": {
            "macos-arm64": {
                "url": f"/hex/files/{zip_sha}/{zip_name}",
                "size": zip_size,
                "sha256": zip_sha,
            },
            "macos-arm64-dmg": {
                "url": f"/hex/files/{dmg_sha}/{dmg_name}",
                "size": dmg_size,
                "sha256": dmg_sha,
            },
        },
        "distributions": [{"kind": "direct"}],
    }
    if notes_url:
        release["notes_url"] = validate_https_url(notes_url)

    return {
        "schema": 1,
        "app": "hex",
        "artifact": "desktop",
        "channel": channel,
        "sequence": sequence,
        "published_at": published_at or utc_now_rfc3339(),
        "release": release,
    }


def rollback_manifest(
    *,
    archived: dict,
    sequence: int,
    published_at: str | None = None,
) -> dict:
    if archived.get("schema") != 1:
        raise ValueError("Archived manifest must be schema 1")
    if archived.get("app") != "hex" or archived.get("artifact") != "desktop":
        raise ValueError("Archived manifest is not a Hex desktop manifest")
    channel = archived.get("channel")
    if channel not in ALLOWED_CHANNELS:
        raise ValueError(f"Archived manifest has invalid channel {channel!r}")
    if sequence < 1:
        raise ValueError("sequence must be >= 1")
    release = archived.get("release")
    if not isinstance(release, dict) or not is_version_name(str(release.get("version", ""))):
        raise ValueError("Archived manifest has invalid release payload")

    return {
        "schema": 1,
        "app": "hex",
        "artifact": "desktop",
        "channel": channel,
        "sequence": sequence,
        "published_at": published_at or utc_now_rfc3339(),
        "release": release,
    }


def update_dev_notes(
    *,
    version: str,
    commit: str,
    published_at: str,
    titles: list[str],
    existing: dict | None = None,
    max_builds: int = 30,
    repo_url: str | None = None,
) -> dict:
    if not is_version_name(version):
        raise ValueError(f"Invalid version name {version!r}")
    seen: set[str] = set()
    items: list[dict] = []
    commit_url = f"{repo_url.rstrip('/')}/commit/{commit}" if repo_url else ""
    for raw in titles:
        cleaned = "".join(ch for ch in raw.strip() if ord(ch) >= 32 and ord(ch) != 127)
        cleaned = cleaned[:MAX_NOTE_TITLE]
        if cleaned and cleaned not in seen:
            seen.add(cleaned)
            items.append({"title": cleaned, "url": commit_url})
            if len(items) >= MAX_NOTES_ITEMS:
                break

    builds: list[dict] = []
    if isinstance(existing, dict) and isinstance(existing.get("builds"), list):
        builds = [
            entry
            for entry in existing["builds"]
            if isinstance(entry, dict) and entry.get("version") != version
        ]

    entry = {
        "version": version,
        "commit": commit,
        "published_at": published_at,
        "items": items,
    }
    return {
        "schema": 1,
        "builds": [entry, *builds][:max_builds],
    }


def next_sequence(current_path: pathlib.Path | None) -> int:
    if current_path is None or not current_path.is_file():
        return 1
    data = json.loads(current_path.read_text(encoding="utf-8"))
    seq = data.get("sequence", 0)
    if not isinstance(seq, int) or seq < 0:
        raise ValueError(f"Invalid sequence in {current_path}: {seq!r}")
    return seq + 1


def write_json_atomic(path: pathlib.Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_build = sub.add_parser("build")
    p_build.add_argument("--channel", required=True, choices=ALLOWED_CHANNELS)
    p_build.add_argument("--version", required=True)
    p_build.add_argument("--sequence", required=True, type=int)
    p_build.add_argument("--zip", dest="zip_path", required=True, type=pathlib.Path)
    p_build.add_argument("--dmg", dest="dmg_path", required=True, type=pathlib.Path)
    p_build.add_argument("--zip-name", default="Hex.zip")
    p_build.add_argument("--dmg-name", default="Hex.dmg")
    p_build.add_argument("--out", required=True, type=pathlib.Path)
    p_build.add_argument("--notes-url")
    p_build.add_argument("--rollout-percent", type=int, default=100)
    p_build.add_argument("--published-at")
    p_build.add_argument("--required", action="store_true")

    p_rollback = sub.add_parser("rollback")
    p_rollback.add_argument("--from", dest="from_path", required=True, type=pathlib.Path)
    p_rollback.add_argument("--sequence", required=True, type=int)
    p_rollback.add_argument("--out", required=True, type=pathlib.Path)
    p_rollback.add_argument("--published-at")

    p_notes = sub.add_parser("notes")
    p_notes.add_argument("--version", required=True)
    p_notes.add_argument("--commit", required=True)
    p_notes.add_argument("--published-at", default=utc_now_rfc3339())
    p_notes.add_argument("--titles-file", required=True, type=pathlib.Path)
    p_notes.add_argument("--existing", type=pathlib.Path)
    p_notes.add_argument("--out", required=True, type=pathlib.Path)
    p_notes.add_argument("--max-builds", type=int, default=30)
    p_notes.add_argument("--repo-url")

    p_seq = sub.add_parser("next-sequence")
    p_seq.add_argument("--current", type=pathlib.Path)

    args = parser.parse_args(argv)
    if args.cmd == "build":
        manifest = build_manifest(
            channel=args.channel,
            version=args.version,
            sequence=args.sequence,
            zip_path=args.zip_path,
            dmg_path=args.dmg_path,
            zip_name=args.zip_name,
            dmg_name=args.dmg_name,
            notes_url=args.notes_url,
            rollout_percent=args.rollout_percent,
            required=args.required,
            published_at=args.published_at,
        )
        write_json_atomic(args.out, manifest)
        return 0

    if args.cmd == "rollback":
        archived = json.loads(args.from_path.read_text(encoding="utf-8"))
        manifest = rollback_manifest(
            archived=archived,
            sequence=args.sequence,
            published_at=args.published_at,
        )
        write_json_atomic(args.out, manifest)
        return 0

    if args.cmd == "notes":
        titles = args.titles_file.read_text(encoding="utf-8").splitlines()
        existing = None
        if args.existing and args.existing.is_file():
            try:
                existing = json.loads(args.existing.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                existing = None
        notes = update_dev_notes(
            version=args.version,
            commit=args.commit,
            published_at=args.published_at,
            titles=titles,
            existing=existing,
            max_builds=args.max_builds,
            repo_url=args.repo_url,
        )
        write_json_atomic(args.out, notes)
        return 0

    if args.cmd == "next-sequence":
        print(next_sequence(args.current))
        return 0

    return 2


if __name__ == "__main__":
    sys.exit(main())
