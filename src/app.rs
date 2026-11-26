//! Entry points used by the installer and the private WIT service.
pub async fn run(command: Option<&str>) -> std::process::ExitCode {
    #[cfg(target_os = "wasi")]
    if let Err(e) = crate::wasi::enter_cwd() {
        eprintln!("mn-client-support: {e}");
        return std::process::ExitCode::FAILURE;
    }
    match command {
        Some("--version" | "-V") => println!(
            "mn-client-support {}",
            crate::release_version(env!("CARGO_PKG_VERSION"))
        ),
        Some("--print-launcher") => print!("{}", include_str!("../scripts/launch-wasm.sh")),
        Some("init-client") => {
            if let Err(e) = crate::config::home().and_then(|root| crate::config::init_client(&root))
            {
                eprintln!("{e}");
                return std::process::ExitCode::FAILURE;
            }
        }
        #[cfg(target_os = "wasi")]
        Some("serve-local") => {
            if let Err(error) = crate::session::serve().await {
                eprintln!("mn-client-support: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
        Some("serve-stdio") => {
            if crate::backend::serve().await.is_err() {
                eprintln!("mn-client-support: incomplete WIT invocation");
                return std::process::ExitCode::FAILURE;
            }
        }
        _ => println!(
            "mn-client-support serves the MirrorNeuron WIT interface on private pipes.\nUsage: mn-client-support serve-stdio | --version"
        ),
    }
    std::process::ExitCode::SUCCESS
}
