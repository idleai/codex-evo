use crate::Error;
use crate::WorkspaceBinding;
use crate::canonical_directory;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

/// Owner-selected filesystem ceiling for a runner on this daemon's local executor.
/// Native platform runtime files remain readable; network access is always denied.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerPolicy {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rules {
    version: u32,
    enabled: Option<bool>,
    read_roots: Option<Vec<PathBuf>>,
    write_roots: Option<Vec<PathBuf>>,
}

impl RunnerPolicy {
    /// Canonicalize an explicit local grant. Roots must remain inside its checkout.
    pub fn new(
        binding: &WorkspaceBinding,
        read_roots: Vec<PathBuf>,
        write_roots: Vec<PathBuf>,
    ) -> Result<Self, Error> {
        let canonicalize = |roots: Vec<PathBuf>| -> Result<Vec<PathBuf>, Error> {
            if roots.len() > 32 {
                return Err(Error::Limit);
            }
            let mut roots = roots
                .iter()
                .map(|root| canonical_directory(root))
                .collect::<Result<Vec<_>, _>>()?;
            roots.sort();
            roots.dedup();
            Ok(roots)
        };
        let policy = Self {
            read_roots: canonicalize(read_roots)?,
            write_roots: canonicalize(write_roots)?,
        };
        policy.validate(binding)?;
        Ok(policy)
    }

    /// Recheck saved directories before reuse, including a replaced symlink root.
    pub fn validate(&self, binding: &WorkspaceBinding) -> Result<(), Error> {
        if !binding.available() || self.read_roots.len() > 32 || self.write_roots.len() > 32 {
            return Err(Error::Denied);
        }
        for root in self.read_roots.iter().chain(&self.write_roots) {
            if !root.starts_with(&binding.checkout_root) || canonical_directory(root)? != *root {
                return Err(Error::Denied);
            }
        }
        if self
            .write_roots
            .iter()
            .any(|write| !self.read_roots.iter().any(|read| write.starts_with(read)))
        {
            return Err(Error::Denied);
        }
        Ok(())
    }

    /// Intersect current authored rules with a private grant or saved thread ceiling.
    /// Unknown runner fields and versions deny admission; unrelated domains are retained.
    pub fn restrict(
        &self,
        binding: &WorkspaceBinding,
        agent_rules: Option<&str>,
    ) -> Result<Self, Error> {
        self.validate(binding)?;
        let Some(document) = agent_rules else {
            return Ok(self.clone());
        };
        if document.len() > 256 * 1024 {
            return Err(Error::Limit);
        }
        let document: serde_json::Value = serde_json::from_str(document)?;
        if document
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            return Err(Error::Version);
        }
        let Some(rules) = document.get("runners") else {
            return Ok(self.clone());
        };
        let rules: Rules = serde_json::from_value(rules.clone())?;
        if rules.version != 1 || rules.enabled == Some(false) {
            return Err(Error::Denied);
        }
        let resolve = |roots: Vec<PathBuf>| -> Result<Vec<PathBuf>, Error> {
            if roots.len() > 32 {
                return Err(Error::Limit);
            }
            roots
                .into_iter()
                .map(|root| {
                    if root.is_absolute() {
                        return Err(Error::Denied);
                    }
                    let root = canonical_directory(&binding.checkout_root.join(root))?;
                    if !root.starts_with(&binding.checkout_root) {
                        return Err(Error::Denied);
                    }
                    Ok(root)
                })
                .collect()
        };
        let read_roots = match rules.read_roots {
            Some(roots) => intersect(&self.read_roots, &resolve(roots)?),
            None => self.read_roots.clone(),
        };
        let write_roots = intersect(&self.write_roots, &read_roots);
        let write_roots = match rules.write_roots {
            Some(roots) => intersect(&write_roots, &resolve(roots)?),
            None => write_roots,
        };
        let policy = Self {
            read_roots,
            write_roots,
        };
        policy.validate(binding)?;
        Ok(policy)
    }

    /// A new grant or rules document cannot widen an existing thread's ceiling.
    pub fn intersect(&self, other: &Self) -> Self {
        let read_roots = intersect(&self.read_roots, &other.read_roots);
        let write_roots = intersect(
            &intersect(&self.write_roots, &other.write_roots),
            &read_roots,
        );
        Self {
            read_roots,
            write_roots,
        }
    }
}

fn intersect(left: &[PathBuf], right: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for left in left {
        for right in right {
            if left.starts_with(right) {
                roots.push(left.clone());
            } else if right.starts_with(left) {
                roots.push(right.clone());
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}
