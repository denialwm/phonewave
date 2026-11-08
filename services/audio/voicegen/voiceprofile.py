#!/usr/bin/env python3
"""Builds a voice call profile, the firmware file the kernel's q6apm voice
session runs, from a device's stock ACDB.

Usage: voiceprofile.py DEVICE_VOICE_TOML OUTPUT [--vault DIR]

voicegen runs the call through the open AudioReach graph services against
fakedsp.c, which records each command; the profile keeps the two graphs'
GRAPH_OPEN payloads and calibration, adds the host-side settings the stock
audio HAL sends (VSID, channels, calibration keys, endpoint formats and
interfaces) and the order to run them in. Format: README.md.
"""
import argparse
import os
import re
import struct
import subprocess
import sys
import tempfile
import tomllib

from audioreach import (GRAPH_OPEN, REGISTER_CFG, SET_CFG, body, connections, modules, params,
                        records)

HERE = os.path.dirname(os.path.abspath(__file__))
ARGS = os.path.join(HERE, 'build', 'args')

APM, VCPM = 1, 4
OPEN, SET, REGISTER, PREPARE, START = 1, 2, 3, 4, 5
RX, TX = 0, 1

MODULES_LIST, MODULE_PROP, MODULE_CONN, CTRL_LINK = 0x08001002, 0x08001003, 0x08001004, 0x08001061
HW_EP_MF_CFG = 0x08001017
I2S_INTF_CFG = 0x08001019
MFC = 0x07001015
MFC_OUTPUT_MEDIA_FORMAT = 0x08001024
CODEC_DMA_INTF_CFG = 0x08001063
VCPM_VSID, VCPM_TX_CHANNELS, VCPM_CAL_KEYS = 0x080011BC, 0x08001310, 0x0800116B
VCPM_CAL_TABLE = 0x08001163
VOLUME_LEVEL, VOLUME_BOOST, BWE = 0x08001166, 0x080011B6, 0x08001167
PCM = 1  # DATA_FORMAT_FIXED_POINT
# The backends a voice graph can run on, as dt-bindings/sound/qcom,q6dsp-lpass-ports.h
PORTS = {
    'PRIMARY_MI2S_RX': 16, 'SECONDARY_MI2S_RX': 18, 'TERTIARY_MI2S_RX': 20,
    'QUATERNARY_MI2S_RX': 22, 'QUINARY_MI2S_RX': 127,
    'WSA_CODEC_DMA_RX_0': 105, 'RX_CODEC_DMA_RX_0': 113,
    'TX_CODEC_DMA_TX_0': 114, 'TX_CODEC_DMA_TX_3': 120, 'VA_CODEC_DMA_TX_0': 110,
}


def u32s(*values):
    return struct.pack(f'<{len(values)}I', *values)


def param(miid, pid, value):
    value = bytes(value)
    return struct.pack('<4I', miid, pid, len(value), 0) + value + bytes(-len(value) % 8)


def keys(pairs):
    return ','.join(f'{k:#x}={v:#x}' for k, v in pairs)


def voicegen(acdb, *args, env=None):
    env = dict(env or os.environ)
    env['LD_LIBRARY_PATH'] = ':'.join(
        [os.path.join(HERE, 'build', 'lib')] +
        [os.path.join(ARGS, d, '.libs') for d in ('gsl', 'acdb', 'ar_osal', 'ar_util', 'spf')])
    run = subprocess.run([os.path.join(HERE, 'build', 'voicegen'), acdb, *args],
                         env=env, capture_output=True, text=True, timeout=60)
    if run.returncode:
        sys.exit(f'voicegen failed:\n{run.stdout}{run.stderr}')
    return run.stdout


def record(voice, acdb, channels):
    """Runs the call through GSL and returns what the stand-in DSP saw."""
    with tempfile.NamedTemporaryFile(suffix='.bin') as log:
        voicegen(acdb, 'call', keys(voice['rx']['keys']), keys(voice['tx']['keys']),
                 str(voice['vsid']), str(channels),
                 env=dict(os.environ, FAKEDSP_LOG=log.name))
        return open(log.name, 'rb').read()


def volume_levels(voice, acdb):
    """The call volume levels the ACDB calibrates the RX graph for."""
    listing = voicegen(acdb, 'list', keys(voice['rx']['keys']), keys(voice['tx']['keys']),
                       env=dict(os.environ, FAKEDSP_LOG=os.devnull))
    rx = listing.split('tx ckvs:')[0]
    levels = set(re.findall(rf'{VOLUME_LEVEL:08x}=([0-9a-f]+)', rx))
    if not levels:
        sys.exit('the ACDB calibrates no call volume levels')
    return len(levels)


