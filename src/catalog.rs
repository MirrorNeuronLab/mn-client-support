use crate::{Error, Result, connection::expand};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
const DEFAULT_REPO: &str = "https://github.com/MirrorNeuronLab/mn-blueprints.git";
fn unavailable() -> Error {
    Error::new(
        503,
        "MN_BLUEPRINT_CATALOG_UNAVAILABLE",
        "Could not read the configured blueprint catalog.",
    )
    .hint("Check the catalog location and Git access.")
}
fn read(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).map_err(|_| unavailable())?).map_err(|_| unavailable())
}
fn within(root: &Path, relative: &str) -> Result<PathBuf> {
    let p = Path::new(relative);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(unavailable());
    }
    root.join(p)
        .canonicalize()
        .ok()
        .filter(|p| p.starts_with(root))
        .ok_or_else(unavailable)
}
fn slug(s: &str) -> String {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
async fn source(home: &Path, explicit: Option<&str>) -> Result<PathBuf> {
    let settings = fs::read_to_string(home.join("docker-compose.env")).unwrap_or_default();
    let option = |name: &str| {
        env::var(name).ok().or_else(|| {
            settings
                .lines()
                .filter_map(|line| line.split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.trim_matches(['"', '\'']).to_string())
        })
    };
    let repo = if let Some(path) = explicit {
        path.into()
    } else {
        match option("MN_BLUEPRINT_SOURCE").as_deref().unwrap_or("github") {
            "local" => {
                return option("MN_BLUEPRINT_LOCAL")
                    .map(|p| expand(Path::new(&p)))
                    .ok_or_else(unavailable);
            }
            "github" => option("MN_BLUEPRINT_REPO").unwrap_or_else(|| DEFAULT_REPO.into()),
            _ => return Err(unavailable()),
        }
    };
    let local = expand(Path::new(&repo));
    if local.is_dir() {
        return Ok(local);
    }
    let repo = if repo.split('/').count() == 2
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
    {
        format!("https://github.com/{}", repo.trim_end_matches(".git"))
    } else {
        repo
    };
    if !(url::Url::parse(&repo).is_ok_and(|u| {
        matches!(u.scheme(), "https" | "http" | "ssh" | "git") && u.host().is_some()
    }) || (repo.starts_with("git@") && repo.contains(':')))
    {
        return Err(unavailable());
    }
    let dest = if repo
        .trim_end_matches(".git")
        .eq_ignore_ascii_case(DEFAULT_REPO.trim_end_matches(".git"))
    {
        home.join("blueprints")
    } else {
        let name = repo
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("blueprints")
            .trim_end_matches(".git");
        home.join("blueprint_repos").join(format!(
            "{}-{}",
            name,
            &format!("{:x}", Sha256::digest(repo.as_bytes()))[..12]
        ))
    };
    if dest.join("index.json").is_file() {
        return Ok(dest);
    }
    let parent = dest.parent().ok_or_else(unavailable)?;
    fs::create_dir_all(parent).map_err(|_| unavailable())?;
    let staging = tempfile::tempdir_in(parent).map_err(|_| unavailable())?;
    let tree = staging.path().join("checkout");
    let mut cmd = tokio::process::Command::new("git");
    cmd.args([
        "clone",
        "--depth",
        "1",
        "--recurse-submodules",
        "--shallow-submodules",
        "--",
        &repo,
    ])
    .arg(&tree)
    .env("GIT_TERMINAL_PROMPT", "0")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    if !tokio::time::timeout(Duration::from_secs(90), cmd.status())
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?
        .success()
    {
        return Err(unavailable());
    }
    if !tree.join("index.json").is_file() {
        return Err(unavailable());
    }
    if fs::rename(tree, &dest).is_err() && !dest.join("index.json").is_file() {
        return Err(unavailable());
    }
    Ok(dest)
}
fn sanitize(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|k, v| {
                !k.starts_with('_')
                    && !matches!(
                        k.as_str(),
                        "version"
                            | "active_blueprint_location"
                            | "api_key"
                            | "blueprint_local"
                            | "blueprint_repo"
                            | "bundle_dir"
                            | "bundle_path"
                            | "config_path"
                            | "host_path"
                            | "litellm_config_path"
                            | "local_peer_auth_token"
                            | "join_token"
                            | "peer_auth_token"
                            | "repo_dir"
                            | "run_dir"
                            | "runs_root"
                            | "secret"
                            | "token"
                    )
                    && !((k == "path" || k.ends_with("_path"))
                        && v.as_str().is_some_and(|s| Path::new(s).is_absolute()))
            });
            map.values_mut().for_each(sanitize);
        }
        Value::Array(a) => a.iter_mut().for_each(sanitize),
        _ => {}
    }
}
fn document(root: &Path, file: Option<&str>) -> Result<Value> {
    match file {
        Some(f) => read(&within(root, f)?),
        None => Ok(json!({})),
    }
}
fn project_setup_defaults(row: &mut Value, config: &Value) {
    use std::sync::LazyLock;
    static PRIVATE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(^|[._-])(password|token|secret|credentials?|api_key|private_key)($|[._-])",
        )
        .unwrap()
    });
    for section in ["setup_guide", "init_config_review"] {
        let Some(fields) = row
            .get_mut(section)
            .and_then(|s| s.get_mut("fields"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for field in fields {
            let Some(path) = field
                .get("path")
                .or_else(|| field.get("key"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if field.get("default").is_some()
                || field["secret"] == true
                || field["sensitive"] == true
                || matches!(field["type"].as_str(), Some("secret" | "password"))
                || PRIVATE.is_match(path)
            {
                continue;
            }
            if let Some(default) = path
                .split('.')
                .try_fold(config, |value, key| value.get(key))
                && (default.is_boolean()
                    || default.is_number()
                    || default.as_str().is_some_and(|s| !s.is_empty()))
            {
                field["default"] = default.clone();
            }
        }
    }
}
fn record(root: &Path, entry: &str) -> Result<Value> {
    let base = within(root, entry)?;
    let manifest = read(&within(&base, "manifest.json")?)?;
    let id = manifest["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(unavailable)?;
    let extension = |key: &str| -> Result<Value> {
        document(&base, manifest["extensions"][key]["file"].as_str())
    };
    let product = extension("mn.product")?;
    let execution = document(&base, manifest["execution"].as_str())?;
    let workflow = document(&base, manifest["workflow"].as_str())?;
    let config = document(&base, manifest["config"].as_str())?;
    let mut row = json!({"id":id,"path":entry,"type":execution["type"].as_str().unwrap_or("batch"),"name":id,
        "category":"General","job_name":execution["job_name"].as_str().map(str::to_owned).unwrap_or_else(||id.replace('_',"-")),"workflow_id":workflow["workflow_id"].as_str().unwrap_or(id),
        "capabilities":[],"runtime_features":[],"skills":[],"pricing":{"model":"free","rate":0,"unit":"hour"},"hourly_rate":"$0/hr","rate_label":"Free","rating":0,"installs":0,"publisher":"MirrorNeuron","accent_color":"#1f7a8c","revision":""});
    row["requirements"] = if manifest["model_requirement"].is_null() {
        execution["requirements"].as_object()
    } else {
        config["resources"].as_object()
    }
    .cloned()
    .unwrap_or_default()
    .into();
    for key in [
        "tagline",
        "summary",
        "description",
        "target_users",
        "output",
        "icon",
        "agent_role",
        "customizable_for",
        "problem",
        "simulation_type",
    ] {
        row[key] = json!(if matches!(key, "tagline" | "summary" | "description") {
            product["one_line"].as_str().unwrap_or("")
        } else {
            ""
        });
    }
    for key in [
        "tagline",
        "summary",
        "description",
        "target_users",
        "output",
        "agent_role",
        "customizable_for",
        "problem",
        "simulation_type",
        "capabilities",
        "runtime_features",
        "category",
        "icon",
        "publisher",
        "rating",
        "installs",
        "business_goal",
        "collaboration_group",
        "collaboration",
    ] {
        let nested = &product["product"];
        if let Some(value) = nested.get(key).or_else(|| {
            matches!(key, "tagline" | "summary" | "description")
                .then(|| nested.get("one_line"))
                .flatten()
        }) {
            row[key] = value.clone();
        }
    }
    for obj in [&product, &manifest, &extension("mn.ui")?] {
        for k in [
            "name",
            "tagline",
            "summary",
            "description",
            "category",
            "capabilities",
            "runtime_features",
            "target_users",
            "output",
            "business_goal",
            "coworker_type",
            "configuration_contract",
            "setup_guide",
            "web_ui",
            "starter_questions",
            "collaboration_group",
            "collaboration",
            "icon",
            "observability_dashboard",
            "pricing",
            "rate_label",
            "hourly_rate",
            "agent_role",
            "customizable_for",
            "problem",
            "simulation_type",
            "publisher",
            "installs",
            "rating",
            "accent_color",
            "revision",
            "init_config_review",
        ] {
            if let Some(v) = obj.get(k) {
                row[k] = v.clone();
            }
        }
    }
    let deps = document(&base, manifest["dependencies"].as_str())?;
    if let Some(skills) = deps["skills"].as_array() {
        row["skills"]=Value::Array(skills.iter().map(|s|json!({"name":s["name"],"version_constraint":s.get("version").unwrap_or(&s["ref"])})).collect());
    }
    if let Some(enabled) = extension("mn.airgap")?.get("enabled") {
        row["air-gapped"] = enabled.clone();
    }
    let mut response = extension("mn.response")?;
    if let Some(object) = response.as_object_mut() {
        object.remove("$schema");
    }
    if response["enabled"] == true {
        row["response_service"] = response;
    }
    let model = row["pricing"]["model"]
        .as_str()
        .unwrap_or("free")
        .to_owned();
    let rate = row["pricing"]["rate"]
        .as_f64()
        .or_else(|| row["pricing"]["rate"].as_str()?.parse().ok())
        .unwrap_or(0.0);
    let unit = row["pricing"]["unit"].as_str().unwrap_or("hour").to_owned();
    if model != "free" && rate != 0.0 {
        for k in ["hourly_rate", "rate_label"] {
            if product[k].is_null() {
                row[k] = format!("${rate}/{unit}").into();
            }
        }
    }
    row["pricing"] = json!({"model":model,"rate":rate,"unit":unit});
    let category = row["category"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("General")
        .to_owned();
    row["category_slug"] = json!(slug(&category));
    row["category"] = category.into();
    project_setup_defaults(&mut row, &config);
    sanitize(&mut row);
    Ok(row)
}
pub async fn list(home: &Path, repository: Option<&str>, category: Option<&str>) -> Result<Value> {
    let root = source(home, repository)
        .await?
        .canonicalize()
        .map_err(|_| unavailable())?;
    let index = read(&within(&root, "index.json")?)?;
    let mut records = BTreeMap::new();
    for entry in index.as_array().ok_or_else(unavailable)? {
        let item = record(&root, entry.as_str().ok_or_else(unavailable)?)?;
        let id = item["id"].as_str().ok_or_else(unavailable)?.to_string();
        if records.insert(id, item).is_some() {
            return Err(unavailable());
        }
    }
    Ok(Value::Array(
        records
            .into_values()
            .filter(|r| {
                category.is_none_or(|c| {
                    c.split(',').all(|p| p.trim().is_empty())
                        || c.split(',').any(|v| r["category_slug"] == slug(v))
                })
            })
            .collect(),
    ))
}
