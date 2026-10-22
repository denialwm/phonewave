#!/usr/bin/env python3
"""Reads what fakedsp.c recorded: GPR commands and the AudioReach parameters
inside them.

Usage: audioreach.py RECORDING   lists the commands, parameters and graphs
"""
import struct
import sys

GRAPH_OPEN, PREPARE, START = 0x01001000, 0x01001001, 0x01001002
SET_CFG, GET_CFG, REGISTER_CFG = 0x01001006, 0x01001007, 0x01001008
REGISTER_SHARED_CFG = 0x0100100A
NAMES = {GRAPH_OPEN: 'GRAPH_OPEN', PREPARE: 'PREPARE', START: 'START',
         SET_CFG: 'SET_CFG', GET_CFG: 'GET_CFG', REGISTER_CFG: 'REGISTER_CFG',
         REGISTER_SHARED_CFG: 'REGISTER_SHARED_CFG', 0x0100100C: 'MEM_MAP',
         0x0100101E: 'AMDB_REGISTER', 0x01001020: 'AMDB_LOAD',
         0x01001021: 'GET_SPF_STATE'}

MODULES_LIST, MODULE_PROP, MODULE_CONN = 0x08001002, 0x08001003, 0x08001004
CTRL_LINK = 0x08001061


def records(data):
    """(opcode, destination, source, token, in-band, out-of-band) per command."""
    o = 0
    while o + 24 <= len(data):
        op, dst, src, tok, isz, osz = struct.unpack_from('<6I', data, o)
        o += 24
        inband = data[o:o + isz]
        o += isz
        oob = data[o:o + osz]
        o += osz
        yield op, dst, src, tok, inband, oob


def body(inband, oob):
    """A command's parameters: out of band, or in band after the APM header."""
    return oob if oob else inband[16:]


def params(data):
    """(instance, parameter, value) per apm_module_param_data record."""
    o = 0
    while o + 16 <= len(data):
        miid, pid, size, _ = struct.unpack_from('<4I', data, o)
        yield miid, pid, data[o + 16:o + 16 + size]
        o += 16 + ((size + 7) & ~7)


def modules(v):
    n, = struct.unpack_from('<I', v)
    o, out = 4, []
    for _ in range(n):
        sg, container, count = struct.unpack_from('<3I', v, o)
        o += 12
        for _ in range(count):
            mid, iid = struct.unpack_from('<2I', v, o)
            o += 8
            out.append((sg, container, mid, iid))
    return out


def connections(v):
    n, = struct.unpack_from('<I', v)
    return [struct.unpack_from('<4I', v, 4 + 16 * i) for i in range(n)]


def show(data):
    for i, (op, dst, src, tok, inband, oob) in enumerate(records(data)):
        print(f'{i:3} {NAMES.get(op, hex(op)):19} dst={dst:#x} in={len(inband)} oob={len(oob)}')
        if op not in (GRAPH_OPEN, SET_CFG, REGISTER_CFG, REGISTER_SHARED_CFG):
            continue
        for miid, pid, v in params(body(inband, oob)):
            if pid == MODULES_LIST:
                for sg, container, mid, iid in modules(v):
                    print(f'      subgraph {sg:#x} container {container:#x} module {mid:#010x} instance {iid:#x}')
            elif pid == MODULE_CONN:
                for s, sp, d, dp in connections(v):
                    print(f'      {s:#x}:{sp} -> {d:#x}:{dp}')
            else:
                print(f'      {miid:#06x} {pid:#010x} {len(v):6} {v[:24].hex()}')


if __name__ == '__main__':
    show(open(sys.argv[1], 'rb').read())
