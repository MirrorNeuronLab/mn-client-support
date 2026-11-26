fn main() -> Result<(), Box<dyn std::error::Error>> {
    let contract = "contract/runtime_control.proto";
    println!("cargo:rerun-if-changed={contract}");
    let descriptors = protox::compile([contract], ["contract"])?;
    tonic_prost_build::configure()
        .build_server(false)
        .build_transport(false)
        .compile_fds(descriptors)?;
    Ok(())
}
