//! dmail: modem service for Qualcomm phones. It talks QMI to the modem over
//! QRTR. See DESIGN.md.

pub mod apn;
pub mod bus;
pub mod config;
pub mod daemon;
pub mod data;
pub mod modem;
pub mod netlink;
pub mod network;
pub mod qmi;
pub mod qrtr;
pub mod sim;
pub mod sms;
