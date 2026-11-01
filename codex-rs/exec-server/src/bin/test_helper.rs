//! Executor fixture with normal process startup for every supported libc.

use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use codex_exec_server::ExecServerRuntimeOptions;
use codex_exec_server::ExecServerTelemetry;
use codex_exec_server::RequestDispatchMode;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;

#[path = "test_helper/paths.rs"]
mod paths;
#[path = "test_helper/protocol.rs"]
mod protocol;
use protocol::DELAYED_OUTPUT_AFTER_EXIT_CHILD_ARG;
use protocol::DELAYED_OUTPUT_AFTER_EXIT_PARENT_ARG;
use protocol::SYSTEM_PROXY_REQUEST_URL_ENV;
use protocol::SYSTEM_PROXY_URL_ENV;
use protocol::TEST_BUILD_COMMIT;

fn main() {
    #[cfg(target_os = "linux")]
    codex_utils_pty::init_spawn_helper(env::args_os());
    let mut args = env::args_os();
    let argv0 = args.next().unwrap_or_default();
    #[cfg(target_os = "linux")]
    if Path::new(&argv0).file_name()
        == Some(OsStr::new(
            codex_sandboxing::landlock::CODEX_LINUX_SANDBOX_ARG0,
        ))
    {
        codex_linux_sandbox::run_main();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = argv0;
    let argv1 = args.next();
    #[cfg(unix)]
    if argv1.as_deref() == Some(OsStr::new(codex_exec_server::CODEX_ARG0_EXEC_HELPER_ARG1)) {
        codex_exec_server::run_arg0_exec_helper_main();
    }
    if argv1.as_deref() == Some(OsStr::new(codex_exec_server::CODEX_FS_HELPER_ARG1)) {
        codex_exec_server::run_fs_helper_main();
    }
    if argv1.as_deref() == Some(OsStr::new(codex_sandboxing::CODEX_WINDOWS_MXC_ARG1)) {
        codex_sandboxing::run_windows_mxc_main();
    }
    #[cfg(windows)]
    if argv1.as_deref() == Some(OsStr::new("--run-as-windows-sandbox")) {
        codex_windows_sandbox::run_windows_sandbox_wrapper_main();
    }
    maybe_run_delayed_output_after_exit_from_test_binary();
    maybe_run_exec_server_from_test_binary();
    eprintln!("expected executor test helper command");
    std::process::exit(1);
}

fn maybe_run_delayed_output_after_exit_from_test_binary() {
    let mut args = env::args();
    let _program = args.next();
    let Some(command) = args.next() else {
        return;
    };
    match command.as_str() {
        DELAYED_OUTPUT_AFTER_EXIT_PARENT_ARG => {
            let release_path = next_release_path_arg(args);
            run_delayed_output_after_exit_parent(&release_path);
        }
        DELAYED_OUTPUT_AFTER_EXIT_CHILD_ARG => {
            let release_path = next_release_path_arg(args);
            run_delayed_output_after_exit_child(&release_path);
        }
        _ => {}
    }
}

fn next_release_path_arg(mut args: impl Iterator<Item = String>) -> PathBuf {
    let Some(release_path) = args.next() else {
        eprintln!("expected release path");
        std::process::exit(1);
    };
    if args.next().is_some() {
        eprintln!("unexpected extra arguments");
        std::process::exit(1);
    }
    PathBuf::from(release_path)
}

fn run_delayed_output_after_exit_parent(release_path: &Path) {
    let current_exe = match env::current_exe() {
        Ok(current_exe) => current_exe,
        Err(error) => {
            eprintln!("failed to resolve current test binary: {error}");
            std::process::exit(1);
        }
    };
    match Command::new(current_exe)
        .arg(DELAYED_OUTPUT_AFTER_EXIT_CHILD_ARG)
        .arg(release_path)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("failed to spawn delayed output child: {error}");
            std::process::exit(1);
        }
    }
}

