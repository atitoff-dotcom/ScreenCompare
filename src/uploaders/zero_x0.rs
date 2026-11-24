use super::{log_upload_debug, CloudUploader};
use crate::create_hidden_cmd;

pub struct ZeroX0Uploader;

impl CloudUploader for ZeroX0Uploader {
    fn max_bytes(&self) -> u64 { 512 * 1024 * 1024 }
    fn upload(&self, zip_file: &str) -> Result<String, String> {
        let out = create_hidden_cmd("curl.exe")
            .args(["-sS", "-L", "--http1.1", "-H", "Expect:", "--connect-timeout", "20", "--max-time", "300", "-A", "ScreenCompare/1.0", "-F", &format!("file=@{}", zip_file), "https://0x0.st"])
            .output().map_err(|e| format!("curl error: {}", e))?;
        let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        log_upload_debug(&format!("0x0.st (exit={:?}): {} | {}", out.status.code(), body, err));
        if out.status.success() && body.starts_with("https://") { Ok(body) } else { Err(format!("0x0.st: {} {}", body, err)) }
    }
}
