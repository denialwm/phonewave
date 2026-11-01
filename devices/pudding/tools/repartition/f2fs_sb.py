"""Print the geometry fields of an f2fs superblock (first 4 KiB of the device)."""
import struct
import sys

b = open(sys.argv[1], 'rb').read()[1024:1024 + 128]
(magic, major, minor, log_sectorsize, log_sectors_per_block, log_blocksize, log_blocks_per_seg,
 segs_per_sec, secs_per_zone, checksum_offset, block_count, section_count, segment_count,
 segment_count_ckpt, segment_count_sit, segment_count_nat, segment_count_ssa, segment_count_main,
 segment0_blkaddr) = struct.unpack('<IHHIIIIIIIQIIIIIIII', b[:76])
assert magic == 0xF2F52010, hex(magic)
print(f'version={major}.{minor} block={1 << log_blocksize} blocks_per_seg={1 << log_blocks_per_seg} '
      f'segs_per_sec={segs_per_sec} block_count={block_count} section_count={section_count} '
      f'segment_count={segment_count} segment_count_main={segment_count_main} segment0_blkaddr={segment0_blkaddr}')
