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
| Boot | works | 2026-11-06, devices-231, probe | The shared image on `denial` (sda13), Android keeps `userdata`; shared init_boot, whose rescue fastboot writes this slot's `boot` and `init_boot` |
| Display | works | 2026-11-09, devices-314, user | RM692H5, 60 to 144 Hz (devices-128); Denial runs it at 144 Hz. Two boots came up with a corrupted screen (devices-130, 2026-10-28); cause unknown |
| Brightness | works | 2026-11-09, devices-310, user | |
| Wake from screen off | works | 2026-11-02, devices-222, user | Power key; also double tap and the fingerprint area. The panel stays in standby and wakes in 12 to 23 ms. Beside a monitor the phone sleeps to its always-on picture without locking |
| GPU | works | 2026-10-27, devices-126, user | Adreno 840: freedreno GLES 3.2, turnip |
| Touch | works | 2026-11-09, devices-310, user | Synaptics S3910, reporting at 240 Hz; its firmware starts at 120 Hz |
| Keys | works | 2026-11-09, devices-310, user | |
| Haptics | works | devices-130, user | AW86938, hapticd |
| Storage | works | | UFS |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | works | 2026-11-09, devices-310, user | DisplayPort; the phone's screen goes off and on while the monitor stays lit |
| Charging | works | devices-130, probe | The firmware charges; USB PD 9 V at about 3 A |
| Battery | works | devices-130, probe | battmgr |
| Wi-Fi | works | 2026-11-09, devices-310, user | WCN7860, ath12k |
| Bluetooth | works | 2026-11-09, devices-310, user | On the image radio-addr gives the controller its address |
| Speakers | works | 2026-11-09, devices-310, user | Through the shared UCM profile; the amplifiers run 6 dB under stock, which has speaker protection in the audio DSP. Droidloom plays at full scale (fixed volume) |
| Microphones | untested | 2026-11-06, devices-233, probe | On the image PipeWire shows the main microphone (AMIC1); not recorded yet |
| Headphone jack | not started | 2026-11-09, devices-310, probe | The codec reports the jack (Headphone Jack, Mic Jack, an input device); no playback or capture route in the UCM profile. Reixo0's patches 0068 and on |
| Mobile data | works | 2026-11-09, devices-310, probe | 5G on Iliad, APN iliad; pings over the data bearer alone |
| SMS | works | 2026-11-09, devices-310, user | dmail |
| Calls | works | 2026-11-08, devices-304, user | Incoming over VoLTE: rings, wakes the screen and shows Calls over the lock screen; answered from it |
| Call audio | works | 2026-11-08, devices-310, user | Both ways, on the speakers and the speaker microphone |
| VoLTE | works | 2026-11-09, devices-310, user | dmail registers IMS on Iliad; calls ring and connect over it |
| GNSS | not started | 2026-11-09, devices-310, probe | No GNSS device |
| NFC | ? | | No NFC device on perpetua |
| Camera | partial | 2026-10-28, devices-155, user | Raw frames through CAMSS at 30 fps; colour orders checked on a chart. braun finds all four cameras; the Camera app is its client, untuned. Main OV50E40 4096x3072 over three C-PHY trios, focus through its DW9785 (OIS off) (user); front OV16A1Q 2304x1728 binned (Quad Bayer) (user); ultrawide OV50D40 4096x3072, a lens chip in the app (user). Auxiliary OV02F10 1600x1200 streams, not in the app. Before devices-155 a switch from the ultrawide to the main camera gave no frame about half the time (user): the stopped CSIPHY stayed out of reset. Now 12 of 12 such switches open, and main to ultrawide 6 of 6 |
| Video decode | not started | | No Iris for Kaanapali |
| Rotation | works | 2026-11-09, devices-310, probe | steinerd reports the accelerometer and gyroscope live |
| Light and proximity | works | 2026-11-09, devices-310, probe | steinerd reports both live |
| Compass and gyroscope | works | 2026-11-09, devices-310, probe | steinerd reports both live |
| Fingerprint | works | 2026-11-09, devices-310, user | Goodix GUM5228 through fprintd and fingerprint-phonewave like the other phones, on the shared image: the lock PIN in the TEE's Gatekeeper and canonical secure storage, both from the package and turned on by the tail; Settings sets the PIN and adds a finger. Before: Goodix GUM5228, about 220 ms from touch to result, through fprintd and fingerprint-phonewave like the other phones; `/usr/lib/modules/<release>/fingerprint/sensor.conf` names the sensor. After an early lift, tries fail for about 5 s. The armed sensor's base-update requests get a base, as on stock |
| Suspend | partial | 2026-10-29, devices-159 | s2idle, woken by the RTC; CX and DDR sleep, XO does not. Wi-Fi reassociates about 8 s after resume. The fingerprint driver refuses suspend while a verify holds the sensor (the test stopped it by hand); nothing suspends the phone on its own yet |
| Cooling | works | 2026-11-09, devices-310, user | Fan from its lowest speed at 40 C to full at 80 C, a level every 2 C, from the prime core and GPU sensors; pump (AWP1921) on from 70 C, off below 65 C; each CPU cluster throttles from 90 C on any of its CPU, LLC and QMX sensors |
| Denial | works | 2026-10-28, user | |
| Native apps | works | user | Settings, Sensors |
| Android apps | works | 2026-11-09, devices-310, user | Droidloom release 30 from the image with the local Google apps package; the old Android storage restored with its apps |
| Shoulder triggers | not started | 2026-11-09, devices-310, probe | No input device for them |
| RGB light | not started | 2026-11-09, devices-310, probe | No LED device |

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
| Microphones | works | 2026-11-03, devices-224, probe | Mic1 and Mic2 record through PipeWire; the stall is fixed in linux-perpetua 9ba37ef1db70 |
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
| Fingerprint | works | 2026-11-05, devices-232, user | UFF driver through fprintd and fingerprint-phonewave like the other phones: Settings enrolls as the phone's user, behind the lock PIN, and it unlocks the phone, lit by the panel through `/dev/fp_illumination`. The boot image's tail names the sensor in fingerprint/sensor.conf. Needs Denial dev 0f47778 or later |
| Suspend | not started | | |
| Cooling | ? | | |
| Denial | works | user | |
| Native apps | works | user | Calls (calls mocked), Camera, Messages, Settings |
| Android apps | works | 2026-11-08, Droidloom 0.1.0-30.8 + Denial r346, user | Droidloom; vibration through hapticd, the three cameras through braun, and app fingerprint prompts through Denial's reader and sensor mark, no Android window |

