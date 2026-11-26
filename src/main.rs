#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    mn_client_support::run(std::env::args().nth(1).as_deref()).await
}
