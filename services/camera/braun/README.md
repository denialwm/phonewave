# braun

Camera service for Qualcomm phones. It finds the cameras the kernel offers,
routes one through qcom-camss to memory as packed 10-bit raw, and does
everything after that itself, without libcamera: auto exposure, white
balance, focus, the preview, and the bracketed burst a photo is developed
from. Apps get previews and photos; the Camera app is one.

## What the phone needs

- A `qcom-camss` media graph, with each sensor's driver linked to a CSIPHY.
  braun routes CSIPHY → `msm_csid0` → `msm_vfe0_rdi0` → `msm_vfe0_video0`.
- Sensor drivers with 10-bit Bayer output and the standard controls:
  `exposure`, `analogue_gain`, `horizontal_blanking`, `pixel_rate`,
  `camera_orientation` and `camera_sensor_rotation`. A focus motor is a
  lens entity on an ancillary link from its sensor, with
  `focus_absolute`.
- The `braun` user, which the package makes, in the `video` group.

## Cameras and their files

braun takes from the kernel what it says: the frame size and Bayer order,
the line time and exposure range, gain codes, facing, rotation and the lens.
What it does not say comes from two kinds of file:

| File | Holds | Comes with |
|---|---|---|
| `/usr/share/braun/sensors/<driver>.toml` | A sensor model's black and white level, how its gain codes map to gain, the most gain auto exposure uses | braun (`sensors/`) |
| `/etc/braun/<id>.toml` | One phone's camera: its role, how wide it sees, a rotation where the device tree is wrong, the focus travel, the illuminant locus, colour matrices and lens shading | the device (`devices/<codename>/rootfs/etc/braun/`) |

A camera's id is its driver's name, with the sensor's address after a `-`
when a phone has two of the same sensor. A camera without a calibration
file still works, untuned: grey world white balance, the raw colours, no
lens shading. A sensor without a profile gets black 64, white 1000, and a
linear gain whose least code is 1x. `braunctl list` shows what braun
assumed for each camera.

A sensor profile:

```toml
# Sony IMX890. Gain is 16384 / (16384 - code), as libcamera's helper has it.
black = 64
white = 1000
analogue_gain = { m0 = 0, c0 = 16384, m1 = -1, c1 = 16384 }
max_gain = 16
```

`analogue_gain` is gain = (m0 × code + c0) / (m1 × code + c1), as SMIA
sensors describe it. A calibration file:

```toml
role = "main"            # main, ultrawide, telephoto or auxiliary
zoom = 1.0               # the main camera's zoom that shows what this one does
rotation = 270           # optional, as camera_sensor_rotation states it

[focus]
travel = [0x500, 0xc00]  # what the sweep covers
start = 0x700            # where the lens waits for the first sweep

[[locus]]                # the light's colour, warm to cool
ct = 2300.0
rg = 1.0038
bg = 0.3092

[[ccm]]                  # colour matrices, warm to cool
ct = 2300.0
m = [...]                # 9 values, row by row

[[shading]]              # lens shading gains, 17 x 13, warm to cool
ct = 2300.0
red = [...]
green = [...]
blue = [...]
```

## Preview

Every frame becomes an upright BGRA preview up to 1024 cells across, one
2x2 cell per pixel, then lens shading, white balance, the colour matrix
with the look, and the tone. The same pass gathers the statistics:

- **Lens shading:** the calibration's gain maps, interpolated in mired
  between colour temperatures. They are applied fully until the scene needs
  digital gain; then they dim, to the power 0.4 at 8x digital gain, so the
  corners do not show the noise they lift.
- **Auto exposure:** meters the mean linear luminance at 18% grey, and lets
  at most 4% of the frame sit above 90% of full scale. It moves by the
  square root of what it asks, at most 1.6x a step, with an 8% dead band,
  every 3 frames. Longest time first (33 ms), then analogue gain, up to the
  camera's most, then digital gain, up to 8x. Digital gain lifts shadows
  and mid tones and leaves the sensor's clipping where it is.
- **White balance:** a 16x12 grid of zones of unclipped colour sums. Only
  zones within 0.06 of the calibration's locus count, and their grey is
  projected onto the locus. That gives the light's colour temperature. A
  light cooler than 5500 K is made neutral; a warmer one is moved 40% of
  the way toward 5500 K and stays a little warm. Gains and the matrix come
  from that temperature.
- **Look and tone:** fitted to the RedMagic 11 Pro+ on a normally exposed
  scene, block by block. After the matrix, hue 4 degrees toward blue. Then
  each channel goes through a filmic S curve: deep shadows, contrasty mid
  tones, highlights rolling off, and light past clipping reaching white. A
  cell near clipping goes to white only when it is near grey (a light
  source); a saturated colour that clips keeps its colour.
