use eframe::egui;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::Duration;

const REPO_API_LATEST: &str = "https://github.com";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Clone, Debug)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Deserialize, Clone, Debug)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub html_url: String,
    pub download_url: String,
    pub asset_name: String,
    pub asset_size: u64,
    pub sha256_url: Option<String>,
}

enum UpdaterMsg {
    CheckDone(Result<Option<UpdateInfo>, String>),
    DownloadProgress(f32, String),
    InstallDone(Result<(), String>),
}

pub struct UpdaterState {
    pub is_open: bool,
    pub is_checking: bool,
    pub is_installing: bool,
    pub progress: f32,
    pub status_msg: String,
    pub available: Option<UpdateInfo>,
    rx: Option<Receiver<UpdaterMsg>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        let mut st = Self {
            is_open: false,
            is_checking: false,
            is_installing: false,
            progress: 0.0,
            status_msg: String::new(),
            available: None,
            rx: None,
        };
        st.start_check(true);
        st
    }
}

fn target_asset_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "ScreenCompare-windows-x86_64.exe"
    } else {
        "ScreenCompare-linux-x86_64"
    }
}

fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let clean = v.trim().trim_start_matches(['v', 'V']);
    let main_part = clean.split(['-', '+']).next()?;
    let mut parts = main_part.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer_version(remote: &str, local: &str) -> bool {
    match (parse_semver(remote), parse_semver(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

pub fn cleanup_old_binary() {
    if let Ok(exe) = std::env::current_exe() {
        for suffix in [".old", ".new"] {
            let mut os = exe.as_os_str().to_os_string();
            os.push(suffix);
            let p = PathBuf::from(os);
            if p.exists() {
                let _ = fs::remove_file(p);
            }
        }
    }
}

fn suffixed_path(exe: &Path, suffix: &str) -> PathBuf {
    let mut os = exe.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

impl UpdaterState {
    pub fn start_check(&mut self, silent: bool) {
        if self.is_checking || self.is_installing { return; }
        self.is_checking = true;
        if !silent { self.status_msg = "Проверка обновлений на GitHub...".to_string(); }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        thread::spawn(move || { let _ = tx.send(UpdaterMsg::CheckDone(fetch_latest_release())); });
    }

    pub fn start_install(&mut self) {
        if self.is_installing { return; }
        let Some(info) = self.available.clone() else { return; };
        self.is_installing = true;
        self.progress = 0.0;
        self.status_msg = "Подготовка к скачиванию...".to_string();
        let (tx, rx) = channel();
        self.rx = Some(rx);
        thread::spawn(move || {
            let res = download_and_replace(&info, |p, m| { let _ = tx.send(UpdaterMsg::DownloadProgress(p, m)); });
            let _ = tx.send(UpdaterMsg::InstallDone(res));
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut clear = false;
        if let Some(ref rx) = self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    UpdaterMsg::CheckDone(res) => {
                        self.is_checking = false; clear = true;
                        match res {
                            Ok(Some(i)) => { self.status_msg = format!("Доступна версия v{}!", i.version); self.available = Some(i); }
                            Ok(None) => { self.available = None; self.status_msg = format!("Актуальная версия v{}.", CURRENT_VERSION); }
                            Err(e) => self.status_msg = format!("Ошибка проверки: {}", e),
                        }
                    }
                    UpdaterMsg::DownloadProgress(p, t) => { self.progress = p; self.status_msg = t; ctx.request_repaint(); }
                    UpdaterMsg::InstallDone(res) => {
                        self.is_installing = false; clear = true;
                        match res {
                            Ok(()) => std::process::exit(0),
                            Err(e) => self.status_msg = format!("Ошибка обновления: {}", e),
                        }
                    }
                }
            }
        }
        if clear { self.rx = None; }
        if self.is_installing { ctx.request_repaint(); }
    }
}

fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(6)).timeout_read(Duration::from_secs(30)).build()
}

fn fetch_latest_release() -> Result<Option<UpdateInfo>, String> {
    let agent = build_agent();
    let resp = agent.get(REPO_API_LATEST)
        .set("User-Agent", &format!("ScreenCompare/{}", CURRENT_VERSION))
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "Релизы на GitHub ещё не опубликованы.".to_string(),
            other => other.to_string(),
        })?;
    let rel: GhRelease = resp.into_json().map_err(|e| format!("Ошибка JSON: {}", e))?;
    let remote_ver = rel.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
    if !is_newer_version(&remote_ver, CURRENT_VERSION) { return Ok(None); }
    let expected_name = target_asset_name();
    let asset = rel.assets.iter().find(|a| a.name == expected_name)
        .ok_or_else(|| format!("В релизе {} не найден файл {}", rel.tag_name, expected_name))?;
    let sha_name = format!("{}.sha256", expected_name);
    let sha256_url = rel.assets.iter().find(|a| a.name == sha_name).map(|a| a.browser_download_url.clone());
    Ok(Some(UpdateInfo {
        version: remote_ver, notes: rel.body.unwrap_or_default(), html_url: rel.html_url,
        download_url: asset.browser_download_url.clone(), asset_name: asset.name.clone(),
        asset_size: asset.size, sha256_url,
    }))
}

