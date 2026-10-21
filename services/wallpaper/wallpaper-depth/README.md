# wallpaper-depth

Cuts the subject out of a wallpaper so the Denial lock screen can draw its
clock behind it. Two models run as native Vulkan compute through wgpu:
BiRefNet_lite finds the subject and Depth Anything V2 Small keeps only the
parts of it in front. On the Ace 3 (Adreno 740) a mask takes about 2.2 s.

Denial runs it in the background when the wallpaper picker closes, if
`wallpaper-depth` is on `PATH`. Without the package the lock screen draws no
cut; there is no setting.

## Use

```sh
wallpaper-depth mask WALLPAPER
```

writes to `$XDG_CACHE_HOME/denial/wallpaper-depth/` (`~/.cache` when unset):

| File | Content |
|---|---|
| `lock-mask.png` | The cut: white, with alpha marking the subject, in wallpaper space |
| `lock-mask.json` | The wallpaper it belongs to, the GPU and the timings |
| `subject.png`, `depth.npy` | The two model outputs it was made from |

The PNGs and the sidecar are replaced atomically, the sidecar last, so a reader
never pairs a new mask with an old sidecar.

The models come from `share/wallpaper-depth/models` in the prefix the binary is
installed in; `WALLPAPER_DEPTH_MODELS` overrides that. For development:

| Command | Does |
|---|---|
| `wallpaper-depth check MODEL INPUT.npy REFERENCE.npy` | Runs a model and compares it with an ONNX Runtime reference |
| `wallpaper-depth profile MODEL INPUT.npy` | Times each kernel |
| `wallpaper-depth half MODEL OUT` | Writes the model with float16 weights |

## Build and install

```sh
make            # fetches and checks the models, halves them, builds
make check
sudo make install
```

`make` downloads the two models from Hugging Face at the revisions pinned in
`models.txt` and checks their SHA-256. With `HALF=1`, the default, the weights
are installed as float16: 155 MB in place of 307 MB. On 15 wallpapers on the
Ace 3, at most 0.12% of a mask's pixels moved by more than 16/255 and the
subject's edge moved on at most 0.2% of its pixels; the time was the same.
`HALF=0` installs them as published.

Cross-build for an Arch Linux ARM phone against the phone-matched sysroot:

```sh
sysroot=/mnt/development/denial-os/cross/sysroot
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=--sysroot=$sysroot -C link-arg=-B$sysroot/lib -C link-arg=-Wl,-rpath-link,$sysroot/usr/lib -C link-arg=-Wl,-rpath-link,$sysroot/lib"
make TARGET=aarch64-unknown-linux-gnu
make install TARGET=aarch64-unknown-linux-gnu DESTDIR=stage
rsync -a stage/usr/ PHONE:/usr/
```

The halving runs on the build machine; only the binary is cross-compiled.

[IMPROVEMENTS.md](IMPROVEMENTS.md) lists what could make the mask faster or
sharper.

## Models

| Model | Source | License |
|---|---|---|
| BiRefNet_lite | [onnx-community/BiRefNet_lite-ONNX](https://huggingface.co/onnx-community/BiRefNet_lite-ONNX) | MIT, `licenses/birefnet_lite.txt` |
| Depth Anything V2 Small | [onnx-community/depth-anything-v2-small](https://huggingface.co/onnx-community/depth-anything-v2-small) | Apache-2.0, `licenses/depth_anything_v2_small.txt` |

Only the Small Depth Anything V2 model is Apache-2.0; the larger ones are not
licensed for commercial use. The code here is MIT.
