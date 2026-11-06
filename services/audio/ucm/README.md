# ucm

The ALSA UCM profiles of the phones' sound cards, laid out as
`/usr/share/alsa/ucm2`, in one package beside Arch's `alsa-ucm-conf`:
`alsa-ucm-phonewave` (`image/recipes/alsa-ucm-phonewave`). ALSA picks a
profile by the card's driver and name, so every phone gets them all and uses
its own.

| Card | Phone |
|---|---|
| `sm8550/astonsnd` | OnePlus Ace 3 |
| `kaanapali/Nubia-NX809J` | RedMagic 11 Pro |
| `kaanapali/OnePlus-Pad4` | OnePlus Pad 4 |
| `kaanapali/Xiaomi-17` | Xiaomi 17 |

WirePlumber sets the phones' cards to software volume
(`image/rootfs/usr/share/wireplumber/wireplumber.conf.d/51-phone-audio.conf`).
