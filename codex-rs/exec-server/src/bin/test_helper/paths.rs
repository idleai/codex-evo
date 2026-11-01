use std::path::PathBuf;

pub struct TestHelperPaths {
    pub executable: PathBuf,
    pub linux_sandbox: Option<PathBuf>,
    #[cfg(target_os = "linux")]
    _aliases: tempfile::TempDir,
}

impl TestHelperPaths {
    pub fn new(executable: PathBuf) -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        let aliases = tempfile::Builder::new()
            .prefix("codex-exec-server-test-helpers")
            .tempdir()?;
        #[cfg(target_os = "linux")]
        let linux_sandbox = {
            // PTY launchers use argv[0] as the program, so this must be an
            // absolute alias rather than a bare name resolved through PATH.
            let alias = aliases
                .path()
                .join(codex_sandboxing::landlock::CODEX_LINUX_SANDBOX_ARG0);
            std::os::unix::fs::symlink(&executable, &alias)?;
            Some(alias)
        };
        #[cfg(not(target_os = "linux"))]
        let linux_sandbox = None;
        Ok(Self {
            executable,
            linux_sandbox,
            #[cfg(target_os = "linux")]
            _aliases: aliases,
        })
    }
}
