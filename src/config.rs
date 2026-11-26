pub use crate::nodes::{connect, remove};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

pub fn user_home() -> Result<PathBuf> {
    env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(Error::storage)
}

pub fn expand(path: &Path) -> PathBuf {
    if let Ok(suffix) = path.strip_prefix("~")
        && let Ok(root) = user_home()
    {
        return root.join(suffix);
    }
    path.into()
}

pub fn home() -> Result<PathBuf> {
    let exe = env::current_exe().and_then(fs::canonicalize).ok();
    if let Some(releases) = exe.as_deref().and_then(|p| p.ancestors().nth(3))
        && releases.file_name().is_some_and(|s| s == "client-releases")
        && let Some(installed) = releases.parent()
        && installed.join("install_metadata.json").is_file()
    {
        return Ok(installed.into());
    }
    if let Some(v) = env::var_os("MN_HOME").filter(|v| !v.is_empty()) {
        return Ok(expand(Path::new(&v)));
    }
    // Installed executables also work through a PATH symlink and from a clean shell.
    if let Some(bin) = exe.as_deref().and_then(Path::parent)
        && bin.file_name().is_some_and(|v| v == "bin")
        && let Some(root) = bin.parent()
        && root.join("install_metadata.json").is_file()
    {
        return Ok(root.to_owned());
    }
    Ok(user_home()?.join(".mn"))
}

