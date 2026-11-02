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
| Wake from screen off | works | 2026-11-02, devices-222, user | Power key; also double tap and the fingerprint area. The panel stays in standby and wakes in 12 to 23 ms. Beside a monitor the phone sleeps to its always-on picture without locking |
| GPU | works | 2026-10-27, devices-126, user | Adreno 840: freedreno GLES 3.2, turnip |
| Touch | works | 2026-10-31, devices-168, user | Synaptics S3910, reporting at 240 Hz; its firmware starts at 120 Hz |
| Keys | works | 2026-10-28, devices-141, user | |
| Haptics | works | devices-130, user | AW86938, hapticd |
| Storage | works | | UFS |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | works | 2026-11-02, devices-222, user | DisplayPort; the phone's screen goes off and on while the monitor stays lit |
| Charging | works | devices-130, probe | The firmware charges; USB PD 9 V at about 3 A |
| Battery | works | devices-130, probe | battmgr |
| Wi-Fi | works | devices-128, user | WCN7860, ath12k |
| Bluetooth | works | devices-130, probe | RX and TX checked with btmon |
| Speakers | works | 2026-10-28, devices-138 | Tested at near-zero volume |
| Microphones | works | 2026-11-02, devices-223, probe | AMIC1 (default), AMIC4 and AMIC3 through PipeWire; each hears a tone from the speakers |
| Headphone jack | not started | | Reixo0's patches 0068 and on |
| Mobile data | works | devices-137, probe | dmail, IPA |
| SMS | works | 2026-10-28, user | dmail |
| Calls | untested | | dmail Voice1, as on aston |
| Call audio | not started | | |
| VoLTE | not started | | |
| GNSS | ? | | |
| NFC | ? | | No NFC device on perpetua |
| Camera | partial | 2026-10-28, devices-155, user | Raw frames through CAMSS at 30 fps; colour orders checked on a chart. braun finds all four cameras; the Camera app is its client, untuned. Main OV50E40 4096x3072 over three C-PHY trios, focus through its DW9785 (OIS off) (user); front OV16A1Q 2304x1728 binned (Quad Bayer) (user); ultrawide OV50D40 4096x3072, a lens chip in the app (user). Auxiliary OV02F10 1600x1200 streams, not in the app. Before devices-155 a switch from the ultrawide to the main camera gave no frame about half the time (user): the stopped CSIPHY stayed out of reset. Now 12 of 12 such switches open, and main to ultrawide 6 of 6 |
| Video decode | not started | | No Iris for Kaanapali |
| Rotation | works | 2026-10-28, user | steinerd |
| Light and proximity | partial | probe | steinerd reads them; Denial doesn't use them |
| Compass and gyroscope | works | 2026-10-28, devices-141, user | steinerd; Sensors app |
| Fingerprint | works | 2026-10-30, devices-160, user | Goodix GUM5228, about 220 ms from touch to result. After an early lift, tries fail for about 5 s. The armed sensor's base-update requests get a base, as on stock |
| Suspend | partial | 2026-10-29, devices-159 | s2idle, woken by the RTC; CX and DDR sleep, XO does not. Wi-Fi reassociates about 8 s after resume. The fingerprint driver refuses suspend while a verify holds the sensor (the test stopped it by hand); nothing suspends the phone on its own yet |
| Cooling | works | 2026-10-28, devices-153, probe | Fan from its lowest speed at 40 C to full at 80 C, a level every 2 C, from the prime core and GPU sensors; pump on from 70 C, off below 65 C |
| Denial | works | 2026-10-28, user | |
| Native apps | works | user | Settings, Sensors |
| Android apps | works | 2026-10-31, devices-168, user | Droidloom 3732e87; Android ready in 4 s, GLES on freedreno, Play Store; Firefox scrolls and moves its panel smoothly |
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
| Microphones | broken | devices-73, probe | Worked on 2026-10-22; capture stalls now. Cause found on nx809j and fixed in linux-perpetua 9ba37ef1db70; not built for aston yet |
| Headphone jack | none | | |
| Mobile data | works | 2026-10-25, probe | dmail, carrier DNS through resolved |
| SMS | works | 2026-10-21, user | dmail |
| Calls | works | 2026-10-21, user | Outgoing and incoming, falls back to 3G |
| Call audio | works | 2026-10-21, devices-51, user | Not tried since the microphone regression |
| VoLTE | not started | | |
| GNSS | ? | | |
| NFC | ? | | |
| Camera | partial | 2026-10-28, devices74 | IMX890 main, IMX355 ultrawide, S5K3P9 front; Camera app (user, 2026-10-22). No 50 MP mode. On braun: an IMX890 photo with `braunctl shot`; either camera reopens at once, and the S5K3P9 opens right after the IMX890, 6 out of 6, and the IMX890 right after the IMX355 or the S5K3P9, 12 out of 12. Before devices74 a reopen within a second of a close timed out in RPMh and stopped the phone: camss dropped its camera NoC votes while the CCI kept TITAN_TOP powered |
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
| Charging | works | 2026-10-31, devices-23, probe | SuperVOOC at 51–55 W on both SC8547D pumps (OnePlus 80 W adapter, 35 to 38 % in 2 min, battery connector 48 °C); 14.7 W from a 5 V wall charger; no PD 9 V yet |
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
| Suspend | broken | 2026-10-31, devices-14, probe | Each boot ends at `PM: suspend entry (deep)` and the tablet boots again |
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

