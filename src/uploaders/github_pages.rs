use super::{log_upload_debug, make_http_agent, CloudUploader};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::File;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::ZipArchive;

const CONFIG_PATH: &str = "github_pages.json";

#[derive(Serialize, Deserialize, Clone)]
pub struct GitHubPagesConfig {
    pub repo: String,
    pub token: String,
    pub retention_days: u64,
}

impl Default for GitHubPagesConfig {
    fn default() -> Self {
        Self {
            repo: "atitoff-dotcom/ScreenCompareF".to_string(),
            token: String::new(),
            retention_days: 14,
        }
    }
}

impl GitHubPagesConfig {
    pub fn load() -> Self {
        std::fs::read_to_string(CONFIG_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(js) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(CONFIG_PATH, js);
        }
    }
}

pub struct GitHubPagesUploader;

impl CloudUploader for GitHubPagesUploader {
    fn max_bytes(&self) -> u64 { 250 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let cfg = GitHubPagesConfig::load();
        if cfg.token.trim().is_empty() {
            return Err("Не задан GitHub Token! Укажите его в окне «История».".to_string());
        }
        let repo = cfg.repo.trim();
        let (owner, repo_name) = repo.split_once('/').ok_or("Неверный формат репозитория (нужно owner/repo)")?;
        let agent = make_http_agent();

        // Auto-cleanup old reports if retention_days > 0 or repo > 700 MB
        let _ = cleanup_old_reports(&agent, &cfg, false);

        let head_sha = ensure_main_branch(&agent, repo, &cfg.token)?;
        let base_tree_sha = get_commit_tree(&agent, repo, &cfg.token, &head_sha)?;

        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let folder = format!("r/{}", stamp);

        let file = File::open(zip_file).map_err(|e| format!("Не открыт ZIP: {}", e))?;
        let mut archive = ZipArchive::new(file).map_err(|e| format!("Ошибка ZIP: {}", e))?;

        let mut tree_entries = Vec::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            if entry.is_dir() { continue; }
            let raw_name = entry.name().replace('\\', "/");
            let clean_name = raw_name.trim_start_matches("./").trim_start_matches('/');
            if clean_name.is_empty() || clean_name.contains("..") { continue; }

            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

            let blob_url = format!("https://github.com{}/git/blobs", repo);
            let blob_res: Value = gh_post(&agent, &blob_url, &cfg.token, json!({
                "content": b64,
                "encoding": "base64"
            }))?;
            let blob_sha = blob_res["sha"].as_str().ok_or("Нет blob sha")?.to_string();

            tree_entries.push(json!({
                "path": format!("{}/{}", folder, clean_name),
                "mode": "100644",
                "type": "blob",
                "sha": blob_sha
            }));
        }

        if tree_entries.is_empty() {
            return Err("ZIP-архив пуст.".to_string());
        }

        let tree_url = format!("https://github.com{}/git/trees", repo);
        let tree_res: Value = gh_post(&agent, &tree_url, &cfg.token, json!({
            "base_tree": base_tree_sha,
            "tree": tree_entries
        }))?;
        let new_tree_sha = tree_res["sha"].as_str().ok_or("Нет tree sha")?;

        let commit_url = format!("https://github.com{}/git/commits", repo);
        let commit_res: Value = gh_post(&agent, &commit_url, &cfg.token, json!({
            "message": format!("Publish report {}", stamp),
            "tree": new_tree_sha,
            "parents": [head_sha]
        }))?;
        let new_commit_sha = commit_res["sha"].as_str().ok_or("Нет commit sha")?;

        let ref_url = format!("https://github.com{}/git/refs/heads/main", repo);
        gh_patch(&agent, &ref_url, &cfg.token, json!({
            "sha": new_commit_sha,
            "force": false
        }))?;

        let page_url = format!("https://{}.github.io/{}/{}/", owner.to_lowercase(), repo_name, folder);
        log_upload_debug(&format!("GitHub Pages commit {} -> {}", new_commit_sha, page_url));
        Ok(page_url)
    }
}

fn gh_get(agent: &ureq::Agent, url: &str, token: &str) -> Result<Value, String> {
    agent.get(url)
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "ScreenCompare/1.0")
        .call()
        .map_err(|e| format!("GitHub GET {}: {}", url, e))?
        .into_json()
        .map_err(|e| format!("GitHub JSON: {}", e))
}

fn gh_post(agent: &ureq::Agent, url: &str, token: &str, body: Value) -> Result<Value, String> {
    agent.post(url)
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "ScreenCompare/1.0")
        .send_json(body)
        .map_err(|e| format!("GitHub POST {}: {}", url, e))?
        .into_json()
        .map_err(|e| format!("GitHub JSON: {}", e))
}

