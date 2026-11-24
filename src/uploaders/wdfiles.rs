use super::{log_upload_debug, CloudUploader};
use crate::create_hidden_cmd;

pub struct WdFilesUploader;

impl CloudUploader for WdFilesUploader {
    fn max_bytes(&self) -> u64 { 2 * 1024 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let ck = std::env::temp_dir().join(format!("sc_wd_{}.ck", std::process::id())).to_string_lossy().to_string();
        let _ = std::fs::remove_file(&ck);
        let page = create_hidden_cmd("curl.exe")
            .args(["-sS", "-L", "--connect-timeout", "15", "--max-time", "30", "-c", &ck, "-b", &ck, "https://wdfiles.ru"])
            .output().map_err(|e| { let _ = std::fs::remove_file(&ck); format!("curl init: {}", e) })?;
        let html = String::from_utf8_lossy(&page.stdout);
        let pos = html.find("file_upload_handler.ajax.php").ok_or_else(|| { let _ = std::fs::remove_file(&ck); "Нет endpoint wdfiles.ru".to_string() })?;
        let start = html[..pos].rfind("https://").ok_or("Нет URL wdfiles.ru")?;
        let end = html[start..].find(['\'', '"']).ok_or("Нет конца URL wdfiles.ru")?;
        let url = &html[start..start + end];
        let up = create_hidden_cmd("curl.exe")
            .args(["-sS", "-L", "--connect-timeout", "20", "--max-time", "300", "-c", &ck, "-b", &ck, "-H", "X-Requested-With: XMLHttpRequest", "-F", &format!("files[]=@{}", zip_file), url])
            .output().map_err(|e| { let _ = std::fs::remove_file(&ck); format!("curl up: {}", e) })?;
        let _ = std::fs::remove_file(&ck);
        let body = String::from_utf8_lossy(&up.stdout).trim().to_string();
        log_upload_debug(&format!("wdfiles.ru (exit={:?}): {}", up.status.code(), body));
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| format!("Ответ wdfiles.ru: {}", body))?;
        if let Some(first) = json.as_array().and_then(|a| a.first()) {
            if let Some(err) = first.get("error").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                return Err(format!("wdfiles.ru: {}", err));
            }
            if let Some(u) = first.get("url").and_then(|v| v.as_str()).filter(|s| s.starts_with("http")) {
                return Ok(u.to_string());
            }
        }
        Err(format!("Нет ссылки от wdfiles.ru: {}", body))
    }
}