def endpoint_channels(recording, endpoint):
    """The channels the ACDB gives an endpoint's media format."""
    for op, dst, src, tok, inband, oob in records(recording):
        if op == SET_CFG:
            for miid, pid, v in params(body(inband, oob)):
                if miid == endpoint and pid == HW_EP_MF_CFG:
                    return struct.unpack_from('<IHH', v)[2]
    sys.exit(f'the ACDB gives endpoint {endpoint:#x} no media format')


def without_modules(payload, drop, join):
    """A GRAPH_OPEN payload without the `drop` modules, their neighbours joined."""
    out = b''
    for miid, pid, v in params(payload):
        n, = struct.unpack_from('<I', v)
        if pid == MODULES_LIST:
            o, groups = 4, []
            for _ in range(n):
                sg, container, count = struct.unpack_from('<3I', v, o)
                o += 12
                mods = [struct.unpack_from('<2I', v, o + 8 * i) for i in range(count)]
                o += 8 * count
                mods = [m for m in mods if m[1] not in drop]
                groups.append(u32s(sg, container, len(mods)) + b''.join(u32s(*m) for m in mods))
            v = u32s(len(groups)) + b''.join(groups)
        elif pid == MODULE_PROP:
            o, kept = 4, []
            for _ in range(n):
                iid, nprops = struct.unpack_from('<2I', v, o)
                start = o
                o += 8
                for _ in range(nprops):
                    _, size = struct.unpack_from('<2I', v, o)
                    o += 8 + size
                if iid not in drop:
                    kept.append(v[start:o])
            v = u32s(len(kept)) + b''.join(kept)
        elif pid == MODULE_CONN:
            conns = [struct.unpack_from('<4I', v, 4 + 16 * i) for i in range(n)]
            touched = [c for c in conns if c[0] in drop or c[2] in drop]
            conns = [c for c in conns if c not in touched]
            if touched and join:
                conns.append(tuple(join))
            v = u32s(len(conns)) + b''.join(u32s(*c) for c in conns)
        elif pid == CTRL_LINK:
            o, kept = 4, []
            for _ in range(n):
                a, _, b, _, nprops = struct.unpack_from('<5I', v, o)
                start = o
                o += 20
                for _ in range(nprops):
                    _, size = struct.unpack_from('<2I', v, o)
                    o += 8 + size
                if a not in drop and b not in drop:
                    kept.append(v[start:o])
            v = u32s(len(kept)) + b''.join(kept)
        out += param(miid, pid, v)
    return out


def calibrated_widths(recording, converters):
    """The sample widths the call calibration gives `converters`. VCPM
    applies it when the call starts, over what the host set before. Its
    tables' layout is the ACDB's own, so the converters' parameter records
    are found by their instance and parameter IDs."""
    widths = {}
    for op, dst, src, tok, inband, oob in records(recording):
        if op != REGISTER_CFG:
            continue
        for _, pid, v in params(body(inband, oob)):
            if pid != VCPM_CAL_TABLE:
                continue
            for o in range(0, len(v) - 23, 4):
                miid, param_id, size, error = struct.unpack_from('<4I', v, o)
                if miid in converters and param_id == MFC_OUTPUT_MEDIA_FORMAT and size >= 8 \
                        and not error:
                    bits = struct.unpack_from('<iH', v, o + 16)[1]
                    if bits != 0xffff:
                        widths[miid] = bits
    return widths


def endpoint_mismatches(payload, recording, endpoint, bits, converters):
    """The last format converters on the paths into `endpoint` whose width
    in a call is not the endpoint's: calibrated to another, or left in the
    ACDB's format. An endpoint given another width plays silence."""
    kinds, sources = {}, {}
    for _, pid, v in params(payload):
        if pid == MODULES_LIST:
            kinds.update((iid, mid) for _, _, mid, iid in modules(v))
        elif pid == MODULE_CONN:
            for src, _, dst, _ in connections(v):
                sources.setdefault(dst, []).append(src)
    calibrated = calibrated_widths(recording, {i for i, m in kinds.items() if m == MFC})
    found, seen, todo = [], set(), [endpoint]
    while todo:
        for src in sources.get(todo.pop(), []):
            if src in seen or src not in kinds:
                continue
            seen.add(src)
            if kinds[src] != MFC:
                todo.append(src)
            elif src in calibrated:
                if calibrated[src] != bits:
                    found.append(f'{src:#x} is calibrated to {calibrated[src]} bits')
            elif src not in converters:
                found.append(f'{src:#x} is left in the ACDB\'s format; list it in converters')
    return found


