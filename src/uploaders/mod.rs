pub mod wdfiles;
pub mod tmpfiles;
pub mod zero_x0;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UploaderId { #[default] WdFiles, TmpFiles, ZeroX0 }

impl UploaderId {
    pub const ALL: &'static [UploaderId] = &[UploaderId::WdFiles, UploaderId::TmpFiles, UploaderId::ZeroX0];
    pub fn label(self) -> &'static str {
        match self {
            UploaderId::WdFiles => "WDfiles.ru (РФ, до 2 ГБ)",
            UploaderId::TmpFiles => "tmpfiles.org (48ч, до 100 МБ)",
            UploaderId::ZeroX0 => "0x0.st (до 512 МБ)",
        }
    }
    pub fn short_name(self) -> &'static str {
        match self {
            UploaderId::WdFiles => "WDfiles.ru",
            UploaderId::TmpFiles => "tmpfiles.org",
            UploaderId::ZeroX0 => "0x0.st",
        }
    }
}

pub trait CloudUploader {
    fn max_bytes(&self) -> u64;
    fn upload(&self, zip_file: &str) -> Result<String, String>;
}

pub fn get_uploader(id: UploaderId) -> Box<dyn CloudUploader> {
    match id {
        UploaderId::WdFiles => Box::new(wdfiles::WdFilesUploader),
        UploaderId::TmpFiles => Box::new(tmpfiles::TmpFilesUploader),
        UploaderId::ZeroX0 => Box::new(zero_x0::ZeroX0Uploader),
    }
}

pub fn make_http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(300))
        .timeout_write(Duration::from_secs(300))
        .build()
}

pub fn post_multipart_file(
    agent: &ureq::Agent,
    url: &str,
    extra_headers: &[(&str, &str)],
    fields: &[(&str, &str)],
    file_field: &str,
    file_path: &str,
) -> Result<(u16, String), String> {
    let boundary = format!("----ScreenCompareBoundary{}", std::process::id());
    let file = File::open(file_path).map_err(|e| format!("Файл не открыт: {}", e))?;
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    let filename = Path::new(file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("report.zip");

    let mut head = Vec::new();
    for (k, v) in fields {
        let _ = write!(head, "--{}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{}\r\n", boundary, k, v);
    }
    let _ = write!(
        head,
        "--{}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: application/zip\r\n\r\n",
        boundary, file_field, filename
    );
    let tail = format!("\r\n--{}--\r\n", boundary).into_bytes();
    let total_len = head.len() as u64 + file_len + tail.len() as u64;

    let mut req = agent
        .post(url)
        .set("Content-Type", &format!("multipart/form-data; boundary={}", boundary))
        .set("Content-Length", &total_len.to_string());
    for (hk, hv) in extra_headers {
        req = req.set(hk, hv);
    }

    let reader = Cursor::new(head).chain(file).chain(Cursor::new(tail));
    match req.send(reader) {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Ok((code, body))
        }
        Err(e) => Err(format!("HTTP ошибка: {}", e)),
    }
}

pub fn log_upload_debug(msg: &str) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("upload_debug.log") {
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

pub fn upload_with_provider(provider: UploaderId, zip_file: &str) -> Result<String, String> {
    let sz = std::fs::metadata(zip_file).map(|m| m.len()).unwrap_or(0);
    let up = get_uploader(provider);
    log_upload_debug(&format!("=== START [{}]: '{}' ({} B) ===", provider.short_name(), zip_file, sz));
    if sz > up.max_bytes() {
        return Err(format!("Превышен лимит {} ({} МБ)", provider.short_name(), up.max_bytes() / (1024 * 1024)));
    }
    match up.upload(zip_file) {
        Ok(u) => { log_upload_debug(&format!("SUCCESS [{}] -> {}", provider.short_name(), u)); Ok(u) }
        Err(e) => { log_upload_debug(&format!("FAILED [{}]: {}", provider.short_name(), e)); Err(e) }
    }
}
