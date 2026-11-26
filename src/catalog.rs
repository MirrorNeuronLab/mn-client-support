mod projection;
mod source;
use crate::{Error, Result};
use projection::record;
use serde_json::Value;
use source::source;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
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
