# linux-msm

linux-msm's [qrtr](https://github.com/linux-msm/qrtr),
[rmtfs](https://github.com/linux-msm/rmtfs) and
[tqftpserv](https://github.com/linux-msm/tqftpserv), the programs a Qualcomm
modem needs on the host, packaged for the phones by
`image/recipes/linux-msm` at the commits it names, with Phonewave's units.

| Package | Gives |
|---|---|
| `qrtr` | `libqrtr`, `qrtr-lookup`, `qrtr-cfg` |
| `rmtfs` | `rmtfs.service`: the modem's file system from the phone's own `modemst1`, `modemst2`, `fsg` and `fsc`, read-only; the modem's writes stay in memory until it stops |
| `tqftpserv` | `tqftpserv.service`: the files the modem asks for over QRTR, its state in `/var/lib/tqftpserv`. Before it starts, `tqftpserv-stock` lays out what stock Android's tftp_server gives the modem: the booted slot's `fsg` read-only at `/run/tqftpserv/fsg` where it holds a filesystem (Motorola's carrier configurations, `/readonly/vendor/fsg/`, through `tqftpserv-carrier-configuration.patch`), and persist's `rfs/msm/mpss` copied once into the state, never over a file the modem wrote (Motorola's option bits and SIM lock data, without which the SIM stays in personalization check) |

## rmtfs on OnePlus phones

`rmtfs-oneplus-nv-backup.patch` applies to rmtfs:

- The OEM NV backups (`/oem/nvbk/static`, `/oem/nvbk/dynamic`) also open
  under the names newer OnePlus phones use, `oplusstanvbk` (slotted) and
  `oplusdycnvbk`.
- Without `-S`, the slot is the one the bootloader started
  (`androidboot.slot_suffix`).
- The modem reads 512 bytes past the end of the static backup, a board block
  stock Android's storage daemon builds. Answered with zeros, the modem
  starts without its IMEIs and the network rejects the phone ("illegal ME").
  rmtfs serves the block after the backup where the kernel has the
  bootloader's project information, linux-perpetua's `oneplus,project-info`
  device (`/sys/devices/platform/project-info/`: `project`, `pcb`, `rf`, each
  phone's own): magic, region `CE`, the start of soc0's serial number,
  project, PCB and RF. The IMEIs stay the modem's own, in the backup; rmtfs
  never writes or makes one.

A phone without that device is served as upstream does.

Neither unit starts at boot. dmail's `modem-setup` starts both when the
device tree gives the modem rmtfs memory (`/dev/qcom_rmtfs_mem1`), then
starts the modem. They are units of their own so they outlive dmail.

## License

BSD-3-Clause, as upstream.
