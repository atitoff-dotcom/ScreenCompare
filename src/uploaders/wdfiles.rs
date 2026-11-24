use super::{log_upload_debug, make_http_agent, post_multipart_file, CloudUploader};

pub struct WdFilesUploader;

impl CloudUploader for WdFilesUploader {
    fn max_bytes(&self) -> u64 { 2 * 1024 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let agent = make_http_agent();
        let html = agent
            .get("https://wdfiles.ru")
            .set("User-Agent", "ScreenCompare/1.0")
            .call()
            .map_err(|e| format!("wdfiles.ru init: {}", e))?
            .into_string()
            .map_err(|e| format!("wdfiles.ru html: {}", e))?;

        let pos = html.find("file_upload_handler.ajax.php").ok_or("Нет endpoint wdfiles.ru")?;
        let start = html[..pos].rfind("https://").ok_or("Нет URL wdfiles.ru")?;
        let end = html[start..].find(['\'', '"']).ok_or("Нет конца URL wdfiles.ru")?;
        let url = &html[start..start + end];

        let (status, body) = post_multipart_file(
            &agent,
            url,
            &[("User-Agent", "ScreenCompare/1.0"), ("X-Requested-With", "XMLHttpRequest")],
            &[],
            "files[]",
            zip_file,
        )?;
        let body = body.trim().to_string();
        log_upload_debug(&format!("wdfiles.ru (status={}): {}", status, body));

        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|_| format!("Ответ wdfiles.ru ({}): {}", status, body))?;
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
