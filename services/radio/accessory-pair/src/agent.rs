//! The pairing agent. BlueZ asks the agent of the client that called
//! `Pair()`, so registering ours as NoInputNoOutput (and never as the
//! default agent) makes the pairings this service starts Just Works,
//! with no prompt, and leaves every other pairing to the desktop's agent.

use crate::Address;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use zbus::zvariant::ObjectPath;

pub const PATH: &str = "/org/phonewave/AccessoryPair/agent";
pub const CAPABILITY: &str = "NoInputNoOutput";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
}

/// The accessories the agent accepts; requests for any other device are
/// rejected.
#[derive(Clone, Default)]
pub struct Allowed(Arc<Mutex<HashSet<Address>>>);

impl Allowed {
    pub fn allow(&self, address: Address) {
        self.0.lock().unwrap().insert(address);
    }

    pub fn contains(&self, device: &ObjectPath<'_>) -> bool {
        Address::from_object_path(device.as_str())
            .is_some_and(|a| self.0.lock().unwrap().contains(&a))
    }
}

pub struct Agent {
    pub allowed: Allowed,
}

impl Agent {
    fn check(&self, what: &str, device: &ObjectPath<'_>) -> Result<(), AgentError> {
        if self.allowed.contains(device) {
            eprintln!("accessory-pair: agent: {what} for {device}: accepted");
            Ok(())
        } else {
            eprintln!("accessory-pair: agent: {what} for {device}: not ours, rejected");
            Err(AgentError::Rejected(format!(
                "{device} is not a docked accessory"
            )))
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {
        eprintln!("accessory-pair: agent released by BlueZ");
    }

    fn request_pin_code(&self, device: ObjectPath<'_>) -> Result<String, AgentError> {
        Err(AgentError::Rejected(format!("no PIN for {device}")))
    }

    fn display_pin_code(&self, device: ObjectPath<'_>, _pincode: &str) -> Result<(), AgentError> {
        Err(AgentError::Rejected(format!("no display for {device}")))
    }

    fn request_passkey(&self, device: ObjectPath<'_>) -> Result<u32, AgentError> {
        Err(AgentError::Rejected(format!("no passkey for {device}")))
    }

    fn display_passkey(&self, _device: ObjectPath<'_>, _passkey: u32, _entered: u16) {}

    fn request_confirmation(
        &self,
        device: ObjectPath<'_>,
        _passkey: u32,
    ) -> Result<(), AgentError> {
        self.check("confirmation", &device)
    }

    fn request_authorization(&self, device: ObjectPath<'_>) -> Result<(), AgentError> {
        self.check("authorization", &device)
    }

    fn authorize_service(&self, device: ObjectPath<'_>, _uuid: &str) -> Result<(), AgentError> {
        self.check("service authorization", &device)
    }

    fn cancel(&self) {
        eprintln!("accessory-pair: agent: request cancelled by BlueZ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allowed_devices() {
        let allowed = Allowed::default();
        allowed.allow("C0:87:06:07:FE:68".parse().unwrap());
        let pen = ObjectPath::try_from("/org/bluez/hci0/dev_C0_87_06_07_FE_69").unwrap();
        let other = ObjectPath::try_from("/org/bluez/hci0/dev_00_11_22_33_44_55").unwrap();
        let agent = Agent { allowed };
        assert!(agent.request_authorization(pen.clone()).is_ok());
        assert!(agent.request_confirmation(pen, 0).is_ok());
        assert!(agent.request_authorization(other.clone()).is_err());
        assert!(agent.authorize_service(other, "").is_err());
    }
}
