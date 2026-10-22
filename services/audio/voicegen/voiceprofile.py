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
import struct
import subprocess
import sys
import tempfile
import tomllib

from audioreach import GRAPH_OPEN, REGISTER_CFG, SET_CFG, body, params, records

HERE = os.path.dirname(os.path.abspath(__file__))
ARGS = os.path.join(HERE, 'build', 'args')

APM, VCPM = 1, 4
OPEN, SET, REGISTER, PREPARE, START = 1, 2, 3, 4, 5
RX, TX = 0, 1

MODULES_LIST, MODULE_PROP, MODULE_CONN, CTRL_LINK = 0x08001002, 0x08001003, 0x08001004, 0x08001061
HW_EP_MF_CFG = 0x08001017
I2S_INTF_CFG = 0x08001019
MFC_OUTPUT_MEDIA_FORMAT = 0x08001024
CODEC_DMA_INTF_CFG = 0x08001063
VCPM_VSID, VCPM_TX_CHANNELS, VCPM_CAL_KEYS = 0x080011BC, 0x08001310, 0x0800116B
VOLUME_LEVEL, VOLUME_BOOST, BWE = 0x08001166, 0x080011B6, 0x08001167
PCM = 1  # DATA_FORMAT_FIXED_POINT


def u32s(*values):
    return struct.pack(f'<{len(values)}I', *values)


def param(miid, pid, value):
    value = bytes(value)
    return struct.pack('<4I', miid, pid, len(value), 0) + value + bytes(-len(value) % 8)


def keys(pairs):
    return ','.join(f'{k:#x}={v:#x}' for k, v in pairs)


def record(voice, acdb, channels):
    """Runs the call through GSL and returns what the stand-in DSP saw."""
    env = dict(os.environ)
    env['LD_LIBRARY_PATH'] = ':'.join(
        [os.path.join(HERE, 'build', 'lib')] +
        [os.path.join(ARGS, d, '.libs') for d in ('gsl', 'acdb', 'ar_osal', 'ar_util', 'spf')])
    with tempfile.NamedTemporaryFile(suffix='.bin') as log:
        env['FAKEDSP_LOG'] = log.name
        run = subprocess.run(
            [os.path.join(HERE, 'build', 'voicegen'), acdb, 'call', keys(voice['rx']['keys']),
             keys(voice['tx']['keys']), str(voice['vsid']), str(channels)],
            env=env, capture_output=True, text=True, timeout=60)
        if run.returncode:
            sys.exit(f'voicegen failed:\n{run.stdout}{run.stderr}')
        return open(log.name, 'rb').read()


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
            if touched:
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


def build(voice, recording, channels):
    rx, tx = voice['rx'], voice['tx']
    drop = set(rx.get('drop', []))
    vsid = voice['vsid']
    graphs, commands = [], []

    def command(kind, graph, destination=APM, data=b''):
        commands.append((kind, graph, destination, data))

    graph = None
    for op, dst, src, tok, inband, oob in records(recording):
        data = body(inband, oob)
        if op == GRAPH_OPEN:
            graph = len(graphs)
            graphs.append(without_modules(data, drop, rx.get('join')) if graph == RX else data)
            command(OPEN, graph)
        elif op == SET_CFG and dst == APM and graph is not None:
            command(SET, graph, APM, b''.join(param(m, p, v) for m, p, v in params(data)
                                               if m not in drop))
        elif op == REGISTER_CFG:
            command(REGISTER, graph, APM, data)

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
    return u32s(0x4153564d, 1, len(mvop), len(commands)) + mvop + out


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
    profile = build(voice, recording, channels)
    with open(options.output, 'wb') as f:
        f.write(profile)
    print(f'{options.output}: {len(profile)} bytes, {channels} microphone channel(s)')


if __name__ == '__main__':
    main()
