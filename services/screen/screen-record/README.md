# screen-record

Records the screen of a Denial phone or tablet. Frames come from Denial
through `ext-image-copy-capture-v1` in DMA-BUFs, are scaled down on the GPU
and go to ffmpeg as constant-rate video timed by Denial's presentation
timestamps, so the CPU never reads a full-resolution frame.

Denial's shade shows a Screen recording tile if `screen-record` is on `PATH`:
a tap starts recording, the next one stops it. Without the package there is no
tile; there is no setting.

## Use

```sh
screen-record          # start recording, or stop the one running
screen-record stop     # stop the one running
```

A recording is 720p (the shorter side) at 60 fps, H.264 in MP4, written to
`~/Videos/Screen recordings/Screen recording 2026-10-22 20.31.05.mp4`. It
runs until it is stopped or gets SIGINT or SIGTERM, then waits for ffmpeg to
finish the file. While the screen is off it records black; a recording
stopped before the screen showed anything leaves no file.

One recording runs at a time: it holds `$XDG_RUNTIME_DIR/screen-record.lock`.
While it records, `$XDG_RUNTIME_DIR/screen-record` holds its process ID and
file on two lines; the file goes once the video is finished.

For other sizes and formats, capture straight into ffmpeg:

```sh
screen-record capture [--size N] [--fps N] -- <ffmpeg output arguments>
```

It needs `ffmpeg` with libx264, a render node at `/dev/dri/renderD128`,
EGL with GBM and DMA-BUF import, and `WAYLAND_DISPLAY`.

## Build

```sh
make && sudo make install        # on the device
./build-aarch64                  # cross-built on the host
```
