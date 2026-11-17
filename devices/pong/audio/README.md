# Audio topology

`Nothing-Phone-2.m4` is the AudioReach topology of the phone's speakers,
derived from `SM8450-HDK.m4` of
[linux-msm/audioreach-topology](https://github.com/linux-msm/audioreach-topology)
(BSD-3-Clause): MultiMedia1 playback to the tertiary MI2S, on its second data
line. `boot/build-boot.sh` builds it with that repository's macros into
`qcom/sm8450/Nothing-Phone-2-tplg.bin`, which the kernel loads from the tail,
and caps the stream volume at -6 dB there, since the amplifiers have no
speaker protection. The UCM profile that gives PipeWire a sink is
`services/audio/ucm/ucm2/Qualcomm/sm8450/Nothing-Phone-2/`.
