use mn_client_support::{
    connection,
    resources::{self, ENDPOINTS},
};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn every_runtime_route_round_trips_its_operation_and_identifiers() {
    for route in ENDPOINTS.iter() {
        let mut path = route.path.clone();
        for (key, param) in &route.parameters {
            if param.source == "path" {
                path = path
                    .replace(&format!("{{{key}}}"), "sample%20id")
                    .replace(&format!("{{{key}:path}}"), "sample%20id");
            }
        }
        let (found, params) = resources::resolve(&route.method, &path).unwrap();
        assert_eq!(&found.action, &route.action, "{} {path}", route.method);
        for value in params.as_object().unwrap().values() {
            assert_eq!(value, "sample id");
        }
    }
    assert_eq!(
        resources::resolve("PATCH", "/health").err().unwrap().status,
        405
    );
    assert_eq!(
        resources::resolve("GET", "/unregistered")
            .err()
            .unwrap()
            .status,
        404
    );
}

#[test]
fn a_resource_identifier_round_trips_as_one_path_segment() {
    let id = "reports/café?x=1#two";
    let path = format!("/runs/{}", resources::resource_id(id));
    let (run, params) = resources::resolve("GET", &path).unwrap();
    let args = run.arguments(&params, &json!({}), &HashMap::new(), None, "surface");
    assert_eq!(args["run_id"], id);
    assert_eq!(args["_principal"], "surface");
}

#[test]
fn historical_profiles_give_reconnection_guidance_without_exposing_tokens() {
    let home = tempfile::tempdir().unwrap();
    let err = connection::Connection::load(home.path()).err().unwrap();
    assert_eq!(err.code, "MN_CLIENT_NOT_CONNECTED");
    std::fs::write(
        home.path().join("client.json"),
        r#"{"api_url":"http://old","token":"fixture-private"}"#,
    )
    .unwrap();
    let err = connection::Connection::load(home.path()).err().unwrap();
    assert_eq!(err.code, "MN_RUNTIME_CONNECTION_UPGRADE_REQUIRED");
    assert!(!err.to_string().contains("fixture-private"));
}
