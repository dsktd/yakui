//! The spaces, opened on first use from `<dir>/<space>/`.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use log::info;
use pane_api::HostId;

use crate::Result;
use crate::registry::Registry;
use crate::space::Space;
use crate::space::SpaceId;

#[derive(Clone)]
pub struct Host(Arc<Mutex<HostInner>>);
pub struct HostInner {
    dir: PathBuf,
    id: HostId,
    registry: Registry,
    spaces: HashMap<SpaceId, Space>,
    /// Tickets of the spaces shared over iroh, by [`Host::share`].
    tickets: HashMap<SpaceId, String>,
}

impl Host {
    pub fn open(
        dir: impl Into<PathBuf>,
        registry: Registry,
    ) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        migrate(&dir)?;
        let id = host_id(&dir)?;
        let inner = HostInner {
            dir,
            id,
            registry,
            spaces: HashMap::new(),
            tickets: HashMap::new(),
        };
        Ok(Self(Arc::new(Mutex::new(inner))))
    }
    /// The space, loaded or made on its first use.
    pub fn space(
        &self,
        id: &SpaceId,
    ) -> Result<Space> {
        let mut host = self.0.lock().unwrap();
        if let Some(space) = host.spaces.get(id) {
            return Ok(space.clone());
        }
        let space = Space::open(space_dir(&host.dir, id), host.registry.clone(), host.id)?;
        host.spaces.insert(id.clone(), space.clone());
        Ok(space)
    }
    /// Open space `id` on an `IrohBus`: a new shared space with no ticket, else the one the
    /// ticket names. Fails if the space is already open on its local bus.
    #[cfg(feature = "iroh")]
    pub async fn share(
        &self,
        id: &SpaceId,
        ticket: Option<crate::iroh_bus::Ticket>,
        relays: crate::iroh_bus::Relays,
    ) -> Result<crate::iroh_bus::Ticket> {
        use crate::bus::Local;
        use crate::iroh_bus::IrohBus;

        let (dir, registry, host) = {
            let host = self.0.lock().unwrap();
            if host.spaces.contains_key(id) {
                return Err(anyhow!("space {id} is already open"));
            }
            (space_dir(&host.dir, id), host.registry.clone(), host.id)
        };
        std::fs::create_dir_all(&dir)?;
        // A joining host takes the shared copy: its own old one could win where its seq is
        // higher, and a seed of its own would race the shared scene.
        let local = match ticket {
            Some(_) => Local::fresh(dir.join("bus.json"), host),
            None => {
                let local = Local::open(dir.join("bus.json"), host)?;
                local.seed();
                local
            }
        };
        let bus = IrohBus::join(local, ticket, relays).await?;
        let ticket = bus.ticket().await;
        let space = Space::open_on(dir, registry, host, Arc::new(bus))?;
        let mut host = self.0.lock().unwrap();
        host.spaces.insert(id.clone(), space);
        host.tickets.insert(id.clone(), ticket.to_string());
        Ok(ticket)
    }
    /// Share every space whose `<space>/space.json` asks for an iroh bus, but `skip` (the
    /// `--share` space, whose flags win). Returns each one's ticket.
    #[cfg(feature = "iroh")]
    pub async fn share_configured(
        &self,
        skip: Option<&SpaceId>,
    ) -> Result<Vec<(SpaceId, crate::iroh_bus::Ticket)>> {
        let dir = self.0.lock().unwrap().dir.clone();
        let mut shared = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let Ok(id) = entry?.file_name().to_string_lossy().parse::<SpaceId>() else {
                continue;
            };
            if Some(&id) == skip {
                continue;
            }
            let Some((ticket, relays)) = BusConfig::read(&space_dir(&dir, &id))?.iroh()? else {
                continue;
            };
            let ticket = self.share(&id, ticket, relays).await?;
            info!("space {id} shared over iroh");
            shared.push((id, ticket));
        }
        Ok(shared)
    }
    /// The ticket of space `id`, if it is shared over iroh.
    pub fn ticket(
        &self,
        id: &SpaceId,
    ) -> Option<String> {
        self.0.lock().unwrap().tickets.get(id).cloned()
    }
    pub fn module(
        &self,
        kind: &str,
    ) -> Option<&'static str> {
        self.0.lock().unwrap().registry.module(kind)
    }
}