- **Focus:** a contrast sweep where the camera has a lens. Coarse over the
  travel, then fine around the best position. It runs when the camera opens
  and on every focus request.
- **Zoom:** crops the middle `1 / zoom` of the frame, in the preview, its
  metering and the photo.

## Photo

A shot takes a bracketed raw burst around the exposure auto exposure chose:

| Group | Exposure | Frames |
|---|---|---|
| Long (shadows) | 2x | 8 |
| Short | 1/4 | 4 |
| Shortest (light sources) | 1/16 | 4 |

That takes about 0.85 s. The burst is then developed on a thread of its
own while the preview goes on:

1. Frames are aligned to the short group's first frame. The shift is a
   global one, in whole 2x2 cells, found coarse to fine on log radiance.
2. Groups are merged on the raw mosaic in radiance, weighted by exposure x
   frames, leaving out cells that clipped. What clips even in the shortest
   group is held white.
3. Lens shading, as in the preview, then demosaicing (Malvar-He-Cutler).
4. White balance, the digital gain, the matrix and the tone of the
   preview at the shutter, with radiance taken at the preview's exposure,
   so the photo looks like the preview with less noise and more detail.
   The image is turned upright, and mirrored for a front camera, as its
   preview is.

Photos are JPEGs at the sensor's full output. A 3072x4096 photo is written
about 2.5 s after the shutter on the Ace 3 and 1.9 s on the RedMagic 11 Pro.

A shot can also keep the raw frame the preview showed at the shutter, for
tuning: packed RAW10 as the sensor sends it, and notes with the sensor,
size, stride, Bayer order, black level, exposure, white balance gains,
colour temperature and matrix.

## Build and install

```sh
make
make check
sudo make install
sudo systemd-sysusers
```

`build-aarch64.sh` cross-builds for the phones against the workspace
sysroot; then `make install BIN=target/aarch64-unknown-linux-gnu/release`.
braun starts on the first call, through D-Bus activation.

## API

System bus name `org.braun`, object `/org/braun`, interface
`org.braun.Camera1`. `Connect` lets in whoever may open the camera nodes:
root, the `video` group, and the user of an active local session.

| Member | Type | Meaning |
|---|---|---|
| `Cameras` | `aa{sv}` | Each camera: `id`, `name`, `facing` (`front`, `back`, `external`), `role` (empty when the device does not say), `zoom` (only when the device says; 1 for the main camera), `width` and `height` upright, `lens` (it can focus), `tuned` (it has a calibration) |
| `Connect()` | `h` | A session socket of the caller's own |

A session is a `SOCK_SEQPACKET` socket, one message a packet,
little-endian, the first byte its kind. File descriptors travel beside a
message. `braun::client` speaks it; `braun::proto` has the encoding.

| Request | Meaning |
|---|---|
| `Open(id)` | Open camera `id` and stream it |
| `Close` | Close the camera |
| `Focus(x, y)` | Focus on this point of the preview, 0 to 1 across and down |
| `Zoom(zoom)` | Crop the middle `1 / zoom` of the frame |
| `Shoot(shot)` with a file, or a file, a raw file and a notes file | Take photo `shot` into the file |
| `Release(slot)` | The preview in `slot` is read |

| Reply | Meaning |
|---|---|
| `Opened(width, height, lens, slots, slot_bytes)` with a memfd | The camera is open; previews come in `slots` slots of the memfd |
| `Frame(slot, width, height, sequence)` | A BGRA preview in `slot`, rows packed. braun skips a frame while every slot is unread |
| `Captured(shot)` | The burst is in; the photo is developing |
| `Saved(shot)` | The photo is written |
| `Failed(shot, text)` | Photo `shot` failed; for 0, the camera did and is closed |

One camera streams at a time. The session that opens one last takes it,
and the one streaming hears `Failed(0, "another app took the camera")`.

```sh
braunctl list
braunctl cameras
braunctl preview ov50e40 frame.ppm
braunctl shot ov50e40 photo.jpg
```

`list` finds the cameras itself, without the daemon; the others go through
it. `preview` and `shot` stream 60 frames first, so exposure, white balance
and focus settle.

## Devices

| Device | Cameras |
|---|---|
| `aston` (OnePlus Ace 3) | IMX890 main with DW9719 focus, S5K3P9 front, both calibrated from the stock tuning; IMX355 ultrawide, untuned |
| `nx809j` (RedMagic 11 Pro) | OV50E40 main with DW9785 focus, OV50D40 ultrawide, OV02F10 auxiliary, OV16A1Q front; untuned |
