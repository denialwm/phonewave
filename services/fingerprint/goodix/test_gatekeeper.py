#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Synthetic Gatekeeper wire tests; no device or real credential is used."""
import os
from pathlib import Path
import subprocess
import tempfile

source = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="goodix-gatekeeper-test-") as directory:
    binary = Path(directory) / "test-gatekeeper"
    subprocess.run([
        os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra", "-Werror",
        "-O1", "-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
        str(source / "test_gatekeeper.c"), str(source / "gatekeeper.c"),
        "-o", str(binary),
    ], check=True)
    subprocess.run([str(binary)], check=True)

    dependencies = Path(os.environ.get("PHONEWAVE_DEPENDENCIES", str(Path.home() / ".cache/phonewave/src")))
    worker_test = Path(directory) / "test-gatekeeper-worker"
    subprocess.run([
        os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra", "-Werror",
        "-O1", "-g", "-pthread", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
        "-ffunction-sections", "-fdata-sections", "-Wl,--gc-sections",
        "-I" + str(dependencies / "quic-teec/libqcomtee/include"),
        str(source / "test_gatekeeper_worker.c"), "-o", str(worker_test),
    ], check=True)
    subprocess.run([str(worker_test)], check=True)
