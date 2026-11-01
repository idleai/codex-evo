use crate::Error;
use crate::GrantAccess;
use crate::GrantScope;
use crate::RunnerPolicy;
use crate::WorkspaceBinding;
use crate::WorkspaceService;
use crate::storage::check_private_path;
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;

/// The executor belongs to the selected compute daemon. Other environments need
/// a separately authorized path mapping before they can host an Idle runner.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunnerEnvironment {
    Local,
}

/// Immutable private binding written before native thread construction.
/// It contains no bearer credential and is retained after grant revocation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerBinding {
    pub thread_id: String,
    pub workspace: WorkspaceBinding,
    pub client_id: String,
    pub grant_id: String,
    pub environment: RunnerEnvironment,
    pub policy: RunnerPolicy,
    pub parent: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedRunner {
    version: u32,
    binding: RunnerBinding,
}

impl WorkspaceService {
    /// Read a thread marker independently of the workspace registry. A broken
    /// marker denies that thread without changing healthy unbound Codex threads.
    pub fn runner(&self, thread_id: &str) -> Result<Option<RunnerBinding>, Error> {
        let path = self.runner_path(thread_id)?;
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        check_private_path(&self.directory, /*directory*/ true)?;
        check_private_path(&path, /*directory*/ false)?;
        let mut bytes = Vec::new();
        File::open(path)?
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 64 * 1024 {
            return Err(Error::Limit);
        }
        let saved: SavedRunner = serde_json::from_slice(&bytes)?;
        if saved.version != 1 || saved.binding.thread_id != thread_id {
            return Err(Error::Version);
        }
        self.runner_path(&saved.binding.thread_id)?;
        if let Some(parent) = &saved.binding.parent {
            self.runner_path(parent)?;
            if parent == thread_id {
                return Err(Error::Conflict);
            }
        }
        saved.binding.workspace.validate()?;
        saved.binding.policy.validate(&saved.binding.workspace)?;
        Ok(Some(saved.binding))
    }

    /// Current private grant checks still apply after editor detach and on reload.
    pub fn runner_access(&self, binding: &RunnerBinding) -> Result<GrantAccess, Error> {
        let access = self.grant_access(&binding.grant_id)?;
        if access.scope != GrantScope::Runner
            || access.client_id != binding.client_id
            || access.binding != binding.workspace
        {
            return Err(Error::Denied);
        }
        let ceiling = access.runner_policy.as_ref().ok_or(Error::Denied)?;
        ceiling.validate(&binding.workspace)?;
        binding.policy.validate(&binding.workspace)?;
        if binding.policy.intersect(ceiling) != binding.policy {
            return Err(Error::Denied);
        }
        Ok(access)
    }

    /// Persist exactly once; an existing native thread can never change workspace,
    /// contributor, executor or permission ceiling through resume or fork.
    pub fn bind_runner(&self, binding: RunnerBinding) -> Result<(), Error> {
        self.runner_access(&binding)?;
        if let Some(existing) = self.runner(&binding.thread_id)? {
            return if existing == binding {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        if let Some(parent_id) = &binding.parent {
            let parent = self.runner(parent_id)?.ok_or(Error::Denied)?;
            if parent.workspace != binding.workspace
                || parent.client_id != binding.client_id
                || parent.grant_id != binding.grant_id
                || parent.environment != binding.environment
                || binding.policy.intersect(&parent.policy) != binding.policy
            {
                return Err(Error::Denied);
            }
        }
        let path = self.runner_path(&binding.thread_id)?;
        // Hold the process owner through the write; uncertain writes fault the
        // workspace service, while the marker itself remains a resume barrier.
        self.edit(|_store| {
            if std::fs::read_dir(&self.directory)?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("runner-"))
                .take(4096)
                .count()
                >= 4096
            {
                return Err(Error::Limit);
            }
            let bytes = serde_json::to_vec(&SavedRunner {
                version: 1,
                binding,
            })?;
            if bytes.len() > 64 * 1024 {
                return Err(Error::Limit);
            }
            let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist_noclobber(path).map_err(|error| error.error)?;
            #[cfg(unix)]
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })
    }

    fn runner_path(&self, thread_id: &str) -> Result<PathBuf, Error> {
        let id = uuid::Uuid::parse_str(thread_id).map_err(|_| Error::InvalidBinding)?;
        if id.to_string() != thread_id {
            return Err(Error::InvalidBinding);
        }
        Ok(self.directory.join(format!("runner-{id}.json")))
    }
}
