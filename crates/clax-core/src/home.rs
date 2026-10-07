//! Layout of the `~/.clax` directory.

use crate::ids::ArtifactId;
use crate::{CoreError, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn at(root: PathBuf) -> Self {
        Home { root }
    }

    /// `$CLAX_HOME`, else `$HOME/.clax`; an empty variable counts as unset.
    ///
    /// # Errors
    /// `Invalid { code: "no_home" }` when neither variable is set.
    pub fn from_env() -> Result<Self> {
        let ax = std::env::var("CLAX_HOME").ok();
        let home = std::env::var("HOME").ok();
        Self::from_env_with(ax.as_deref(), home.as_deref())
    }

    /// [`Home::from_env`] with the variable values passed in.
    pub fn from_env_with(clax_home: Option<&str>, home: Option<&str>) -> Result<Self> {
        let set = |v: Option<&str>| v.filter(|s| !s.is_empty()).map(PathBuf::from);
        match (set(clax_home), set(home)) {
            (Some(p), _) => Ok(Home::at(p)),
            (None, Some(h)) => Ok(Home::at(h.join(".clax"))),
            (None, None) => Err(CoreError::invalid(
                "no_home",
                "neither CLAX_HOME nor HOME is set",
            )),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join("clax.db")
    }
    pub fn daemon_json(&self) -> PathBuf {
        self.root.join("daemon.json")
    }
    /// `starting.json`: what a daemon that has not yet written
    /// `daemon.json` is doing (opening the store, recording the audit
    /// backfill), for the client that started it and `clax status`.
    pub fn starting_json(&self) -> PathBuf {
        self.root.join("starting.json")
    }
    pub fn daemon_lock(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }
    pub fn log_path(&self) -> PathBuf {
        self.root.join("logs").join("daemon.log")
    }
    /// `logs/hooks.log`: one line per hook run and per failed launcher resolution.
    pub fn hooks_log_path(&self) -> PathBuf {
        self.root.join("logs").join("hooks.log")
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
    /// `artifacts/<aid>/clips`: the clip images of the artifact's threads.
    pub fn clips_dir(&self, id: &ArtifactId) -> PathBuf {
        self.artifact_dir(id).join("clips")
    }
    /// `artifacts/<aid>/clips/<tid>.png`: the clip image of thread `thread_id`.
    pub fn clip_path(&self, id: &ArtifactId, thread_id: &str) -> PathBuf {
        self.clips_dir(id).join(format!("{thread_id}.png"))
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.root)?;
        }
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
        assert_eq!(home.db_path(), PathBuf::from("/tmp/ax/clax.db"));
        assert_eq!(home.daemon_json(), PathBuf::from("/tmp/ax/daemon.json"));
        assert_eq!(home.daemon_lock(), PathBuf::from("/tmp/ax/daemon.lock"));
        assert_eq!(home.log_path(), PathBuf::from("/tmp/ax/logs/daemon.log"));
        assert_eq!(
            home.hooks_log_path(),
            PathBuf::from("/tmp/ax/logs/hooks.log")
        );
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
    fn from_env_prefers_clax_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_env_with(Some(dir.path().to_str().unwrap()), Some("/never")).unwrap();
        assert_eq!(home.root(), dir.path());
        let home = Home::from_env_with(None, Some("/home/x")).unwrap();
        assert_eq!(home.root(), Path::new("/home/x/.clax"));
        let home = Home::from_env_with(Some(""), Some("/home/x")).unwrap();
        assert_eq!(home.root(), Path::new("/home/x/.clax"));
    }

    #[test]
    fn from_env_fails_without_clax_home_or_home() {
        for (ax, home) in [(None, None), (Some(""), Some("")), (None, Some(""))] {
            let e = Home::from_env_with(ax, home).unwrap_err();
            assert_eq!(e.to_string(), "neither CLAX_HOME nor HOME is set");
        }
    }

    #[test]
    fn ensure_dirs_creates_root_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        home.ensure_dirs().unwrap();
        assert!(home.root().is_dir());
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.root()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "newly created root is private");
        }
        assert!(home.root().join("logs").is_dir());
        assert!(home.root().join("artifacts").is_dir());
    }
}
