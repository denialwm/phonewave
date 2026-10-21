# Possible improvements

Not started. Each works on its own, but they work best together: run BiRefNet
on fewer pixels, then recover what that loses with depth and the wallpaper's
own colours. Times are from the Ace 3, where a mask takes 2.2 s and BiRefNet's
GPU time is 1.7 s of that.

## Run BiRefNet on fewer pixels

BiRefNet sees every wallpaper squashed into 1024×1024. Its time and memory
grow with the number of input pixels, so a quarter of the pixels should take
roughly a quarter of the time (estimated, not measured):

| Input | Pixels | BiRefNet GPU time |
|---|---|---|
| 1024×1024 (today) | 100% | 1.7 s |
| 768×768 | 56% | ~1 s |
| 576×1024 | 56% | ~1 s |
| 384×672 | 25% | ~0.45 s |

Inputs in the wallpaper's own shape (576×1024, 384×672) keep more vertical
detail for the same cost than a square. A lower resolution loses thin strands
and edge detail; the two ideas below recover them. To do: check which sizes the
exported model accepts (its backbone works in 32-pixel steps), then change the
shape in `mask()`; `lock_mask()` already reads the logits' size.

## Snap the edges back to the wallpaper

The mask is enlarged from BiRefNet's 1024×1024 (or smaller) output with a plain
bilinear resize, so its edge only roughly follows the subject. A guided filter
enlarges it with the full-size wallpaper as the guide instead, so the edge
lands on the real colour boundaries. It is a classic image filter, no model,
and should take a fraction of a second at 2048 high. It would replace the
probability resize at the start of `lock_mask()`.

## Give depth more say

Today depth can only take pixels away: the gate drops what lies far behind
the subject (`LOW`/`HIGH` in `pipeline.rs`). It could also decide the pixels
BiRefNet is unsure about, with probability between about 0.1 and 0.9: as near
as the subject means keep, far means drop. A lower-resolution BiRefNet leaves
more unsure pixels, and Depth Anything already runs, so this costs almost
nothing.