def build(voice, recording, channels, levels):
    rx, tx = voice['rx'], voice['tx']
    drops = [set(side.get('drop', [])) for side in (rx, tx)]
    vsid = voice['vsid']
    graphs, commands = [], []

    def command(kind, graph, destination=APM, data=b''):
        commands.append((kind, graph, destination, data))

    graph = None
    for op, dst, src, tok, inband, oob in records(recording):
        data = body(inband, oob)
        if op == GRAPH_OPEN:
            graph = len(graphs)
            side = (rx, tx)[graph]
            graphs.append(without_modules(data, drops[graph], side.get('join'))
                          if drops[graph] else data)
            command(OPEN, graph)
        elif op == SET_CFG and dst == APM and graph is not None:
            command(SET, graph, APM, b''.join(param(m, p, v) for m, p, v in params(data)
                                               if m not in drops[graph]))
        elif op == REGISTER_CFG:
            command(REGISTER, graph, APM, data)

    wrong = endpoint_mismatches(graphs[RX], recording, rx['endpoint'], rx['format'][1],
                                set(rx.get('converters', [])))
    if wrong:
        sys.exit(f'rx: endpoint {rx["endpoint"]:#x} plays {rx["format"][1]} bits, but converter ' +
                 '; '.join(wrong))

    command(SET, RX, VCPM, param(VCPM, VCPM_VSID, u32s(vsid)))
    command(SET, TX, VCPM, param(VCPM, VCPM_TX_CHANNELS, u32s(vsid, channels)))
    # The kernel's "Voice Call Volume" control rewrites the volume level.
    command(SET, RX, VCPM, param(VCPM, VCPM_CAL_KEYS, u32s(
        vsid, 3, VOLUME_LEVEL, 0, VOLUME_BOOST, 0, BWE, 0)))

    for side, graph, count in ((rx, RX, rx['format'][2:3]), (tx, TX, [channels])):
        rate, bits = side['format'][:2]
        count = count[0]
        data = b''
        for miid in side.get('converters', []):
            channel_map = struct.pack(f'<{count}H', *range(1, count + 1))
            data += param(miid, MFC_OUTPUT_MEDIA_FORMAT,
                          struct.pack('<iHH', rate, bits, count) + channel_map)
        data += param(side['endpoint'], HW_EP_MF_CFG, struct.pack('<IHHI', rate, bits, count, PCM))
        if 'i2s' in side:
            lpaif, interface, line, word_select = side['i2s']
            data += param(side['endpoint'], I2S_INTF_CFG,
                          struct.pack('<IIHH', lpaif, interface, line, word_select))
        if 'codec_dma' in side:
            lpaif, interface = side['codec_dma']
            data += param(side['endpoint'], CODEC_DMA_INTF_CFG,
                          u32s(lpaif, interface, (1 << count) - 1))
        command(SET, graph, APM, data)
        command(PREPARE, graph)
        command(START, graph)

    mvop = u32s(0x504f564d, 1, len(graphs[RX]), len(graphs[TX])) + graphs[RX] + graphs[TX]
    out = b''
    for sequence, (kind, graph, destination, data) in enumerate(commands, 1):
        out += u32s(kind, graph, destination, len(data), sequence, 0) + data
    board = u32s(PORTS[rx['port']], PORTS[tx['port']], rx['format'][0], rx['format'][1],
                 (1 << channels) - 1, levels)
    return u32s(0x4153564d, 2, len(mvop), len(commands)) + board + mvop + out


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('voice', help="the device's audio/voice.toml")
    parser.add_argument('output')
    parser.add_argument('--vault', default=os.environ.get(
        'PHONEWAVE_VAULT', '/mnt/development/denial-os/device-vault'))
    options = parser.parse_args()
    with open(options.voice, 'rb') as f:
        voice = tomllib.load(f)
    acdb = os.path.join(options.vault, voice['acdb'])

    # The DSP's channel info must match what the ACDB gives the microphones.
    channels = 1
    recording = record(voice, acdb, channels)
    found = endpoint_channels(recording, voice['tx']['endpoint'])
    if found != channels:
        channels = found
        recording = record(voice, acdb, channels)
    levels = volume_levels(voice, acdb)
    profile = build(voice, recording, channels, levels)
    with open(options.output, 'wb') as f:
        f.write(profile)
    print(f'{options.output}: {len(profile)} bytes, {channels} microphone channel(s), '
          f'{levels} volume levels')


if __name__ == '__main__':
    main()
