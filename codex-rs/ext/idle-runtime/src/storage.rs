use crate::Error;
use crate::WorkspaceBinding;
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registry {
    version: u32,
    host_id: String,
    bindings: Vec<WorkspaceBinding>,
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
                registry
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Registry {
                version: 1,
                host_id: host_id.to_string(),
                bindings: Vec::new(),
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
        self.registry.bindings.push(binding);
        let bytes = serde_json::to_vec(&self.registry)?;
        if bytes.len() as u64 > MAX_BYTES {
            self.registry.bindings.pop();
            return Err(Error::Limit);
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join("workspaces.json"))
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
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
