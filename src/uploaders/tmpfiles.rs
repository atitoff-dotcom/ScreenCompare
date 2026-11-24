use super::{log_upload_debug, CloudUploader};
use crate::create_hidden_cmd;

pub struct TmpFilesUploader;

impl CloudUploader for TmpFilesUploader {
    fn max_bytes(&self) -> u64 { 100 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let out = create_hidden_cmd("curl.exe")
            .args(["-sS", "-L", "--http1.1", "-H", "Expect:", "--connect-timeout", "20", "--max-time", "300", "-F", "expire=172800", "-F", &format!("file=@{}", zip_file), "https://tmpfiles.org"])
            .output().map_err(|e| format!("curl error: {}", e))?;
        let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        log_upload_debug(&format!("tmpfiles.org (exit={:?}): {} | {}", out.status.code(), body, err));
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| format!("tmpfiles.org: {} {}", body, err))?;
        if json["status"].as_str() == Some("success") {
            if let Some(u) = json["data"]["url"].as_str() {
                return Ok(u.replacen("tmpfiles.org/", "tmpfiles.org/dl/", 1));
            }
        }
        Err(format!("tmpfiles.org: {}", json))
    }
}
