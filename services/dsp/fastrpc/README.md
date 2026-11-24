# fastrpc

Qualcomm's FastRPC userspace (github.com/qualcomm/fastrpc, BSD-3-Clause) for
the CDSP and the ADSP audio domain. The shared image installs fastrpc-audio.
Installing fastrpc adds the CDSP libraries, listener and udev rule.

- The CDSP udev rule starts cdsprpcd. A phone can also enable
  adsprpcd_audiopd.service from its boot tail to load stock audio modules
  from /run/dsp/adsp. The ADSP audio and sensor domains use separate
  listeners. hexagonrpcd continues to serve the sensors.
- The shells the CDSP runs a user process in (fastrpc_shell_3,
  fastrpc_shell_unsigned_3) are Qualcomm-signed images of each phone's stock
  firmware. They are not shipped: fastrpc-dsp-partition.service mounts the
  booted slot's dsp partition at /run/dsp, read-only (block device set
  read-only, journal never replayed). /usr/share/qcom/phonewave/dsp points
  there. The package installs the Pad 3's model-specific search path in
  /usr/share/qcom/conf.d/oneplus-pad3.yaml, so the CDSP finds its files in
  /run/dsp/cdsp on a fresh installation.
- Programs that use FastRPC run as a member of the fastrpc group.

Smoke test, with the test program and skeletons of the upstream source tree
built for the phone (repository test/: v75 skeletons run on the v79 CDSP):

    fastrpc_test -d 3 -U 1 -t linux -a v75

calculator and multithreading pass on an unsigned process; hap_example's
HAP_mem DMA-handle part needs a memory map the mainline fastrpc driver does
not offer.