fn gh_patch(agent: &ureq::Agent, url: &str, token: &str, body: Value) -> Result<Value, String> {
    agent.request("PATCH", url)
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "ScreenCompare/1.0")
        .send_json(body)
        .map_err(|e| format!("GitHub PATCH {}: {}", url, e))?
        .into_json()
        .map_err(|e| format!("GitHub JSON: {}", e))
}

fn ensure_main_branch(agent: &ureq::Agent, repo: &str, token: &str) -> Result<String, String> {
    let ref_url = format!("https://github.com{}/git/ref/heads/main", repo);
    if let Ok(v) = gh_get(agent, &ref_url, token) {
        if let Some(sha) = v["object"]["sha"].as_str() {
            return Ok(sha.to_string());
        }
    }
    // Repository is empty: initialize with .nojekyll via Contents API
    let init_url = format!("https://github.com{}/contents/.nojekyll", repo);
    let res = agent.put(&init_url)
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "ScreenCompare/1.0")
        .send_json(json!({
            "message": "Initialize GitHub Pages repository",
            "content": "",
            "branch": "main"
        }))
        .map_err(|e| format!("Ошибка инициализации репозитория: {}", e))?
        .into_json::<Value>()
        .map_err(|e| e.to_string())?;

    res["commit"]["sha"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Не удалось получить SHA инициализирующего коммита".to_string())
}

fn get_commit_tree(agent: &ureq::Agent, repo: &str, token: &str, commit_sha: &str) -> Result<String, String> {
    let url = format!("https://github.com{}/git/commits/{}", repo, commit_sha);
    let v = gh_get(agent, &url, token)?;
    v["tree"]["sha"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Не найден tree sha".to_string())
}

pub fn cleanup_old_reports(agent: &ureq::Agent, cfg: &GitHubPagesConfig, force_all: bool) -> Result<String, String> {
    let repo = cfg.repo.trim();
    let token = cfg.token.trim();
    if token.is_empty() {
        return Err("Не задан GitHub Token".to_string());
    }
    let repo_info = gh_get(agent, &format!("https://github.com{}", repo), token)?;
    let repo_kb = repo_info["size"].as_u64().unwrap_or(0);
    let over_quota = repo_kb > 750 * 1024;

    let head_sha = match ensure_main_branch(agent, repo, token) {
        Ok(s) => s,
        Err(_) => return Ok("Репозиторий пуст".to_string()),
    };
    let tree_sha = get_commit_tree(agent, repo, token, &head_sha)?;
    let tree_url = format!("https://github.com{}/git/trees/{}?recursive=1", repo, tree_sha);
    let tree_data = gh_get(agent, &tree_url, token)?;
    let items = tree_data["tree"].as_array().ok_or("Пустое дерево")?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let max_age = if over_quota { 24 * 3600 } else { cfg.retention_days * 24 * 3600 };

    let mut kept = Vec::new();
    let mut removed_folders = std::collections::HashSet::new();

    for item in items {
        if item["type"].as_str() != Some("blob") { continue; }
        let path = item["path"].as_str().unwrap_or("");
        let sha = item["sha"].as_str().unwrap_or("");
        let mode = item["mode"].as_str().unwrap_or("100644");

        let mut drop_it = false;
        if let Some(rest) = path.strip_prefix("r/") {
            if let Some((ts_str, _)) = rest.split_once('/') {
                if force_all {
                    drop_it = true;
                } else if let Ok(ts) = ts_str.parse::<u64>() {
                    if cfg.retention_days > 0 && now.saturating_sub(ts) > max_age {
                        drop_it = true;
                    }
                }
                if drop_it {
                    removed_folders.insert(ts_str.to_string());
                }
            }
        }
        if !drop_it {
            kept.push(json!({ "path": path, "mode": mode, "type": "blob", "sha": sha }));
        }
    }

    if removed_folders.is_empty() && !over_quota {
        return Ok(format!("Очистка не требуется ({:.1} МБ)", repo_kb as f64 / 1024.0));
    }

    // Create orphan root commit (parents: []) so deleted blobs are garbage-collected by GitHub
    let new_tree = gh_post(agent, &format!("https://github.com{}/git/trees", repo), token, json!({
        "tree": kept
    }))?;
    let new_tree_sha = new_tree["sha"].as_str().ok_or("Нет tree sha")?;

    let new_commit = gh_post(agent, &format!("https://github.com{}/git/commits", repo), token, json!({
        "message": format!("Cleanup {} old reports (orphan compaction)", removed_folders.len()),
        "tree": new_tree_sha,
        "parents": []
    }))?;
    let new_commit_sha = new_commit["sha"].as_str().ok_or("Нет commit sha")?;

    gh_patch(agent, &format!("https://github.com{}/git/refs/heads/main", repo), token, json!({
        "sha": new_commit_sha,
        "force": true
    }))?;

    let msg = format!("Удалено отчётов: {} (история сжата)", removed_folders.len());
    log_upload_debug(&msg);
    Ok(msg)
}
