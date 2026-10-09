use crate::Error;
use crate::GrantAccess;
use crate::RelayConfiguration;
use crate::WorkspaceBinding;
use crate::remote::StoredGrant;
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_BINDINGS: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registry {
    version: u32,
    host_id: String,
    bindings: Vec<WorkspaceBinding>,
    #[serde(default)]
    relay: Option<RelayConfiguration>,
    #[serde(default)]
    coordination_helper: Option<PathBuf>,
    #[serde(default)]
    grants: Vec<StoredGrant>,
}

#[derive(Debug)]
pub(crate) struct Store {
    directory: PathBuf,
    registry: Registry,
    _owner: File,
}

impl Store {
    pub(crate) fn open(directory: &Path, host_id: &str) -> Result<Self, Error> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
        check_private_path(directory, true)?;
        let lock_path = directory.join("owner.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if lock_path.symlink_metadata().is_ok() {
            check_private_path(&lock_path, false)?;
        }
        let owner = options.open(lock_path)?;
        owner.try_lock().map_err(|_| Error::Busy)?;
        let registry_path = directory.join("workspaces.json");
        let registry = match registry_path.symlink_metadata() {
            Ok(_) => {
                check_private_path(&registry_path, false)?;
                let mut bytes = Vec::new();
                File::open(&registry_path)?
                    .take(MAX_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err(Error::Limit);
                }
                let registry: Registry = serde_json::from_slice(&bytes)?;
                if registry.version != 1 {
                    return Err(Error::Version);
                }
                if registry.host_id != host_id || registry.bindings.len() > MAX_BINDINGS {
                    return Err(Error::Conflict);
                }
                for (index, binding) in registry.bindings.iter().enumerate() {
                    binding.validate()?;
                    if registry.bindings[..index]
                        .iter()
                        .any(|other| overlaps(other, binding))
                    {
                        return Err(Error::Conflict);
                    }
                }
                if registry.grants.len() > 256 {
                    return Err(Error::Limit);
                }
                for (index, grant) in registry.grants.iter().enumerate() {
                    grant.validate()?;
                    if !registry
                        .bindings
                        .iter()
                        .any(|binding| binding.checkout_id == grant.checkout_id)
                        || registry.grants[..index]
                            .iter()
                            .any(|other| other.id == grant.id)
                    {
                        return Err(Error::Conflict);
                    }
                }
                registry
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Registry {
                version: 1,
                host_id: host_id.to_string(),
                bindings: Vec::new(),
                relay: None,
                coordination_helper: None,
                grants: Vec::new(),
            },
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            directory: directory.to_path_buf(),
            registry,
            _owner: owner,
        })
    }

    pub(crate) fn bindings(&self) -> &[WorkspaceBinding] {
        &self.registry.bindings
    }

    pub(crate) fn attach(&mut self, binding: WorkspaceBinding) -> Result<(), Error> {
        if self.registry.bindings.contains(&binding) {
            return Ok(());
        }
        if self
            .registry
            .bindings
            .iter()
            .any(|existing| overlaps(existing, &binding))
        {
            return Err(Error::Conflict);
        }
        if self.registry.bindings.len() >= MAX_BINDINGS {
            return Err(Error::Limit);
        }
        let mut registry = self.registry.clone();
        registry.bindings.push(binding);
        self.save(registry)
    }

    pub(crate) fn relay_configuration(&self) -> Option<&RelayConfiguration> {
        self.registry.relay.as_ref()
    }

    pub(crate) fn configure_relay(
        &mut self,
        configuration: Option<RelayConfiguration>,
    ) -> Result<(), Error> {
        let mut registry = self.registry.clone();
        if let Some(configuration) = &configuration {
            registry.coordination_helper = Some(configuration.helper_path.clone());
        }
        registry.relay = configuration;
        self.save(registry)
    }

    pub(crate) fn coordination_helper(&self) -> Option<&PathBuf> {
        self.registry.coordination_helper.as_ref().or_else(|| {
            self.registry
                .relay
                .as_ref()
                .map(|configuration| &configuration.helper_path)
        })
    }

    pub(crate) fn grant(&self, id: &str) -> Option<&StoredGrant> {
        self.registry.grants.iter().find(|grant| grant.id == id)
    }

    pub(crate) fn grant_access(&self, grant: &StoredGrant, now: u64) -> Result<GrantAccess, Error> {
        if grant.expires_at <= now {
            return Err(Error::Expired);
        }
        let binding = self
            .registry
            .bindings
            .iter()
            .find(|binding| binding.checkout_id == grant.checkout_id)
            .ok_or(Error::Denied)?;
        Ok(GrantAccess {
            grant_id: grant.id.clone(),
            client_id: grant.client_id.clone(),
            binding: binding.clone(),
            expires_at: grant.expires_at,
            scope: grant.scope,
        })
    }

    pub(crate) fn issue_grant(
        &mut self,
        grant: StoredGrant,
        now: u64,
    ) -> Result<GrantAccess, Error> {
        let access = self.grant_access(&grant, now)?;
        let mut registry = self.registry.clone();
        registry.grants.retain(|grant| grant.expires_at > now);
        if registry.grants.len() >= 256 {
            return Err(Error::Limit);
        }
        registry.grants.push(grant);
        self.save(registry)?;
        Ok(access)
    }

    pub(crate) fn revoke_grant(&mut self, id: &str) -> Result<(), Error> {
        let mut registry = self.registry.clone();
        registry.grants.retain(|grant| grant.id != id);
        self.save(registry)
    }

    fn save(&mut self, registry: Registry) -> Result<(), Error> {
        let bytes = serde_json::to_vec(&registry)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::Limit);
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join("workspaces.json"))
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        self.registry = registry;
        Ok(())
    }
}

fn overlaps(left: &WorkspaceBinding, right: &WorkspaceBinding) -> bool {
    left.checkout_id == right.checkout_id
        || left.checkout_root == right.checkout_root
        || left.chain_directory == right.chain_directory
        || (left.workspace_id == right.workspace_id && left.chain_id != right.chain_id)
}

fn check_private_path(path: &Path, directory: bool) -> Result<(), Error> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(Error::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Unavailable);
        }
    }
    Ok(())
}