/// Which bus a space opens on, from `<space>/space.json`; no file is [`BusConfig::Local`].
#[cfg(feature = "iroh")]
#[derive(serde::Deserialize, Default, Debug, PartialEq)]
#[serde(tag = "bus", rename_all = "lowercase")]
enum BusConfig {
    #[default]
    Local,
    /// An `IrohBus`: a new shared space, or the one `join` names.
    Iroh {
        join: Option<String>,
        #[serde(default = "relays_on")]
        relays: bool,
    },
}
#[cfg(feature = "iroh")]
fn relays_on() -> bool {
    true
}
#[cfg(feature = "iroh")]
impl BusConfig {
    fn read(dir: &Path) -> Result<Self> {
        let path = dir.join("space.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|err| anyhow!("{}: {err}", path.display()))
            }
            Err(_) => Ok(Self::default()),
        }
    }
    /// The ticket to join and the relays, if the space is shared.
    fn iroh(self) -> Result<Option<(Option<crate::iroh_bus::Ticket>, crate::iroh_bus::Relays)>> {
        let Self::Iroh { join, relays } = self else {
            return Ok(None);
        };
        let ticket = join.map(|join| join.parse()).transpose()?;
        let relays = match relays {
            true => crate::iroh_bus::Relays::Default,
            false => crate::iroh_bus::Relays::Off,
        };
        Ok(Some((ticket, relays)))
    }
}

/// This host's id, made once and kept in `<dir>/host-id`; pane ids and bus messages carry it.
fn host_id(dir: &Path) -> Result<HostId> {
    let path = dir.join("host-id");
    if let Ok(text) = std::fs::read_to_string(&path) {
        return text.trim().parse().map_err(|err: String| anyhow!(err));
    }
    let id = HostId(rand::random());
    std::fs::write(&path, id.to_string())?;
    info!("new host id {id}");
    Ok(id)
}

fn space_dir(
    dir: &Path,
    id: &SpaceId,
) -> PathBuf {
    dir.join(id.as_str())
}

/// Before spaces, `<dir>/layout.json` and `<dir>/panes/` were the one layout: move them into
/// a new space and log its id, so the link to the old notes is in the log.
fn migrate(dir: &Path) -> Result<()> {
    let layout = dir.join("layout.json");
    if !layout.exists() {
        return Ok(());
    }
    let id = SpaceId::random();
    let space = space_dir(dir, &id);
    std::fs::create_dir_all(&space)?;
    std::fs::rename(&layout, space.join("layout.json"))?;
    let panes = dir.join("panes");
    if panes.exists() {
        std::fs::rename(&panes, space.join("panes"))?;
    }
    info!("moved the old layout and panes into space {id}");
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_space_dir() {
        let id: SpaceId = "0123456789abcdef".parse().unwrap();
        assert_eq!(
            space_dir(Path::new("/data"), &id),
            PathBuf::from("/data/0123456789abcdef")
        );
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn test_bus_config() {
        let parse = |json: &str| serde_json::from_str::<BusConfig>(json).unwrap();
        assert_eq!(parse(r#"{ "bus": "local" }"#), BusConfig::Local);
        assert_eq!(
            parse(r#"{ "bus": "iroh" }"#),
            BusConfig::Iroh {
                join: None,
                relays: true
            }
        );
        assert_eq!(
            parse(r#"{ "bus": "iroh", "relays": false }"#),
            BusConfig::Iroh {
                join: None,
                relays: false
            }
        );
        assert_eq!(
            BusConfig::read(Path::new("/nonexistent")).unwrap(),
            BusConfig::Local
        );
    }
}
