use std::path::PathBuf;

pub(crate) mod exec_server;

#[path = "../../src/bin/test_helper/protocol.rs"]
#[allow(dead_code)]
mod helper_protocol;
#[allow(unused_imports)]
pub(crate) use helper_protocol::*;
#[path = "../../src/bin/test_helper/paths.rs"]
mod helper_paths;

#[macro_export]
macro_rules! skip_if_mxc_unavailable {
    ($return_value:expr $(,)?) => {{
        if !codex_sandboxing::windows_mxc_available() {
            eprintln!("skipping test: native MXC is unavailable on this host");
            return $return_value;
        }
    }};
}

// Keep the helper separate from libtest: musl initializes Rust's command-line
// arguments after startup constructors, so constructor dispatch sees no arguments.
pub(crate) fn current_test_binary_helper_paths() -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    static PATHS: std::sync::OnceLock<anyhow::Result<helper_paths::TestHelperPaths>> =
        std::sync::OnceLock::new();
    let paths = PATHS
        .get_or_init(|| {
            let helper = codex_utils_cargo_bin::cargo_bin("codex-exec-server-test-helper")?;
            Ok(helper_paths::TestHelperPaths::new(helper)?)
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!("initialize executor test helper: {error:#}"))?;
    Ok((paths.executable.clone(), paths.linux_sandbox.clone()))
}
