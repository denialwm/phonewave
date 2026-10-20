#!/usr/bin/env python3
"""Build and inspect the static Aston first-boot PID 1 ramdisk root."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path


DEFAULT_SOURCE = Path(__file__).resolve().parent / "init.c"
DEFAULT_OUTPUT = (
    Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    / "phonewave/aston/firstboot"
)


class FirstbootError(RuntimeError):
    """First-boot build or validation failure."""


def run(command: Sequence[object], *, capture: bool = False) -> str:
    argv = [os.fspath(item) for item in command]
    print("+ " + " ".join(argv), file=sys.stderr)
    result = subprocess.run(
        argv,
        check=True,
        stdout=subprocess.PIPE if capture else None,
        text=True,
    )
    return result.stdout if capture else ""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_init(path: Path, readelf: str) -> dict[str, object]:
    header = run([readelf, "-h", path], capture=True)
    dynamic = run([readelf, "-d", path], capture=True)
    if "Machine:" not in header or "AArch64" not in header:
        raise FirstbootError(f"not an AArch64 executable: {path}")
    if "There is no dynamic section" not in dynamic:
        raise FirstbootError(f"PID 1 is dynamically linked: {path}")
    if not os.access(path, os.X_OK):
        raise FirstbootError(f"PID 1 is not executable: {path}")
    return {
        "path": os.fspath(path),
        "size": path.stat().st_size,
        "sha256": sha256_file(path),
        "architecture": "AArch64",
        "static": True,
    }


def build(arguments: argparse.Namespace) -> None:
    source = arguments.source.resolve()
    output = arguments.output.resolve()
    firmware_bundle = (
        arguments.firmware_bundle.resolve() if arguments.firmware_bundle else None
    )
    if not source.is_file():
        raise FirstbootError(f"missing PID 1 source: {source}")
    if firmware_bundle is not None and not (firmware_bundle / "lib/firmware").is_dir():
        raise FirstbootError(
            f"firmware bundle lacks lib/firmware: {firmware_bundle}"
        )
    for command in (arguments.cc, arguments.host_cc, arguments.readelf):
        if shutil.which(command) is None:
            raise FirstbootError(f"missing compiler tool: {command}")
    if output.exists() and not arguments.force:
        raise FirstbootError(f"output exists; pass --force to replace: {output}")

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".firstboot-", dir=output.parent) as temp:
        staged = Path(temp) / "output"
        ramdisk = staged / "ramdisk-root"
        staged.mkdir()
        for directory, mode in (
            ("dev", 0o755),
            ("proc", 0o555),
            ("sys", 0o555),
            ("run", 0o755),
        ):
            (ramdisk / directory).mkdir(parents=True, mode=mode)

        selftest = staged / "init-selftest"
        run(
            [
                arguments.host_cc,
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-function",
                "-O2",
                "-DASTON_SELFTEST",
                source,
                "-o",
                selftest,
            ]
        )
        run([selftest])

        init = ramdisk / "init"
        run(
            [
                arguments.cc,
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Os",
                "-static",
                "-s",
                source,
                "-o",
                init,
            ]
        )
        init.chmod(0o755)
        init_info = validate_init(init, arguments.readelf)
        init_info["path"] = "ramdisk-root/init"
        firmware_files: list[str] = []
        if firmware_bundle is not None:
            firmware_source = firmware_bundle / "lib/firmware"
            firmware_target = ramdisk / "lib/firmware"
            shutil.copytree(firmware_source, firmware_target)
            firmware_files = sorted(
                os.fspath(path.relative_to(ramdisk))
                for path in firmware_target.rglob("*")
                if path.is_file()
            )
        compiler = run([arguments.cc, "--version"], capture=True).splitlines()[0]
        manifest = {
            "format": "aston-firstboot-init-v1",
            "source": os.fspath(source),
            "source_sha256": sha256_file(source),
            "compiler": compiler,
            "init": init_info,
            "firmware_bundle": (
                os.fspath(firmware_bundle) if firmware_bundle is not None else None
            ),
            "firmware_files": firmware_files,
            "ramdisk_entries": [
                "init",
                "dev/",
                *firmware_files,
                "proc/",
                "run/",
                "sys/",
            ],
        }
        (staged / "manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n"
        )
        selftest.unlink()

        if output.exists():
            shutil.rmtree(output)
        os.replace(staged, output)

    print(f"first-boot ramdisk root: {output / 'ramdisk-root'}")
    print(f"static PID 1 SHA-256:   {sha256_file(output / 'ramdisk-root/init')}")


def inspect(arguments: argparse.Namespace) -> None:
    init = arguments.output.resolve() / "ramdisk-root/init"
    info = validate_init(init, arguments.readelf)
    print(json.dumps(info, indent=2, sort_keys=True))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    build_parser = subparsers.add_parser("build")
    build_parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    build_parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    build_parser.add_argument("--cc", default="aarch64-linux-gnu-gcc")
    build_parser.add_argument("--host-cc", default="cc")
    build_parser.add_argument("--readelf", default="aarch64-linux-gnu-readelf")
    build_parser.add_argument(
        "--firmware-bundle",
        type=Path,
        help="copy BUNDLE/lib/firmware into the embedded initramfs",
    )
    build_parser.add_argument("--force", action="store_true")
    build_parser.set_defaults(handler=build)

    inspect_parser = subparsers.add_parser("inspect")
    inspect_parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    inspect_parser.add_argument("--readelf", default="aarch64-linux-gnu-readelf")
    inspect_parser.set_defaults(handler=inspect)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    try:
        arguments.handler(arguments)
    except FirstbootError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    except subprocess.CalledProcessError as error:
        print(f"error: command exited with {error.returncode}", file=sys.stderr)
        return error.returncode or 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
