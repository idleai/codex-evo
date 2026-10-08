use crate::Error;
use crate::WorkspaceBinding;
use crate::WorkspaceService;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Owner-selected executables. Configuration never comes from repository files.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayConfiguration {
    pub helper_path: PathBuf,
    pub credential_program: PathBuf,
}

impl RelayConfiguration {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        for path in [&self.helper_path, &self.credential_program] {
            if !path.is_absolute() || path.to_str().is_none() || !path.is_file() {
                return Err(Error::InvalidBinding);
            }
        }
        Ok(())
    }
}

/// Private credential. Debug output never exposes its contents.
#[derive(Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret([redacted])")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StoredGrant {
    pub(crate) id: String,
    pub(crate) client_id: String,
    pub(crate) checkout_id: String,
    pub(crate) expires_at: u64,
    pub(crate) token_hash: [u8; 32],
}

impl StoredGrant {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if [&self.id, &self.client_id, &self.checkout_id]
            .into_iter()
            .any(|id| !valid_id(id))
        {
            return Err(Error::InvalidBinding);
        }
        Ok(())
    }

    pub(crate) fn authenticates(&self, token: &str) -> bool {
        token.len() <= 256 && constant_time_eq::constant_time_eq(&self.token_hash, &hash(token))
    }
}

/// Authenticated identity and the exact binding allowed on this connection.
#[derive(Clone, Debug)]
pub struct GrantAccess {
    pub grant_id: String,
    pub client_id: String,
    pub binding: WorkspaceBinding,
    pub expires_at: u64,
}

/// A new private grant. The registry retains only the credential hash.
#[derive(Debug)]
pub struct IssuedGrant {
    pub access: GrantAccess,
    pub token: Secret,
}

impl WorkspaceService {
    /// Read the owner-approved relay setup without creating state on unused installs.
    pub fn relay_configuration(&self) -> Result<Option<RelayConfiguration>, Error> {
        let state = self.state.lock().map_err(|_| Error::Unavailable)?;
        match &*state {
            crate::State::Dormant => Ok(None),
            crate::State::Ready(store) => Ok(store.relay_configuration().cloned()),
            crate::State::Failed(error) => Err(error.status_error()),
            crate::State::Stopped => Err(Error::Stopped),
        }
    }

    /// Persist local-owner relay setup for daemon restarts.
    pub fn configure_relay(&self, configuration: Option<RelayConfiguration>) -> Result<(), Error> {
        if let Some(configuration) = &configuration {
            configuration.validate()?;
        }
        self.edit(|store| store.configure_relay(configuration))
    }

    /// Issue a bounded workspace grant. Only a trusted local owner may call this.
    pub fn issue_grant(
        &self,
        checkout_id: &str,
        client_id: &str,
        expires_at: u64,
    ) -> Result<IssuedGrant, Error> {
        let now = now_ms()?;
        if !valid_id(client_id)
            || expires_at <= now
            || expires_at > now.saturating_add(7 * 24 * 60 * 60 * 1000)
        {
            return Err(Error::InvalidBinding);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let token = Secret(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        let grant = StoredGrant {
            id,
            client_id: client_id.into(),
            checkout_id: checkout_id.into(),
            expires_at,
            token_hash: hash(&token.0),
        };
        let access = self.edit(|store| store.issue_grant(grant, now))?;
        Ok(IssuedGrant { access, token })
    }

    /// Revoke a grant durably. Existing connections revalidate before every RPC.
    pub fn revoke_grant(&self, grant_id: &str) -> Result<(), Error> {
        self.edit(|store| store.revoke_grant(grant_id))
    }

    /// Authenticate the runtime credential separately from the relay connect token.
    pub fn authenticate(&self, grant_id: &str, token: &Secret) -> Result<GrantAccess, Error> {
        self.read(|store| {
            let grant = store.grant(grant_id).ok_or(Error::Denied)?;
            if !grant.authenticates(&token.0) {
                return Err(Error::Denied);
            }
            store.grant_access(grant, now_ms()?)
        })
    }

    /// Revalidate an already-authenticated connection against current saved grants.
    pub fn grant_access(&self, grant_id: &str) -> Result<GrantAccess, Error> {
        self.read(|store| {
            store.grant_access(store.grant(grant_id).ok_or(Error::Denied)?, now_ms()?)
        })
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn now_ms() -> Result<u64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or(Error::Unavailable)
}
