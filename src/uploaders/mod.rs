pub mod wdfiles;
pub mod tmpfiles;
pub mod zero_x0;
use std::io::Write;

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
