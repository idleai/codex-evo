//! Local configuration plus the app-server's opaque workspace paths.
//!
//! A remote server can use a different path convention from the TUI. Keep its
//! paths out of core's native `AbsolutePathBuf` configuration and only localize
//! them when starting a local execution backend.

use codex_app_server_client::legacy_core::config as native;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::ops::Deref;
use std::ops::DerefMut;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    local: native::Config,
    pub cwd: PathBuf,
    pub workspace_roots: Vec<PathBuf>,
}

impl From<native::Config> for Config {
    fn from(local: native::Config) -> Self {
        Self {
            cwd: local.cwd.to_path_buf(),
            workspace_roots: local
                .workspace_roots
                .iter()
                .map(AbsolutePathBuf::to_path_buf)
                .collect(),
            local,
        }
    }
}

impl Deref for Config {
    type Target = native::Config;

    fn deref(&self) -> &Self::Target {
        &self.local
    }
}

impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.local
    }
}

impl Config {
    /// Configuration for a local executor. Foreign paths must never be resolved
    /// against the TUI's working directory or passed to its filesystem.
    pub fn native_config(&self) -> std::io::Result<native::Config> {
        let mut local = self.local.clone();
        local.cwd = AbsolutePathBuf::from_absolute_path_checked(&self.cwd)?;
        local.workspace_roots = self.native_workspace_roots()?;
        local
            .permissions
            .set_workspace_roots(local.workspace_roots.clone());
        Ok(local)
    }

    pub fn native_workspace_roots(&self) -> std::io::Result<Vec<AbsolutePathBuf>> {
        self.workspace_roots
            .iter()
            .map(AbsolutePathBuf::from_absolute_path_checked)
            .collect()
    }

    pub fn local_config(&self) -> &native::Config {
        &self.local
    }

    pub fn local_cwd(&self) -> &AbsolutePathBuf {
        &self.local.cwd
    }

    /// Native roots are only a compatibility projection; remote roots remain in
    /// `workspace_roots` and the server's canonical permission profile.
    pub fn sync_native_workspace_roots(&mut self) {
        let roots = self.native_workspace_roots().unwrap_or_default();
        self.local.permissions.set_workspace_roots(roots);
    }

    #[expect(
        clippy::expect_used,
        reason = "cwd is validated by native config loading or server-path deserialization"
    )]
    pub fn cwd_uri(&self) -> codex_utils_path_uri::PathUri {
        crate::remote_paths::path_uri(&self.cwd)
            .expect("configuration cwd must be an absolute server path")
    }

    pub fn effective_workspace_roots(&self) -> Vec<codex_utils_path_uri::PathUri> {
        let mut roots = self
            .workspace_roots
            .iter()
            .filter_map(|root| crate::remote_paths::path_uri(root).ok())
            .collect::<Vec<_>>();
        roots.extend(
            self.permissions
                .profile_workspace_roots()
                .iter()
                .map(|root| root.as_uri().clone()),
        );
        let mut seen = std::collections::HashSet::new();
        roots.retain(|root| seen.insert(root.to_string()));
        roots
    }

    #[cfg(test)]
    pub fn legacy_sandbox_policy(&self) -> codex_protocol::protocol::SandboxPolicy {
        self.permissions.legacy_sandbox_policy(self.cwd.as_path())
    }

    #[cfg(test)]
    pub async fn load_default_with_cli_overrides_for_codex_home(
        codex_home: PathBuf,
        cli_overrides: Vec<(String, toml::Value)>,
    ) -> std::io::Result<Self> {
        native::Config::load_default_with_cli_overrides_for_codex_home(codex_home, cli_overrides)
            .await
            .map(Into::into)
    }
}

#[derive(Clone, Default)]
pub struct ConfigBuilder(native::ConfigBuilder);

impl ConfigBuilder {
    pub fn codex_home(self, value: PathBuf) -> Self {
        Self(self.0.codex_home(value))
    }
    pub fn cli_overrides(self, value: Vec<(String, toml::Value)>) -> Self {
        Self(self.0.cli_overrides(value))
    }
    pub fn harness_overrides(self, value: native::ConfigOverrides) -> Self {
        Self(self.0.harness_overrides(value))
    }
    pub fn loader_overrides(self, value: LoaderOverrides) -> Self {
        Self(self.0.loader_overrides(value))
    }
    pub fn strict_config(self, value: bool) -> Self {
        Self(self.0.strict_config(value))
    }
    pub fn cloud_config_bundle(self, value: CloudConfigBundleLoader) -> Self {
        Self(self.0.cloud_config_bundle(value))
    }
    pub fn fallback_cwd(self, value: Option<PathBuf>) -> Self {
        Self(self.0.fallback_cwd(value))
    }
    pub async fn build(self) -> std::io::Result<Config> {
        self.0.build().await.map(Into::into)
    }
}
