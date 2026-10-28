#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Synthetic encrypted-storage stand-ins; never opens a device or real keys."""
import os
from pathlib import Path
import subprocess
import tempfile
source = Path(__file__).resolve().parent
mink = Path(os.environ.get('PHONEWAVE_DEPENDENCIES', str(Path.home()/'.cache/phonewave/src'))) / 'minkipc/listeners/libfsservice'
with tempfile.TemporaryDirectory(prefix='nx809j-secure-storage-test-') as folder:
    binary = Path(folder) / 'test'
    subprocess.run([os.environ.get('CC', 'cc'), '-std=gnu11', '-Wall', '-Wextra', '-Werror',
                    '-g', '-O1', '-fsanitize=address,undefined', '-fno-omit-frame-pointer',
                    '-I'+str(mink/'gpfs'), '-I'+str(mink/'cmn'),
                    str(source/'test_secure_storage.c'), str(source/'secure-storage.c'),
                    '-o', str(binary)], check=True)
    subprocess.run([str(binary), folder], check=True)
