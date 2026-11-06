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
| `tqftpserv` | `tqftpserv.service`: the files the modem asks for over QRTR, its state in `/var/lib/tqftpserv` |

Neither unit starts at boot. dmail's `modem-setup` starts both when the
device tree gives the modem rmtfs memory (`/dev/qcom_rmtfs_mem1`), then
starts the modem. They are units of their own so they outlive dmail.

## License

BSD-3-Clause, as upstream.
