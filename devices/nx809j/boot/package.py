#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Package a Perpetua NX809J release: boot_b with the kernel, init_boot_b with
a USB SSH rescue ramdisk and an optional disk root."""
import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import tempfile
import uuid
from pathlib import Path


HERE = Path(__file__).resolve().parent
WORKSPACE = HERE.parents[3]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def static_arm64(path):
    data = path.read_bytes()
    assert data[:6] == b"\x7fELF\x02\x01", f"not ELF64 little endian: {path}"
    assert struct.unpack_from("<H", data, 18)[0] == 183, f"not AArch64: {path}"
    offset = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    assert all(struct.unpack_from("<I", data, offset + i * size)[0] != 3
               for i in range(count)), f"requires a dynamic linker: {path}"


def ramdisk(args, release_name, listing):
    with tempfile.TemporaryDirectory(prefix="nx809j-initramfs-") as tmp:
        root = Path(tmp)
        shutil.copytree(HERE / "initramfs", root, dirs_exist_ok=True)
        if args.initramfs_overlay:
            shutil.copytree(args.initramfs_overlay, root, dirs_exist_ok=True)
        for name in ["bin", "dev", "proc", "sys", "run", "tmp", "etc/dropbear", "root/.ssh"]:
            (root / name).mkdir(parents=True, exist_ok=True)
        (root / "root").chmod(0o700)
        (root / "root/.ssh").chmod(0o700)
        (root / "tmp").chmod(0o1777)
        (root / "etc/shadow").chmod(0o600)
        host_key = args.vault / "ssh/dropbear_ed25519_host_key"
        if host_key.is_file():
            shutil.copyfile(host_key, root / "etc/dropbear/dropbear_ed25519_host_key")
            (root / "etc/dropbear/dropbear_ed25519_host_key").chmod(0o600)
        for src, name in [(args.busybox, "busybox"), (args.dropbear, "dropbearmulti")]:
            shutil.copyfile(src, root / "bin" / name)
            (root / "bin" / name).chmod(0o755)
        (root / "bin/sh").symlink_to("busybox")
        keys = args.authorized_keys.read_bytes()
        (root / "root/.ssh/authorized_keys").write_bytes(keys.rstrip() + b"\n")
        (root / "root/.ssh/authorized_keys").chmod(0o600)
        if args.root_partuuid:
            (root / "etc/root-partuuid").write_text(args.root_partuuid + "\n")
        (root / "etc/rescue-release").write_text(release_name + "\n")
        names = ["."] + sorted(str(p.relative_to(root)) for p in root.rglob("*"))
        archive = subprocess.check_output(
            ["cpio", "--create", "--format=newc", "--owner=0:0", "--reproducible", "--quiet"],
            input=("\n".join(names) + "\n").encode(), cwd=root)
        listed = subprocess.check_output(["cpio", "--list", "--quiet"], input=archive).decode().splitlines()
        assert set(names) == set(listed), "ramdisk contents mismatch"
        for name in ["init", "bin/busybox", "bin/dropbearmulti", "bin/sh", "root/.ssh/authorized_keys"]:
            assert name in listed, f"ramdisk missing {name}"
        listing.extend(listed)
        # The bootloader appends this after its own LZ4 legacy vendor ramdisk,
        # which has no end marker: the kernel reads on through the next
        # archive as more LZ4 chunks, so this one must be LZ4 legacy too.
        compressed = subprocess.check_output(["lz4", "-l", "-9", "-c"], input=archive)
        assert subprocess.check_output(["lz4", "-dc"], input=compressed) == archive
        return compressed, host_key.is_file()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path)
    parser.add_argument("--busybox", type=Path, required=True)
    parser.add_argument("--dropbear", type=Path, required=True)
    parser.add_argument("--authorized-keys", type=Path, required=True)
    parser.add_argument("--root-partuuid", type=lambda value: str(uuid.UUID(value)),
                        help="mount this GPT partition as ext4 and start its /sbin/init")
    parser.add_argument("--initramfs-overlay", type=Path,
                        help="diagnostic files from the session scratchpad")
    parser.add_argument("--vault", type=Path,
                        default=Path(os.environ.get("PHONEWAVE_VAULT", str(WORKSPACE / "device-vault"))) / "nx809j")
    parser.add_argument("--output", type=Path,
                        default=Path.home() / ".cache/phonewave/nx809j/rescue")
    args = parser.parse_args()

    release = args.release.resolve()
    manifest = json.loads((release / "manifest.json").read_text())
    assert manifest["device"] == "nx809j", "release is for another device"
    image = release / "artifacts/Image"
    assert sha(image) == manifest["artifacts"]["Image"], "kernel manifest mismatch"
    static_arm64(args.busybox)
    static_arm64(args.dropbear)
    keys = args.authorized_keys.read_bytes()
    assert keys.strip() and b"PRIVATE KEY" not in keys, "expected SSH public keys"
    subprocess.run(["ssh-keygen", "-lf", args.authorized_keys], check=True,
                   stdout=subprocess.DEVNULL)

    # The templates are this unit's own slot-B images, checked against the
    # backup manifest; the backups themselves are never changed.
    backups = json.loads((args.vault / "backups/manifest.json").read_text())
    templates = {}
    for part in ["boot_b", "init_boot_b"]:
        path = args.vault / "backups/slot_b" / f"{part}.img"
        assert sha(path) == backups[part]["sha256"], f"{part} template mismatch"
        templates[part] = path

    args.output.mkdir(mode=0o700, parents=True, exist_ok=True)
    listing = []
    data, packaged_host_key = ramdisk(args, manifest["release"], listing)
    rescue = args.output / "rescue.cpio.lz4"
    rescue.write_bytes(data)

    adapter = WORKSPACE / "linux-perpetua/devices/boot/android-gki"
    record = dict(release=manifest["release"], kernel_manifest_sha256=sha(release / "manifest.json"),
                  busybox_sha256=sha(args.busybox), dropbear_sha256=sha(args.dropbear),
                  authorized_keys_sha256=sha(args.authorized_keys),
                  packaged_host_key=packaged_host_key, root_partuuid=args.root_partuuid,
                  ssh_address="10.77.73.2", host_address="10.77.73.1", user="root", images={})
    for part, kind, payload in [("boot_b", "boot", image), ("init_boot_b", "init_boot", rescue)]:
        output = args.output / f"{part}.img"
        evidence = args.output / f"evidence-{part}"
        for stale in [output]:
            if stale.exists():
                stale.unlink()  # This generated output is reused; backups are never changed.
        if evidence.exists():
            shutil.rmtree(evidence)
        result = subprocess.check_output([adapter, kind, templates[part], payload, output, evidence])
        record["images"][part] = json.loads(result)
        record["images"][part]["template_sha256"] = sha(templates[part])
        output.chmod(0o600)
    if args.initramfs_overlay:
        overlay = args.initramfs_overlay.resolve()
        record["diagnostic_files"] = {
            str(path.relative_to(overlay)): sha(path)
            for path in sorted(overlay.rglob("*")) if path.is_file()
        }
    (args.output / "manifest.json").write_text(json.dumps(record, indent=2) + "\n")
    (args.output / "ramdisk-contents.txt").write_text("\n".join(listing) + "\n")
    rescue.chmod(0o600)
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
