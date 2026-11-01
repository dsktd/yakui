//! The spaces, opened on first use from `<dir>/<space>/`.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use crate::Result;
use crate::registry::Registry;
use crate::space::Space;
use crate::space::SpaceId;

#[derive(Clone)]
pub struct Host(Arc<Mutex<HostInner>>);
pub struct HostInner {
    dir: PathBuf,
    registry: Registry,
    spaces: HashMap<SpaceId, Space>,
}

impl Host {
    pub fn open(
        dir: impl Into<PathBuf>,
        registry: Registry,
    ) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let inner = HostInner {
            dir,
            registry,
            spaces: HashMap::new(),
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
        let space = Space::open(space_dir(&host.dir, id), host.registry.clone())?;
        host.spaces.insert(id.clone(), space.clone());
        Ok(space)
    }
    pub fn module(
        &self,
        kind: &str,
    ) -> Option<&'static str> {
        self.0.lock().unwrap().registry.module(kind)
    }
}

fn space_dir(
    dir: &Path,
    id: &SpaceId,
) -> PathBuf {
    dir.join(id.as_str())
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
}
