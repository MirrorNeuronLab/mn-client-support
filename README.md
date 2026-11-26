# mn-client-support

This native Rust component owns the client connection profile and the gRPC transport to Core. The CLI and HTTP server import the versioned `mirrorneuron:client/support@1.0.0` interface in [client.wit](wit/client.wit). Each invocation launches the matching `mn-client-support serve-stdio` process and exchanges wRPC frames over private pipes. Dropping a stream closes its connection.

[The Core protobuf](contract/runtime_control.proto) and [the SDK resource inventory](contract/endpoints.json) are bundled for standalone builds. Build-time bindings use protox, Prost and Tonic. The client installs under `~/.mn`, stores its local API credential in `api.token`, and persists the authenticated Core connection in `client.json`.

Build with `cargo build --locked --release`. To install and exercise the client, place the three Rust repositories beside `mn-deploy` and run `./install.sh --no-runtime` there.
