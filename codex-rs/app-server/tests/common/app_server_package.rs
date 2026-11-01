use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;

/// Stages the server and its runtime helpers together for installed-layout tests.
pub fn stage_app_server_package(program: &Path, directory: &Path) -> Result<PathBuf> {
    let host = codex_utils_cargo_bin::cargo_bin("codex-code-mode-host")?;
    let staged_program = directory.join(
        program
            .file_name()
            .context("app-server executable should have a filename")?,
    );
    let staged_host = directory.join(
        host.file_name()
            .context("code-mode host executable should have a filename")?,
    );
    let binaries = vec![
        (program.to_path_buf(), staged_program.clone()),
        (host, staged_host),
    ];
    #[cfg(target_os = "linux")]
    let binaries = {
        // Runner commands clear inherited variables, including Bazel runfile settings.
        // Keep bubblewrap discoverable through the normal installed resource layout.
        let resources = directory.join("codex-resources");
        std::fs::create_dir(&resources)?;
        let mut binaries = binaries;
        binaries.push((
            codex_utils_cargo_bin::cargo_bin("bwrap")?,
            resources.join("bwrap"),
        ));
        binaries
    };
    for (source, destination) in binaries {
        std::fs::hard_link(&source, &destination)
            .or_else(|_| codex_utils_cargo_bin::copy_executable(&source, &destination))
            .with_context(|| format!("stage executable {}", source.display()))?;
    }
    Ok(staged_program)
}