## pudding: Xiaomi 17 (SM8850)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | 2026-11-01, devices-198 | `boot_b` and `init_boot_b`; Arch on its own partition beside Android on slot A |
| Display | works | 2026-11-01, devices-191, user | Xiaomi P3 OLED, our panel driver, 120 Hz, DSC |
| Brightness | works | 2026-11-02, devices-217, user | |
| Wake from screen off | works | 2026-11-01, devices-210, user | Power key; also double tap. The touch controller stays in gesture mode with the screen off |
| GPU | works | 2026-11-01, devices-181, user | Adreno 840: freedreno GLES 3.2, desktop GL 4.6 |
| Touch | works | 2026-11-01, devices-196, user | Synaptics S3910 with Xiaomi's THP firmware: the driver finds the fingers in raw frames; 265 Hz |
| Keys | works | 2026-11-01, devices-203, user | Power and volume down (PMIC), volume up (gpio-keys) |
| Haptics | works | 2026-11-01, devices-203, user | PMIH0108 HAP530, qcom-spmi-haptics, hapticd |
| Storage | works | | UFS |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | ? | | |
| Charging | partial | 2026-11-02, devices-217, probe | Charges from a USB-PD wall charger (PPS offered) at a flat 0.5 A into the battery; no fast charging |
| Battery | works | 2026-11-01, devices-207, user | bq27z561 fuel gauge (bq27xxx); Settings shows charge, capacity and cycles |
| Wi-Fi | works | 2026-11-01, devices-202, probe | WCN7860, ath12k; country from `/etc/conf.d/wireless-regdom` (IT: 2.4, 5 and 6 GHz). Joined a 5 GHz network on devices-198 (user) |
| Bluetooth | works | 2026-11-02, devices-220, probe | WCN7860 with Xiaomi's Brahma firmware; finds 15-17 devices nearby; this computer's adapter heard at -33 dBm and hears the phone at -24 dBm; radio-addr gives the address |
| Speakers | works | 2026-11-02, devices-217, user | Two CS35L43, top left and bottom right; stock speaker protection with the factory calibration; PipeWire |
| Microphones | not started | | |
| Headphone jack | ? | | |
| Mobile data | works | 2026-11-01, devices-203, probe | dmail at boot, IPA; Iliad 5G NSA (LTE B3 + NR leg during transfers), shown as 5G as on Android; 20 MB at 1.0 MB/s, stock Android 1.46 MB/s at the same spot |
| SMS | works | 2026-11-02, devices-217, user | dmail |
| Calls | ? | | |
| Call audio | ? | | |
| VoLTE | ? | | |
| GNSS | ? | | |
| NFC | ? | | |
| Camera | not started | | |
| Video decode | not started | | No Iris for Kaanapali |
| Rotation | works | 2026-11-02, devices-217, user | steinerd |
| Light and proximity | works | 2026-11-01, devices-211, user | Goodix glsx light, proximity; steinerd; Sensors app |
| Compass and gyroscope | works | 2026-11-01, devices-211, user | steinerd: lsm6dsv accelerometer and gyroscope, qmc6309h magnetometer; Sensors app |
| Fingerprint | ? | | |
| Suspend | ? | | |
| Cooling | ? | | |
| Denial | works | 2026-11-01, user | |
| Native apps | works | 2026-11-01, user | Settings joins Wi-Fi |
| Android apps | ? | | |