fn download_and_replace<F>(info: &UpdateInfo, mut on_progress: F) -> Result<(), String> where F: FnMut(f32, String) {
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let temp_exe = suffixed_path(&current_exe, ".new");
    let old_exe = suffixed_path(&current_exe, ".old");
    let agent = build_agent();

    let expected_hash = if let Some(ref sha_url) = info.sha256_url {
        on_progress(0.02, "Загрузка SHA-256...".to_string());
        let text = agent.get(sha_url).set("User-Agent", &format!("ScreenCompare/{}", CURRENT_VERSION))
            .call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
        text.split_whitespace().next().map(|s| s.to_lowercase())
    } else { None };

    on_progress(0.05, format!("Скачивание {}...", info.asset_name));
    let resp = agent.get(&info.download_url).set("User-Agent", &format!("ScreenCompare/{}", CURRENT_VERSION))
        .call().map_err(|e| format!("Ошибка скачивания: {}", e))?;
    let total = resp.header("Content-Length").and_then(|s| s.parse::<u64>().ok()).unwrap_or(info.asset_size).max(1);
    let mut reader = resp.into_reader();
    let mut file = File::create(&temp_exe).map_err(|e| format!("Ошибка записи: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut downloaded: u64 = 0;

    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Сбой сети: {}", e))?;
        if n == 0 { break; }
        file.write_all(&buf[..n]).map_err(|e| format!("Ошибка диска: {}", e))?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        let ratio = (downloaded as f32 / total as f32).clamp(0.0, 1.0);
        on_progress(0.05 + ratio * 0.90, format!("Скачивание: {:.1} / {:.1} МБ", downloaded as f64 / 1048576.0, total as f64 / 1048576.0));
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    if let Some(exp) = expected_hash {
        on_progress(0.96, "Проверка SHA-256...".to_string());
        if format!("{:x}", hasher.finalize()) != exp {
            let _ = fs::remove_file(&temp_exe);
            return Err("Контрольная сумма SHA-256 не совпала!".to_string());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&temp_exe).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_exe, perms).map_err(|e| e.to_string())?;
    }

    on_progress(0.98, "Замена файла...".to_string());
    if old_exe.exists() { let _ = fs::remove_file(&old_exe); }
    fs::rename(&current_exe, &old_exe).map_err(|e| format!("Не удалось переименовать текущий файл: {}", e))?;
    if let Err(e) = fs::rename(&temp_exe, &current_exe) {
        let _ = fs::rename(&old_exe, &current_exe);
        return Err(format!("Не удалось установить новый файл: {}", e));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    Command::new(&current_exe).args(args).spawn().map_err(|e| format!("Перезапуск не удался: {}", e))?;
    Ok(())
}

pub fn render_header_button(ui: &mut egui::Ui, st: &mut UpdaterState) {
    if let Some(ref info) = st.available {
        let btn = egui::Button::new(egui::RichText::new(format!(" ⬆ v{} ", info.version)).color(egui::Color32::WHITE).strong())
            .fill(egui::Color32::from_rgb(35, 134, 54));
        if ui.add(btn).on_hover_text("Доступно обновление!").clicked() { st.is_open = true; }
    } else {
        let label = if st.is_checking { format!(" ⏳ v{} ", CURRENT_VERSION) } else { format!(" v{} ", CURRENT_VERSION) };
        if ui.button(label).on_hover_text("Проверить обновления").clicked() {
            st.is_open = true;
            st.start_check(false);
        }
    }
}

pub fn render_updater_window(ctx: &egui::Context, st: &mut UpdaterState) {
    if !st.is_open { return; }
    let mut open = st.is_open;
    egui::Window::new("🔄 Обновление ScreenCompare")
        .open(&mut open).collapsible(false).resizable(false).default_width(440.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Текущая версия:"); ui.strong(format!("v{}", CURRENT_VERSION));
                if let Some(ref info) = st.available {
                    ui.label("→ Новая:");
                    ui.label(egui::RichText::new(format!("v{}", info.version)).color(egui::Color32::from_rgb(63, 185, 80)).strong());
                }
            });
            ui.add_space(6.0);
            if let Some(info) = st.available.clone() {
                ui.label(format!("Пакет: {} ({:.1} МБ)", info.asset_name, info.asset_size as f64 / 1048576.0));
                if !info.notes.trim().is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Что нового:").strong());
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| { ui.label(info.notes.trim()); });
                }
                ui.add_space(8.0);
                if st.is_installing { ui.add(egui::ProgressBar::new(st.progress).show_percentage()); }
                if !st.status_msg.is_empty() { ui.label(&st.status_msg); }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if !st.is_installing && ui.button(egui::RichText::new(" ⬇ Установить и перезапустить ").strong()).clicked() {
                        st.start_install();
                    }
                    ui.hyperlink_to("Открыть на GitHub", &info.html_url);
                });
            } else {
                if !st.status_msg.is_empty() { ui.label(&st.status_msg); }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!st.is_checking, egui::Button::new("Проверить снова")).clicked() { st.start_check(false); }
                    ui.hyperlink_to("Релизы на GitHub", "https://github.com");
                });
            }
        });
    st.is_open = open;
}
