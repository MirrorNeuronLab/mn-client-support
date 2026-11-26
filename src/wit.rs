pub mod client {
    wit_bindgen_wrpc::generate!({ world: "client", generate_all });
}
pub mod backend {
    wit_bindgen_wrpc::generate!({ world: "backend", generate_all });
}
