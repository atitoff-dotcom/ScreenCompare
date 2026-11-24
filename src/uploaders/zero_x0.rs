use super::{log_upload_debug, make_http_agent, post_multipart_file, CloudUploader};

pub struct ZeroX0Uploader;

impl CloudUploader for ZeroX0Uploader {
    fn max_bytes(&self) -> u64 { 512 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let agent = make_http_agent();
        let (status, body) = post_multipart_file(
            &agent,
            "https://0x0.st",
            &[("User-Agent", "ScreenCompare/1.0")],
            &[],
            "file",
            zip_file,
        )?;
        let body = body.trim().to_string();
        log_upload_debug(&format!("0x0.st (status={}): {}", status, body));
        if (200..300).contains(&status) && body.starts_with("https://") {
            Ok(body)
        } else {
            Err(format!("0x0.st ({}): {}", status, body))
        }
    }
}
