# qmi-tools

Small QMI clients that talk to the modem over QRTR, used during Ace 3 (aston)
cellular bring-up. Each one sends a single request or a short fixed sequence
and prints the result; message layouts follow libqmi's `data/qmi-service-*.json`.

| Tool | Does |
|---|---|
| `qmi-status uim\|dms\|slots\|nas NODE PORT` | read-only card, operating mode, slot and NAS status |
| `qmi-ids DMS_NODE DMS_PORT` | IMEI length, TAC and check digit; ESN/MEID presence only |
| `qmi-nas-reject NAS_NODE NAS_PORT SECONDS` | prints network reject causes seen within SECONDS |
| `qmi-online DMS_NODE DMS_PORT` | sets the operating mode to online |
| `qmi-provision UIM_NODE UIM_PORT SLOT` | activates the GW provisioning session on SLOT |
| `qmi-verify-pin UIM_NODE UIM_PORT < PIN` | verifies PIN1 once, reading it from stdin |

Node and port come from `qrtr-lookup`. Build with libqrtr from linux-msm/qrtr
(tested at 27d2c9dfb4e8653ac314ebe2b7980fed2f4bac2e, static):

    make CPPFLAGS=-I$PREFIX/include LDFLAGS=-L$PREFIX/lib

The phone has copies in /usr/local/bin. No unit runs them.
