# FFmpeg patches

The recipe applies the ones `_patches` in `../PKGBUILD` lists, in that
order. To change one, raise `pkgrel` and run `image/pack ffmpeg-denial`.

- `av_stream_get_first_dts-for-Chromium.patch`: Arch's, unchanged.
- `v4l2-m2m-drm-prime-av1.patch`: the V4L2 mem2mem decoders offer
  AV_PIX_FMT_DRM_PRIME beside the software format. Each capture buffer's
  planes are exported once with VIDIOC_EXPBUF and described in an
  AVDRMFrameDescriptor kept with the buffer; the frame's `buf[0]` queues the
  buffer back when released. Adds `av1_v4l2m2m` and P010 capture, and makes
  the stateful decoders behave on Iris: a 10-bit stream gets new P010
  buffers at its visible size, frames the stream does not show are not
  handed out, and timestamps survive when the caller sets no time base
  (Firefox). Made on n9.0.2 and refreshed on n9.1; on a new release,
  rebase it there and refresh the file with `git format-patch`.

## Testing on a phone

Each clip's hardware decode against FFmpeg's software decode, frame by
frame:

    ffmpeg -c:v hevc_v4l2m2m -i CLIP -pix_fmt p010le -f framemd5 - | grep ^0,
    ffmpeg -i CLIP -pix_fmt p010le -f framemd5 - | grep ^0,

with `nv12` for 8-bit clips; the hash columns match. Test from a container
(MP4, WebM), as players feed the decoders. In Firefox, `MOZ_LOG=FFmpegVideo:5,Dmabuf:4` shows
`V4L2 codec hevc_v4l2m2m`, `Requesting pixel format DRM PRIME` and a `V4L2
Got one frame` line per frame.
