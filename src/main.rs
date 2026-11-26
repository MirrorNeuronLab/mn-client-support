#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => println!("mn-client-support {}", env!("CARGO_PKG_VERSION")),
        Some("init-client") => {
            if let Err(e) = mn_client_support::connection::home()
                .and_then(|root| mn_client_support::connection::init_client(&root))
            {
                eprintln!("{e}");
                return std::process::ExitCode::FAILURE;
            }
        }
        Some("serve-stdio") => {
            if mn_client_support::backend::serve().await.is_err() {
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
