//! Absolute paths owned by the app-server host, which may use another OS.
//!
//! Preserve wire spelling and validate without resolving against the client's
//! cwd. Executor request boundaries continue to use native `AbsolutePathBuf`.

use codex_utils_path_uri::LegacyAppPathString;
use serde::Deserialize;
use serde::Deserializer;
use serde::de::Error;
use std::path::PathBuf;

fn validate<E: Error>(path: PathBuf) -> Result<PathBuf, E> {
    LegacyAppPathString::from_path(&path)
        .to_inferred_path_uri()
        .ok_or_else(|| E::custom("app-server path is not an absolute Windows or POSIX path"))?;
    Ok(path)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    validate(PathBuf::deserialize(deserializer)?)
}

pub(crate) fn deserialize_vec<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<PathBuf>, D::Error> {
    Vec::<PathBuf>::deserialize(deserializer)?
        .into_iter()
        .map(validate)
        .collect()
}

pub(crate) fn deserialize_optional_vec<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<PathBuf>>, D::Error> {
    Option::<Vec<PathBuf>>::deserialize(deserializer)?
        .map(|paths| paths.into_iter().map(validate).collect())
        .transpose()
}

pub(crate) fn deserialize_optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<PathBuf>, D::Error> {
    Option::<PathBuf>::deserialize(deserializer)?
        .map(validate)
        .transpose()
}

#[cfg(test)]
mod tests {
    use crate::SandboxPolicy;
    use crate::ThreadForkResponse;
    use crate::ThreadResumeResponse;
    use crate::ThreadStartParams;
    use crate::ThreadStartResponse;
    use serde_json::json;
    use std::path::PathBuf;

    fn thread_response(cwd: &str) -> serde_json::Value {
        json!({
            "thread": {
                "id": "01900000-0000-7000-8000-000000000001",
                "sessionId": "01900000-0000-7000-8000-000000000001",
                "preview": "", "ephemeral": true, "modelProvider": "openai",
                "createdAt": 0, "updatedAt": 0, "status": {"type": "idle"},
                "cwd": cwd, "cliVersion": "test", "source": "cli", "turns": []
            },
            "model": "test-model", "modelProvider": "openai", "cwd": cwd,
            "runtimeWorkspaceRoots": [cwd], "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": {"type": "workspaceWrite", "writableRoots": [cwd], "networkAccess": false}
        })
    }

    #[test]
    fn server_paths_round_trip_across_operating_systems() {
        for path in [
            r"C:\Users\alice\project with spaces",
            r"D:/source/project",
            r"\\build-server\share\project",
            r"\\?\C:\source\project",
            "/home/alice/project with spaces",
        ] {
            let value = thread_response(path);
            let started: ThreadStartResponse = serde_json::from_value(value.clone()).expect(path);
            let resumed: ThreadResumeResponse = serde_json::from_value(value.clone()).expect(path);
            let forked: ThreadForkResponse = serde_json::from_value(value).expect(path);
            for (cwd, thread_cwd, roots, sandbox) in [
                (
                    started.cwd,
                    started.thread.cwd,
                    started.runtime_workspace_roots,
                    started.sandbox,
                ),
                (
                    resumed.cwd,
                    resumed.thread.cwd,
                    resumed.runtime_workspace_roots,
                    resumed.sandbox,
                ),
                (
                    forked.cwd,
                    forked.thread.cwd,
                    forked.runtime_workspace_roots,
                    forked.sandbox,
                ),
            ] {
                assert_eq!(cwd, PathBuf::from(path));
                assert_eq!(thread_cwd, cwd);
                assert_eq!(roots, vec![cwd]);
                assert_eq!(
                    serde_json::to_value(sandbox).expect("serialize")["writableRoots"],
                    json!([path])
                );
            }
            let request: ThreadStartParams = serde_json::from_value(json!({
                "runtimeWorkspaceRoots": [path]
            }))
            .expect("server-native request paths can be carried by any client");
            assert_eq!(
                serde_json::to_value(request).expect("serialize")["runtimeWorkspaceRoots"],
                json!([path])
            );
        }
    }

    #[test]
    fn relative_server_paths_are_rejected_in_every_session_location() {
        for path in ["relative/project", r"C:relative", "", "../project"] {
            for pointer in [
                "/cwd",
                "/thread/cwd",
                "/runtimeWorkspaceRoots/0",
                "/sandbox/writableRoots/0",
            ] {
                let mut value = thread_response(r"C:\source\project");
                *value.pointer_mut(pointer).expect("fixture pointer") = json!(path);
                assert!(
                    serde_json::from_value::<ThreadStartResponse>(value).is_err(),
                    "{pointer}: {path}"
                );
            }
            assert!(
                serde_json::from_value::<ThreadStartParams>(
                    json!({"runtimeWorkspaceRoots": [path]})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn executor_conversion_keeps_native_absolute_path_validation() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("relative")],
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        assert!(policy.try_to_core().is_err());
        #[cfg(unix)]
        {
            let foreign: SandboxPolicy = serde_json::from_value(json!({
                "type": "workspaceWrite", "writableRoots": [r"C:\source\project"],
                "networkAccess": false
            }))
            .expect("portable wire decode");
            assert!(foreign.try_to_core().is_err());
        }
    }
}
