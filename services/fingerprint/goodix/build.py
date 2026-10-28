#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build the native Goodix diagnostics and assemble the signed split ELF."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess

SOURCE = Path(__file__).resolve().parent
WORKSPACE = SOURCE.parents[3]
CACHE = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "phonewave"
PINS = {
    "quic-teec": ("https://github.com/qualcomm/quic-teec.git",
                  "736419e25a2036aac3292a10a93e394a90750ca3"),
    "qcbor": ("https://github.com/laurencelundblade/QCBOR.git",
              "a3fce5f34d7b30f12d7b9161185171c6fe137737"),
    "minkipc": ("https://github.com/qualcomm/minkipc.git",
                "6ab4aedfbbacd366f0e064c1c72b09ad54b31761"),
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def assemble(directory):
    """Preserve every signed segment, including identical overlapping headers."""
    header = (directory / "goodixfp64.b00").read_bytes()
    if len(header) < 64 or header[:6] != b"\x7fELF\x02\x01":
        raise ValueError("Expected little-endian ELF64 firmware")
    phoff = struct.unpack_from("<Q", header, 32)[0]
    phsize, phnum = struct.unpack_from("<HH", header, 54)
    if phsize != 56 or not 1 <= phnum <= 32 or phoff + phsize * phnum > len(header):
        raise ValueError("Invalid program headers")
    parts = []
    for index in range(phnum):
        ph = struct.unpack_from("<IIQQQQQQ", header, phoff + index * phsize)
        offset, size = ph[2], ph[5]
        path = directory / f"goodixfp64.b{index:02d}"
        data = path.read_bytes()
        if len(data) != size or offset + size > 32 * 1024 * 1024:
            raise ValueError(f"Invalid segment extent: {path.name}")
        for previous_offset, previous, _ in parts:
            begin = max(offset, previous_offset)
            end = min(offset + size, previous_offset + len(previous))
            if begin < end and data[begin-offset:end-offset] != previous[begin-previous_offset:end-previous_offset]:
                raise ValueError(f"Conflicting overlapping segment: {path.name}")
        parts.append((offset, data, path))
    data = bytearray(max(offset + len(part) for offset, part, _ in parts))
    for offset, part, _ in parts:
        data[offset:offset+len(part)] = part
    return data, {path.name: sha(path) for _, _, path in parts}


def dependency(directory, name):
    url, pin = PINS[name]
    path = directory / name
    if not path.exists():
        directory.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "--quiet", "--no-checkout", url, path], check=True)
        subprocess.run(["git", "-C", path, "checkout", "--quiet", "--detach", pin], check=True)
    actual = subprocess.check_output(["git", "-C", path, "rev-parse", "HEAD"], text=True).strip()
    if actual != pin:
        raise ValueError(f"{path} is at {actual}; expected {pin}. Existing checkouts are never changed.")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware", type=Path, default=Path(os.environ.get("PHONEWAVE_VAULT", str(WORKSPACE / "device-vault"))) / "nx809j/firmware/vendor-firmware_mnt/image")
    parser.add_argument("--output", type=Path, default=CACHE / "nx809j/fingerprint")
    parser.add_argument("--dependencies", type=Path, default=CACHE / "src")
    parser.add_argument("--kernel", type=Path, default=WORKSPACE / "linux-perpetua")
    parser.add_argument("--cc", default="aarch64-linux-gnu-gcc")
    parser.add_argument("--ar", default="aarch64-linux-gnu-ar")
    parser.add_argument("--assemble-only", action="store_true")
    args = parser.parse_args()
    # Never permit generated outputs to replace the acquired firmware inputs.
    output = args.output.resolve()
    vault = Path(os.environ.get("PHONEWAVE_VAULT", str(WORKSPACE / "device-vault"))).resolve()
    if output.is_relative_to(vault) or output.is_relative_to(args.firmware.resolve()):
        raise ValueError("Outputs must be outside the vault and firmware inputs")
    output.mkdir(parents=True, exist_ok=True, mode=0o700)
    data, segments = assemble(args.firmware)
    firmware = output / "goodixfp64.elf"
    firmware.write_bytes(data)
    firmware.chmod(0o600)
    record = {"bytes": len(data), "sha256": sha(firmware), "segments": segments}
    (output / "firmware-manifest.json").write_text(json.dumps(record, indent=2) + "\n")
    if args.assemble_only:
        print(json.dumps(record, indent=2))
        return
    teec = dependency(args.dependencies, "quic-teec")
    qcbor = dependency(args.dependencies, "qcbor")
    mink = dependency(args.dependencies, "minkipc")
    includes = output / "include"
    (includes / "linux").mkdir(parents=True, exist_ok=True)
    tee_header = args.kernel / "include/uapi/linux/tee.h"
    shutil.copyfile(tee_header, includes / "linux/tee.h")
    control_header = args.kernel / "include/uapi/linux/goodix_tee_fp.h"
    shutil.copyfile(control_header, includes / "linux/goodix_tee_fp.h")
    include_flags = ["-I" + str(p) for p in [includes, teec / "libqcomtee/include",
                                            teec / "libqcomtee/src", qcbor / "inc"]]
    libraries = {
        "qcomtee": [teec / "libqcomtee/src" / p for p in
                    ["qcomtee_object.c", "objects/credentials_obj.c", "objects/mem_obj.c"]],
        "qcbor": [qcbor / "src" / p for p in
                  ["UsefulBuf.c", "ieee754.c", "qcbor_encode.c", "qcbor_decode.c", "qcbor_err_to_str.c"]],
    }
    probe_sources = [SOURCE / "goodix-probe.c", SOURCE / "keymaster-shared-secret.c", SOURCE / "keymaster-wrapped-key.c"]
    inputs = [Path(__file__).resolve(), *probe_sources, SOURCE / "keymaster-shared-secret.h", SOURCE / "keymaster-wrapped-key.h", tee_header, control_header]
    for tree in [teec / "libqcomtee", qcbor / "inc"]:
        inputs.extend(tree.rglob("*.h"))
    archives = []
    commands = []
    for name, sources in libraries.items():
        objects = []
        for source in sources:
            obj = output / (name + "-" + source.stem + ".o")
            command = [args.cc, "-std=gnu11", "-O2", "-DUSE_QCBOR", "-D_GNU_SOURCE",
                       "-pthread", *include_flags, "-c", str(source), "-o", str(obj)]
            subprocess.run(command, check=True)
            commands.append(command)
            objects.append(str(obj))
            inputs.append(source)
        archive = output / ("lib" + name + ".a")
        archive.unlink(missing_ok=True)
        subprocess.run([args.ar, "rcs", str(archive), *objects], check=True)
        archives.append(str(archive))
    command = [args.cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O2", "-g",
               "-static", "-pthread", *include_flags, *map(str, probe_sources),
               *archives, "-o", str(output / "goodix-probe")]
    subprocess.run(command, check=True)
    commands.append(command)
    manifest = {"commands": commands, "compiler": subprocess.check_output([args.cc, "--version"], text=True),
                "pins": {name: pin for name, (_, pin) in PINS.items()},
                "inputs": {str(path): sha(path) for path in sorted(set(inputs))},
                "output_sha256": sha(output / "goodix-probe"), "firmware": record}
    (output / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    worker_sources = [SOURCE / "goodix-service-worker.c", SOURCE / "goodix-worker-ops.c",
                      SOURCE / "keymaster-shared-secret.c", SOURCE / "keymaster-wrapped-key.c"]
    storage_includes = [mink / "listeners/libfsservice" / p for p in ("fs", "gpfs", "cmn")]
    worker_command = [args.cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O2", "-g",
                      "-static", "-pthread", *include_flags,
                      *["-I" + str(p) for p in storage_includes], *map(str, worker_sources),
                      *archives, "-o", str(output / "goodix-service-worker")]
    subprocess.run(worker_command, check=True)
    worker_inputs = set(inputs) | set(worker_sources) | {
        SOURCE / "goodix-storage.h", SOURCE / "goodix-resident.h", SOURCE / "goodix-worker-ops.h",
        SOURCE / "goodix-authorization.h"}
    for path in storage_includes:
        worker_inputs.update(path.glob("*.h"))
    worker_manifest = {**manifest, "commands": [*commands[:-1], worker_command],
                       "inputs": {str(p): sha(p) for p in sorted(worker_inputs)},
                       "output_sha256": sha(output / "goodix-service-worker")}
    (output / "worker-manifest.json").write_text(json.dumps(worker_manifest, indent=2) + "\n")
    gatekeeper_sources = [SOURCE / "gatekeeper-worker.c", SOURCE / "gatekeeper.c",
                          SOURCE / "keymaster-shared-secret.c"]
    gatekeeper_command = [args.cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O2", "-g",
                          "-static", "-pthread", *include_flags, *map(str, gatekeeper_sources),
                          *archives, "-o", str(output / "goodix-gatekeeper-worker")]
    subprocess.run(gatekeeper_command, check=True)
    gatekeeper_inputs = set(inputs) | set(gatekeeper_sources) | {SOURCE / "gatekeeper.h"}
    gatekeeper_manifest = {**manifest, "commands": [*commands[:-1], gatekeeper_command],
        "inputs": {str(p): sha(p) for p in sorted(gatekeeper_inputs)},
        "output_sha256": sha(output / "goodix-gatekeeper-worker")}
    (output / "gatekeeper-manifest.json").write_text(json.dumps(gatekeeper_manifest, indent=2) + "\n")
    secure_sources = [SOURCE / "secure-storage-service.c", SOURCE / "secure-storage.c"]
    secure_command = [args.cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", "-O2", "-g",
                      "-static", "-pthread", *include_flags,
                      *["-I" + str(p) for p in storage_includes], *map(str, secure_sources),
                      *archives, "-o", str(output / "goodix-secure-storage")]
    subprocess.run(secure_command, check=True)
    secure_inputs = worker_inputs | set(secure_sources) | {SOURCE / "secure-storage.h"}
    secure_manifest = {**manifest, "commands": [*commands[:-1], secure_command],
        "inputs": {str(p): sha(p) for p in sorted(secure_inputs)},
        "output_sha256": sha(output / "goodix-secure-storage")}
    (output / "secure-storage-manifest.json").write_text(json.dumps(secure_manifest, indent=2) + "\n")
    helper = SOURCE / "native-helper"
    target = WORKSPACE / ".worktrees/targets/goodix-helper"
    helper_env = dict(os.environ, CARGO_TARGET_DIR=str(target),
                      CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=args.cc)
    helper_command = ["cargo", "build", "--locked", "--release", "--target",
                      "aarch64-unknown-linux-gnu", "--manifest-path", str(helper / "Cargo.toml")]
    subprocess.run(helper_command, check=True, env=helper_env)
    shutil.copyfile(target / "aarch64-unknown-linux-gnu/release/goodix-fprint-helper",
                    output / "goodix-fprint-helper")
    (output / "goodix-fprint-helper").chmod(0o755)
    helper_inputs = [helper / "Cargo.toml", helper / "Cargo.lock", helper / "src/main.rs"]
    helper_manifest = {"command": helper_command, "inputs": {str(p): sha(p) for p in helper_inputs},
                       "output_sha256": sha(output / "goodix-fprint-helper")}
    (output / "native-helper-manifest.json").write_text(json.dumps(helper_manifest, indent=2) + "\n")
    print(f"Built {output / 'goodix-fprint-helper'}: {helper_manifest['output_sha256']}")
    print(f"Built {output / 'goodix-secure-storage'}: {secure_manifest['output_sha256']}")
    print(f"Built {output / 'goodix-gatekeeper-worker'}: {gatekeeper_manifest['output_sha256']}")
    print(f"Built {output / 'goodix-service-worker'}: {worker_manifest['output_sha256']}")

    print(f"Built {output / 'goodix-probe'}: {manifest['output_sha256']}")


if __name__ == "__main__":
    main()