fn run_delayed_output_after_exit_child(release_path: &Path) {
    for _ in 0..1_000 {
        if release_path.exists() {
            let mut stdout = std::io::stdout().lock();
            if let Err(error) = writeln!(stdout, "late output after exit") {
                eprintln!("failed to write delayed output: {error}");
                std::process::exit(1);
            }
            if let Err(error) = stdout.flush() {
                eprintln!("failed to flush delayed output: {error}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!(
        "timed out waiting for release path {}",
        release_path.display()
    );
    std::process::exit(1);
}

fn maybe_run_exec_server_from_test_binary() {
    let mut args = env::args();
    let _program = args.next();
    let Some(command) = args.next() else {
        return;
    };
    if command != "exec-server" {
        return;
    }
    // Enable requested child diagnostics so integration tests can observe background failures.
    if env::var_os("RUST_LOG").is_some()
        && let Err(error) = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init()
    {
        eprintln!("failed to initialize executor test logging: {error}");
        std::process::exit(1);
    }
    // Initialize in the executor child, just as the real CLI does at startup.
    codex_build_info::BuildInfo::initialize(TEST_BUILD_COMMIT);

    let Some(flag) = args.next() else {
        eprintln!("expected --listen");
        std::process::exit(1);
    };
    if flag != "--listen" {
        eprintln!("expected --listen, got `{flag}`");
        std::process::exit(1);
    }
    let Some(listen_url) = args.next() else {
        eprintln!("expected listen URL");
        std::process::exit(1);
    };
    let remaining_args = args.collect::<Vec<_>>();
    let request_dispatch_mode = match remaining_args.as_slice() {
        [] => RequestDispatchMode::Inline,
        [flag, value] if flag == "--concurrent-requests" => match value.parse() {
            Ok(mode) => mode,
            Err(error) => {
                eprintln!("invalid concurrent request count: {error}");
                std::process::exit(1);
            }
        },
        args => {
            eprintln!("unexpected exec-server arguments: {args:?}");
            std::process::exit(1);
        }
    };

    let current_exe = match env::current_exe() {
        Ok(current_exe) => current_exe,
        Err(error) => {
            eprintln!("failed to resolve current test binary: {error}");
            std::process::exit(1);
        }
    };
    let helper_paths = match paths::TestHelperPaths::new(current_exe) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("failed to prepare executor test helper paths: {error}");
            std::process::exit(1);
        }
    };
    let mut runtime_paths = match ExecServerRuntimeOptions::new(
        helper_paths.executable.clone(),
        helper_paths.linux_sandbox,
    ) {
        Ok(runtime_paths) => runtime_paths,
        Err(error) => {
            eprintln!("failed to configure exec-server runtime paths: {error}");
            std::process::exit(1);
        }
    };
    runtime_paths.prefer_mxc = env::var("CODEX_EXEC_SERVER_TEST_PREFER_MXC")
        .ok()
        .map(|value| {
            value.parse().unwrap_or_else(|error| {
                eprintln!("invalid test MXC preference: {error}");
                std::process::exit(1);
            })
        });
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to build Tokio runtime: {error}");
            std::process::exit(1);
        }
    };
    let http_client_factory = match (
        env::var(SYSTEM_PROXY_REQUEST_URL_ENV),
        env::var(SYSTEM_PROXY_URL_ENV),
    ) {
        (Ok(request_url), Ok(proxy_url)) => {
            codex_http_client::cache_system_proxy_route_for_test(&request_url, proxy_url);
            HttpClientFactory::new(OutboundProxyPolicy::RespectSystemProxy)
        }
        (Err(env::VarError::NotPresent), Err(env::VarError::NotPresent)) => {
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
        }
        _ => {
            eprintln!("system proxy test configuration requires both request and proxy URLs");
            std::process::exit(1);
        }
    };
    let exit_code = match runtime.block_on(async {
        #[cfg(target_os = "macos")]
        let runtime_paths = {
            let home = codex_utils_home_dir::find_codex_home()?;
            let config = codex_config::loader::load_config_layers_state(
                &codex_exec_server::LocalFileSystem::unsandboxed(),
                home.as_path(),
                /*cwd*/ None,
                &[],
                codex_config::LoaderOverrides::default(),
                &codex_config::NoopThreadConfigLoader,
            )
            .await?;
            runtime_paths.with_allowed_symlinked_codex_home(
                codex_config::allowed_symlinked_codex_home(&config, &home),
            )
        };
        codex_exec_server::run_main_with_telemetry(
            &listen_url,
            runtime_paths,
            ExecServerTelemetry::default(),
            http_client_factory,
            request_dispatch_mode,
            codex_websocket_auth::WebsocketAuthSettings::default(),
        )
        .await
    }) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("exec-server failed: {error}");
            1
        }
    };
    std::process::exit(exit_code);
}
