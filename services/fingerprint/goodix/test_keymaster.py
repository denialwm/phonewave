#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Run synthetic Keymaster protocol/flow tests with memory/UB sanitizers."""
import os
from pathlib import Path
import subprocess
import tempfile

source = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="goodix-keymaster-test-") as directory:
    binary = Path(directory) / "test-keymaster"
    subprocess.run([
        os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra", "-Werror",
        "-O1", "-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
        str(source / "test_keymaster.c"), str(source / "keymaster-shared-secret.c"),
        str(source / "keymaster-wrapped-key.c"),
        "-o", str(binary),
    ], check=True)
    subprocess.run([str(binary)], check=True)
