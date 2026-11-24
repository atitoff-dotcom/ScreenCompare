use super::{log_upload_debug, make_http_agent, post_multipart_file, CloudUploader};

pub struct TmpFilesUploader;

impl CloudUploader for TmpFilesUploader {
    fn max_bytes(&self) -> u64 { 100 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let agent = make_http_agent();
        let (status, body) = post_multipart_file(
            &agent,
            "https://tmpfiles.org",
            &[("User-Agent", "ScreenCompare/1.0")],
            &[("expire", "172800")],
            "file",
            zip_file,
        )?;
        let body = body.trim().to_string();
        log_upload_debug(&format!("tmpfiles.org (status={}): {}", status, body));

        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|_| format!("tmpfiles.org ({}): {}", status, body))?;
        if json["status"].as_str() == Some("success") {
            if let Some(u) = json["data"]["url"].as_str() {
                return Ok(u.replacen("tmpfiles.org/", "tmpfiles.org/dl/", 1));
            }
        }
        Err(format!("tmpfiles.org: {}", json))
    }
}
