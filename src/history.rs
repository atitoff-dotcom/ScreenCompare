use crate::uploaders::{github_pages::{cleanup_old_reports, GitHubPagesConfig}, make_http_agent, upload_with_provider, UploaderId};
use crate::create_hidden_cmd;
use crate::picker::format_mb;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct HistoryEntry {
    pub id: String,
    pub created_at: String,
    pub name_a: String,
    pub name_b: String,
    pub pairs_count: usize,
    pub unpacked_bytes: u64,
    pub zip_path: String,
    pub bugsmash_url: String,
}

#[derive(Default)]
pub struct HistoryState {
    pub is_open: bool,
    pub entries: Vec<HistoryEntry>,
    pub edit_idx: Option<usize>,
    pub edit_url_buf: String,
    pub status_msg: String,
    pub uploading: bool,
    pub selected_uploader: UploaderId,
    pub gh_cfg: GitHubPagesConfig,
    pub show_gh_settings: bool,
    pub upload_rx: Option<Receiver<(Option<usize>, Result<String, String>)>>,
}

impl HistoryState {
    pub fn load() -> Self {
        let entries: Vec<HistoryEntry> = std::fs::read_to_string("history.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { entries, gh_cfg: GitHubPagesConfig::load(), ..Default::default() }
    }
}

pub fn save_history(st: &HistoryState) {
    if let Ok(js) = serde_json::to_string_pretty(&st.entries) {
        let _ = std::fs::write("history.json", js);
    }
}

pub fn now_timestamp() -> (String, String) {
    let out = create_hidden_cmd("powershell")
        .args(["-NoProfile", "-Command", "Get-Date -Format 'yyyyMMdd_HHmmss|dd.MM.yyyy HH:mm'"])
        .output();
    if let Ok(o) = out {
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if let Some((a, b)) = s.split_once('|') {
            return (a.to_string(), b.to_string());
        }
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (secs.to_string(), format!("ts:{}", secs))
}

pub fn record_saved_report_ready(
    st: &mut HistoryState, name_a: &str, name_b: &str,
    pairs_count: usize, unpacked_bytes: u64,
    stamp: String, human: String, zip_dest: String,
) {
    if let Some(first) = st.entries.first_mut() {
        if first.name_a == name_a && first.name_b == name_b && first.bugsmash_url.is_empty() {
            first.created_at = human;
            first.pairs_count = pairs_count;
            first.unpacked_bytes = unpacked_bytes;
            first.zip_path = zip_dest;
            save_history(st);
            return;
        }
    }
    st.entries.insert(0, HistoryEntry {
        id: stamp, created_at: human, name_a: name_a.to_string(), name_b: name_b.to_string(),
        pairs_count, unpacked_bytes, zip_path: zip_dest, bugsmash_url: String::new(),
    });
    save_history(st);
}

pub fn start_cloud_upload(st: &mut HistoryState, zip_file: &str, entry_idx: Option<usize>) -> Result<(), String> {
    if st.uploading {
        return Err("Загрузка уже выполняется, подождите...".to_string());
    }
    if !Path::new(zip_file).exists() {
        return Err("Архив не найден. Сначала нажмите «Сохранить выбранные и создать ZIP».".to_string());
    }
    let (tx, rx) = mpsc::channel();
    let zip_owned = zip_file.to_string();
    st.uploading = true;
    st.upload_rx = Some(rx);
    let prov = st.selected_uploader;
    st.status_msg = format!("⏳ Загрузка ZIP на {}...", prov.short_name());
    std::thread::spawn(move || {
        let res = upload_with_provider(prov, &zip_owned);
        let _ = tx.send((entry_idx, res));
    });
    Ok(())
}

pub fn poll_cloud_upload(st: &mut HistoryState, ctx: &egui::Context) -> Option<Result<String, String>> {
    let mut finished = None;
    if let Some(ref rx) = st.upload_rx {
        if let Ok((entry_idx, res)) = rx.try_recv() {
            finished = Some((entry_idx, res));
        }
    }
    if let Some((entry_idx, res)) = finished {
        st.uploading = false;
        st.upload_rx = None;
        match res {
            Ok(url) => {
                let idx = entry_idx.unwrap_or(0);
                if let Some(entry) = st.entries.get_mut(idx) {
                    entry.bugsmash_url = url.clone();
                    save_history(st);
                }
                ctx.copy_text(url.clone());
                st.status_msg = format!("✅ Загружено (ссылка скопирована): {}", url);
                return Some(Ok(url));
            }
            Err(err) => {
                st.status_msg = format!("❌ Ошибка загрузки: {}", err);
                return Some(Err(err));
            }
        }
    }
    if st.uploading {
        ctx.request_repaint();
    }
    None
}

pub fn render_history_window(ctx: &egui::Context, st: &mut HistoryState) {
    if !st.is_open { return; }
    let mut open = st.is_open;
    egui::Window::new("📜 История отчётов и ссылок")
        .open(&mut open)
        .default_size([620.0, 380.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Куда публиковать:");
                egui::ComboBox::from_id_source("h_up")
                    .selected_text(st.selected_uploader.label())
                    .show_ui(ui, |ui| {
                        for &p in UploaderId::ALL {
                            ui.selectable_value(&mut st.selected_uploader, p, p.label());
                        }
                    });
                if ui.button("⚙ GitHub Pages").clicked() {
                    st.show_gh_settings = !st.show_gh_settings;
                }
            });
            if st.show_gh_settings {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Репозиторий:");
                        ui.add(egui::TextEdit::singleline(&mut st.gh_cfg.repo).desired_width(210.0));
                        ui.label("Хранить (дней):");
                        ui.add(egui::DragValue::new(&mut st.gh_cfg.retention_days).clamp_range(1..=365));
                    });
                    ui.horizontal(|ui| {
                        ui.label("GitHub Token:");
                        ui.add(egui::TextEdit::singleline(&mut st.gh_cfg.token).password(true).desired_width(260.0));
                        if ui.button("💾 Сохранить").clicked() {
                            st.gh_cfg.save();
                            st.status_msg = "✅ Настройки GitHub Pages сохранены.".to_string();
                        }
                        if ui.button("🧹 Очистить старые").clicked() {
                            st.gh_cfg.save();
                            let agent = make_http_agent();
                            match cleanup_old_reports(&agent, &st.gh_cfg, false) {
                                Ok(m) => st.status_msg = format!("✅ {}", m),
                                Err(e) => st.status_msg = format!("❌ {}", e),
                            }
                        }
                    });
                });
            }
            ui.separator();
            if !st.status_msg.is_empty() {
                ui.horizontal(|ui| {
                    if st.uploading { ui.spinner(); }
                    ui.label(egui::RichText::new(&st.status_msg).color(egui::Color32::from_rgb(160, 225, 175)).size(12.0));
                });
                ui.separator();
            }
            if st.entries.is_empty() {
                ui.label("История пуста. Сохраните выбранные пары — запись появится здесь.");
                return;
            }
            let mut delete_idx: Option<usize> = None;
            let mut upload_req: Option<(String, usize)> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for idx in 0..st.entries.len() {
                    let e = st.entries[idx].clone();
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&e.created_at).strong());
                            ui.label(format!("| {} ↔ {} | {} пар | {}", e.name_a, e.name_b, e.pairs_count, format_mb(e.unpacked_bytes)));
                        });
                        if st.edit_idx == Some(idx) {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut st.edit_url_buf).desired_width(340.0).hint_text("https://catbox.moe..."));
                                if ui.button("💾 Сохранить").clicked() {
                                    st.entries[idx].bugsmash_url = st.edit_url_buf.trim().to_string();
                                    st.edit_idx = None;
                                    save_history(st);
                                }
                                if ui.button("Отмена").clicked() { st.edit_idx = None; }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                if !e.bugsmash_url.is_empty() {
                                    ui.hyperlink_to(&e.bugsmash_url, &e.bugsmash_url);
                                    if ui.small_button("📋 Копировать").clicked() {
                                        ui.output_mut(|o| o.copied_text = e.bugsmash_url.clone());
                                        st.status_msg = "Ссылка скопирована!".to_string();
                                    }
                                } else {
                                    ui.label(egui::RichText::new("Ссылка пока не получена").weak());
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            let zip_exists = !e.zip_path.is_empty() && PathBuf::from(&e.zip_path).exists();
                            if ui.add_enabled(!st.uploading && zip_exists, egui::Button::new("☁ Загрузить ZIP в облако")).clicked() {
                                upload_req = Some((e.zip_path.clone(), idx));
                            }
                            if ui.button("✏ Вставить ссылку").clicked() {
                                st.edit_idx = Some(idx);
                                st.edit_url_buf = e.bugsmash_url.clone();
                            }
                            if zip_exists {
                                if ui.button("📁 Архив").clicked() {
                                    let _ = create_hidden_cmd("explorer").args([&format!("/select,{}", e.zip_path.replace('/', "\\"))]).spawn();
                                }
                            }
                            if ui.button("🗑").clicked() { delete_idx = Some(idx); }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
            if let Some((zp, idx)) = upload_req {
                if let Err(err) = start_cloud_upload(st, &zp, Some(idx)) {
                    st.status_msg = err;
                }
            }
            if let Some(idx) = delete_idx {
                let removed = st.entries.remove(idx);
                if !removed.zip_path.is_empty() { let _ = std::fs::remove_file(&removed.zip_path); }
                save_history(st);
            }
        });
    if !open { st.is_open = false; }
}
