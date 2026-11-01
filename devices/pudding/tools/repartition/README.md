# Repartition: room for Denial beside Android

These tools shrank Android's `userdata` and added a `linux` partition at
the end of `sda` while Android kept running. They need KernelSU root over
adb.

- `f2fs-resize.c`: shrinks a mounted f2fs with `F2FS_IOC_RESIZE_FS`.
- `f2fs_sb.py`: prints the geometry of an f2fs superblock, so the new size
  can be checked before the table is written.
- `gpt_tool.py`: checks a GPT pair and builds the new one. `check` verifies
  both headers, both entry arrays and that the two copies agree. `build`
  ends `userdata` at a given block and adds `linux` from a given block to the
  last usable one.

`sda` uses 4 KiB blocks. Its primary GPT is blocks 0 to 5 and its backup
the last 5 blocks of the disk. `/data` is f2fs on a `default-key` mapping
of `userdata` from offset 0, so filesystem blocks are partition blocks.
Read the superblock through the mapping (`/dev/block/dm-N` of `/data`):
on the partition itself it is encrypted.

## Build

```sh
mkdir -p ~/.cache/phonewave/pudding/repartition
aarch64-linux-gnu-gcc -static -O2 -Wall \
  -o ~/.cache/phonewave/pudding/repartition/f2fs-resize f2fs-resize.c
```

## Steps

At every mount, vold runs `resize.f2fs -F` on `userdata`, which grows the
filesystem back to the size of its partition. Shrink and table write
therefore happen in one Android session, with no reboot between them.

1. Save both GPT copies (`dd` of blocks 0-5 and of the last 5 blocks) to
   the device vault and run `gpt_tool.py check` on them.
2. Pick the layout: `linux` starts on a 2 MiB boundary (a multiple of 512
   blocks) and ends at the last usable block; `userdata` ends one block
   before it.
3. Build the new table with `gpt_tool.py build`. Before it goes near the
   phone, write both copies into a sparse file the size of the disk and run
   `sfdisk --sector-size 4096 --verify` on it; `cmp -l` against the old
   table should show only the two CRCs, the end of `userdata` and the new
   entry.
4. Check that the table on the phone still equals the saved one.
5. Shrink: `f2fs-resize /data BLOCKS`, with BLOCKS a multiple of 512 and a
   few MiB under the new size of `userdata`. f2fs rounds the result up by a
   few hundred blocks: read the superblock with `f2fs_sb.py` and check that
   `block_count` is no larger than the new `userdata`.
6. Write the primary to block 0 and the backup to the last 5 blocks with
   `dd ... conv=notrunc,fsync`, then read both back and compare hashes.
   Read back in its own `adb exec-out` call: anything else the command
   prints ends up in the read.
7. Reboot. Check the sizes in `/sys/block/sda/sdaN/size` (512-byte
   sectors), `/data` in `df`, and the fsck time in
   `ro.boottime.init.fsck.data`.

Android can format `userdata` itself, which leaves `linux` alone. The
saved tables bring back the old layout.
