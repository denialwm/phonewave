#!/usr/bin/env python3
"""Guarded inactive-slot partition writer for Aston's Arch userspace."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path


PARTITIONS = {
    "boot_b": 201_326_592,
    "init_boot_b": 8_388_608,
}
COPY_CHUNK = 4 * 1024 * 1024


class FlashError(RuntimeError):
    """A safety check or exact I/O operation failed."""


def sha256_exact(path: Path, size: int) -> str:
    digest = hashlib.sha256()
    remaining = size
    with path.open("rb", buffering=0) as source:
        while remaining:
            chunk = source.read(min(COPY_CHUNK, remaining))
            if not chunk:
                raise FlashError(f"short read from {path}: {size - remaining}/{size}")
            digest.update(chunk)
            remaining -= len(chunk)
    return digest.hexdigest()


def copy_exact(source_path: Path, target_path: Path, size: int) -> None:
    remaining = size
    try:
        target_is_block = stat.S_ISBLK(target_path.stat().st_mode)
    except FileNotFoundError:
        target_is_block = False
    target_flags = os.O_WRONLY | os.O_CLOEXEC
    if not target_is_block:
        target_flags |= os.O_CREAT | os.O_TRUNC
    target_fd = os.open(target_path, target_flags, 0o600)
    with (
        source_path.open("rb", buffering=0) as source,
        os.fdopen(target_fd, "wb", buffering=0) as target,
    ):
        while remaining:
            chunk = source.read(min(COPY_CHUNK, remaining))
            if not chunk:
                raise FlashError(
                    f"short read from {source_path}: {size - remaining}/{size}"
                )
            cursor = memoryview(chunk)
            while cursor:
                written = target.write(cursor)
                if written is None or written <= 0:
                    raise FlashError(f"short write to {target_path}")
                cursor = cursor[written:]
            remaining -= len(chunk)
        target.flush()
        os.fsync(target.fileno())


def partition_record(
    name: str, sys_class_block: Path = Path("/sys/class/block")
) -> tuple[Path, int, int, int]:
    for entry in sorted(sys_class_block.iterdir()):
        try:
            properties = dict(
                line.split("=", 1)
                for line in (entry / "uevent").read_text().splitlines()
                if "=" in line
            )
        except (FileNotFoundError, OSError, ValueError):
            continue
        if properties.get("PARTNAME") != name:
            continue
        try:
            major_text, minor_text = (entry / "dev").read_text().strip().split(":", 1)
            sectors = int((entry / "size").read_text().strip())
            return entry, int(major_text), int(minor_text), sectors * 512
        except (FileNotFoundError, OSError, ValueError) as error:
            raise FlashError(f"invalid sysfs record for {name}: {error}") from error
    raise FlashError(f"partition not found: {name}")


def ensure_block_node(path: Path, major: int, minor: int) -> Path:
    expected = os.makedev(major, minor)
    if path.is_symlink():
        raise FlashError(f"refusing symlink target node: {path}")
    try:
        metadata = path.stat()
    except FileNotFoundError:
        os.mknod(path, stat.S_IFBLK | 0o600, expected)
        metadata = path.stat()
    if not stat.S_ISBLK(metadata.st_mode) or metadata.st_rdev != expected:
        raise FlashError(f"refusing unexpected target node: {path}")
    return path


def running_slot(
    bootconfig: Path = Path("/proc/bootconfig"),
    cmdline: Path = Path("/proc/cmdline"),
) -> str | None:
    texts: list[str] = []
    for path in (bootconfig, cmdline):
        try:
            texts.append(path.read_text(errors="replace"))
        except (FileNotFoundError, OSError):
            pass
    pattern = re.compile(r"androidboot\.slot_suffix\s*(?:=|:)\s*[\"']?(_[ab])")
    for text in texts:
        match = pattern.search(text)
        if match:
            return match.group(1)
    return None


def validate_sha(value: str, label: str) -> str:
    normalized = value.lower()
    if len(normalized) != 64 or any(char not in "0123456789abcdef" for char in normalized):
        raise FlashError(f"invalid {label}: {value}")
    return normalized


def partition_spec(name: str) -> tuple[str, int]:
    try:
        return name, PARTITIONS[name]
    except KeyError as error:
        raise FlashError(f"partition is not allowlisted: {name}") from error


def flash(arguments: argparse.Namespace) -> dict[str, object]:
    if not arguments.apply:
        raise FlashError("refusing write without --apply")
    partition, expected_size = partition_spec(arguments.partition)
    image = arguments.image.resolve()
    if not image.is_file() or image.stat().st_size != expected_size:
        actual = image.stat().st_size if image.exists() else "missing"
        raise FlashError(
            f"{partition} image size is {actual}; expected {expected_size}"
        )
    expected_sha = validate_sha(arguments.expected_sha, "expected image SHA-256")
    actual_sha = sha256_exact(image, expected_size)
    if actual_sha != expected_sha:
        raise FlashError(f"uploaded image SHA-256 is {actual_sha}; expected {expected_sha}")

    slot = running_slot()
    expected_current = validate_sha(
        arguments.expect_current_sha, "current-image SHA-256"
    )
    if slot is not None and slot != "_b":
        raise FlashError(f"refusing to write {partition} while running slot {slot}")

    sysfs, major, minor, partition_size = partition_record(partition)
    if partition_size != expected_size:
        raise FlashError(
            f"{partition} size is {partition_size}; expected exactly {expected_size}"
        )
    node = arguments.node or Path(f"/dev/aston-{partition}")
    target = ensure_block_node(node, major, minor)
    arguments.backup_dir.mkdir(parents=True, exist_ok=True)
    backup = arguments.backup_dir / f"{partition}-before-{expected_sha[:12]}.img"
    copy_exact(target, backup, expected_size)
    backup_sha = sha256_exact(backup, expected_size)
    if backup_sha != expected_current:
        raise FlashError(
            f"current {partition} SHA-256 is {backup_sha}; expected {expected_current}"
        )

    copy_exact(image, target, expected_size)
    os.sync()
    readback_sha = sha256_exact(target, expected_size)
    if readback_sha != expected_sha:
        raise FlashError(
            f"{partition} readback SHA-256 is {readback_sha}; expected {expected_sha}"
        )
    return {
        "format": "aston-remote-partition-flash-v1",
        "partition": partition,
        "running_slot": slot or "hash-verified",
        "sysfs": os.fspath(sysfs),
        "device": os.fspath(target),
        "size": expected_size,
        "image_sha256": expected_sha,
        "backup": os.fspath(backup),
        "backup_sha256": backup_sha,
        "readback_sha256": readback_sha,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--partition", choices=sorted(PARTITIONS), default="boot_b")
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--expected-sha", required=True)
    parser.add_argument("--expect-current-sha", required=True)
    parser.add_argument("--backup-dir", type=Path, default=Path("/run/aston-flash"))
    parser.add_argument("--node", type=Path)
    return parser


def main() -> int:
    try:
        result = flash(build_parser().parse_args())
    except (FlashError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
