#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Host tests for native Goodix operation ordering, cancellation and storage."""
import os
from pathlib import Path
import subprocess
import tempfile

source = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="goodix-worker-test-") as directory:
    binary = Path(directory) / "test-worker"
    subprocess.run([
        os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra", "-Werror",
        "-O1", "-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
        str(source / "test_worker.c"), str(source / "goodix-worker-ops.c"),
        "-o", str(binary),
    ], check=True)
    subprocess.run([str(binary)], check=True)

    dependencies = Path(os.environ.get("PHONEWAVE_DEPENDENCIES", str(Path.home() / ".cache/phonewave/src")))
    includes = [dependencies / "quic-teec/libqcomtee/include"]
    includes += [dependencies / "minkipc/listeners/libfsservice" / p for p in ("fs", "gpfs", "cmn")]
    storage_binary = Path(directory) / "test-storage"
    subprocess.run([
        os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra", "-Werror",
        "-Wno-unused-function", "-O1", "-g", "-pthread", "-fsanitize=address,undefined",
        "-fno-omit-frame-pointer", "-ffunction-sections", "-fdata-sections", "-Wl,--gc-sections",
        *["-I" + str(p) for p in includes], str(source / "test_storage.c"),
        "-o", str(storage_binary),
    ], check=True)
    storage_directory = Path(directory) / "storage"
    storage_directory.mkdir()
    subprocess.run([str(storage_binary), str(storage_directory)], check=True)

    authorization_binary = Path(directory) / "test-authorization"
    subprocess.run([
        os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra", "-Werror",
        "-O1", "-g", "-pthread", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
        str(source / "test_authorization.c"), "-o", str(authorization_binary),
    ], check=True)
    subprocess.run([str(authorization_binary)], check=True)
