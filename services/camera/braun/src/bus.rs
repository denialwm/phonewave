//! `org.braun.Camera1` on `/org/braun`: the cameras there are, and a
//! session for each client that connects (`proto`).

use std::collections::HashMap;
use std::sync::OnceLock;

use zbus::message::Header;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};
use zbus::{fdo, interface};

use crate::{NAME, PATH, proto, session};

struct Service;

fn owned<'a>(value: impl Into<Value<'a>>) -> OwnedValue {
    value
        .into()
        .try_to_owned()
        .expect("no descriptors in a camera's description")
}

#[interface(name = "org.braun.Camera1")]
impl Service {
    /// A session socket: the other end is served on a thread of its own
    /// until the client closes it.
    async fn connect(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> fdo::Result<OwnedFd> {
        allowed(connection, &header).await?;
        let (ours, theirs) =
            proto::pair().map_err(|error| fdo::Error::Failed(error.to_string()))?;
        session::start(ours).map_err(|error| fdo::Error::Failed(error.to_string()))?;
        Ok(theirs.into())
    }

    /// Each camera: its `id`, `name`, `facing` (front, back or external),
    /// `role` (main, ultrawide, telephoto, auxiliary, or empty), its frame
    /// upright (`width`, `height`), whether it has a `lens` to focus and
    /// whether it is `tuned`.
    #[zbus(property)]
    fn cameras(&self) -> Vec<HashMap<String, OwnedValue>> {
        session::cameras(false)
            .iter()
            .map(|camera| {
                let (width, height) = camera.upright();
                HashMap::from([
                    ("id".to_string(), owned(camera.id.as_str())),
                    ("name".to_string(), owned(camera.name.as_str())),
                    ("facing".to_string(), owned(camera.facing.name())),
                    (
                        "role".to_string(),
                        owned(camera.role.map_or("", |role| role.name())),
                    ),
                    ("width".to_string(), owned(width)),
                    ("height".to_string(), owned(height)),
                    ("lens".to_string(), owned(camera.lens.is_some())),
                    ("tuned".to_string(), owned(camera.tuned)),
                ])
            })
            .collect()
    }
}

/// Whoever may open the camera nodes may connect: root, the `video` group,
/// and the user of an active local session, as uaccess has it.
async fn allowed(connection: &zbus::Connection, header: &Header<'_>) -> fdo::Result<()> {
    let sender = header
        .sender()
        .ok_or_else(|| fdo::Error::AccessDenied("no sender".into()))?;
    let credentials = fdo::DBusProxy::new(connection)
        .await?
        .get_connection_credentials(sender.clone().into())
        .await?;
    if credentials.unix_user_id() == Some(0) {
        return Ok(());
    }
    if let (Some(video), Some(groups)) = (video_group(), credentials.unix_group_ids())
        && groups.contains(&video)
    {
        return Ok(());
    }
    if let Some(pid) = credentials.process_id()
        && active_local(connection, pid).await.unwrap_or(false)
    {
        return Ok(());
    }
    Err(fdo::Error::AccessDenied(
        "the cameras are for root, the video group and the active local session".into(),
    ))
}

fn video_group() -> Option<u32> {
    static VIDEO: OnceLock<Option<u32>> = OnceLock::new();
    *VIDEO.get_or_init(|| {
        // SAFETY: the first call only, under the OnceLock; the entry is read
        // before any other getgr* call.
        let group = unsafe { libc::getgrnam(c"video".as_ptr()) };
        (!group.is_null()).then(|| unsafe { (*group).gr_gid })
    })
}

/// The process is in logind's active session on a seat, not a remote one.
async fn active_local(connection: &zbus::Connection, pid: u32) -> zbus::Result<bool> {
    const LOGIN: &str = "org.freedesktop.login1";
    let manager = zbus::Proxy::new(
        connection,
        LOGIN,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let path: OwnedObjectPath = manager.call("GetSessionByPID", &(pid,)).await?;
    let session =
        zbus::Proxy::new(connection, LOGIN, path, "org.freedesktop.login1.Session").await?;
    let active: bool = session.get_property("Active").await?;
    let remote: bool = session.get_property("Remote").await?;
    Ok(active && !remote)
}

/// Serves the interface on the system bus for as long as the connection is
/// kept.
pub fn serve() -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::system()?
        .serve_at(PATH, Service)?
        .name(NAME)?
        .build()
}
