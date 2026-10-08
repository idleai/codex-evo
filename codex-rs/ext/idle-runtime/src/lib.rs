//! Daemon-owned Idle workspace bindings, independent of model and client lifetimes.

mod remote;
mod storage;

pub use remote::GrantAccess;
pub use remote::IssuedGrant;
pub use remote::RelayConfiguration;
pub use remote::Secret;

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use storage::Store;

/// Explicit local installation. Repository files never create this authorization.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBinding {
    pub workspace_id: String,
    pub repository_id: String,
    pub checkout_id: String,
    pub chain_id: String,
    pub checkout_root: PathBuf,
    pub chain_directory: PathBuf,
}

impl WorkspaceBinding {
    fn validate(&self) -> Result<(), Error> {
        for id in [
            &self.workspace_id,
            &self.repository_id,
            &self.checkout_id,
            &self.chain_id,
        ] {
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                return Err(Error::InvalidBinding);
            }
        }
        if !self.checkout_root.is_absolute()
            || !self.chain_directory.is_absolute()
            || self.checkout_root.to_str().is_none()
            || self.chain_directory.to_str().is_none()
        {
            return Err(Error::InvalidBinding);
        }
        Ok(())
    }

    fn canonicalize(mut self) -> Result<Self, Error> {
        self.validate()?;
        self.checkout_root = canonical_directory(&self.checkout_root)?;
        self.chain_directory = canonical_directory(&self.chain_directory)?;
        Ok(self)
    }

    fn available(&self) -> bool {
        canonical_directory(&self.checkout_root).is_ok_and(|path| path == self.checkout_root)
            && canonical_directory(&self.chain_directory)
                .is_ok_and(|path| path == self.chain_directory)
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_dir() {
        return Err(Error::InvalidBinding);
    }
    let path = path.canonicalize()?;
    if path.to_str().is_none() {
        return Err(Error::InvalidBinding);
    }
    Ok(path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub binding: WorkspaceBinding,
    /// False when a saved path is missing or now resolves somewhere else.
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeStatus {
    /// Codex's persisted installation identity, scoped to this CODEX_HOME.
    pub host_id: String,
    /// Changes on each app-server start; never substitutes for host identity.
    pub runtime_id: String,
    pub workspaces: Vec<WorkspaceStatus>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Idle workspace binding")]
    InvalidBinding,
    #[error("Idle workspace binding conflicts with an existing attachment")]
    Conflict,
    #[error("another app-server owns the Idle workspace registry")]
    Busy,
    #[error("Idle workspace registry is unavailable")]
    Unavailable,
    #[error("Idle workspace registry format is unsupported")]
    Version,
    #[error("Idle workspace service is stopping")]
    Stopped,
    #[error("Idle workspace limit reached")]
    Limit,
    #[error("Idle workspace access is not authorized")]
    Denied,
    #[error("Idle workspace connection grant has expired")]
    Expired,
    #[error("Idle workspace storage failed")]
    Io(#[from] std::io::Error),
    #[error("Idle workspace registry is invalid")]
    Json(#[from] serde_json::Error),
}

impl Error {
    fn status_error(&self) -> Self {
        match self {
            Self::Busy => Self::Busy,
            Self::Version => Self::Version,
            Self::Conflict => Self::Conflict,
            Self::Limit => Self::Limit,
            Self::InvalidBinding => Self::InvalidBinding,
            Self::Stopped => Self::Stopped,
            Self::Denied => Self::Denied,
            Self::Expired => Self::Expired,
            Self::Unavailable | Self::Io(_) | Self::Json(_) => Self::Unavailable,
        }
    }
}

#[derive(Debug)]
enum State {
    Dormant,
    Ready(Store),
    Failed(Error),
    Stopped,
}

/// One process owner. Dropping a client does not release its attachments.
#[derive(Debug)]
pub struct WorkspaceService {
    directory: PathBuf,
    host_id: String,
    runtime_id: String,
    state: Mutex<State>,
}

impl WorkspaceService {
    /// Restore saved attachments if present. Unused installations create no Idle files.
    /// Startup failure affects Idle RPCs without preventing ordinary Codex work.
    pub fn new(codex_home: &Path, installation_id: String) -> Self {
        let directory = codex_home.join("idle-runtime");
        let state = if directory.exists() {
            match Store::open(&directory, &installation_id) {
                Ok(store) => State::Ready(store),
                Err(error) => State::Failed(error),
            }
        } else {
            State::Dormant
        };
        Self {
            directory,
            host_id: installation_id,
            runtime_id: uuid::Uuid::new_v4().to_string(),
            state: Mutex::new(state),
        }
    }

    /// Attach an explicitly approved checkout. Only a trusted local caller may use this.
    /// Repeating the exact binding is idempotent; identities cannot silently move.
    pub fn attach(&self, binding: WorkspaceBinding) -> Result<RuntimeStatus, Error> {
        let binding = binding.canonicalize()?;
        let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
        if matches!(*state, State::Dormant) {
            *state = State::Ready(Store::open(&self.directory, &self.host_id)?);
        }
        let store = match &mut *state {
            State::Ready(store) => store,
            State::Stopped => return Err(Error::Stopped),
            State::Failed(error) => return Err(error.status_error()),
            State::Dormant => return Err(Error::Unavailable),
        };
        if let Err(error) = store.attach(binding) {
            // A failed write can have an uncertain durable outcome. Reload on restart.
            if matches!(error, Error::Io(_) | Error::Json(_)) {
                *state = State::Failed(Error::Unavailable);
            }
            return Err(error);
        }
        self.snapshot(&state)
    }

    pub fn status(&self) -> Result<RuntimeStatus, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
        // Another app-server may have claimed a previously unused installation.
        if matches!(*state, State::Dormant) && self.directory.exists() {
            *state = State::Ready(Store::open(&self.directory, &self.host_id)?);
        }
        self.snapshot(&state)
    }

    fn snapshot(&self, state: &State) -> Result<RuntimeStatus, Error> {
        let bindings = match state {
            State::Dormant => &[][..],
            State::Ready(store) => store.bindings(),
            State::Failed(error) => return Err(error.status_error()),
            State::Stopped => return Err(Error::Stopped),
        };
        Ok(RuntimeStatus {
            host_id: self.host_id.clone(),
            runtime_id: self.runtime_id.clone(),
            workspaces: bindings
                .iter()
                .map(|binding| WorkspaceStatus {
                    binding: binding.clone(),
                    available: binding.available(),
                })
                .collect(),
        })
    }

    /// Stop admission and release the exclusive registry owner after RPCs drain.
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = State::Stopped;
        }
    }

    fn edit<T>(&self, operation: impl FnOnce(&mut Store) -> Result<T, Error>) -> Result<T, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
        if matches!(*state, State::Dormant) {
            *state = State::Ready(Store::open(&self.directory, &self.host_id)?);
        }
        let result = match &mut *state {
            State::Ready(store) => operation(store),
            State::Failed(error) => return Err(error.status_error()),
            State::Stopped => return Err(Error::Stopped),
            State::Dormant => return Err(Error::Unavailable),
        };
        if matches!(&result, Err(Error::Io(_) | Error::Json(_))) {
            *state = State::Failed(Error::Unavailable);
        }
        result
    }

    fn read<T>(&self, operation: impl FnOnce(&Store) -> Result<T, Error>) -> Result<T, Error> {
        let state = self.state.lock().map_err(|_| Error::Unavailable)?;
        match &*state {
            State::Ready(store) => operation(store),
            State::Failed(error) => Err(error.status_error()),
            State::Stopped => Err(Error::Stopped),
            State::Dormant => Err(Error::Denied),
        }
    }
}
