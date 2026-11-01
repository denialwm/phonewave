"""Check a GPT pair (primary LBA 0..5, backup last 5 LBAs) and build pudding's new table.

check PRIMARY BACKUP
    verify signatures, both header CRCs, both entry-array CRCs, and that the
    two entry arrays are identical.
build PRIMARY BACKUP OUTDIR USERDATA_END LINUX_START PARTUUID
    shrink userdata to end at USERDATA_END, add 'linux' (Linux filesystem)
    from LINUX_START to the last usable LBA, write new primary and backup.
"""
import struct
import sys
import uuid
import zlib

BS = 4096
HDR = struct.Struct('<8sIIIIQQQQ16sQIII')
LINUX_FS = uuid.UUID('0fc63daf-8483-4772-8e79-3d69d8477de4')


def parse(blob, hdr_lba_in_blob, ent_lba_in_blob):
    h = list(HDR.unpack(blob[hdr_lba_in_blob * BS:hdr_lba_in_blob * BS + 92]))
    n, size = h[11], h[12]
    ents = blob[ent_lba_in_blob * BS:ent_lba_in_blob * BS + n * size]
    return h, bytearray(ents)


def header_bytes(h):
    h = list(h)
    h[3] = 0
    raw = HDR.pack(*h)
    h[3] = zlib.crc32(raw)
    return HDR.pack(*h)


def check(primary, backup):
    ph, pe = parse(primary, 1, 2)
    bh, be = parse(backup, 4, 0)
    for name, h, e in (('primary', ph, pe), ('backup', bh, be)):
        assert h[0] == b'EFI PART', name
        assert header_bytes(h) == HDR.pack(*h), f'{name} header CRC'
        assert zlib.crc32(e) == h[13], f'{name} entries CRC'
    assert pe == be, 'entry arrays differ'
    assert ph[5] == 1 and bh[5] == ph[6] and bh[6] == 1, 'header LBAs'
    return ph, pe, bh


def entries(e, size):
    for i in range(len(e) // size):
        x = e[i * size:(i + 1) * size]
        if x[:16] != b'\0' * 16:
            start, end = struct.unpack('<QQ', x[32:48])
            yield i, start, end, x[56:128].decode('utf-16-le').rstrip('\0')


def main():
    cmd = sys.argv[1]
    primary = open(sys.argv[2], 'rb').read()
    backup = open(sys.argv[3], 'rb').read()
    ph, pe, bh = check(primary, backup)
    size = ph[12]
    used = list(entries(pe, size))
    print(f'ok: {len(used)} entries in use, last usable {ph[8]}, backup header at {ph[6]}')
    if cmd == 'check':
        return
    out, ud_end, lx_start, partuuid = sys.argv[4], int(sys.argv[5]), int(sys.argv[6]), uuid.UUID(sys.argv[7])
    last = ph[8]
    (ud_i, ud_start, _, _), = [u for u in used if u[3] == 'userdata']
    assert ud_start <= ud_end < lx_start <= last
    assert all(end < ud_start or i == ud_i for i, _, end, _ in used), 'userdata is not the last partition'
    free = next(i for i in range(len(pe) // size) if pe[i * size:i * size + 16] == b'\0' * 16)
    pe[ud_i * size + 40:ud_i * size + 48] = struct.pack('<Q', ud_end)
    new = LINUX_FS.bytes_le + partuuid.bytes_le + struct.pack('<QQQ', lx_start, last, 0)
    new += 'linux'.encode('utf-16-le').ljust(72, b'\0')
    pe[free * size:(free + 1) * size] = new.ljust(size, b'\0')
    crc = zlib.crc32(pe)
    ph[13] = crc
    bh[13] = crc
    new_primary = bytearray(primary)
    new_primary[BS:BS + 92] = header_bytes(ph)
    new_primary[2 * BS:2 * BS + len(pe)] = pe
    new_backup = bytearray(backup)
    new_backup[4 * BS:4 * BS + 92] = header_bytes(bh)
    new_backup[0:len(pe)] = pe
    check(bytes(new_primary), bytes(new_backup))
    open(f'{out}/sda-primary-lba0-5.bin', 'wb').write(new_primary)
    open(f'{out}/sda-backup-lba{ph[6] - 4}-{ph[6]}.bin', 'wb').write(new_backup)
    print(f'userdata entry {ud_i}: {ud_start}-{ud_end}; linux entry {free}: {lx_start}-{last} partuuid {partuuid}')


main()
