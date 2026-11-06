# OnePlus Ace 3 (aston)

The phone runs Arch Linux on linux-perpetua from slot B, from the image every
phone shares (`image/`). Slot A keeps stock Android and is never written.
boot_b holds the kernel with its device tree built in, and after it the tail:
modules, the GPU, Wi-Fi and Bluetooth firmware, the voice profile and boot/tail/.
init_boot_b holds the init_boot every phone shares (`image/init-boot`). The ADSP, CDSP, modem and IPA firmware come from the phone's
own `modem_b` (`image/stock-firmware`); the GPU zap shader and the video
firmware live in vendor, inside super, so the tail carries them. `ssh aston`
reaches the phone as root over USB at 10.77.71.2.

The phone moved to the image on 2026-11-05. What the old root held and where
each piece goes is in REMINDERS.md; rootfs/, packaging/ and the old tools stay
until each piece has its new place.

After installing files on the phone, run `sync` before anything that can
hang it: a forced reboot keeps the new names and loses their data.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/aston`:
boot/ the built images, the initramfs busybox and dropbear, and the old boot
images; firmware/tail/ the firmware the tail carries, laid out as its
firmware/; backups/ partition copies, among them the stock slot-B partitions
(aston-survey-20261018), the
modem EFS and the old root (aston-before-image-20261105); firmware/ also the
factory fingerprint firmware; private-rootfs/ root's authorized_keys, which
init_boot also carries, and sudoers; reference/ device trees; build-deps/ the
fingerprint worker and libfprint source.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds it
through linux-perpetua-build unless the release exists, and writes the vault's
boot/boot-NAME.img on the devices224 boot image as header template.
`boot/tail/stock-firmware.conf` and `.sha256` name the stock firmware the
kernel was tested with.

Installing a kernel: write boot-NAME.img to /dev/disk/by-partlabel/boot_b,
compare its sha256 and reboot. The userdata image goes in with
`image/install-userdata aston` from a running phone, or from rescue.

Charging: since devices59 the kernel's `sc8517-charger` owns the SuperVOOC
PHY, the PM8550B USB switches and negotiation. It loads from the device tree;
no charging service or live-node/autostart module is needed. Since devices61 it
requests up to 5.2 A with the tested adapter (model 0x4b), staying below a 50 W
ceiling throughout the permitted input-voltage range. Other models retain a
3 A ceiling. The higher tier requires 20 to 35 degrees C, less than 75% charge
and less than 8.648 V pack voltage. It derates to 3 A at 35 degrees C and 2 A
at 40 degrees C, and keeps the lower limit for the rest of the cable session.
The battery-current cutoff is 6 A, below the vendor's 9.5 A limit.

Above 80% it still requests up to 2 A (roughly 17 to 18 W), tapering to 1.2 A
(roughly 10 to 11 W) at 85%. Voltage and temperature can taper it earlier.
At 90% the PMIC buck path takes over and ordinary charging continues. Battery
power is lower than these adapter-side estimates because of losses and phone
consumption. Fast charging also stops below 12 degrees C, at 43 degrees C, or
at a 9.088 V pack voltage. Hardware V1X overvoltage protection stays at the
vendor's 4.725 V threshold; V2X protection is enabled at 9.5 V. Both are checked
before connecting the battery. A communication timeout, gauge failure or hardware
fault opens the direct path and restores the PMIC buck input. The driver holds
a wakeup source while negotiating and charging, and stops before shutdown.

`/sys/class/power_supply/sc8517-charger` reports `online=1` and
`charge_type=Fast` after negotiation. Check the BQ28Z610 supply's voltage,
current and temperature for the actual battery state: the PMIC's battery
endpoint is a placeholder, and its USB current measures the buck path. The
OnePlus SM8550 GLINK dialect reads the cable classification from USB_ADAP_TYPE;
USB_TYPE describes the firmware's supply class. The advertised current alone
does not identify an adapter. The kernel requires SuperVOOC identification
before enabling the direct path and leaves USB data ports on the PMIC path.
After manually unbinding or reloading the PHY driver with the charger attached,
reconnect the cable to let the PMIC classify the charger again.

Modem: dmail (services/modem/dmail) owns it, as on every phone: its
`modem-setup` starts rmtfs on the phone's own EFS partitions, read-only, and
tqftpserv (services/modem/linux-msm). The tail turns dmail on and carries its
device file, `boot/tail/dmail/device.toml`.

DNS: systemd-resolved, so that dmail can give it the carrier's servers for
`rmnet_data0`; without it, cellular has no DNS, since NetworkManager's
resolv.conf holds Wi-Fi's servers only. The image links `/etc/resolv.conf`
to resolved's stub and turns LLMNR and mDNS off.

Lock screen: wallpaper-depth (services/wallpaper/wallpaper-depth) is installed
with float16 weights, cross-built as its README says; Denial runs it when
the wallpaper picker closes. Removing it turns the cut off.

Speakers: since devices41 two TFA9873 amplifiers (top 0x34, bottom 0x35 on
I2C6) play Secondary MI2S, left and right; the kernel's tfa987x driver sets
them up without speaker protection. The UCM profile is `sm8550/astonsnd` in
services/audio/ucm (HiFi verb, Speaker device on MultiMedia1); the image's
WirePlumber rule gives PipeWire software volume because the DSP volume
control's dB metadata is invalid, as on the Pad 4.

Microphones: the WCD9380 takes AMIC1 on ADC1 (TX SMIC SWR_MIC0) and AMIC5
on ADC4, input INP5 (SWR_MIC1), the handset pair of the stock
mixer_paths.xml. ADC1/2 and ADC3/4 share one SoundWire master port, so one
mic records at a time: UCM devices Mic1 and Mic2 conflict, and PipeWire
offers them as the profiles "HiFi (Mic1, Speaker)" (default) and
"HiFi (Mic2, Speaker)". The capture PCM is stereo only; both decimators take
the chosen mic. AMIC1 is much less sensitive than AMIC5: Mic1 uses the
stock recorder gain (oprec-mic, ADC 14 = +21 dB), Mic2 the handset gain
(ADC 6 = +9 dB); both decimators stay at 84 (0 dB).

Calls: the ADSP carries call speech between the modem and the hardware once
the kernel's q6apm voice session (devices51 on) has started its two graphs.
They come from `/lib/firmware/qcom/sm8550/aston-voice.bin`, which
services/audio/voicegen builds on the host from the stock ACDB as
`audio/voice.toml` describes; the vault keeps it, with its provenance, under
`firmware/usr/lib/firmware/qcom/sm8550/`. The UCM verb "Voice Call" puts
Speaker and Mic on the policy PCM 13; its Mic device routes AMIC5 to
decimator 0, as the stock handset path, and asks the kernel to start the
graphs. callaudio (services/audio/callaudio, a user unit enabled from
rootfs/) switches the card to that verb while dmail has a call. Speech plays
on both amplifiers; the earpiece alone needs per-amplifier switches in
tfa987x. "Voice Call Volume" takes the stock HAL's six levels (5 loudest,
the default); "TX_AIF1 Capture Switch" mutes the microphone.

Packages: on top of the image, `pacman -S` installs `noto-fonts-emoji`, the
colour emoji font the native apps draw emoji with, `noto-fonts-cjk` for
Chinese, and `bluez bluez-utils`, with `systemctl enable bluetooth`.
`/etc/locale.gen` enables `en_US.UTF-8` and `zh_CN.UTF-8`, then
`locale-gen`: the languages Settings offers. rootfs/ carries
`48-phone-settings.rules`, which lets Settings set the clock, its time zone
and the language without a password.

Screen recording: screen-record (services/screen/screen-record), installed
in /usr/bin, gives the shade its Screen recording tile; it uses the image's
`ffmpeg`. Videos go to `~/Videos/Screen recordings`.

Battery stats: battery-stats (services/power/battery-stats), installed with
its `make install`, books the battery per state, hour and build; enable
`battery-statsd.service`. It reads the BQ28Z610, which sorts before the PMIC's
placeholder battery. No `/etc/battery-stats/tags`: no link names the running
Denial set, so the kernel release alone names the build.

Camera: braun (services/camera/braun), installed with its `make install`,
finds the IMX890 main camera, the IMX355 ultrawide and the S5K3P9 front
camera, and develops the Camera app's photos. rootfs/ carries
`/etc/braun/imx890.toml` and `s5k3p9.toml`: the IMX890's role, its rotation
(270, where the device tree says 90) and focus travel, and for both the
stock tuning's illuminant locus, colour matrices and lens shading.
`imx355.toml` names the IMX355 the ultrawide, at the main camera's 0.6x
(112 against 84 degrees across the diagonal); it has no colour tuning yet.

Always-on display: the AA551 slows a still frame down to 1 Hz in normal
mode, so from devices77 the driver's always-on blank keeps the panel lit at
that refresh floor instead of turning the display off: the lock screen's
always-on picture stays exactly as Denial drew it, at the same backlight
code, while the display hardware is off. The tail's
`denial/ambient-display.json`, which the image copies to `/etc/denial`,
names the switch and that code (`brightness`); Denial sets the switch for
each always-on blank, lights the output once a minute for the clock, and
clears it on wake. The image lets the video group write it.

Radio addresses: the ABL passes no Wi-Fi or Bluetooth address, so the
WCN7850 reports its firmware's shared default MAC and its Bluetooth
controller stays unconfigured. radio-addr (services/radio/radio-addr),
in every image, derives both from the SoC serial.

Memory: zram-swap (services/memory/zram-swap) gives 7.3 GB of zstd swap, half
of memory; the image keeps GPU buffers out of swap, as on the Pad 4.
Since devices40 the kernel backs GPU buffers with huge pages
(`msm.transparent_hugepage`, on by default); `dmesg` says "Using Transparent
Hugepage".

Standby: devices62 disabled the unused QCE crypto accelerator on the board;
since devices63 the SM8550 description supplies that default for every board,
as do the shared Kaanapali and Eliza descriptions. QCE's driver is marked
BROKEN; leaving its device-tree node enabled kept two
interconnect providers waiting for a driver and retained the maximum boot
bandwidth floor, pinning DDR at 4224 MHz even with the display and GPU off.
Disabling it lets all 15 providers synchronize and memory frequency follow
actual requests. The separate UFS inline crypto engine stays enabled.

USB standby: [the shared USB notes](../usb-standby.md) describe the controller
policy and the missing firmware cable events. The investigation's Aston udev
rule was removed before devices63 validation and is not installed in rootfs/.