## roadstr: Motorola Edge 70 (Eliza)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | 2026-11-04, devices-35, probe | Stock firmware W1WR36.39-115 from `modem_b` |
| Display | works | 2026-11-04, devices-34, user | 120 Hz; boot console, then Denial, without a reset |
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
| Battery | works | 2026-11-04, devices-35, probe | |
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
| Fingerprint | works | 2026-11-05, devices-230, user | JIIOV JV0307, optical, through fprintd and fingerprint-phonewave like the other phones: Settings adds a finger and it unlocks the phone, lit by the panel through `/dev/fp_illumination`. Needs Denial dev 0f47778 or later |
| Suspend | ? | | |
| Cooling | works | devices-12, probe | battmgr cooling device, checked with emulated temperatures |
| Denial | works | user | |
| Native apps | works | 2026-10-28, devices-14, user | |
| Android apps | works | user | Droidloom with its power HAL |

## iceland: OnePlus Pad 4 (SM8850)

| Feature | Status | Tested | Notes |
| --- | --- | --- | --- |
| Boot | works | 2026-11-11, devices-337, user, probe | The shared image on `denial`; shared init_boot, whose rescue fastboot writes this slot's `boot` and `init_boot` |
| Display | works | 2026-11-11, devices-337, user, probe | NT36536, both DSI links, DSC 1.2. Taken over from the bootloader as it runs: the timing engine counts on at 120 frames a second through the kernel's start, no frame lost on two boots |
| Brightness | works | 2026-10-28, devices-14, user | |
| Wake from screen off | works | 2026-11-12, devices-349, user | Power key; also double tap. The panel stays powered while blanked, as on stock, and its touch controller waits in gesture mode |
| GPU | works | user | Adreno 840 |
| Touch | works | user | |
| Keys | works | 2026-10-28, devices-14, user | |
| Haptics | none | | |
| Storage | works | | |
| USB device | works | probe | SSH over USB |
| USB host | ? | | |
| Video out (USB-C) | works | 2026-10-15, user | DisplayPort |
| Charging | works | 2026-11-11, devices-337, user; 2026-11-03, devices-225, probe | SuperVOOC at 51–55 W on both SC8547D pumps (OnePlus 80 W adapter, 35 to 38 % in 2 min, battery connector 48 °C); 14.7 W from a 5 V wall charger; USB-PD chargers raised to 9 V at 1.5 A (8.89 V × 1.49 A, 13.2 W in, battery +2.6 A at 14 %; still +0.1 A under full CPU load) |
| Battery | works | 2026-11-11, devices-337, user | |
| Wi-Fi | works | 2026-11-06, devices-243, user, probe | ath12k. The country set in Settings reaches the WCN7860 (`iw reg get`: phy#0 IT). Joins its saved 5180 MHz network at boot, 160 MHz EHT 2x2. The chip runs on one MAC (`qcom,no-dbs`): with DBS its scans missed 5180–5320 MHz (REMINDERS) |
| Bluetooth | works | 2026-10-28, devices-14, user | |
| Speakers | works | 2026-10-28, devices-14, user | ADSP from modem_b since devices-235: sound card up, probe |
| Microphones | works | 2026-09-30, probe | |
| Headphone jack | ? | | |
| Mobile data | none | | Wi-Fi model |
| SMS | none | | |
| Calls | none | | |
| Call audio | none | | |
| VoLTE | none | | |
| GNSS | ? | | |
| NFC | none | | |
| Camera | partial | 2026-11-11, devices-344, user, probe | Rear SmartSens C658 4160x3120 on CSIPHY4 and front D154 3264x2448 on CSIPHY5, RAW10 over four lanes. braun finds both and opens each 10 of 10 times; the Camera app shows both, untuned (user). Photos are landscape as the panel. Both send BGGR: as RGGB, orange came out blue on the rear and blue red on the front (user). Rear: text comes out upright and unmirrored; gain steps 1.5x to 6x within 4 %. Front: orientation and mirroring right (user). Not checked: rear gains of 8x and above. No focus: the rear lens actuator is not driven |
| Video decode | not started | | No Iris for Kaanapali; the stock vendor firmware has APV |
| Rotation | works | 2026-11-11, devices-322, user | steinerd, with the tail's `steinerd/top` naming the sensor axis at the screen's top edge |
| Light and proximity | works | 2026-11-11, devices-322, user | stk36c01; light tested by the user, proximity answers (probe) |
| Compass and gyroscope | works | 2026-11-11, devices-322, probe | steinerd: bmi3x0 accelerometer and gyroscope, ak0991x magnetometer, bu52053nvx hall |
| Fingerprint | none | | |
| Suspend | partial | 2026-11-12, devices-349, user | s2idle, the default, sleeps until the RTC wakes it (306 s, woken by an alarm Droidloom's Android set) or a double tap does (the touch interrupt wakes it and the screen comes on). Wi-Fi comes back after resume. deep returns at once from the firmware (it rebooted the tablet on devices-14). Nothing suspends the tablet on its own: the image turns suspend off |
| Cooling | partial | 2026-11-12, devices-353, probe | Throttles on an estimate of the case, held at 45 °C by the power allocator, one clock step a cycle. Under a GPU burn and eight busy CPU threads it held 44.8 to 45.2 °C for four minutes, the GPU between 222 and 382 MHz. With the battery connector thermistor in the estimate (devices-353), charging held it over 45 °C in a game menu and the GPU at 160 MHz (user: lagging hard); devices-354 estimates from the board (40 %), subboard (40 %) and skin (20 %) thermistors, not yet tried. The XO thermistor throttles fully from 78 °C, the SoC from 105 °C. Not checked: the estimate against a thermometer on the case |
| Denial | works | 2026-11-11, devices-337, user | The published image's `denial` package (0.6.3), on the `denial` partition |
| Native apps | works | 2026-10-28, devices-14, user | |
| Android apps | works | 2026-11-12, devices-354, Droidloom 0.1.0-33.4, user | Droidloom with the Google apps package. Minimizing and restoring a game quickly, again and again, never leaves it frozen |
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
