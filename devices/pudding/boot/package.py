#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Package a Perpetua pudding release: boot.img with the kernel, init_boot.img
with a USB SSH rescue ramdisk and an optional disk root. Both slots run the
same stock images, so the result fits either slot.

The bootloader always loads init_boot's ramdisk, also for `fastboot boot`.
The rescue therefore lives in /pudding-rescue and the kernel starts it by
rdinit: init_boot can carry Android's ramdisk after it, whose /init the
stock kernel keeps starting."""
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
    with tempfile.TemporaryDirectory(prefix="pudding-initramfs-") as tmp:
        top = Path(tmp)
        root = top / "pudding-rescue"
        shutil.copytree(HERE / "initramfs", root)
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
        subprocess.run([WORKSPACE / "cross/aarch64-linux-gnu-gcc-droidloom",
                        f"--sysroot={WORKSPACE / 'cross/sysroot'}", "-static", "-Os", "-s",
                        "-o", root / "bin/reboot-to", HERE / "reboot-to.c"], check=True)
        static_arm64(root / "bin/reboot-to")
        keys = args.authorized_keys.read_bytes()
        (root / "root/.ssh/authorized_keys").write_bytes(keys.rstrip() + b"\n")
        (root / "root/.ssh/authorized_keys").chmod(0o600)
        if args.root_partuuid:
            (root / "etc/root-partuuid").write_text(args.root_partuuid + "\n")
        if args.log_partuuid:
            (root / "etc/log-partuuid").write_text(args.log_partuuid + "\n")
        if args.reboot_after:
            (root / "etc/reboot-after").write_text(f"{args.reboot_after}\n")
        (root / "etc/rescue-release").write_text(release_name + "\n")
        # Without Android's ramdisk, /init starts the rescue too; with it,
        # Android's /init replaces this link.
        (top / "init").symlink_to("pudding-rescue/load")
        names = ["."] + sorted(str(p.relative_to(top)) for p in top.rglob("*"))
        archive = subprocess.check_output(
            ["cpio", "--create", "--format=newc", "--owner=0:0", "--reproducible", "--quiet"],
            input=("\n".join(names) + "\n").encode(), cwd=top)
        listed = subprocess.check_output(["cpio", "--list", "--quiet"], input=archive).decode().splitlines()
        assert set(names) == set(listed), "ramdisk contents mismatch"
        for name in ["init", "load", "bin/busybox", "bin/dropbearmulti", "bin/sh", "bin/reboot-to",
                     "root/.ssh/authorized_keys"]:
            name = name if name == "init" else f"pudding-rescue/{name}"
            assert name in listed, f"ramdisk missing {name}"
        listing.extend(listed)
        # The bootloader appends this after its own LZ4 legacy vendor ramdisk,
        # which has no end marker: the kernel reads on through the next
        # archive as more LZ4 chunks, so this one must be LZ4 legacy too.
        compressed = subprocess.check_output(["lz4", "-l", "-9", "-c"], input=archive)
        assert subprocess.check_output(["lz4", "-dc"], input=compressed) == archive
        return compressed, host_key.is_file()


def android_ramdisk(path):
    """The ramdisk of an Android init_boot image (header v3 or v4)."""
    data = path.read_bytes()
    assert data[:8] == b"ANDROID!", f"not an Android boot image: {path}"
    kernel_size, ramdisk_size = struct.unpack_from("<II", data, 8)
    assert struct.unpack_from("<I", data, 40)[0] in (3, 4), f"not header v3 or v4: {path}"
    assert kernel_size == 0, f"init_boot carries a kernel: {path}"
    payload = data[4096:4096 + ramdisk_size]
    assert payload[:4] == b"\x02\x21\x4c\x18", f"ramdisk is not LZ4 legacy: {path}"
    return payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path)
    parser.add_argument("--busybox", type=Path, required=True)
    parser.add_argument("--dropbear", type=Path, required=True)
    parser.add_argument("--authorized-keys", type=Path, required=True)
    parser.add_argument("--root-partuuid", type=lambda value: str(uuid.UUID(value)),
                        help="mount this GPT partition as ext4 and start its /sbin/init")
    parser.add_argument("--log-partuuid", type=lambda value: str(uuid.UUID(value)),
                        help="write the kernel log to the start of this GPT partition, "
                             "which must be named linux")
    parser.add_argument("--reboot-after", type=int,
                        help="reboot at this uptime in seconds unless /run/hold exists")
    parser.add_argument("--android-init-boot", type=Path,
                        help="carry this init_boot's ramdisk after the rescue, so the stock "
                             "kernel still starts Android from the same init_boot")
    parser.add_argument("--initramfs-overlay", type=Path,
                        help="diagnostic files from the session scratchpad")
    parser.add_argument("--vault", type=Path,
                        default=Path(os.environ.get("PHONEWAVE_VAULT", str(WORKSPACE / "device-vault"))) / "pudding")
    parser.add_argument("--output", type=Path,
                        default=Path.home() / ".cache/phonewave/pudding/rescue")
    args = parser.parse_args()

    release = args.release.resolve()
    manifest = json.loads((release / "manifest.json").read_text())
    assert manifest["device"] == "pudding", "release is for another device"
    image = release / "artifacts/Image"
    assert sha(image) == manifest["artifacts"]["Image"], "kernel manifest mismatch"
    static_arm64(args.busybox)
    static_arm64(args.dropbear)
    keys = args.authorized_keys.read_bytes()
    assert keys.strip() and b"PRIVATE KEY" not in keys, "expected SSH public keys"
    subprocess.run(["ssh-keygen", "-lf", args.authorized_keys], check=True,
                   stdout=subprocess.DEVNULL)

    # The templates are this unit's stock OS3.0.335.0 images from the slot A
    # dump (slot B was aligned to the same images), checked against the
    # backup checksums; the backups themselves are never changed.
    backups = args.vault / "backups"
    sums = {}
    for line in (backups / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        sums[name.removeprefix("./")] = digest
    templates = {}
    for kind, name in [("boot", "slot-a-20261101/boot_a.img"),
                       ("init_boot", "slot-a-20261101/init_boot_a.stock.img")]:
        path = backups / name
        assert sha(path) == sums[name], f"{name} template mismatch"
        templates[kind] = path

    args.output.mkdir(mode=0o700, parents=True, exist_ok=True)
    listing = []
    data, packaged_host_key = ramdisk(args, manifest["release"], listing)
    rescue = args.output / "rescue.cpio.lz4"
    rescue.write_bytes(data)
    init_boot = rescue
    if args.android_init_boot:
        android = android_ramdisk(args.android_init_boot)
        combined = data + android
        unpacked = subprocess.check_output(["lz4", "-dc"], input=combined)
        assert unpacked == (subprocess.check_output(["lz4", "-dc"], input=data) +
                            subprocess.check_output(["lz4", "-dc"], input=android))
        init_boot = args.output / "init_boot-ramdisk.lz4"
        init_boot.write_bytes(combined)

    adapter = WORKSPACE / "linux-perpetua/devices/boot/android-gki"
    record = dict(release=manifest["release"], kernel_manifest_sha256=sha(release / "manifest.json"),
                  busybox_sha256=sha(args.busybox), dropbear_sha256=sha(args.dropbear),
                  authorized_keys_sha256=sha(args.authorized_keys),
                  packaged_host_key=packaged_host_key, root_partuuid=args.root_partuuid,
                  log_partuuid=args.log_partuuid, reboot_after=args.reboot_after,
                  android_init_boot_sha256=args.android_init_boot and sha(args.android_init_boot),
                  ssh_address="10.77.74.2", host_address="10.77.74.1", user="root", images={})
    for kind, payload in [("boot", image), ("init_boot", init_boot)]:
        output = args.output / f"{kind}.img"
        evidence = args.output / f"evidence-{kind}"
        if output.exists():
            output.unlink()  # This generated output is reused; backups are never changed.
        if evidence.exists():
            shutil.rmtree(evidence)
        result = subprocess.check_output([adapter, kind, templates[kind], payload, output, evidence])
        record["images"][kind] = json.loads(result)
        record["images"][kind]["template_sha256"] = sha(templates[kind])
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
    init_boot.chmod(0o600)
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
