# dmail

Modem service for Qualcomm phones. It talks QMI to the modem over QRTR and
replaces ModemManager on Phonewave devices.

Named after D-Mail: in Steins;Gate, the Phonewave sends text messages.

Status: on aston since 2026-10-21, as `dmail.service`: `Modem1`, `Data1` and
`Messages1` on the system bus, with reconnect and the data toggle, delivery
reports, archive, blocking, search and the SIM slot setting. Denial's
shell and the Messages app (denial-apps) read it. `dmailctl` talks QMI
directly, for bring-up.

## Install

`make install` (PREFIX=/usr) installs `dmail`, `dmailctl`, the unit, the bus
policy and activation file, and a sysusers file for the `dmail` group. The
phone adds `/etc/dmail/device.toml`, its `modem-setup` if it needs one, and
puts its user in `dmail` from `devices/<codename>/rootfs/`. Processes started
before the user joined the group have no access until they restart: the
Denial session, and the user manager (it lingers on aston).

## Decisions

Decided on 2026-10-21.

- One service for `aston` and `roadstr`. What differs per phone goes in
  `devices/<codename>/`.
- What a phone needs before its modem answers goes in an optional
  `modem-setup` script in `devices/<codename>/`. dmail runs it if present and
  stops if it fails. Aston has one, roadstr doesn't.
- Its own D-Bus API, `org.dmail.Modem1`. Not a copy of ModemManager's.
- It replaces ModemManager. The unit conflicts with `ModemManager.service`.
- denial-apps (Dialer, Messages) talk to dmail only. Denial's shell keeps its
  ModemManager code and uses whichever of the two is running.
- IMS and VoLTE are built in. No OpenIMSd, no postmarketOS code.
- Droidloom does no telephony.
- NetworkManager stays for Wi-Fi. dmail owns the modem, every data session on
  it, reconnect included, and the rmnet link: NetworkManager ignores rmnet
  links without ModemManager (tested on aston).
- Cellular DNS goes through systemd-resolved, set per link by dmail.
- New-SMS notifications come from a background service of the Messages app,
  as on Android. dmail never notifies.
- Delivery reports are off by default; a setting turns them on.
- Dual SIM: one SIM at a time, slot 1 by default, chosen once in a setting.
- Archive and the block list live in dmail's store, so every client sees
  them. A blocked sender's messages are kept in a hidden folder, not dropped.
- Rust, with its own QMI codec over QRTR sockets. No libqmi, no libqrtr.

## What it does

The modem firmware runs SIP and call audio. dmail starts the modem and
configures it.

| Job | QMI service |
|---|---|
| Identity, power | DMS |
| SIM, PIN | UIM |
| Registration, signal, operator | NAS |
| Data sessions: internet and IMS, over rmnet on IPA | WDS |
| SMS | WMS |
| Calls | Voice |
| Carrier profile for the SIM | PDC |
| VoLTE setup, IMS data requests, IMS registration | IMS Settings, IMSDCM, IMSA |

Getting the modem to answer is the phone's `modem-setup` script, when it needs
one.

## Devices

| Device | State |
|---|---|
| `aston` | Proven by hand on devices23: both IMEIs, LTE, IPv4, SMS send and receive. rmtfs must serve the board response the modem asks for, from the phone's own private EFS copy, never this repo. With Iliad's VoLTE carrier profile the modem is voice-centric with VoLTE preferred; while IMS is not running, the network rejects packet service with cause 7 about a minute after each LTE attach. Data-centric with voice over 2G/3G, data stayed up: 12 of 12 HTTPS checks over 6 minutes on 4G (2026-10-21) |
| `roadstr` | Waits on the kernel: MPSS PAS, the rmtfs carve-out, Eliza IPA |

## MVP, on aston

SMS, mobile data with a toggle and the network type (2G to 5G), voice as
stubs. Design, approved on 2026-10-21: [DESIGN.md](DESIGN.md).

## Open

- Live checks for the MVP: see [DESIGN.md](DESIGN.md#open).
- IMSDCM message layouts: libqmi may not define them.
- Whether aston's and roadstr's firmware expose the IMS services, and whether
  the carrier offers VoLTE.

## Prior art

[`../qmi-tools`](../qmi-tools): the QMI clients used for aston's bring-up.
