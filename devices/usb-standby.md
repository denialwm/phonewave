# USB standby

USB runtime power management belongs to the controller and connector drivers.
Do not install a rule that sets every USB controller or PHY to `power/control=auto`.
A sleeping peripheral needs a working cable event to resume before enumeration.

## Shared kernel behavior

Implemented by linux-perpetua commit `4daa98a10ed7` and installed on Aston
as `7.3.0-devices63-aston`.

The flattened Qualcomm DWC3 driver (`dwc3-qcom`) handles role-controlled ports
without board names, physical addresses or power-supply names. A device role
holds a runtime-PM reference, including while a connected host is enumerating.
A `none` role clears forced VBUS and releases that reference; the normal
five-second autosuspend then removes the controller's clock and interconnect
requests. The next device-role event resumes it and restores forced VBUS.
Host mode continues to use the host controller's runtime PM and PHY wake IRQs.
A gadget restart preserves the connector role rather than asserting VBUS while
the port is detached. The initial default device role stays awake until the
connector supplies its first event, so missing firmware events do not silently
break an otherwise working USB data connection.

This changes the shared flattened driver, including all three current device
SoCs. It does not change `dwc3-qcom-legacy` or enable autosuspend on a fixed-role
port that has no reliable cable-detection path. An administrator can still
keep a controller awake with `power/control=on`.

## Connector hardware and remaining gaps

| Device | Current cable-event path | Consequence |
|---|---|---|
| Aston / SM8550 | The old DT declared SuperSpeed/DP and an FSA4480 SBU switch that did not respond; UCSI port registration deferred until retries stopped. Stock Aston config uses a high-speed controller and disables the SuperSpeed PHY. | The devices64 candidate describes the USB2-only port and removes the false mux dependency. Native cable-event validation is pending. |
| Iceland / Kaanapali | The board DT deliberately uses a high-speed peripheral and removes `usb-role-switch`; its Type-C role path is not wired up. | Keep the existing runtime-PM default until the kernel connector path can deliver and act on both cable edges. |
| Roadstr / Eliza | The board has a role switch and only USB 2.0 lanes. | The shared role policy applies; physical unplug/replug and host mode still need validation on this device. |

Power-supply `online` is not a general replacement for a Type-C data role:
a port can supply power while acting as a USB device, or consume power while
acting as a host. Do not broadcast a single charger's state to every controller.
Describe the actual connector hardware and use its native role protocol.

## Aston diagnostic fallback

The standby investigation temporarily used
`/run/udev/rules.d/80-aston-usb-power.rules` on Aston. It is not installed in
rootfs/ or `/usr/lib/udev/rules.d` and was removed before devices63 validation. Earlier versions
also set `power/control` on cable edges; with the shared kernel policy, only
forwarding `none` on detach and `device` on attach is needed for the device-mode
test. A rule that changes runtime PM without clearing forced VBUS leaves the
UDC reporting `configured` after physical unplug and prevents suspension.

The hard-coded controller and supply paths in that rule are test equipment,
not a device-independent implementation. No USB power-saving claim should be
made for another device from that experiment. Do not use the fallback during
validation of a repaired native firmware/Type-C event path.

## Validation recorded on Aston

With the temporary rule absent, devices63 booted with a cable attached and
USB enumerated under the kernel's `auto` policy. Two explicit `none` / `device`
cycles suspended with all USB interconnect requests zero, resumed to a configured
gadget, and kept device mode awake beyond the autosuspend delay. Resuming and
restarting the gadget while its role was `none` kept it detached and allowed
another suspension. A 1 MiB read over the USB gadget network then matched its
expected SHA-256. The USB3 PHY retained its default `on` policy throughout.

Those tests validate the shared controller path; they supplied connector roles
explicitly and do not validate native physical cable events, host mode, or PD
role swaps. The later devices64 candidate corrects the DT dependency that
prevented UCSI port registration; the earlier retry message alone did not
establish a firmware-protocol failure. Suspend still logs the pre-existing `port-1 HS-PHY not in L2`
diagnostic. There were no kernel WARN/call traces in the capture. The other
devices require physical validation before claiming their power improvement.

## Validation procedure

Use a connection independent of USB (for example Wi-Fi SSH). Record the kernel,
boot ID, controller runtime state and policy, UDC state, cable state, and USB
interconnect requests. Confirm enumeration and an actual USB-network transfer,
then unplug for longer than the autosuspend delay: the controller must suspend
and its interconnect requests must fall to zero. Reconnect without sysfs writes
or a service restart: enumeration and a new USB-network transfer must work.
Repeat the cycle, and test the cable already attached at boot. Check the kernel
log for PM, PHY and gadget errors. Record whether cable events were native or
provided by the diagnostic bridge. Test host mode separately on a port that
supports it; gadget-mode success does not validate OTG or PD role swaps.

Measure battery power unplugged with the display off and unchanged services.
Exclude charging samples, use the real battery gauge, and allow its current
filter to settle. A short current change after a sysfs write is evidence for
further measurement, not a validated battery-life improvement.