pub fn object(path: &Path) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(Error::storage),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(json!({})),
        Err(_) => Err(Error::storage()),
    }
}
pub fn role(root: &Path) -> Result<String> {
    let metadata = object(&root.join("install_metadata.json"))?;
    match metadata.get("install_role").and_then(Value::as_str) {
        None if metadata.get("install_role").is_none() => Ok("full".into()),
        Some(s @ ("client" | "runtime" | "full")) => Ok(s.into()),
        _ => Err(Error::new(
            503,
            "MN_INSTALL_ROLE_INVALID",
            "Unknown installation role.",
        )),
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Connection {
    pub(crate) version: u32,
    pub target: String,
    pub(crate) token: String,
    pub node_name: String,
    pub runtime_id: String,
    #[serde(default)]
    pub runtime_home: String,
}
impl Connection {
    pub fn public(&self) -> Value {
        json!({"status":"connected","transport":"grpc","target":self.target,"node_name":self.node_name,"runtime_id":self.runtime_id,"local":!self.runtime_home.is_empty()})
    }
    pub fn load(root: &Path) -> Result<Self> {
        let value = object(&root.join("client.json"))?;
        if value.get("api_url").is_some() {
            return Err(Error::new(
                409,
                "MN_RUNTIME_CONNECTION_UPGRADE_REQUIRED",
                "Reconnect the saved API profile using a runtime gRPC hostname and token.",
            ));
        }
        if value == json!({}) {
            return Err(Error::new(
                503,
                "MN_CLIENT_NOT_CONNECTED",
                "No runtime node is connected.",
            )
            .hint("Run mn node add <HOST> --token <RUNTIME_NODE_TOKEN>, or mn node add local."));
        }
        let mut c: Self = serde_json::from_value(value).map_err(|_| Error::storage())?;
        if c.version != 1
            || [&c.token, &c.node_name, &c.runtime_id]
                .iter()
                .any(|s| s.is_empty())
        {
            return Err(Error::storage());
        }
        c.target = target(&c.target, None)?;
        Ok(c)
    }
    pub(crate) fn save_locked(&self, root: &Path) -> Result<()> {
        let mut tmp = tempfile::NamedTempFile::new_in(root).map_err(|_| Error::storage())?;
        serde_json::to_writer(&mut tmp, self).map_err(|_| Error::storage())?;
        tmp.write_all(b"\n")
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|_| Error::storage())?;
        tmp.persist(root.join("client.json"))
            .map_err(|_| Error::storage())?;
        Ok(())
    }
}
pub(crate) fn lock(root: &Path) -> Result<File> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|_| Error::storage())?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join(".client.lock"))
        .map_err(|_| Error::storage())?;
    f.lock().map_err(|_| Error::storage())?;
    Ok(f)
}
pub fn target(input: &str, port: Option<u16>) -> Result<String> {
    let bad = || {
        Error::new(
            422,
            "MN_RUNTIME_TARGET_INVALID",
            "Use a runtime hostname or IP, optionally with its gRPC port.",
        )
    };
    let input = input.trim();
    if input.is_empty()
        || input.contains(['/', '@', '?', '#', '\\'])
        || input.chars().any(char::is_whitespace)
    {
        return Err(bad());
    }
    let authority: http::uri::Authority = input.parse().map_err(|_| bad())?;
    let host = authority.host();
    if host.is_empty() {
        return Err(bad());
    }
    let suffix = input.strip_prefix(host).ok_or_else(bad)?;
    let supplied = if suffix.is_empty() {
        None
    } else {
        Some(
            suffix
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .filter(|p| *p != 0)
                .ok_or_else(bad)?,
        )
    };
    if port == Some(0) || port.zip(supplied).is_some_and(|(a, b)| a != b) {
        return Err(bad());
    }
    Ok(format!(
        "{}:{}",
        host.to_ascii_lowercase(),
        port.or(supplied).unwrap_or(55051)
    ))
}
/// Complete the installer transaction while preserving identity and credentials.
pub fn init_client(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let _lock = lock(root)?;
    let path = root.join("install_metadata.json");
    let mut meta = object(&path)?;
    if meta["install_role"] != "client" {
        return Err(Error::storage());
    }
    let token = root.join("api.token");
    match fs::symlink_metadata(&token) {
        Ok(m) if m.file_type().is_file() => {
            if fs::read_to_string(&token)
                .map_err(|_| Error::storage())?
                .trim()
                .is_empty()
            {
                return Err(Error::storage());
            }
            fs::set_permissions(&token, fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::storage())?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            use std::io::Read;
            let mut secret = [0u8; 32];
            File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut secret))
                .map_err(|_| Error::storage())?;
            let mut f = tempfile::NamedTempFile::new_in(root).map_err(|_| Error::storage())?;
            use base64::Engine;
            writeln!(
                f,
                "{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
            )
            .and_then(|_| f.as_file().sync_all())
            .map_err(|_| Error::storage())?;
            f.persist_noclobber(token).map_err(|_| Error::storage())?;
        }
        _ => return Err(Error::storage()),
    }
    meta["client_implementation"] = json!("rust");
    meta["cli_version"] = json!(env!("CARGO_PKG_VERSION"));
    meta["install_version"] = json!(env!("CARGO_PKG_VERSION"));
    let mut f = tempfile::NamedTempFile::new_in(root).map_err(|_| Error::storage())?;
    serde_json::to_writer_pretty(&f, &meta).map_err(|_| Error::storage())?;
    writeln!(f)
        .and_then(|_| f.as_file().sync_all())
        .map_err(|_| Error::storage())?;
    f.persist(path).map_err(|_| Error::storage())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reinstall_preserves_the_owner_token_and_installation_identity() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let metadata = root.path().join("install_metadata.json");
        fs::write(
            &metadata,
            br#"{"install_role":"client","installation_id":"saved"}"#,
        )
        .unwrap();
        init_client(root.path()).unwrap();
        let token = root.path().join("api.token");
        let original = fs::read(&token).unwrap();
        init_client(root.path()).unwrap();
        assert!(!original.is_empty());
        assert_eq!(fs::read(&token).unwrap(), original);
        assert_eq!(
            fs::metadata(token).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(object(&metadata).unwrap()["installation_id"], "saved");
    }

    #[test]
    fn endpoints_preserve_explicit_ports_and_ipv6() {
        assert_eq!(
            target("[2001:db8::9]", Some(56051)).unwrap(),
            "[2001:db8::9]:56051"
        );
        assert_eq!(
            target("Node.Example:55051", None).unwrap(),
            "node.example:55051"
        );
        assert!(target("node.example:55051", Some(55052)).is_err());
        assert!(target("https://node.example/api/v1", None).is_err());
    }

    #[test]
    fn previous_profiles_are_readable_and_legacy_web_profiles_require_attachment() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join("client.json");
        fs::write(&profile, r#"{"version":1,"target":"node:55051","token":"private","node_name":"worker","runtime_id":"one","runtime_home":""}"#).unwrap();
        let client = Connection::load(home.path()).unwrap();
        assert_eq!(client.public()["node_name"], "worker");
        assert!(client.public().get("token").is_none());
        let legacy = r#"{"api_url":"https://node/api/v1","token":"legacy"}"#;
        fs::write(&profile, legacy).unwrap();
        assert_eq!(
            Connection::load(home.path()).err().unwrap().code,
            "MN_RUNTIME_CONNECTION_UPGRADE_REQUIRED"
        );
        assert_eq!(fs::read_to_string(profile).unwrap(), legacy);
    }
}
