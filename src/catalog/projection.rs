use super::{read, slug, unavailable, within};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;
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
pub(super) fn record(root: &Path, entry: &str) -> Result<Value> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_rows_drop_private_paths_and_credentials_recursively() {
        let mut row = json!({"name":"OCR","skills":[{"token":"private","model_path":"/local/model","info":"public"}]});
        sanitize(&mut row);
        assert_eq!(row, json!({"name":"OCR","skills":[{"info":"public"}]}));
    }
}
