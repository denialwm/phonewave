# phonewave-gatekeeper

The lock PIN behind root, for phones whose hardware has no PIN provider.

Denial keeps the lock PIN with a PIN provider wherever one listens: its
record in the person's home holds an opaque handle and the PIN's secure ID,
never the PIN or its hash. A Goodix phone's provider is its TEE's Gatekeeper
at `/run/gatekeeper/worker.sock` (`services/fingerprint`); its fingerprint
reader also wants the PIN proven before it adds a finger. This worker speaks
the same protocol at `/run/gatekeeper/software.sock`, for every phone, and a
hardware provider wins where both exist. A root program, such as Droidloom's
supervisor checking a PIN for Android, can then trust a PIN check even though
the record is the person's own file.

## How it works

`phonewave-gatekeeper.socket` hands each connection to one
`phonewave-gatekeeper-worker` running as root. The phone's user reaches the
socket through the `gatekeeper` group. Each connection carries one request in
the providers' `NXGK1` framing:

| Op | Takes | Gives |
|---|---|---|
| 1, enroll | the new PIN | a handle and a new secure ID, only while the phone has no PIN |
| 2, change | the handle, the current PIN, the new PIN | a new handle under the same secure ID |
| 3, verify | the handle, the PIN, a challenge | an Android password auth token for the challenge |
| 4, reset | the caller's account password, the new PIN | a new handle under the same secure ID, or a new one when the phone has no PIN |

The PIN's Argon2id hash, the failure count and the key that signs handles and
tokens live in `/var/lib/phonewave-gatekeeper`, root's alone. A handle stops
working once the PIN changes. Wrong PINs wait as Android's software
Gatekeeper makes them wait: 30 s after every fifth up to ten, after each one
up to thirty, then doubling every ten, up to a day. The wait is kept on disk.

A phone has one PIN: an enrollment that does not prove the current PIN is
refused while one exists. The account password replaces it, as it sets a
forgotten PIN on the lock screen's other phones: for a reset the worker
checks the password of the account on the other end of the socket through
PAM's `login` stack, so PAM's lockout counts wrong passwords, and a reset
also ends the wait for wrong PINs.

## Install

```sh
make check
make
sudo make install
```

Every phone turns the socket on (`50-phonewave.preset`).
