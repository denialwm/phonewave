# dmail design

MVP on `aston`: SMS, mobile data with a toggle and the network type (2G to
5G), voice as stubs.

Approved on 2026-10-21. Sources: the hand-run bring-up on devices23 (QMI
sequences below) and checks on devices39 on 2026-10-21. "To verify" marks what
needs the modem running.

## Inside

One process, in Rust 2024 with zbus 5 and async-io, like hapticd.

| Module | Job |
|---|---|
| `qrtr` | AF_QIPCRTR sockets, service lookup, services appearing and leaving |
| `qmi` | 7-byte header and TLV codec, one client per service, transaction ids, indications |
| `modem` | the device's `modem-setup`, crash-loop guard, DMS online |
| `sim` | UIM card status, provisioning sessions, PIN |
| `network` | NAS registration, network type, signal, operator |
| `data` | DPM, WDA, WDS, reconnect, rmnet link, addresses, routes and DNS |
| `sms` | WMS, PDU codec, multipart, delivery reports, message store |
| `voice` | stub interface |
| `bus` | the D-Bus interfaces |
| `config` | device file, settings |

Message layouts are written by hand for the messages dmail uses, with libqmi's
definitions as the reference, the way `qmi-tools` does it. No libqmi, no
libqrtr.

Indications drive everything. No polling loops: the modem wakes the SoC only
for registration changes, signal crossing a threshold, data status, SMS and,
later, calls. The one exception is the short wait while a modem that just
started is not ready yet (Boot).

The MVP runs as root under systemd sandboxing: the device's `modem-setup`
needs it, the rmnet link needs CAP_NET_ADMIN, and it opens QRTR sockets.

## Boot

1. If `/usr/lib/dmail/modem-setup` exists, dmail runs it and waits. It comes
   from `devices/<codename>/rootfs/` and does whatever the phone needs before
   its modem answers. Exit 0 means the services will appear. A failure, or no
   exit within 60 s, sets State to `failed`; its output goes to the journal.
   No script: dmail goes straight on. Aston has one, roadstr has none.
2. dmail waits for the QRTR services it uses: DMS, UIM, NAS, WDS, WMS, DPM,
   WDA. Not all of them within 30 s: `failed`.
