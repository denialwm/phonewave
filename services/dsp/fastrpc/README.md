# fastrpc (CDSP)

Qualcomm's FastRPC userspace (github.com/qualcomm/fastrpc, BSD-3-Clause) for
the CDSP: libcdsprpc and the other domain libraries, and cdsprpcd, the
default listener of the CDSP's root process.

- Only the CDSP is wired up. Upstream's udev rules and adsprpcd units are not
  installed: hexagonrpcd (services/sensors/hexagonrpcd) serves the ADSP, and a
  second listener on its nodes would take its place. 61-fastrpc-cdsp.rules
  gives the CDSP's nodes to the fastrpc group and starts cdsprpcd.
- The shells the CDSP runs a user process in (fastrpc_shell_3,
  fastrpc_shell_unsigned_3) are Qualcomm-signed images of each phone's stock
  firmware. They are not shipped: fastrpc-dsp-partition.service mounts the
  booted slot's dsp partition at /run/dsp, read-only (block device set
  read-only, journal never replayed). /usr/share/qcom/phonewave/dsp points
  there; each phone's devices/<codename>/rootfs/usr/share/qcom/conf.d/ file
  names its folder by the phone's device tree model (erhai: cdsp/).
- Programs that use FastRPC run as a member of the fastrpc group.

Smoke test, with the test program and skeletons of the upstream source tree
built for the phone (repository test/: v75 skeletons run on the v79 CDSP):

    fastrpc_test -d 3 -U 1 -t linux -a v75

calculator and multithreading pass on an unsigned process; hap_example's
HAP_mem DMA-handle part needs a memory map the mainline fastrpc driver does
not offer.
