//! Connect, verify and remove runtime nodes.
use crate::{
    Error, Result,
    config::{Connection, expand, lock, object, role, target, user_home},
    control::Client,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio::time::Duration;
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);
// Federation validates the peer, registers on both Cores, waits for reciprocal
// readiness and may roll back. Each of those steps has its own 30-second budget.
const FEDERATION_TIMEOUT_SECONDS: u64 = 300;
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
            return Err(Error::new(
                422,
                "MN_RUNTIME_TARGET_INVALID",
                "--runtime-home requires the local target.",
            ));
        }
        (
            target(host, port)?,
            token.unwrap_or_default().into(),
            String::new(),
            String::new(),
        )
    };
    if token.trim().is_empty() {
        let mut error = Error::new(
            401,
            "MN_RUNTIME_TOKEN_REQUIRED",
            "A runtime node token is required.",
        );
        error.exit_code = 2;
        return Err(error);
    }
    let mut candidate = Connection {
        version: 1,
        target,
        token: token.trim().into(),
        node_name: String::new(),
        runtime_id: String::new(),
        runtime_home: local_path,
    };
    let info = ready(&Client::new(&candidate)?, &candidate.target).await?;
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
            let entry = Client::new(&c)?;
            let info = ready(&entry, &c.target).await?;
            if info["runtime_id"] != c.runtime_id {
                return Err(Error::new(
                    409,
                    "MN_RUNTIME_IDENTITY_MISMATCH",
                    "The saved entry endpoint belongs to a different runtime installation.",
                ));
            }
            return entry
                .command(
                    "node.add",
                    json!({"target":candidate.target,"token":candidate.token}),
                    FEDERATION_TIMEOUT_SECONDS,
                )
                .await
                .map_err(|e| membership_error(e, &c.target, "joining", &candidate.target));
        }
        Ok(current) if candidate.runtime_home.is_empty() => {
            // An advertised address and loopback can identify the same runtime.
            // Keep the verified local attachment used by filesystem blueprints.
            candidate.runtime_home = current.runtime_home;
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

async fn ready(client: &Client, target: &str) -> Result<Value> {
    client
        .capabilities_when_ready(READINESS_TIMEOUT)
        .await
        .map_err(|mut e| {
            if e.code == "MN_RUNTIME_UNAVAILABLE" {
                e.message = format!("Could not verify the runtime at {target}.");
                e.hint = Some(
                    "Check the Core gRPC endpoint and that its native runtime service is ready."
                        .into(),
                );
            }
            e
        })
}

fn membership_error(mut e: Error, entry: &str, action: &str, peer: &str) -> Error {
    if e.code == "MN_RUNTIME_UNAVAILABLE" {
        e.message = format!("The runtime at {entry} did not confirm {action} {peer}.");
        e.hint = Some(
            "Check membership with mn node list. Verify the two Cores can reach each other's gRPC endpoints from their runtime networks before retrying."
                .into(),
        );
    }
    e
}

pub async fn remove(root: &Path, name: &str) -> Result<Value> {
    let c = Connection::load(root)?;
    let v = Client::new(&c)?
        .command("node.remove", json!({"node_name":name}), 30)
        .await
        .map_err(|e| membership_error(e, &c.target, "removing", name))?;
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
