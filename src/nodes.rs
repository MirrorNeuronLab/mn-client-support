//! Connect, verify and remove runtime nodes.
use crate::{
    Error, Result,
    config::{Connection, expand, lock, object, role, target, user_home},
    control::{Client, Method},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio::time::Duration;
const CAPABILITIES: [&str; 9] = [
    "catalog",
    "preparation",
    "operations",
    "events",
    "files",
    "uploads",
    "job_ui",
    "cluster",
    "mcp",
];

fn local(root: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    let candidates = if let Some(p) = explicit {
        vec![expand(p)]
    } else if role(root)? == "full" && root.join("runtime.token").is_file() {
        vec![root.into()]
    } else {
        let user = user_home()?;
        vec![user.join(".mn"), user.join(".mn-runtime")]
    };
    let mut found = Vec::new();
    for p in candidates {
        if p.join("runtime.token").is_file() && matches!(role(&p)?.as_str(), "full" | "runtime") {
            let p = p.canonicalize().map_err(|_| Error::storage())?;
            if !found.contains(&p) {
                found.push(p);
            }
        }
    }
    if found.len() == 1 {
        Ok(found.remove(0))
    } else {
        Err(Error::new(
            if found.is_empty() { 404 } else { 409 },
            if found.is_empty() {
                "MN_LOCAL_RUNTIME_NOT_FOUND"
            } else {
                "MN_LOCAL_RUNTIME_AMBIGUOUS"
            },
            "Choose a local runtime with --runtime-home <PATH>.",
        ))
    }
}

pub async fn connect(
    root: &Path,
    host: &str,
    token: Option<&str>,
    port: Option<u16>,
    runtime_home: Option<&Path>,
) -> Result<Value> {
    let (target, token, local_path, expected) = if host == "local" {
        let p = local(root, runtime_home)?;
        let endpoints = object(&p.join("runtime-endpoints.json"))?;
        let target = target(
            endpoints
                .pointer("/grpc/target")
                .and_then(Value::as_str)
                .unwrap_or("127.0.0.1:55051"),
            port,
        )?;
        let token = fs::read_to_string(p.join("runtime.token")).map_err(|_| Error::storage())?;
        let expected = object(&p.join("install_metadata.json"))?["installation_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        (target, token, p.to_string_lossy().into_owned(), expected)
    } else {
        if runtime_home.is_some() {
            return Err(Error::invalid("--runtime-home requires the local target."));
        }
        (
            target(host, port)?,
            token.unwrap_or_default().into(),
            String::new(),
            String::new(),
        )
    };
    if token.trim().is_empty() {
        return Err(Error::new(
            401,
            "MN_RUNTIME_TOKEN_REQUIRED",
            "A runtime node token is required.",
        ));
    }
    let mut candidate = Connection {
        version: 1,
        target,
        token: token.trim().into(),
        node_name: String::new(),
        runtime_id: String::new(),
        runtime_home: local_path,
    };
    let info = Client::new(&candidate)?
        .call(
            Method::Capabilities,
            "runtime.capabilities",
            json!({}),
            "",
            Duration::from_secs(30),
        )
        .await?
        .value;
    let node = info["node_name"].as_str().unwrap_or_default();
    let id = info["runtime_id"].as_str().unwrap_or_default();
    if node.is_empty()
        || id.is_empty()
        || info["runtime_control_version"] != 1
        || !info["capabilities"]
            .as_array()
            .is_some_and(|c| CAPABILITIES.iter().all(|s| c.iter().any(|v| v == s)))
    {
        return Err(Error::new(
            412,
            "MN_RUNTIME_INCOMPATIBLE",
            "The node does not support this runtime connection contract.",
        ));
    }
    if !expected.is_empty() && expected != id {
        return Err(Error::new(
            409,
            "MN_RUNTIME_IDENTITY_MISMATCH",
            "The endpoint belongs to a different runtime installation.",
        ));
    }
    candidate.node_name = node.into();
    candidate.runtime_id = id.into();
    let guard = lock(root)?;
    match Connection::load(root) {
        Ok(c) if c.runtime_id != id => {
            drop(guard);
            return Client::new(&c)?
                .command(
                    "node.add",
                    json!({"target":candidate.target,"token":candidate.token}),
                    30,
                )
                .await;
        }
        Err(e)
            if !matches!(
                e.code.as_str(),
                "MN_CLIENT_NOT_CONNECTED" | "MN_RUNTIME_CONNECTION_UPGRADE_REQUIRED"
            ) =>
        {
            return Err(e);
        }
        _ => {}
    }
    candidate.save_locked(root)?;
    Ok(candidate.public())
}
pub async fn remove(root: &Path, name: &str) -> Result<Value> {
    let c = Connection::load(root)?;
    let v = Client::new(&c)?
        .command("node.remove", json!({"node_name":name}), 30)
        .await?;
    if v["status"] != "removed" || v["node_name"] != name {
        return Err(Error::new(
            502,
            "MN_NODE_REMOVAL_UNCONFIRMED",
            "The runtime did not confirm removal.",
        ));
    }
    if name == c.node_name {
        let _lock = lock(root)?;
        match Connection::load(root) {
            Ok(current) if current == c => {
                fs::remove_file(root.join("client.json")).map_err(|_| Error::storage())?
            }
            Err(e) if e.code != "MN_CLIENT_NOT_CONNECTED" => return Err(e),
            _ => {}
        }
    }
    Ok(v)
}
