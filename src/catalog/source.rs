use super::unavailable;
use crate::{Result, connection::expand};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
const DEFAULT_REPO: &str = "https://github.com/MirrorNeuronLab/mn-blueprints.git";
pub(super) async fn source(home: &Path, explicit: Option<&str>) -> Result<PathBuf> {
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
