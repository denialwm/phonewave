//! The services calls-manager talks to: dmail's calls, hapticd, the
//! notification server, and logind's lock.

use std::collections::HashMap;

use zbus::proxy;
use zbus::zvariant::{OwnedObjectPath, Value};

/// dmail's call: id, number (empty when withheld), direction (`in` or
/// `out`), state, and the Unix time it connected (0 until then).
pub type CallRow = (u8, String, String, String, i64);

#[proxy(
    interface = "org.dmail.Voice1",
    default_service = "org.dmail",
    default_path = "/org/dmail"
)]
pub trait Voice1 {
    /// States: dialing, alerting, incoming, waiting, active, held, ending.
    #[zbus(property)]
    fn calls(&self) -> zbus::Result<Vec<CallRow>>;

    fn answer(&self, id: u8) -> zbus::Result<()>;

    /// Ends a call, or turns down an incoming one.
    fn hangup(&self, id: u8) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.hapticd.Haptics1",
    default_service = "org.hapticd",
    default_path = "/org/hapticd/Haptics"
)]
pub trait Haptics1 {
    /// Steps of duration in ms and intensity, 0 to 1; on the default device
    /// when `device` is empty. Returns the effect's id.
    fn play_pattern(&self, device: &str, steps: &[(u32, f64)]) -> zbus::Result<u64>;

    fn cancel(&self, effect: u64) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn activation_token(&self, id: u32, activation_token: String) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn get_user(&self, uid: u32) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.login1.User",
    default_service = "org.freedesktop.login1"
)]
trait User {
    /// The user's graphical session: its id and object.
    #[zbus(property)]
    fn display(&self) -> zbus::Result<(String, OwnedObjectPath)>;
}

#[proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
pub trait Session {
    /// Denial keeps it: true while the session is locked.
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
}

/// The user's graphical session, on the system bus.
pub async fn graphical_session(bus: &zbus::Connection) -> zbus::Result<SessionProxy<'static>> {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let user = ManagerProxy::new(bus).await?.get_user(uid).await?;
    let user = UserProxy::builder(bus).path(user)?.build().await?;
    let (_, session) = user.display().await?;
    SessionProxy::builder(bus).path(session)?.build().await
}
