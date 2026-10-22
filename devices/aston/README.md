# OnePlus Ace 3 (aston)

The phone runs Arch Linux on the shared devices kernel from slot B. rootfs/ holds
the files installed for this device, at their install path and byte for byte as
on the phone. boot/ packages a kernel release; tools/ runs on the host, reaches
the phone as root through `ssh aston` and writes to ~/.cache/phonewave/aston.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/aston`:
boot/ the installed and rollback boot images with the evidence each package
chains on, and the init_boot image; backups/ partition copies, among them the
stock slot-B partitions (aston-survey-20261018) and the modem EFS; firmware/
unpackaged and factory fingerprint firmware; private-rootfs/ the SSH key and
sudoers; reference/ device trees; build-deps/ the deployed fingerprint worker
and libfprint source, which no longer rebuild.

Packaging a kernel release: build linux-perpetua through linux-perpetua-build,
which leaves releases/devices-N-aston, then run `boot/package_kernel.py
--generation N --previous P`, where P is the release the phone runs. It checks
the vault's boot_b-devicesP.img against kernel-flash-evidence-devicesP.json,
uses it as the android-gki template and writes boot_b-devicesN.img,
boot-evidence-devicesN/ and boot-package-devicesN.json to the vault. Offline.

Installing it: `tools/stage_modules.py --generation N` installs the module
directory without loading anything. `tools/activate_kernel.py --generation N
--previous-generation P` copies the image over, checks init_boot_b against
boot-preflight.json, the current boot_b and the modules, then writes boot_b with
the phone's /var/lib/aston-release/remote_boot_flash.py (equal to tools/), which
backs up, reads back and records kernel-flash-evidence-devicesN.json.
`tools/reboot_candidate.py --generation N` fetches the backup, rechecks both
partitions and reboots. Only boot_b is written; the previous image is the
rollback. init_boot_b stays the devices10 image: boot/firstboot's static PID 1
as a root-owned newc cpio, lz4 -l, on the stock init_boot_b.

Fingerprint: build as services/fingerprint/uff/README.md says, then run
tools/fingerprint/run_probe.py (identify the sensor), stage_service.py (worker,
helper, presentation client, backend.json), build_libfprint.py (builds the
snapshot on the phone in /opt/libfprint-uff) and install_fprintd.py (selects it,
restarts fprintd). stage_compositor.py stages the deniald named by the cached
compositor-build.json; activate_compositor.py selects it through
denial.service.d/zz-fingerprint.conf and restarts the graphical session.

Modem: dmail (services/modem/dmail) owns it. rootfs/ carries
`/usr/lib/dmail/modem-setup`, the `aston-modem-rmtfs` and
`aston-modem-tqftpserv` units it starts, `/etc/dmail/device.toml` and
`aston-dmail.conf`, which puts logix in the `dmail` group. The EFS copy never
enters this repo: on the phone `/var/lib/aston-modem/efs` links to the copy
directory, and `efs.handset` beside it holds the SoC serial, written on the
phone from `/sys/devices/soc0/serial_number` and never printed. Install dmail
with its `make install` layout, then run `systemd-sysusers`, reload systemd
and D-Bus and enable `dmail.service`. The unit names match the cellular kit's
transient units, so `modem-setup` never starts a second rmtfs next to them.

Lock screen: wallpaper-depth (services/wallpaper/wallpaper-depth) is installed
with float16 weights, cross-built as its README says; Denial runs it when
the wallpaper picker closes. Removing it turns the cut off.

Speakers: since devices41 two TFA9873 amplifiers (top 0x34, bottom 0x35 on
I2C6) play Secondary MI2S, left and right; the kernel's tfa987x driver sets
them up without speaker protection. rootfs/ carries the UCM profile
(`conf.d/sm8550/astonsnd.conf`, HiFi verb, Speaker device on MultiMedia1)
and `51-aston-audio.conf`, which gives PipeWire software volume because the
DSP volume control's dB metadata is invalid, as on the Pad 4.

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
colour emoji font the native apps draw emoji with, and `bluez bluez-utils`,
with `systemctl enable bluetooth`.

Radio addresses: the ABL passes no Wi-Fi or Bluetooth address, so the
WCN7850 reports its firmware's shared default MAC and its Bluetooth
controller stays unconfigured. radio-addr (services/radio/radio-addr),
installed with its `make install`, derives both from the SoC serial.
rootfs/ carries `90-radio-addr-wifi.rules`, which sets wlan0's MAC before
NetworkManager sees it, and the `bluetooth.target.wants` link that runs
`radio-addr-bluetooth.service`.

Memory: zram-swap (services/memory/zram-swap) gives 7.3 GB of zstd swap, half
of memory; rootfs/ enables it with a swap.target.wants link and keeps GPU
buffers out of swap with `aston-msm-gpu-eviction.conf`, as on the Pad 4.
Since devices40 the kernel backs GPU buffers with huge pages
(`msm.transparent_hugepage`, on by default); `dmesg` says "Using Transparent
Hugepage".
