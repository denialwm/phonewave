# Features

What works on each device, and when someone last tried it. Read this before
bringing anything up. When a test passes or something breaks, change the row
in the same commit.

Status:

- **works**: tried on the device and it does its job.
- **partial**: works, with the gap named in Notes.
- **broken**: worked before, or should, and fails now.
- **untested**: nobody has tried it yet.
- **not started**: work is needed before it can be tried.
- **none**: the device doesn't have it.
- **?**: not recorded yet.

Tested: the date, the kernel build and who tried it: *user* (in hand) or
*probe* (checked from a shell on the device).

Wake from screen off means the power key brings the screen back; gestures
that also wake it are in Notes. Video decode means the hardware decoder.

Every table has the same rows in the same order; rows only one device has come
last.

## nx809j: Nubia RedMagic 11 Pro (SM8850)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | 2026-10-28, devices-141 | `boot_b` and `init_boot_b`; Arch beside Android |
| Display | works | 2026-10-27, devices-125, user | RM692H5, 60 to 144 Hz (devices-128). Two boots came up with a corrupted screen (devices-130, 2026-10-28); cause unknown |
| Brightness | works | 2026-10-28, devices-141, user | |
| Wake from screen off | works | 2026-10-28, devices-141, user | Power key; also double tap and the fingerprint area. The panel stays in standby and wakes in 12 to 23 ms |
| GPU | works | 2026-10-27, devices-126, user | Adreno 840: freedreno GLES 3.2, turnip |
| Touch | works | 2026-10-27, devices-127, user | Synaptics S3910 |
| Keys | works | 2026-10-28, devices-141, user | |
| Haptics | works | devices-130, user | AW86938, hapticd |
| Storage | works | | UFS |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | ? | | |
| Charging | works | devices-130, probe | The firmware charges; USB PD 9 V at about 3 A |
| Battery | works | devices-130, probe | battmgr |
| Wi-Fi | works | devices-128, user | WCN7860, ath12k |
| Bluetooth | works | devices-130, probe | RX and TX checked with btmon |
| Speakers | works | 2026-10-28, devices-138 | Tested at near-zero volume |
| Microphones | broken | 2026-10-28, devices-139, probe | Capture starts but no data arrives; aston too. Suspect 7c8cc30d7fc4 |
| Headphone jack | not started | | Reixo0's patches 0068 and on |
| Mobile data | works | devices-137, probe | dmail, IPA |
| SMS | works | 2026-10-28, user | dmail |
| Calls | untested | | dmail Voice1, as on aston |
| Call audio | not started | | |
| VoLTE | not started | | |
| GNSS | ? | | |
| NFC | ? | | No NFC device on perpetua |
| Camera | partial | 2026-10-28, devices-147, user | Raw frames through CAMSS at 30 fps; colour orders checked on a chart. Camera app (untuned): ultrawide OV50D40 4096x3072 (user), front OV16A1Q 2304x1728 binned (Quad Bayer). Auxiliary OV02F10 1600x1200 streams, not in the app. Main OV50E40 needs C-PHY |
| Video decode | not started | | No Iris for Kaanapali |
| Rotation | works | 2026-10-28, user | steinerd |
| Light and proximity | partial | probe | steinerd reads them; Denial doesn't use them |
| Compass and gyroscope | works | 2026-10-28, devices-141, user | steinerd; Sensors app |
| Fingerprint | works | 2026-10-28, devices-141, user | Goodix GUM5228, about 220 ms from touch to result. After an early lift, tries fail for about 5 s |
| Suspend | untested | | |
| Cooling | partial | devices-137, probe | Fan and pump follow the CPU trip points; the policy needs tuning |
| Denial | works | 2026-10-28, user | |
| Native apps | works | user | Settings, Sensors |
| Android apps | ? | | |
| Shoulder triggers | ? | | |
| RGB light | ? | | |

