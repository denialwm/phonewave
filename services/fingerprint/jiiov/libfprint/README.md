# Experimental libfprint patches

Apply in filename order to upstream v1.94.100, commit
`80a4b5ec612892c5c056c48dddaa561452cf37ec`.

1. Fix the no-introspection test fallback.
2. Discover platform sensors by their bound driver and device-tree compatible.
3. Add the experimental JIIOV FpDevice driver and its build option.
4. Account for actual worker power state: touchscreen waiting leaves the
   sensor off and retains thermal checks for capture. This adds the private
   `JFP1 power 0/1` records and permits an indefinitely cancellable idle wait.

5. Report confirmed early-contact cancellation as a retry (`JFP1 result 3 0`),
   preserving reader-failure backoff for actual errors. This requires the
   matching helper; malformed records and unconfirmed cleanup still fail closed.

6. Preserve idle verification across suspend through a bounded worker and
   presentation handshake. The touch descriptors remain open, sensor power
   stays off, and input is consumed again after resume. The patch includes
   driver tests using the real libfprint suspend/resume API and a pipe worker.
   It requires the matched helper/presentation update and conditional resume
   hook recorded in `moto70edge/investigation/fingerprint-suspend1/`.

7. Create the worker control pipe explicitly and map its read end from a
   descriptor above stderr. This avoids GLib replacing stdin with `/dev/null`
   when fprintd has released descriptor 0. Add regression coverage for that
   state and repeated rejection, early-lift retry and match after suspend.
   See `moto70edge/investigation/fingerprint-suspend2/`.

The third patch requires `jiiov-fprint-helper.py`, the matching native probe,
and diagnostic illumination client in the installation paths documented in
`moto70edge/tools/fingerprint/install-libfprint-phone.py`. The driver uses the private JFP1 worker
interface. It is not a standalone upstream-ready patch: the current worker is
restricted to the validated Roadstr device. Denial verification uses its
privileged presentation protocol; enrollment retains the unlocked diagnostic UI.
See `moto70edge/investigation/fingerprint-native2/README.md` for live evidence,
known limitations and the exact installed build.
