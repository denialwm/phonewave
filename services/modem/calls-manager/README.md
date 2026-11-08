# calls-manager

Handles the modem's calls for whichever calls app the user set, as Android's
telephony does for the dialer. It follows dmail's `org.dmail.Voice1.Calls`:

- While a call rings it asks
  [hapticd](../../haptics/hapticd) for Android's default ring pattern, 1 s
  on and 1 s off, until the call is answered, turned down or missed. A call
  waiting behind another does not vibrate.
- It posts the call as a notification of the calls app set in Denial
  (`lockScreen.callsApp`, by default denial-apps' Calls): a banner with
  Decline and Answer while it rings, then a quiet one with Hang up. The
  caller's name comes from the user's contacts (vCard files in
  `$XDG_DATA_HOME/contacts`). Answer opens the calls app.
- While the phone is locked, a ringing call starts the calls app. Denial
  shows that app's window over the lock screen while its call notification
  stands.

The calls app itself talks to dmail for everything else: dialling, the call
screen, the call log. Call audio is [callaudio](../../audio/callaudio)'s.

## Build and install

Requires Rust 1.88+.

```sh
make
make check
sudo make install
systemctl --user enable --now calls-manager
```

The phones get it from the `calls-manager` recipe
(`image/recipes/calls-manager`), which every image installs and turns on. It
runs in the user's graphical session; the user needs access to dmail (group
`dmail`).

## License

MIT
