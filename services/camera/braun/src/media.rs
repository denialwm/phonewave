//! The media controller: a graph's entities, the device nodes they have and
//! the links between them (MEDIA_IOC_G_TOPOLOGY), and turning data links on
//! and off.

use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use crate::v4l2::{ioctl, iowr, open};

/// Entity functions.
pub const ENT_F_CAM_SENSOR: u32 = 0x0002_0001;
pub const ENT_F_LENS: u32 = 0x0002_0003;

const LNK_FL_ENABLED: u32 = 1;
const LNK_FL_LINK_TYPE: u32 = 0xf << 28;
const LNK_FL_DATA_LINK: u32 = 0;
const LNK_FL_INTERFACE_LINK: u32 = 1 << 28;
const LNK_FL_ANCILLARY_LINK: u32 = 2 << 28;
const PAD_FL_SINK: u32 = 1;

#[repr(C)]
struct DeviceInfo {
    driver: [u8; 16],
    model: [u8; 32],
    serial: [u8; 40],
    bus_info: [u8; 32],
    media_version: u32,
    hw_revision: u32,
    driver_version: u32,
    reserved: [u32; 31],
}

#[repr(C)]
#[derive(Default)]
struct Topology {
    topology_version: u64,
    num_entities: u32,
    reserved1: u32,
    ptr_entities: u64,
    num_interfaces: u32,
    reserved2: u32,
    ptr_interfaces: u64,
    num_pads: u32,
    reserved3: u32,
    ptr_pads: u64,
    num_links: u32,
    reserved4: u32,
    ptr_links: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V2Entity {
    id: u32,
    name: [u8; 64],
    function: u32,
    flags: u32,
    reserved: [u32; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V2Interface {
    id: u32,
    intf_type: u32,
    flags: u32,
    reserved: [u32; 9],
    major: u32,
    minor: u32,
    raw: [u32; 14],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct V2Pad {
    id: u32,
    entity_id: u32,
    flags: u32,
    index: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct V2Link {
    id: u32,
    source_id: u32,
    sink_id: u32,
    flags: u32,
    reserved: [u32; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PadDesc {
    entity: u32,
    index: u16,
    flags: u32,
    reserved: [u32; 2],
}

#[repr(C)]
struct LinkDesc {
    source: PadDesc,
    sink: PadDesc,
    flags: u32,
    reserved: [u32; 2],
}

const MEDIA_IOC_DEVICE_INFO: libc::c_ulong = iowr(b'|', 0x00, size_of::<DeviceInfo>());
const MEDIA_IOC_SETUP_LINK: libc::c_ulong = iowr(b'|', 0x03, size_of::<LinkDesc>());
const MEDIA_IOC_G_TOPOLOGY: libc::c_ulong = iowr(b'|', 0x04, size_of::<Topology>());

/// An entity of the graph, with its device node, if it has one.
#[derive(Clone, Debug)]
pub struct Entity {
    pub id: u32,
    pub name: String,
    pub function: u32,
    pub node: Option<PathBuf>,
}

struct Pad {
    id: u32,
    entity: u32,
    sink: bool,
}

struct Link {
    source: u32,
    sink: u32,
    flags: u32,
}

pub struct Graph {
    media: File,
    /// The driver that made the graph: which pipeline it is.
    pub driver: String,
    pub entities: Vec<Entity>,
    pads: Vec<Pad>,
    links: Vec<Link>,
}

fn text(bytes: &[u8]) -> String {
    CStr::from_bytes_until_nul(bytes)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The `/dev` node of character device `major:minor`.
fn node_of(major: u32, minor: u32) -> Option<PathBuf> {
    if major == 0 && minor == 0 {
        return None;
    }
    let link = PathBuf::from(format!("/dev/char/{major}:{minor}"));
    if let Ok(path) = std::fs::canonicalize(&link) {
        return Some(path);
    }
    // No /dev/char links: look for the node itself.
    let wanted = libc::makedev(major, minor);
    std::fs::read_dir("/dev").ok()?.flatten().find_map(|entry| {
        use std::os::unix::fs::MetadataExt;
        let meta = entry.metadata().ok()?;
        (meta.file_type().is_char_device() && meta.rdev() == wanted).then(|| entry.path())
    })
}

/// The media devices there are, in order.
pub fn devices() -> Vec<PathBuf> {
    let mut devices: Vec<(u32, PathBuf)> = std::fs::read_dir("/dev")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name();
                    let index = name.to_str()?.strip_prefix("media")?.parse().ok()?;
                    Some((index, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    devices.sort();
    devices.into_iter().map(|(_, path)| path).collect()
}

impl Graph {
    pub fn open(path: &Path) -> io::Result<Self> {
        let media = open(path)?;
        // SAFETY: all-zero is a valid DeviceInfo.
        let mut info: DeviceInfo = unsafe { std::mem::zeroed() };
        ioctl(&media, MEDIA_IOC_DEVICE_INFO, &mut info)?;
        // Once for the sizes, then again for the elements; the graph may have
        // grown between the two, so ask until it holds still.
        let (entities, interfaces, pads, links) = loop {
            let mut topology = Topology::default();
            ioctl(&media, MEDIA_IOC_G_TOPOLOGY, &mut topology)?;
            let version = topology.topology_version;
            // SAFETY: all-zero is valid for these plain structs.
            let mut entities =
                vec![unsafe { std::mem::zeroed::<V2Entity>() }; topology.num_entities as usize];
            let mut interfaces = vec![
                unsafe { std::mem::zeroed::<V2Interface>() };
                topology.num_interfaces as usize
            ];
            let mut pads = vec![V2Pad::default(); topology.num_pads as usize];
            let mut links = vec![V2Link::default(); topology.num_links as usize];
            topology.ptr_entities = entities.as_mut_ptr() as u64;
            topology.ptr_interfaces = interfaces.as_mut_ptr() as u64;
            topology.ptr_pads = pads.as_mut_ptr() as u64;
            topology.ptr_links = links.as_mut_ptr() as u64;
            ioctl(&media, MEDIA_IOC_G_TOPOLOGY, &mut topology)?;
            if topology.topology_version == version {
                break (entities, interfaces, pads, links);
            }
        };
        let node_of_entity = |entity: u32| {
            links
                .iter()
                .filter(|link| {
                    link.flags & LNK_FL_LINK_TYPE == LNK_FL_INTERFACE_LINK && link.sink_id == entity
                })
                .find_map(|link| {
                    interfaces
                        .iter()
                        .find(|interface| interface.id == link.source_id)
                })
                .and_then(|interface| node_of(interface.major, interface.minor))
        };
        Ok(Self {
            driver: text(&info.driver),
            entities: entities
                .iter()
                .map(|entity| Entity {
                    id: entity.id,
                    name: text(&entity.name),
                    function: entity.function,
                    node: node_of_entity(entity.id),
                })
                .collect(),
            pads: pads
                .iter()
                .map(|pad| Pad {
                    id: pad.id,
                    entity: pad.entity_id,
                    sink: pad.flags & PAD_FL_SINK != 0,
                })
                .collect(),
            links: links
                .iter()
                .filter(|link| link.flags & LNK_FL_LINK_TYPE != LNK_FL_INTERFACE_LINK)
                .map(|link| Link {
                    source: link.source_id,
                    sink: link.sink_id,
                    flags: link.flags,
                })
                .collect(),
            media,
        })
    }

    pub fn entity(&self, name: &str) -> io::Result<&Entity> {
        self.entities
            .iter()
            .find(|entity| entity.name == name)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no {name} in the media graph"),
                )
            })
    }

    fn by_id(&self, id: u32) -> Option<&Entity> {
        self.entities.iter().find(|entity| entity.id == id)
    }

    fn pad(&self, id: u32) -> Option<&Pad> {
        self.pads.iter().find(|pad| pad.id == id)
    }

    /// The entities `entity`'s source pads send to.
    pub fn sinks_of(&self, entity: &Entity) -> Vec<&Entity> {
        self.links
            .iter()
            .filter(|link| link.flags & LNK_FL_LINK_TYPE == LNK_FL_DATA_LINK)
            .filter(|link| {
                self.pad(link.source)
                    .is_some_and(|pad| pad.entity == entity.id && !pad.sink)
            })
            .filter_map(|link| self.pad(link.sink).and_then(|pad| self.by_id(pad.entity)))
            .collect()
    }

    /// The entities that serve `entity`, such as its lens.
    pub fn ancillaries_of(&self, entity: &Entity) -> Vec<&Entity> {
        self.links
            .iter()
            .filter(|link| {
                link.flags & LNK_FL_LINK_TYPE == LNK_FL_ANCILLARY_LINK && link.source == entity.id
            })
            .filter_map(|link| self.by_id(link.sink))
            .collect()
    }

    /// Turns the data link from `source`'s pad `source_pad` to `sink`'s pad
    /// `sink_pad` on or off. Turning off a link the graph does not have
    /// succeeds: it is off.
    pub fn link(
        &self,
        source: &str,
        source_pad: u16,
        sink: &str,
        sink_pad: u16,
        on: bool,
    ) -> io::Result<()> {
        let (Ok(source), Ok(sink)) = (self.entity(source), self.entity(sink)) else {
            return if on {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "link endpoint missing",
                ))
            } else {
                Ok(())
            };
        };
        let mut desc = LinkDesc {
            source: PadDesc {
                entity: source.id,
                index: source_pad,
                ..Default::default()
            },
            sink: PadDesc {
                entity: sink.id,
                index: sink_pad,
                ..Default::default()
            },
            flags: if on { LNK_FL_ENABLED } else { 0 },
            reserved: [0; 2],
        };
        match ioctl(&self.media, MEDIA_IOC_SETUP_LINK, &mut desc) {
            Err(error) if !on && error.raw_os_error() == Some(libc::EINVAL) => Ok(()),
            result => result.map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("linking {} to {}: {error}", source.name, sink.name),
                )
            }),
        }
    }

    /// Opens `name`'s device node.
    pub fn node(&self, name: &str) -> io::Result<File> {
        let entity = self.entity(name)?;
        let path = entity.node.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{name} has no device node"),
            )
        })?;
        open(path)
    }
}

const _: () = assert!(size_of::<DeviceInfo>() == 256);
const _: () = assert!(size_of::<Topology>() == 72);
const _: () = assert!(size_of::<V2Entity>() == 96);
const _: () = assert!(size_of::<V2Interface>() == 112);
const _: () = assert!(size_of::<V2Pad>() == 32);
const _: () = assert!(size_of::<V2Link>() == 40);
const _: () = assert!(size_of::<LinkDesc>() == 52);
