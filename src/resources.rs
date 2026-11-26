//! REST-to-runtime mappings taken from the runtime's closed operation inventory.
use crate::{Error, Result, control::Method};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
};

#[derive(Deserialize)]
pub struct Parameter {
    pub source: String,
    pub name: String,
}
#[derive(Deserialize)]
pub struct Endpoint {
    pub action: String,
    pub method: String,
    pub path: String,
    pub parameters: BTreeMap<String, Parameter>,
}

pub static ENDPOINTS: LazyLock<Vec<Endpoint>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../contract/endpoints.json"))
        .expect("invalid bundled operation contract")
});
static PATTERNS: LazyLock<Vec<(usize, Regex)>> = LazyLock::new(|| {
    let placeholder = Regex::new(r"\{(\w+)(:path)?\}").unwrap();
    let mut items: Vec<_> = ENDPOINTS
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let pattern = placeholder.replace_all(&e.path, |c: &regex::Captures<'_>| {
                format!(
                    "(?P<{}>{})",
                    &c[1],
                    if c.get(2).is_some() { ".+" } else { "[^/]+" }
                )
            });
            (
                i,
                Regex::new(&format!("^{pattern}$")).expect("invalid path template"),
            )
        })
        .collect();
    items.sort_by_key(|(i, _)| ENDPOINTS[*i].path.matches('{').count());
    items
});
pub fn resolve(method: &str, path: &str) -> Result<(&'static Endpoint, Value)> {
    let mut known = false;
    for (i, re) in PATTERNS.iter() {
        if let Some(vars) = re.captures(path) {
            let e = &ENDPOINTS[*i];
            known = true;
            if method != e.method {
                continue;
            }
            let mut values = json!({});
            for name in re.capture_names().flatten() {
                let v = percent_encoding::percent_decode_str(&vars[name])
                    .decode_utf8()
                    .map_err(|_| Error::invalid("Invalid resource identifier."))?;
                values[name] = json!(v);
            }
            return Ok((e, values));
        }
    }
    Err(Error::new(
        if known { 405 } else { 404 },
        "MN_RUNTIME_OPERATION_UNSUPPORTED",
        "Unknown resource or request method.",
    ))
}
pub fn endpoint(action: &str) -> Result<&'static Endpoint> {
    ENDPOINTS
        .iter()
        .find(|e| e.action == action)
        .ok_or_else(Error::protocol)
}
impl Endpoint {
    pub fn rpc(&self) -> Method {
        let a = &self.action;
        if a.starts_with("stream_") {
            return Method::Events;
        }
        if matches!(
            a.as_str(),
            "download_run_artifact" | "download_run_output" | "export_run" | "create_job_backup"
        ) {
            return Method::ReadFile;
        }
        if a.contains("blueprint") && self.method == "GET" {
            return Method::Catalog;
        }
        if a.contains("preparation")
            || matches!(
                a.as_str(),
                "create_blueprint_run" | "create_blueprint_addition"
            )
        {
            return Method::Prepare;
        }
        if matches!(self.method.as_str(), "GET" | "HEAD") {
            Method::Read
        } else {
            Method::Execute
        }
    }
    pub fn deadline(&self, v: &Value) -> u32 {
        let b = &v["request"];
        match self.action.as_str() {
            "delete_job" | "delete_run" => 330,
            "update_job" if b["status"] == "archived" => 330,
            "create_job" | "replace_job_bundle" | "create_job_run" => 3600,
            "update_job"
                if b["refresh_contract"] == true || !b["resolved_configuration"].is_null() =>
            {
                3600
            }
            _ if self.rpc() == Method::Prepare => 3600,
            _ => 30,
        }
    }
    pub fn arguments(
        &self,
        path: &Value,
        query: &Value,
        headers: &HashMap<String, String>,
        body: Option<&Value>,
        principal: &str,
    ) -> Value {
        let mut args = json!({});
        for (key, p) in &self.parameters {
            let value = match p.source.as_str() {
                "path" => path.get(&p.name).cloned(),
                "query" => query.get(&p.name).cloned(),
                "header" => headers.get(&p.name.to_ascii_lowercase()).map(|s| json!(s)),
                "body" => body.cloned(),
                "principal" => Some(json!(principal)),
                _ => None,
            };
            if let Some(v) = value {
                args[key] = v;
            }
        }
        args
    }
}
pub fn resource_id(id: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(id, SET).to_string()
}
pub fn decode_id(s: &str) -> Result<String> {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| Error::invalid("Invalid resource identifier."))
}