## aston: OnePlus Ace 3 (SM8550)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | devices-73 | `boot_b` |
| Display | works | user | |
| Brightness | works | 2026-10-28, devices-73, user | |
| Wake from screen off | works | user | Power key; also double tap |
| GPU | works | user | Adreno 740 |
| Touch | works | user | Synaptics TouchComm |
| Keys | works | 2026-10-28, devices-73, user | |
| Haptics | works | 2026-10-28, devices-73, user | hapticd |
| Storage | works | | UFS |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | none | 2026-10-21, user | |
| Charging | works | devices-61, probe | sc8517 in the kernel, SuperVOOC |
| Battery | works | 2026-10-24, probe | bq28z610 |
| Wi-Fi | works | devices-73, probe | WCN7850, ath12k; power save only while every screen is dark |
| Bluetooth | works | 2026-10-28, devices-73, user | |
| Speakers | works | devices-42, user | 2x TFA9873, left and right checked |
| Microphones | broken | devices-73, probe | Worked on 2026-10-22; capture stalls now, as on nx809j |
| Headphone jack | none | | |
| Mobile data | works | 2026-10-25, probe | dmail, carrier DNS through resolved |
| SMS | works | 2026-10-21, user | dmail |
| Calls | works | 2026-10-21, user | Outgoing and incoming, falls back to 3G |
| Call audio | works | 2026-10-21, devices-51, user | Not tried since the microphone regression |
| VoLTE | not started | | |
| GNSS | ? | | |
| NFC | ? | | |
| Camera | works | 2026-10-22, user | IMX890 main, IMX355 ultrawide, S5K3P9 front; Camera app. No 50 MP mode |
| Video decode | works | 2026-10-28, devices-73, probe | Iris: H.264, HEVC and VP9 at 1080p decode bit-exact against software, 12 to 18 times real time. VP9 logs decode errors without losing frames. AV1 is listed, not tried |
| Rotation | not started | | steinerd needs the phone's stock sensor files |
| Light and proximity | not started | | |
| Compass and gyroscope | not started | | |
| Fingerprint | works | user | UFF driver |
| Suspend | not started | | |
| Cooling | ? | | |
| Denial | works | user | |
| Native apps | works | user | Calls (calls mocked), Camera, Messages, Settings |
| Android apps | works | user | Droidloom |

## roadstr: Motorola Edge 70 (Eliza)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | devices-14 | |
| Display | works | user | 120 Hz |
| Brightness | works | 2026-10-28, devices-14, user | |
| Wake from screen off | works | user | Power key |
| GPU | works | user | After a GPU hang, only a hard reboot recovers |
| Touch | works | user | |
| Keys | works | 2026-10-28, devices-14, user | |
| Haptics | works | 2026-10-28, devices-14, user | hapticd |
| Storage | works | | |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | none | | |
| Charging | works | devices-12, probe | Back-temperature throttling |
| Battery | works | 2026-10-19, probe | |
| Wi-Fi | partial | 2026-10-19, probe | Receives no broadcast or multicast |
| Bluetooth | works | 2026-10-28, devices-14, user | |
| Speakers | ? | | |
| Microphones | ? | | |
| Headphone jack | ? | | |
| Mobile data | not started | | No dmail yet |
| SMS | not started | | |
| Calls | not started | | |
| Call audio | not started | | |
| VoLTE | not started | | |
| GNSS | ? | | |
| NFC | ? | | |
| Camera | not started | | No `/dev/video*` on perpetua |
| Video decode | not started | | No `/dev/video*` on perpetua |
| Rotation | ? | | |
| Light and proximity | ? | | |
| Compass and gyroscope | ? | | |
| Fingerprint | not started | | JIIOV JV0307. Worked on the 6.6 vendor kernel; what to port and where: `services/fingerprint/jiiov/README.md` |
| Suspend | ? | | |
| Cooling | works | devices-12, probe | battmgr cooling device, checked with emulated temperatures |
| Denial | works | user | |
| Native apps | works | 2026-10-28, devices-14, user | |
| Android apps | works | user | Droidloom with its power HAL |

