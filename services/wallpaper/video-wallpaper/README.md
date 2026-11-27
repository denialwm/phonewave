# video-wallpaper

Plays a video, looped, as the wallpaper of a Denial phone: on the home screen
and on the lock screen. The phone's hardware decoder (V4L2, such as Qualcomm
Iris) writes each frame into a DMA-BUF that goes to Denial as it is, through
`zwp_linux_dmabuf_v1`, on a `wlr-layer-shell` background surface cropped to
the screen with `wp_viewporter`.

Frames go out only when Denial asks for one, and Denial asks only while some
screen shows the wallpaper. Under an opaque app, or with the screen off, the
decoder fills its spare buffers and stops; the video goes on from the same
frame when the wallpaper shows again. While a video wallpaper is mapped, the
screen turns off without the always-on display.

## Use

Write the video's path to `~/.config/denial/video-wallpaper.conf`:

```
video = /home/user/Videos/wallpaper.mp4
```

then start it, or log in again:

```sh
systemctl --user start video-wallpaper
```

The user unit starts with the graphical session when that file exists. To
go back to the still wallpaper, stop the unit and remove the file.
`video-wallpaper FILE` plays a file without the configuration.

The video is an MP4 with H.264 or HEVC in its first video track, which the
decoder must take. It plays at the track's mean frame rate, covering the
screen and cropped at the long side, without sound.

## Build and install

```sh
make
make check
sudo make install
```

For a phone, `image/pack video-wallpaper` builds the package and
`tools/phone-install HOST video-wallpaper` installs it.
