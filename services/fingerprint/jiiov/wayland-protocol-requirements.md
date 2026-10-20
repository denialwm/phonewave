# Wayland fingerprint presentation protocol

We need a private, versioned Wayland extension that lets an authorized
fingerprint client request a compositor-managed presentation session for a
specific sensor. This is a display mode with defined wake and visibility
behavior, not just a request to draw an image.

When the screen is off, the compositor must prepare a black scene containing
only the fingerprint image at the sensor's calibrated physical location, then
enable the display without exposing the normal UI or flashing previous content.
The device remains locked. Authentication and unlocking are separate decisions.
When the screen is already on, keep its UI visible and overlay only the
fingerprint image. Use panel-local fingerprint illumination in both cases;
do not raise global screen brightness to maximum.

The protocol must cover:

- **Presentation role and placement:** associate a DMA-BUF image with a sensor;
  validate its rectangle in native panel coordinates independently of desktop
  rotation and scaling.
- **Display policy:** support screen-off, locked-screen and awake-screen
  presentation, with the compositor controlling visible content and panel state.
- **Readiness:** acknowledge the specific presentation request only after the
  required image and panel state are active and hardware timing requirements
  are satisfied. Acquisition must wait for this acknowledgment.
- **Lifecycle:** request, activate, fail, revoke and release a presentation
  session. Cancellation, timeout or client disconnect must clean up safely.
- **Wake transitions:** restore screen-off after a session when appropriate,
  while preserving a subsequent user wake or another legitimate display change.
- **Access control:** restrict presentation requests to authorized clients;
  the compositor enforces visibility and placement even if the UI client fails.

Conceptual flow: request session → configure geometry → submit DMA-BUF image →
establish display state → acknowledge readiness → acquire → release.

The system fingerprint backend needs a bridge to the Wayland client; this
extension does not replace fprintd or carry captured fingerprint data. Device
profiles provide geometry and hardware requirements so the same protocol can
support different sensors and phones.

The current [fprintd device API](https://fprint.freedesktop.org/fprintd-dev/Device.html)
does not expose under-display placement or screen coordinates; `scan-type=press`
does not distinguish an under-display reader. Add backend/device-profile
metadata for sensor modality, display identity, native-panel geometry and
illumination requirements, with a system interface that the presentation bridge
can query (for example, optional libfprint properties exposed through fprintd).
The compositor maps that geometry to its output; readiness and screen-off
transitions remain part of the Wayland session protocol.

The implementation is now in Denial's
`compositor/protocol/denial-fingerprint-v1.xml` and
`compositor/src/bin/deniald/fingerprint_presentation.rs`. Physical capture and
trusted unlock succeeded. The user requested that Flutter own all rendering
and transitions after the native-painted version caused display corruption.
The DMA-BUF is now an external texture in Flutter's secure stage, with a
layout acknowledgement and subsequent scanout gate, a short black idle hold
for retries, and an authenticated fade to home. See
`investigation/denial-fingerprint2/` for the replacement validation.