3. DMS: operating mode online.
4. NAS: usage data-centric, voice over 2G/3G preferred, until the modem
   restarts. The carrier profile the modem picks (Iliad's VoLTE one on aston)
   makes it voice-centric with VoLTE preferred. While IMS is not running, the
   network then rejects packet service with cause 7 about a minute after each
   LTE attach. Data-centric it stays attached (aston, 2026-10-21). Once dmail
   runs IMS, it sets VoLTE back.
5. UIM: open the primary provisioning session on the chosen slot's SIM
   (`Modem1.SimSlot`, see Dual SIM). If PIN1 is enabled and not verified,
   State is `locked` until `UnlockPin`. One attempt per call, retries
   reported, the PIN never stored. dmail follows the modem's card status
   indications, so a PIN entered elsewhere, or a SIM taken out or put in,
   shows too. A card the modem has not detected yet, or a session that does
   not open yet, waits for the next indication.
6. NAS registers by itself. dmail registers for the indications it needs.

**Not ready yet.** Right after its services appear, the modem answers some
requests with "not ready": QMI errors InformationUnavailable (0x4a),
DeviceNotReady (0x34) and UimUninitialized (0x25). dmail asks again every
0.5 s, for at most 20 s. Any other error, no answer within 10 s, or the modem
going away ends the wait. Steps 3 and 4 and the indication requests repeat as
one: each is safe to repeat. On aston after a cold boot (2026-10-21): the
operating mode answered 0x4a 1.5 s after the modem came up and was ready
0.5 s later; listing the SIM's stored SMS as the SIM became ready answered 0x34
for 2.5 s. Before this, dmail took the first 0x4a as fatal and stayed
`failed` until restarted by hand.

**Failed.** `failed` is for what waiting does not fix: `modem-setup`
failing, services missing after 30 s, any other QMI error, the modem going
away 3 times in 10 minutes. The cause goes to the journal. dmail stays on the
bus and follows the name service: when one of its services registers again,
because the modem restarted by kernel recovery or by hand, it starts over
from step 1.

`modem-setup` must be safe to run again with the modem up: it then exits 0 and
changes nothing. Daemons it needs, such as rmtfs and tqftpserv, run as their
own units that it starts. Started from the script itself they would live in
dmail's cgroup and die with dmail, taking the modem down.

On aston the script checks that the private EFS copy belongs to this handset
(`efs.handset` beside it holds the SoC serial of the phone it came from) and
matches its checksums, starts rmtfs read-only on it and tqftpserv, waits until
rmtfs serves, turns remoteproc recovery and coredumps off, and starts the
modem, or recovers it after a crash.

Modem crash: QRTR reports the services gone, also while the modem is still
coming up. dmail runs `modem-setup` again, if there is one, and rebuilds its
clients from step 2. Without a script, kernel recovery restarts the modem.
After 3 crashes in 10 minutes dmail stops restarting it and reports `failed`
until the services come back. Coredumps stay off.

## Network type

From the NAS system info indication, read once at start. Signal from signal
info with thresholds, not polling.

| Shows | When |
|---|---|
| `2g` | GSM service |
| `3g` | WCDMA service, HSPA included |
| `4g` | LTE service, no NR leg |
| `5g` | NR5G service (SA), or LTE with NR5G active (NSA) |
| `none` | no service |

Signal is 0–100, from RSRP on LTE and NR, RSSI on 2G and 3G.

Aston has no 5G: it tops out at `4g`. `5g` stays in the API for roadstr. To
verify on roadstr: which TLV reports NSA.

## Mobile data

One internet session on mux 1. The proven sequence:

1. DPM open port: embedded endpoint 1, rx 2, tx 24 (device config).
2. WDA data format: raw IP, QMAPv5 both ways, 16 datagrams of 8192 bytes
   (device config).
3. rmnet: `rmnet_data0` on `rmnet_ipa0`, mux 1, deaggregation and MAPv5
   checksums on.
4. WDS: bind the mux port, IPv4, start network with the APN, read the current
   settings.
5. dmail sets address, MTU and routes on `rmnet_data0` over rtnetlink.

**dmail owns `rmnet_data0`.** NetworkManager passes rmnet links only to its
ModemManager plugin. Tested on aston (NetworkManager 1.56.1): it logged
"Device factory NMWwanFactory ignores device rmnet_test0" and never created a
device. NetworkManager will not touch the link, and will not take settings for
it either.

**Default route metric 10000.** NetworkManager gives Wi-Fi 600 by default
(aston's profile sets 3015), so Wi-Fi wins. NetworkManager adds 20000 to the
default route of a device without internet (`NetworkManager.conf`,
connectivity section), so behind a captive portal or on dead Wi-Fi, cellular
wins. No extra routing tables.

**DNS through systemd-resolved.** NetworkManager gives resolved the Wi-Fi DNS;
dmail gives it the carrier DNS for `rmnet_data0` (`SetLinkDNS`). resolved sends
lookups to every link marked as a default route, in parallel, and that would
wake the modem for each lookup on Wi-Fi. So dmail marks `rmnet_data0` as a DNS
default route (`SetLinkDefaultRoute`) only while its default route is the best
one, following the routing table over rtnetlink. Not done yet: resolved is
masked on aston (see Device config), so lookups go over Wi-Fi's DNS.

**Toggle.** `Data1.SetEnabled`, saved in `/var/lib/dmail/`. When on, the
session is up whenever the modem is registered with packet service attached,
Wi-Fi or not: failover is instant, and IMS will need the link later. When off:
stop the network, remove the link and routes.

**Reconnect.** When the WDS packet service status reports a disconnect, remove
the address and route, then retry after 2 s, 4 s, 8 s and so on, up to 60 s.
Retry at once when NAS reports packet service attached again. Log the end
reason. After a reject with cause 7, 8 or 14 dmail stops retrying until packet
service attaches again, and Data1 shows `failed` (Open, 1).

**APN.** `/etc/dmail/apn.toml` if present, else mobile-broadband-provider-info
(installed on aston) by the SIM's home network, else the modem's default
profile.

IPv4 only in the MVP.

## SMS

Receive:

1. WMS routes: new messages go to dmail. On aston the routes are "transfer
   only", so dmail acks; class 2 messages still go to the SIM.
2. On the event report, dmail writes the PDU to its store, then acks. If dmail
   dies before the ack, the network sends the message again.
3. At start, and when the SIM unlocks, dmail reads what the modem or SIM
   stored while it was down. It leaves the messages there: deleting is
   irreversible, and the store skips a PDU it already has.
4. Decode GSM 7-bit, 8-bit and UCS-2. Join multipart messages by sender,
   reference and total. Parts still missing after 24 hours show as they are.

Send: GSM 7-bit when every character fits, else UCS-2. Long texts are split
with concatenation headers. The SMSC comes from the SIM. State goes
`sending`, then `sent` or `failed`, the cause in the journal.

Delivery reports: off by default, `Messages1.SetDeliveryReports` turns them
on. Each report is an SMS from the network. When on:

1. Every part asks for a status report (TP-SRR). The modem gives each part
   its message reference (TP-MR) and returns it from the send; dmail records
   it per part.
2. The SMS-STATUS-REPORT arrives like a received message (aston's routes
   transfer status reports to clients) and is acked the same way. dmail
   matches it to the newest part still waiting with that reference whose
   message's peer matches the recipient by the conversation rule; references
   wrap at 256.
3. TP-Status 0x00–0x1f is delivered, 0x20–0x3f means the service centre keeps
   trying and another report follows, 0x46 is expired, anything else
   undelivered.
4. Once every part has an outcome, the message goes from `sent` to
   `delivered` if all were, else to the first other outcome, `undelivered` or
   `expired`. A message that failed to send keeps `failed`.

Checked on aston, 2026-10-21, to the SIM's own number: `sent` after 1.1 s,
the copy received after 1.2 s, `delivered` after 6.7 s; the modem's reference
matched the report's.

Store: SQLite in `/var/lib/dmail/messages.db`, readable by dmail only. Apps
read it through D-Bus. Schema steps are counted in `PRAGMA user_version` and
only ever added: each runs once, at start, in a transaction of its own. Step
1 is the first schema, which stores from before the count already have; step
2 adds blocked messages, the archive, the block list, delivery reports and
search, and indexes the text already stored. An older dmail on a newer store
still runs and ignores what it does not know. Before a phone's store takes a
step for the first time, a copy goes beside it as `messages.db.before-<date>`.

Conversations: one per peer. Phone numbers match on their last 9 digits, as
on Android (whole numbers when shorter than 7 digits): the network sends
+39 333…, the SIM's own number has no plus, people type 333…. The first form
seen names the conversation.

Archive: a flag per conversation in the store. `ListConversations` leaves
archived conversations out, `ListArchivedConversations` lists them. A new
message in an archived conversation, received or sent, brings it back, as on
Android. Deleting the conversation clears the flag.

Blocking: a list of numbers and sender names in the store, matched like
conversations. A blocked sender's message is still stored and acked, so the
network stops resending it, but goes to the blocked folder: read, in no
conversation, without `MessageReceived`, left out of unread counts and
search. `ListBlockedMessages` lists the folder; `Delete` takes its ids.
Blocking leaves the conversation as it is. `Unblock` brings that sender's
messages from the folder back into its conversation, read. Messages to a
blocked number go out as usual.

Search: SQLite FTS5 over message text (the bundled SQLite has it), kept in
step by triggers. Each word typed matches words starting with it, case and
accents ignored. Newest first; the blocked folder is left out.

## Dual SIM

Aston has two SIM slots and two IMEIs: the modem can run two subscriptions.
dmail uses one SIM, the one in `Modem1.SimSlot`: slot 1 by default, chosen
once with `SetSimSlot`.

dmail's NAS, WMS and WDS clients use subscription 1, which runs on the
primary provisioning session. So a switch needs no client bindings: dmail
stops the data session, ends the primary session if it runs on the other
slot, ends the secondary one if it runs on the chosen SIM, and opens the
primary session there. NAS registers with that SIM, SMS and data follow, and
the APN is picked again. A slot without a SIM leaves State `no-sim` and the
other SIM unused. It applies at once, and at every start: the modem keeps no
choice across boots on aston, its EFS writes stay in rmtfs memory.

The other way, binding each client to subscription 2, also needs the data
subscription moved (DDS); one primary session covers SMS and data with
nothing else to keep in step.

Checked on aston, 2026-10-21, with one SIM, in slot 1: slot 2 ended the
primary session and showed `no-sim`; slot 1 opened it again, the PIN state
and its retries unchanged.

## Voice

`org.dmail.Voice1` exists in its final shape. Every method returns
`org.dmail.Error.NotSupported` and `Calls` is empty. Incoming calls are not
handled: the caller hears ringing until voicemail.

## D-Bus API

System bus, name `org.dmail`, object `/org/dmail`. States are strings.
Changes go out as PropertiesChanged.

| Interface | Member | Type |
|---|---|---|
| `org.dmail.Modem1` | `State` | `s`: off, starting, no-sim, locked, searching, registered, failed |
| | `Technology` | `s`: none, 2g, 3g, 4g, 5g |
| | `Signal` | `y`: 0–100 |
| | `Operator` | `s` |
| | `Roaming` | `b` |
| | `PinRetries` | `u` |
| | `SimSlot` | `y`: 1 or 2, the SIM for SMS and data |
| | `UnlockPin(s pin)` | |
| | `SetSimSlot(y slot)` | saved, applied at once |
| `org.dmail.Data1` | `Enabled` | `b` |
| | `State` | `s`: off, connecting, connected, failed |
| | `Interface` | `s` |
| | `SetEnabled(b enabled)` | |
| `org.dmail.Messages1` | `DeliveryReports` | `b`, default false |
| | `SetDeliveryReports(b enabled)` | saved, for messages sent from then on |
| | `ListConversations()` | `a(ssxbu)`: peer, last text, time, last outgoing, unread; archived left out |
| | `ListArchivedConversations()` | `a(ssxbu)`: the same, archived only |
| | `ListMessages(s peer, x before, u limit)` | `a(tbsxs)`: id, outgoing, text, time, state |
| | `Send(s peer, s text)` | `t`: id |
| | `MarkRead(s peer)` | then `ConversationRead` |
| | `Delete(t id)` | |
| | `DeleteConversation(s peer)` | then `ConversationDeleted` |
| | `SetArchived(s peer, b archived)` | then `ConversationArchived` |
| | `Search(s text, u limit)` | `a(stbsxs)`: peer, id, outgoing, text, time, state; newest first |
| | `Block(s number)`, `Unblock(s number)` | then `BlockedChanged` |
| | `ListBlocked()` | `as` |
| | `ListBlockedMessages(x before, u limit)` | `a(tssx)`: id, sender, text, time |
| | signal `MessageReceived(t id, s peer)` | |
| | signal `MessageChanged(t id, s state)` | |
| | signal `ConversationRead(s peer)` | |
| | signal `ConversationDeleted(s peer)` | |
| | signal `ConversationArchived(s peer, b archived)` | |
| | signal `BlockedChanged()` | |
| `org.dmail.Voice1` | `Calls` | empty |
| | `Dial(s number)`, `Answer()`, `Hangup()` | NotSupported |

Message states: `sending`, `sent`, `failed`, `delivered`, `undelivered` and
`expired` going out; `received` coming in. Peers in signals are conversation
peers, as `ListConversations` names them. `before` 0 means the newest.

Access: root and the `dmail` group. Device config puts the phone's user in it.

Settings belong to dmail, one number per file in `/var/lib/dmail/`:
`data-enabled` (default 1), `delivery-reports` (0) and `sim-slot` (1). Apps
change them with the setters; access to the bus is the permission.

## Device config

`/etc/dmail/device.toml`, installed from `devices/<codename>/rootfs/`:

```toml
[data]
ipa_netdev = "rmnet_ipa0"
dpm = { ep_type = "embedded", iface = 1, rx = 2, tx = 24 }
qmap = "v5"
dl_datagrams = 16
dl_datagram_size = 8192
mtu_max = 1492
```

`modem-setup` and the rmtfs and tqftpserv units live in the same device tree.
They point at the phone's EFS copy, installed from `device-vault/<codename>/`;
the copy never enters this repo.

The device tree also enables systemd-resolved and links `/etc/resolv.conf` to
its stub. On aston resolved has been masked since the image was built on
28 May; nothing records why. To verify: NetworkManager then hands Wi-Fi DNS to
resolved by itself.

## Denial and apps

- Denial's shell gets a dmail backend next to its ModemManager one, picked by
  which bus name exists, the way `NetworkService` picks NetworkManager or iwd.
  `MobileNetworkSnapshot` gains the network type; the ModemManager backend
  fills it from `AccessTechnologies`. The data tile calls `Data1.SetEnabled`,
  the SIM PIN panel calls `Modem1.UnlockPin`.
- The Messages app replaces the samples in `src/data.rs` with
  `org.dmail.Messages1`.
- New-SMS notifications come from Messages, as on Android, where the default
  SMS app posts them. Messages ships a small background service: a second
  binary in its crate, zbus only, no GPUI, started by a systemd user unit at
  login. It sleeps on D-Bus, wakes on `MessageReceived`, and posts the
  notification through `org.freedesktop.Notifications`. Tapping it opens
  Messages. dmail itself never notifies.
- Dialer: after the MVP.

## Open

1. Why the network answers VoLTE-preferred-without-IMS with cause 7 is not
   known; boot step 4 avoids it. Should a cause 7 still come, the modem stops
   trying packet service until the radio goes off and on. Retrying goes against
   3GPP, so dmail reports it on Data1 and does not cycle the radio by itself.
   Checked live on 2026-10-21: SMS routes are "transfer only", so dmail acks.
   NSA reporting still to check on roadstr.
2. Dual SIM with two SIMs in: taking a SIM the modem runs as subscription 2
   for the primary session is untested; only one SIM is at hand. Choosing a
   slot per message, as Android's picker does, would need clients bound to
   subscription 2 and a data subscription switch; not started.