## iceland: OnePlus Pad 4 (SM8850)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | devices-14 | |
| Display | works | user | |
| Brightness | works | 2026-10-28, devices-14, user | |
| Wake from screen off | works | user | Power key |
| GPU | works | user | Adreno 840 |
| Touch | works | user | |
| Keys | works | 2026-10-28, devices-14, user | |
| Haptics | none | | |
| Storage | works | | |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | works | 2026-10-15, user | DisplayPort |
| Charging | partial | 2026-10-28, devices-14, user | Slow |
| Battery | works | 2026-10-28, devices-14, user | |
| Wi-Fi | works | user | ath12k |
| Bluetooth | works | 2026-10-28, devices-14, user | |
| Speakers | works | 2026-10-28, devices-14, user | |
| Microphones | works | 2026-09-30, probe | |
| Headphone jack | ? | | |
| Mobile data | none | | Wi-Fi model |
| SMS | none | | |
| Calls | none | | |
| Call audio | none | | |
| VoLTE | none | | |
| GNSS | ? | | |
| NFC | none | | |
| Camera | ? | | |
| Video decode | not started | | No Iris for Kaanapali; the stock vendor firmware has APV |
| Rotation | not started | | steinerd needs the tablet's stock sensor files |
| Light and proximity | not started | | |
| Compass and gyroscope | not started | | |
| Fingerprint | none | | |
| Suspend | ? | | |
| Cooling | ? | | GPU thermal service installed |
| Denial | works | user | |
| Native apps | works | 2026-10-28, devices-14, user | |
| Android apps | works | user | Droidloom |
| Pen | ? | | |
| Keyboard cover | ? | | |

## tb373fu: Lenovo Idea Tab Pro (MT6897)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | devices-122 | `boot_b` |
| Display | works | devices-43, user | Own mediatek-dpu, 2944x1840 at 144 Hz |
| Brightness | works | devices-14, probe | 2x SGM37604A |
| Wake from screen off | works | devices-48, user | Power key |
| GPU | works | devices-43, user | Panthor; panvk needs `PAN_I_WANT_A_BROKEN_VULKAN_DRIVER=1` |
| Touch | works | devices-43 | Fingers |
| Keys | works | devices-47 | Power and volume |
| Haptics | none | | |
| Storage | works | | UFS |
| USB device | works | probe | High speed only |
| USB host | not started | | |
| Video out (USB-C) | none | | |
| Charging | partial | probe | Charges at 500 mA as the bootloader left it; no driver, no PD |
| Battery | works | probe | mm8013 |
| Wi-Fi | partial | devices-122 | Connects to WPA2 at about 12 Mbit/s |
| Bluetooth | not started | | |
| Speakers | not started | | No sound card on perpetua |
| Microphones | not started | | |
| Headphone jack | ? | | |
| Mobile data | none | | Wi-Fi model |
| SMS | none | | |
| Calls | none | | |
| Call audio | none | | |
| VoLTE | none | | |
| GNSS | ? | | |
| NFC | ? | | |
| Camera | not started | | |
| Video decode | not started | | |
| Rotation | not started | | |
| Light and proximity | not started | | |
| Compass and gyroscope | ? | | |
| Fingerprint | not started | | |
| Suspend | not started | | |
| Cooling | not started | | |
| Denial | works | user | |
| Native apps | works | | Settings, through panvk |
| Android apps | ? | | |
| Pen | partial | devices-43 | Registered; coordinates not transformed |
| CPU frequency | works | devices-47 | mediatek-cpufreq-hw |
| Cover sensor | not started | | |

## lavender: Xiaomi Redmi Note 7 (SDM660)

Booted (release 16). Unsupported for the time being, so it has no feature
table.
