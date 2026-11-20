//! The claktory-apps host as factoryd runs it: frames on stdin and stdout,
//! its log on stderr.

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("CLAKTORY_APPS_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "could not start");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(claktory_apps::serve(tokio::io::stdin(), tokio::io::stdout())) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "stopped");
            std::process::ExitCode::FAILURE
        }
    }
}
