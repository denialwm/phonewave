# ufs-boot-lpm

Keeps the UFS host out of its low-power states while the system boots and
gives them back once it is up, as the stock Qualcomm Android init does
(`init.qcom.rc`: `clkscale_enable`, `auto_hibern8` and `clkgate_enable` of
the `ufshc` written 0 at init, back to 1, 5000 and 1 at `sys.boot_completed`).
Boot reads many small files; with clock scaling off the host runs at its top
clock and gear, without clock gating and auto-hibern8 no read waits for the
link to wake up.

`ufs-boot-lpm.service` runs `ufs-boot-lpm hold` before `sysinit.target`: for
every platform device with the kernel's ufshcd attributes it saves
`clkscale_enable`, `clkgate_enable` and `auto_hibern8` to `/run/ufs-boot-lpm`
and writes 0 to them. It pulls in `ufs-boot-lpm-release.service`, which runs
`ufs-boot-lpm release` after `multi-user.target` (and `graphical.target` when
the boot has one) and writes the saved values back, gating last. Stopping
`ufs-boot-lpm.service` releases too.

The values restored are the kernel's, not stock's: the kernel's auto-hibern8
timer is 150 ms, stock sets 5 ms after boot.

Devices turn it on from their boot image's tail with a
`systemd/system/sysinit.target.wants/ufs-boot-lpm.service` link.

`make check` runs shellcheck and the tests (`tests/run.sh`, a fake sysfs).
