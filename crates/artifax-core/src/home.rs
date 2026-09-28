//! Layout of the `~/.artifax` directory.

use crate::ids::ArtifactId;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn at(root: PathBuf) -> Self {
        Home { root }
    }

    /// `$ARTIFAX_HOME`, else `$HOME/.artifax`.
    pub fn from_env() -> Self {
        let ax = std::env::var("ARTIFAX_HOME").ok();
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self::from_env_with(ax.as_deref(), &home)
    }

    pub fn from_env_with(artifax_home: Option<&str>, home: &str) -> Self {
        match artifax_home.filter(|s| !s.is_empty()) {
            Some(p) => Home::at(PathBuf::from(p)),
            None => Home::at(Path::new(home).join(".artifax")),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join("artifax.db")
    }
    pub fn daemon_json(&self) -> PathBuf {
        self.root.join("daemon.json")
    }
    pub fn daemon_lock(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }
    pub fn log_path(&self) -> PathBuf {
        self.root.join("logs").join("daemon.log")
    }
    pub fn artifact_dir(&self, id: &ArtifactId) -> PathBuf {
        self.root.join("artifacts").join(id.as_str())
    }
    pub fn version_dir(&self, id: &ArtifactId, n: u32) -> PathBuf {
        self.artifact_dir(id).join("versions").join(n.to_string())
    }
    pub fn assets_dir(&self, id: &ArtifactId) -> PathBuf {
        self.artifact_dir(id).join("assets")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.root.join("logs"))?;
        std::fs::create_dir_all(self.root.join("artifacts"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ArtifactId;

    #[test]
    fn paths_hang_off_root() {
        let home = Home::at("/tmp/ax".into());
        let id = ArtifactId::parse("7q3k9mzx2b4t").unwrap();
        assert_eq!(home.db_path(), PathBuf::from("/tmp/ax/artifax.db"));
        assert_eq!(home.daemon_json(), PathBuf::from("/tmp/ax/daemon.json"));
        assert_eq!(home.daemon_lock(), PathBuf::from("/tmp/ax/daemon.lock"));
        assert_eq!(home.log_path(), PathBuf::from("/tmp/ax/logs/daemon.log"));
        assert_eq!(
            home.artifact_dir(&id),
            PathBuf::from("/tmp/ax/artifacts/7q3k9mzx2b4t")
        );
        assert_eq!(
            home.version_dir(&id, 3),
            PathBuf::from("/tmp/ax/artifacts/7q3k9mzx2b4t/versions/3")
        );
        assert_eq!(
            home.assets_dir(&id),
            PathBuf::from("/tmp/ax/artifacts/7q3k9mzx2b4t/assets")
        );
    }

    #[test]
    fn from_env_prefers_artifax_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_env_with(Some(dir.path().to_str().unwrap()), "/never");
        assert_eq!(home.root(), dir.path());
        let home = Home::from_env_with(None, "/home/x");
        assert_eq!(home.root(), Path::new("/home/x/.artifax"));
    }

    #[test]
    fn ensure_dirs_creates_root_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        assert!(home.root().is_dir());
        assert!(home.root().join("logs").is_dir());
        assert!(home.root().join("artifacts").is_dir());
    }
}
