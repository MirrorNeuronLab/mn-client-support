# mn-client-support

This Rust component owns the client connection profile and the gRPC transport to Core. The CLI and HTTP server import the versioned `mirrorneuron:client/support@1.0.0` interface in [client.wit](wit/client.wit). The installed CLI and API launch the matching support Wasm component under stock Wasmtime. Each WIT invocation uses a separate mutually authenticated loopback stream. A process-private rendezvous file contains the temporary session credential. Standalone Wasmtime invocations can use the linked support implementation over in-memory wRPC pipes. The standalone `serve-stdio` entry point also accepts private wRPC pipes.

[The Core protobuf](contract/runtime_control.proto) and [the SDK resource inventory](contract/endpoints.json) are bundled for standalone builds. Build-time bindings use protox, Prost and Tonic. The client installs under `~/.mn`, stores its local API credential in `api.token`, and persists the authenticated Core connection in `client.json`.

Node attachment checks the candidate's capabilities and retries interrupted readiness probes for up to 30 seconds. Joining a second node also checks the saved entry's identity before sending it the peer credential. The join has five minutes to return its receipt. If the connection breaks during the join or removal, inspect membership with `mn node list` before submitting another change.

Production installs download `mn-client-support.wasm` from GAR. Use `./install.sh --no-runtime --mode local` in a sibling `mn-deploy` checkout to build the Wasm clients from source.
