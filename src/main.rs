#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod history;
mod uploaders;
use uploaders::UploaderId;
mod picker;
mod updater;
use updater::{cleanup_old_binary, render_header_button, render_updater_window, UpdaterState};
use history::{poll_cloud_upload, render_history_window, start_cloud_upload, HistoryState};
use picker::{build_pair_items, format_mb, render_pair_picker_viewport, selected_total_bytes, HTMLKIN_MAX_BYTES};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const DEFAULT_FFMPEG: &str = r"C:\Users\user\Programs\Video\eac3to\ffmpeg.exe";
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Serialize, Deserialize, Clone)]
struct MatchRecord {
    id: usize,
    time_a_sec: f64,
    time_b_sec: f64,
    time_a_formatted: String,
    time_b_formatted: String,
    time_delta_sec: f64,
    hash_diff: u32,
    file_a: String,
    file_b: String,
}

pub use picker::{PairItem, PickerState};

enum WorkerMsg {
    Status(String),
    Progress(f32),
    Done(Result<(PathBuf, Vec<MatchRecord>, String, String), String>),
}

#[derive(Clone)]
struct FrameSig {
    hash: [u64; 4],
    norm_luma: Vec<f32>,
}

fn compute_frame_sig(raw_gray: &[u8]) -> Option<FrameSig> {
    if raw_gray.len() < 160 * 160 {
        return None;
    }
    const W: usize = 160;
    const C: usize = 136;
    const OFF: usize = 12;

    let mut crop = vec![0f32; C * C];
    let mut sum = 0.0f32;
    for y in 0..C {
        for x in 0..C {
            let v = raw_gray[(y + OFF) * W + (x + OFF)] as f32;
            crop[y * C + x] = v;
            sum += v;
        }
    }
    let n = (C * C) as f32;
    let mean = sum / n;
    let mut sq_sum = 0.0f32;
    for v in &mut crop {
        *v -= mean;
        sq_sum += *v * *v;
    }
    let std_dev = sq_sum.sqrt();
    if std_dev < 1e-3 {
        return None;
    }
    for v in &mut crop {
        *v /= std_dev;
    }

    let mut hash = [0u64; 4];
    let mut bit = 0usize;

    for r in 0..8 {
        let y = (r * C) / 8 + 8;
        for c in 0..16 {
            let x1 = (c * C) / 17;
            let x2 = ((c + 1) * C) / 17;
            if crop[y * C + x2] > crop[y * C + x1] {
                hash[bit / 64] |= 1u64 << (bit % 64);
            }
            bit += 1;
        }
    }
    for r in 0..16 {
        let y1 = (r * C) / 17;
        let y2 = ((r + 1) * C) / 17;
        for c in 0..8 {
            let x = (c * C) / 8 + 8;
            if crop[y2 * C + x] > crop[y1 * C + x] {
                hash[bit / 64] |= 1u64 << (bit % 64);
            }
            bit += 1;
        }
    }

    Some(FrameSig {
        hash,
        norm_luma: crop,
    })
}

fn hamming_256(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (a[0] ^ b[0]).count_ones()
        + (a[1] ^ b[1]).count_ones()
        + (a[2] ^ b[2]).count_ones()
        + (a[3] ^ b[3]).count_ones()
}

fn zncc_score(a: &FrameSig, b: &FrameSig) -> f32 {
    let mut dot = 0.0f32;
    for (x, y) in a.norm_luma.iter().zip(b.norm_luma.iter()) {
        dot += x * y;
    }
    dot.clamp(-1.0, 1.0)
}

fn create_hidden_cmd<P: AsRef<Path>>(program: P) -> Command {
    let mut cmd = Command::new(program.as_ref());
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

struct ScreenCompareApp {
    video_a: String,
    video_b: String,
    from_time_str: String,
    to_time_str: String,
    count: usize,
    samples: usize,
    window_sec: f64,

    is_running: bool,
    status_text: String,
    progress: f32,
    finished_html: Option<PathBuf>,
    show_about: bool,
    history: HistoryState,
    picker: Arc<Mutex<PickerState>>,
    updater: UpdaterState,

    rx: Option<Receiver<WorkerMsg>>,
}

impl Default for ScreenCompareApp {
    fn default() -> Self {
        Self {
            video_a: String::new(),
            video_b: String::new(),
            from_time_str: "00:05:00".to_string(),
            to_time_str: "00:25:00".to_string(),
            count: 10,
            samples: 24,
            window_sec: 20.0,
            is_running: false,
            status_text: "Перетащите два видеофайла мышкой в окно программы...".to_string(),
            progress: 0.0,
            finished_html: None,
            show_about: false,
            history: HistoryState::load(),
            picker: Arc::new(Mutex::new(PickerState::default())),
            updater: UpdaterState::default(),
            rx: None,
        }
    }
}

impl eframe::App for ScreenCompareApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.input(|i| {
            if !i.raw.dropped_files.is_empty() {
                let paths: Vec<PathBuf> = i.raw.dropped_files.iter()
                    .filter_map(|f| f.path.clone())
                    .collect();

                if paths.len() >= 2 {
                    self.video_a = paths[0].to_string_lossy().to_string();
                    self.video_b = paths[1].to_string_lossy().to_string();
                    self.status_text = "Файлы А и Б успешно добавлены!".to_string();
                } else if paths.len() == 1 {
                    if self.video_a.is_empty() {
                        self.video_a = paths[0].to_string_lossy().to_string();
                        self.status_text = "Файл А добавлен. Перетащите Файл Б.".to_string();
                    } else {
                        self.video_b = paths[0].to_string_lossy().to_string();
                        self.status_text = "Файл Б добавлен. Можно запускать!".to_string();
                    }
                }
            }
        });

        self.updater.poll(ctx);

        if let Some(res) = poll_cloud_upload(&mut self.history, ctx) {
            match res {
                Ok(url) => self.status_text = format!("✅ Загружено в облако (ссылка скопирована): {}", url),
                Err(err) => self.status_text = format!("❌ {}", err),
            }
        }

        if let Some(ref rx) = self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    WorkerMsg::Status(s) => self.status_text = s,
                    WorkerMsg::Progress(p) => self.progress = p,
                    WorkerMsg::Done(res) => {
                        self.is_running = false;
                        match res {
                            Ok((html_path, records, name_a, name_b)) => {
                                self.status_text = format!("Найдено пар: {}. Отметьте нужные и нажмите «Сохранить выбранные» для создания ZIP.", records.len());
                                self.finished_html = Some(html_path);
                                if let Ok(mut st) = self.picker.lock() {
                                    st.pairs = build_pair_items(records);
                                    st.current_idx = 0;
                                    st.show_b = false;
                                    st.zoom = 1.0;
                                    st.pan = egui::Vec2::ZERO;
                                    st.name_a = name_a;
                                    st.name_b = name_b;
                                    st.textures.clear();
                                    st.failed_loads.clear();
                                    st.zip_created = false;
                                    st.status_msg = "[Пробел]/клик — А/Б, [←/→] — пары, [Enter] — выбрать.".to_string();
                                    st.is_open = !st.pairs.is_empty();
                                }
                            }
                            Err(e) => self.status_text = format!("Ошибка: {}", e),
                        }
                    }
                }
            }
        }

        let mut zip_status_ui: Option<(f32, String)> = None;
        if let Ok(mut st) = self.picker.lock() {
            let (in_prog, finished) = picker::poll_zip_job(&mut st);
            zip_status_ui = in_prog;
            if let Some(res) = finished {
                match res {
                    Ok(info) => {
                        self.status_text = format!("✅ Архив matches.zip готов ({}).", format_mb(info.total_bytes));
                        history::record_saved_report_ready(
                            &mut self.history, &st.name_a, &st.name_b,
                            info.pairs_count, info.total_bytes,
                            info.stamp, info.human, info.zip_dest,
                        );
                    }
                    Err(err) => {
                        self.status_text = format!("❌ Ошибка сборки ZIP: {}", err);
                    }
                }
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🎬 ScreenCompare: Поиск и блинк-сравнение кадров");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(" ℹ About ").clicked() {
                        self.show_about = !self.show_about;
                    }
                    render_header_button(ui, &mut self.updater);
                    let hist_lbl = format!(" 📜 История ({}) ", self.history.entries.len());
                    if ui.button(hist_lbl).clicked() {
                        self.history.is_open = !self.history.is_open;
                    }
                });
            });
            ui.add_space(6.0);

            ui.group(|ui| {
                ui.label(egui::RichText::new("Релиз А (Базовый тайминг):").strong());
                ui.horizontal(|ui| {
                    ui.add_sized([ui.available_width() - 85.0, 24.0], egui::TextEdit::singleline(&mut self.video_a));
                    if ui.button("Обзор...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.video_a = path.to_string_lossy().to_string();
                        }
                    }
                });
            });

            ui.add_space(4.0);

            ui.group(|ui| {
                ui.label(egui::RichText::new("Релиз Б (Целевой, поиск соответствий):").strong());
                ui.horizontal(|ui| {
                    ui.add_sized([ui.available_width() - 85.0, 24.0], egui::TextEdit::singleline(&mut self.video_b));
                    if ui.button("Обзор...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.video_b = path.to_string_lossy().to_string();
                        }
                    }
                });
            });

            ui.add_space(6.0);

            ui.group(|ui| {
                ui.label(egui::RichText::new("Параметры анализа:").strong());
                ui.horizontal(|ui| {
                    ui.label("От:");
                    ui.add_sized([75.0, 20.0], egui::TextEdit::singleline(&mut self.from_time_str));
                    ui.label("До:");
                    ui.add_sized([75.0, 20.0], egui::TextEdit::singleline(&mut self.to_time_str));

                    ui.separator();

                    ui.label("Пар:");
                    if ui.add(egui::DragValue::new(&mut self.count).clamp_range(2..=100)).changed() {
                        self.samples = ((self.count as f32) * 1.5).round() as usize;
                    }
                });
            });

            ui.add_space(8.0);

            ui.add(egui::ProgressBar::new(self.progress).show_percentage());
            ui.add_space(2.0);
            ui.label(egui::RichText::new(&self.status_text).color(egui::Color32::from_rgb(140, 200, 255)));

            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if !self.is_running {
                    let can_start = !self.video_a.trim().is_empty() && !self.video_b.trim().is_empty();
                    let btn = ui.add_enabled(can_start, egui::Button::new(
                        egui::RichText::new(" 🚀 НАЧАТЬ СРАВНЕНИЕ ").strong().size(15.0)
                    ));
                    if btn.clicked() {
                        self.start_processing();
                    }
                } else {
                    ui.spinner();
                    ui.label(" Идет обработка кадров...");
                }

                if let Some(ref path) = self.finished_html {
                    if path.exists() {
                        if ui.button(egui::RichText::new(" 🔍 Выбрать пары (весь экран) ").strong().size(14.0).color(egui::Color32::from_rgb(255, 210, 90))).clicked() {
                            if let Ok(mut st) = self.picker.lock() {
                                if st.pairs.is_empty() {
                                    if let Ok(js) = std::fs::read_to_string("matches/matches.json") {
                                        if let Ok(recs) = serde_json::from_str::<Vec<MatchRecord>>(&js) {
                                            st.pairs = build_pair_items(recs);
                                            st.zoom = 1.0;
                                            st.name_a = Path::new(&self.video_a).file_name().unwrap_or_default().to_string_lossy().to_string();
                                            st.name_b = Path::new(&self.video_b).file_name().unwrap_or_default().to_string_lossy().to_string();
                                        }
                                    }
                                }
                                st.is_open = true;
                            }
                        }
                        if ui.button(egui::RichText::new(" 🌐 Открыть отчет в браузере ").size(14.0)).clicked() {
                            let _ = create_hidden_cmd("cmd").args(["/c", "start", "", &path.to_string_lossy()]).spawn();
                        }
                    }
                    let total_bytes = self.picker.lock().map(|st| selected_total_bytes(&st.pairs)).unwrap_or(0);
                    let zip_busy = zip_status_ui.is_some();
                    let zip_ok = !zip_busy && Path::new("matches.zip").exists();
                    let can_up = zip_ok && !self.history.uploading && total_bytes <= HTMLKIN_MAX_BYTES;
                    egui::ComboBox::from_id_source("m_up")
                        .selected_text(self.history.selected_uploader.short_name())
                        .width(105.0)
                        .show_ui(ui, |ui| {
                            for &p in UploaderId::ALL {
                                ui.selectable_value(&mut self.history.selected_uploader, p, p.label());
                            }
                        });
                    let lbl_up = if self.history.uploading { " ⏳ Загрузка ZIP... " } else { " ☁ Загрузить ZIP в облако " };
                    let btn = egui::Button::new(egui::RichText::new(lbl_up).size(14.0).color(if can_up { egui::Color32::from_rgb(80, 200, 120) } else { egui::Color32::GRAY }));
                    let r = ui.add_enabled(can_up, btn);
                    if !zip_ok {
                        r.on_disabled_hover_text("Сначала выберите пары и создайте matches.zip.");
                    } else if r.clicked() {
                        match start_cloud_upload(&mut self.history, "matches.zip", Some(0)) {
                            Ok(()) => { self.status_text = "⏳ Загрузка matches.zip на облако...".to_string(); }
                            Err(err) => { self.status_text = err; }
                        }
                    }
                    if zip_ok {
                        if ui.button("📁 Показать ZIP").clicked() {
                            let _ = create_hidden_cmd("explorer").args(["/select,matches.zip"]).spawn();
                        }
                    } else if let Some(parent) = path.parent() {
                        if parent.exists() && ui.button("📁 Показать файлы").clicked() {
                            let _ = create_hidden_cmd("explorer").arg(parent).spawn();
                        }
                    }
                }
            });

            if let Some((z_prog, ref z_lbl)) = zip_status_ui {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(z_lbl).color(egui::Color32::from_rgb(255, 220, 120)).size(13.0));
                });
                ui.add(egui::ProgressBar::new(z_prog).show_percentage());
            } else if self.finished_html.is_some() {
                ui.add_space(4.0);
                let (zip_ready, total_bytes) = self.picker.lock().map(|st| (st.zip_created, selected_total_bytes(&st.pairs))).unwrap_or((false, 0));
                if self.history.uploading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new("Идёт отправка matches.zip на файлообменник...").color(egui::Color32::from_rgb(160, 220, 255)).size(12.0));
                    });
                } else if let Some(url) = self.history.entries.first().map(|e| e.bugsmash_url.clone()).filter(|u| !u.is_empty()) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Ссылка на архив:").color(egui::Color32::from_rgb(180, 220, 180)).size(12.0));
                        ui.hyperlink_to(&url, &url);
                        if ui.small_button("📋 Копировать").clicked() {
                            ui.output_mut(|o| o.copied_text = url);
                        }
                    });
                } else if zip_ready {
                    ui.label(egui::RichText::new(format!("Архив matches.zip готов ({}). Нажмите «Загрузить ZIP в облако» для получения прямой ссылки.", format_mb(total_bytes)))
                        .color(egui::Color32::from_rgb(180, 220, 180)).size(11.5));
                } else {
                    ui.label(egui::RichText::new("Откройте окно отбора пар, отметьте нужные кадры и нажмите «Сохранить выбранные», чтобы создать matches.zip.")
                        .color(egui::Color32::from_rgb(245, 210, 120)).size(11.5));
                }
            }

            ui.add_space(6.0);
            ui.separator();
            ui.label(egui::RichText::new("💡 Совет: можно просто выделить два файла в проводнике и бросить их в это окно.")
                .weak().size(11.0));
        });

        render_history_window(ctx, &mut self.history);

        if self.show_about {
            let mut open = self.show_about;
            egui::Window::new("О программе — ScreenCompare")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(380.0);
                    ui.heading(format!("🎬 ScreenCompare v{}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(4.0);
                    ui.label("Утилита точного поиска совпадающих кадров между двумя видеорелизами, визуального блинк-сравнения и генерации веб-отчётов.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Автор / связь:").strong());
                        ui.hyperlink_to("a.tit.off@gmail.com", "mailto:a.tit.off@gmail.com");
                        if ui.small_button("📋 Копировать").clicked() {
                            ui.output_mut(|o| o.copied_text = "a.tit.off@gmail.com".to_string());
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Горячие клавиши в окне отбора пар:").strong());
                    ui.label("• Пробел / клик по кадру — переключить Кадр А / Кадр Б");
                    ui.label("• ← / → — переход между парами");
                    ui.label("• Enter — включить / исключить пару из отчёта");
                    ui.label("• Колесо мыши / двойной клик — масштаб 50%–800% / сброс");
                    ui.label("• Esc — закрыть окно отбора");
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Закрыть").clicked() {
                            self.show_about = false;
                        }
                    });
                });
            if !open {
                self.show_about = false;
            }
        }

        render_updater_window(ctx, &mut self.updater);

        let picker_vp_id = egui::ViewportId::from_hash_of("pair_picker_fullscreen");
        if self.picker.lock().map(|st| st.is_open).unwrap_or(false) {
            let picker_arc = Arc::clone(&self.picker);
            ctx.show_viewport_deferred(
                picker_vp_id,
                egui::ViewportBuilder::default()
                    .with_title("ScreenCompare — Полноэкранный отбор пар (A / B)")
                    .with_maximized(true),
                move |ctx, _class| render_pair_picker_viewport(ctx, &picker_arc),
            );
        } else {
            ctx.send_viewport_cmd_to(picker_vp_id, egui::ViewportCommand::Close);
        }

        if self.is_running || zip_status_ui.is_some() {
            ctx.request_repaint();
        }
    }
}

impl ScreenCompareApp {
    fn start_processing(&mut self) {
        self.samples = ((self.count as f32) * 1.5).round() as usize;
        self.is_running = true;
        self.progress = 0.0;
        self.status_text = "Запуск фонового процесса...".to_string();
        self.finished_html = None;

        let (tx, rx) = channel();
        self.rx = Some(rx);

        let video_a = PathBuf::from(self.video_a.trim());
        let video_b = PathBuf::from(self.video_b.trim());
        let from_sec = parse_time(&self.from_time_str).unwrap_or(300.0);
        let to_sec = parse_time(&self.to_time_str).unwrap_or(1500.0);
        let count = self.count;
        let samples = self.samples;
        let window_sec = self.window_sec;

        thread::spawn(move || {
            let res = run_comparison(video_a, video_b, from_sec, to_sec, count, samples, window_sec, &tx);
            let _ = tx.send(WorkerMsg::Done(res));
        });
    }
}

fn find_ffmpeg() -> Result<PathBuf, String> {
    let bin = if cfg!(target_os = "windows") { "ffmpeg.exe" } else { "ffmpeg" };
    if cfg!(target_os = "windows") && Path::new(DEFAULT_FFMPEG).exists() {
        return Ok(PathBuf::from(DEFAULT_FFMPEG));
    }
    if Path::new(bin).exists() {
        return Ok(PathBuf::from(bin));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&paths) {
            let candidate = p.join(bin);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(format!("FFmpeg не найден! Укажите путь или поместите {} рядом с программой.", bin))
}

fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 3 {
            let h: f64 = parts[0].parse().ok()?;
            let m: f64 = parts[1].parse().ok()?;
            let sec: f64 = parts[2].parse().ok()?;
            return Some(h * 3600.0 + m * 60.0 + sec);
        } else if parts.len() == 2 {
            let m: f64 = parts[0].parse().ok()?;
            let sec: f64 = parts[1].parse().ok()?;
            return Some(m * 60.0 + sec);
        }
    }
    s.parse::<f64>().ok()
}

fn format_time(sec: f64) -> String {
    let s_int = sec as u64;
    let ms = ((sec - s_int as f64) * 1000.0) as u64;
    let ss = s_int % 60;
    let mm = (s_int / 60) % 60;
    let hh = s_int / 3600;
    format!("{:02}:{:02}:{:02}.{:03}", hh, mm, ss, ms)
}

fn get_duration(video: &Path, ffmpeg: &Path) -> f64 {
    let ffprobe = ffmpeg.with_file_name("ffprobe.exe");
    let cmd_path = if ffprobe.exists() { ffprobe } else { PathBuf::from("ffprobe.exe") };

    let output = create_hidden_cmd(cmd_path)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(video)
        .output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if let Ok(val) = text.parse::<f64>() {
            return val;
        }
    }
    7200.0
}

fn log_debug(msg: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open("search_debug.log") {
        let _ = writeln!(f, "{}", msg);
    }
}

fn parse_showinfo_pts(stderr: &[u8]) -> Vec<f64> {
    let text = String::from_utf8_lossy(stderr);
    let mut pts_list = Vec::new();
    for line in text.lines() {
        if let Some(pos) = line.find("pts_time:") {
            let rest = &line[pos + 9..];
            let num_str: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            if let Ok(val) = num_str.parse::<f64>() {
                pts_list.push(val);
            }
        }
    }
    pts_list
}

fn extract_native_frame(video: &Path, approx_ts: f64, ffmpeg: &Path) -> Option<(f64, FrameSig)> {
    let seek_pre = (approx_ts - 1.5).max(0.0);
    let seek_post = (approx_ts - seek_pre).max(0.0);
    let output = create_hidden_cmd(ffmpeg)
        .args([
            "-ss", &format!("{:.3}", seek_pre),
            "-i", &video.to_string_lossy(),
            "-ss", &format!("{:.3}", seek_post),
            "-frames:v", "1",
            "-vf", "scale=160:160:flags=area,format=gray,showinfo",
            "-f", "rawvideo",
            "-pix_fmt", "gray",
            "-"
        ])
        .output()
        .ok()?;

    let frame_bytes = 160 * 160;
    if output.stdout.len() < frame_bytes {
        return None;
    }
    let sig = compute_frame_sig(&output.stdout[..frame_bytes])?;
    Some((approx_ts, sig))
}

fn find_best_match_in_window(
    video: &Path,
    start_sec: f64,
    duration_sec: f64,
    sig_a: &FrameSig,
    ffmpeg: &Path,
) -> Option<(f64, u32, f32, f32, usize)> {
    let seek_pre = (start_sec - 1.5).max(0.0);
    let seek_post = (start_sec - seek_pre).max(0.0);
    let mut child = create_hidden_cmd(ffmpeg)
        .args([
            "-ss", &format!("{:.3}", seek_pre),
            "-i", &video.to_string_lossy(),
            "-ss", &format!("{:.3}", seek_post),
            "-t", &format!("{:.3}", duration_sec),
            "-vf", "scale=160:160:flags=area,format=gray",
            "-f", "rawvideo",
            "-pix_fmt", "gray",
            "-"
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let mut reader = BufReader::with_capacity(160 * 160 * 8, stdout);
    let mut buf = vec![0u8; 160 * 160];

    let mut frame_idx: usize = 0;
    let mut best_match: Option<(f64, u32, f32, f32, usize)> = None;
    let mut best_any: Option<(f64, u32, f32)> = None;

    loop {
        if reader.read_exact(&mut buf).is_err() {
            break;
        }
        let approx_ts = start_sec + (frame_idx as f64) * 0.041708;
        frame_idx += 1;

        if let Some(sig_b) = compute_frame_sig(&buf) {
            let hdiff = hamming_256(&sig_a.hash, &sig_b.hash);
            let base_zncc = zncc_score(sig_a, &sig_b);
            if best_any.map_or(true, |(_, _, s)| base_zncc > s) {
                best_any = Some((approx_ts, hdiff, base_zncc));
            }
            if hdiff <= 68 && base_zncc >= 0.76 {
                let det = detail_score(sig_a, &sig_b);
                if best_match.map_or(true, |(_, _, _, best_d, _)| det > best_d) {
                    best_match = Some((approx_ts, hdiff, base_zncc, det, 0));
                }
            }
        }
    }

    let _ = child.wait();
    if let Some((ts, hd, zn, dt, _)) = best_match {
        Some((ts, hd, zn, dt, frame_idx))
    } else {
        if let Some((ts, hd, zn)) = best_any {
            log_debug(&format!("    [window {:.2}..{:.2}s] frames_read={} | NO PASS (best_any: ts={:.3}s hdiff={} zncc={:.4})", start_sec, start_sec + duration_sec, frame_idx, ts, hd, zn));
        } else {
            log_debug(&format!("    [window {:.2}..{:.2}s] frames_read={} | EMPTY/FLAT FRAMES", start_sec, start_sec + duration_sec, frame_idx));
        }
        None
    }
}

fn detail_score(a: &FrameSig, b: &FrameSig) -> f32 {
    let base = zncc_score(a, b);
    const C: usize = 136;
    let mut dot_g = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for y in 1..(C - 1) {
        for x in 1..(C - 1) {
            let idx = y * C + x;
            let gx_a = a.norm_luma[idx + 1] - a.norm_luma[idx - 1];
            let gy_a = a.norm_luma[idx + C] - a.norm_luma[idx - C];
            let gx_b = b.norm_luma[idx + 1] - b.norm_luma[idx - 1];
            let gy_b = b.norm_luma[idx + C] - b.norm_luma[idx - C];
            dot_g += gx_a * gx_b + gy_a * gy_b;
            norm_a += gx_a * gx_a + gy_a * gy_a;
            norm_b += gx_b * gx_b + gy_b * gy_b;
        }
    }
    let edge_corr = if norm_a > 1e-6 && norm_b > 1e-6 {
        (dot_g / (norm_a * norm_b).sqrt()).clamp(-1.0, 1.0)
    } else {
        base
    };
    0.55 * base + 0.45 * edge_corr
}

fn refine_exact_match(
    file_a: &Path,
    ts_a: f64,
    file_b: &Path,
    approx_ts_b: f64,
    ffmpeg: &Path,
) -> Option<(f64, f64, f64, usize, u32, f32, f32)> {
    // Извлекаем высокодетальный кадр А (320x320) и его точный PTS
    let (exact_ts_a, sig_a_hi) = extract_hires_frame(file_a, ts_a, ffmpeg)?;

    // Декодируем окно +-0.80с (~38 нативных кадров) вокруг approx_ts_b в 320x320
    let win_start = (approx_ts_b - 0.80).max(0.0);
    let seek_pre = (win_start - 1.5).max(0.0);
    let seek_post = (win_start - seek_pre).max(0.0);

    let output = create_hidden_cmd(ffmpeg)
        .args([
            "-ss", &format!("{:.4}", seek_pre),
            "-i", &file_b.to_string_lossy(),
            "-ss", &format!("{:.4}", seek_post),
            "-t", "1.60",
            "-vf", "scale=320:320:flags=lanczos,format=gray",
            "-f", "rawvideo",
            "-pix_fmt", "gray",
            "-"
        ])
        .output()
        .ok()?;

    let frame_bytes = 320 * 320;
    let count = output.stdout.len() / frame_bytes;
    if count == 0 {
        return None;
    }
    let _pts_list = parse_showinfo_pts(&output.stderr);

    let mut best: Option<(f64, usize, u32, f32, f32)> = None;
    log_debug(&format!("    [refine 320x320] exact_ts_a={:.4}s | candidates={}:", exact_ts_a, count));
    for i in 0..count {
        let slice = &output.stdout[i * frame_bytes..(i + 1) * frame_bytes];
        if let Some(sig_b_hi) = compute_hires_sig(slice) {
            let hdiff = hamming_256(&sig_a_hi.hash, &sig_b_hi.hash);
            let base = zncc_score(&sig_a_hi, &sig_b_hi);
            let det = hires_detail_score(&sig_a_hi, &sig_b_hi);
            let ts_b = win_start + (i as f64) * (1001.0 / 24000.0);
            log_debug(&format!("      cand[{:02}] pts_b={:.4}s | hdiff={:3} | zncc={:.4} | det={:.4}", i, ts_b, hdiff, base, det));
            if best.map_or(true, |(_, _, _, _, best_d)| det > best_d) {
                best = Some((ts_b, i, hdiff, base, det));
            }
        }
    }

    let (best_ts_b, best_idx, hdiff, base, det) = best?;
    Some((exact_ts_a, best_ts_b, win_start, best_idx, hdiff, base, det))
}

fn extract_hires_frame(video: &Path, approx_ts: f64, ffmpeg: &Path) -> Option<(f64, FrameSig)> {
    let seek_pre = (approx_ts - 1.5).max(0.0);
    let seek_post = (approx_ts - seek_pre).max(0.0);
    let output = create_hidden_cmd(ffmpeg)
        .args([
            "-ss", &format!("{:.4}", seek_pre),
            "-i", &video.to_string_lossy(),
            "-ss", &format!("{:.4}", seek_post),
            "-frames:v", "1",
            "-vf", "scale=320:320:flags=lanczos,format=gray,showinfo",
            "-f", "rawvideo",
            "-pix_fmt", "gray",
            "-"
        ])
        .output()
        .ok()?;

    let frame_bytes = 320 * 320;
    if output.stdout.len() < frame_bytes {
        return None;
    }
    let sig = compute_hires_sig(&output.stdout[..frame_bytes])?;
    Some((approx_ts, sig))
}

fn compute_hires_sig(raw_gray: &[u8]) -> Option<FrameSig> {
    if raw_gray.len() < 320 * 320 {
        return None;
    }
    const W: usize = 320;
    const C: usize = 272;
    const OFF: usize = 24;

    let mut crop = vec![0f32; C * C];
    let mut sum = 0.0f32;
    for y in 0..C {
        for x in 0..C {
            let v = raw_gray[(y + OFF) * W + (x + OFF)] as f32;
            crop[y * C + x] = v;
            sum += v;
        }
    }
    let n = (C * C) as f32;
    let mean = sum / n;
    let mut sq_sum = 0.0f32;
    for v in &mut crop {
        *v -= mean;
        sq_sum += *v * *v;
    }
    let std_dev = sq_sum.sqrt();
    if std_dev < 1e-3 {
        return None;
    }
    for v in &mut crop {
        *v /= std_dev;
    }

    let mut hash = [0u64; 4];
    let mut bit = 0usize;
    for r in 0..8 {
        let y = (r * C) / 8 + 16;
        for c in 0..16 {
            let x1 = (c * C) / 17;
            let x2 = ((c + 1) * C) / 17;
            if crop[y * C + x2] > crop[y * C + x1] {
                hash[bit / 64] |= 1u64 << (bit % 64);
            }
            bit += 1;
        }
    }
    for r in 0..16 {
        let y1 = (r * C) / 17;
        let y2 = ((r + 1) * C) / 17;
        for c in 0..8 {
            let x = (c * C) / 8 + 16;
            if crop[y2 * C + x] > crop[y1 * C + x] {
                hash[bit / 64] |= 1u64 << (bit % 64);
            }
            bit += 1;
        }
    }

    Some(FrameSig { hash, norm_luma: crop })
}

fn hires_detail_score(a: &FrameSig, b: &FrameSig) -> f32 {
    let base = zncc_score(a, b);
    const C: usize = 272;
    let mut dot_g = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for y in 1..(C - 1) {
        for x in 1..(C - 1) {
            let idx = y * C + x;
            let gx_a = a.norm_luma[idx + 1] - a.norm_luma[idx - 1];
            let gy_a = a.norm_luma[idx + C] - a.norm_luma[idx - C];
            let gx_b = b.norm_luma[idx + 1] - b.norm_luma[idx - 1];
            let gy_b = b.norm_luma[idx + C] - b.norm_luma[idx - C];
            dot_g += gx_a * gx_b + gy_a * gy_b;
            norm_a += gx_a * gx_a + gy_a * gy_a;
            norm_b += gx_b * gx_b + gy_b * gy_b;
        }
    }
    let edge_corr = if norm_a > 1e-6 && norm_b > 1e-6 {
        (dot_g / (norm_a * norm_b).sqrt()).clamp(-1.0, 1.0)
    } else {
        base
    };
    0.45 * base + 0.55 * edge_corr
}

fn is_valid_png(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 1024).unwrap_or(false)
}

fn save_window_frame_png(video: &Path, win_start: f64, frame_idx: usize, out: &Path, ffmpeg: &Path) -> bool {
    let seek_pre = (win_start - 1.5).max(0.0);
    let seek_post = (win_start - seek_pre).max(0.0);
    let vf = format!("trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS,format=rgb24", frame_idx, frame_idx + 1);
    let res = create_hidden_cmd(ffmpeg)
        .args([
            "-y",
            "-ss", &format!("{:.4}", seek_pre),
            "-i", &video.to_string_lossy(),
            "-ss", &format!("{:.4}", seek_post),
            "-vf", &vf,
            "-frames:v", "1",
            "-c:v", "png",
            &out.to_string_lossy(),
        ])
        .output();

    match res {
        Ok(o) if o.status.success() && is_valid_png(out) => true,
        Ok(o) => {
            log_debug(&format!("save_window_frame_png failed for {:?}: status={:?}, stderr={}", out, o.status.code(), String::from_utf8_lossy(&o.stderr)));
            false
        }
        Err(e) => {
            log_debug(&format!("save_window_frame_png spawn err for {:?}: {}", out, e));
            false
        }
    }
}

fn save_frame_png(video: &Path, exact_pts: f64, out: &Path, ffmpeg: &Path) -> bool {
    let seek_pre = (exact_pts - 1.5).max(0.0);
    let seek_post = (exact_pts - seek_pre).max(0.0);
    let res = create_hidden_cmd(ffmpeg)
        .args([
            "-y",
            "-ss", &format!("{:.4}", seek_pre),
            "-i", &video.to_string_lossy(),
            "-ss", &format!("{:.4}", seek_post),
            "-vf", "format=rgb24",
            "-frames:v", "1",
            "-c:v", "png",
            &out.to_string_lossy(),
        ])
        .output();

    match res {
        Ok(o) if o.status.success() && is_valid_png(out) => true,
        Ok(o) => {
            log_debug(&format!("save_frame_png failed for {:?}: status={:?}, stderr={}", out, o.status.code(), String::from_utf8_lossy(&o.stderr)));
            false
        }
        Err(e) => {
            log_debug(&format!("save_frame_png spawn err for {:?}: {}", out, e));
            false
        }
    }
}

fn run_comparison(
    file_a: PathBuf,
    file_b: PathBuf,
    from_sec: f64,
    to_sec: f64,
    count: usize,
    samples: usize,
    window_sec: f64,
    tx: &Sender<WorkerMsg>,
) -> Result<(PathBuf, Vec<MatchRecord>, String, String), String> {
    let ffmpeg = find_ffmpeg()?;

    let _ = std::fs::write("search_debug.log", "");
    let dur_a = get_duration(&file_a, &ffmpeg);
    let dur_b = get_duration(&file_b, &ffmpeg);
    log_debug(&format!("=== START COMPARISON ==="));
    log_debug(&format!("File A: {:?} (dur={:.3}s)", file_a, dur_a));
    log_debug(&format!("File B: {:?} (dur={:.3}s)", file_b, dur_b));
    log_debug(&format!("Params: from={:.2}s to={:.2}s samples={} count={} window={:.2}s", from_sec, to_sec, samples, count, window_sec));

    let start_a = from_sec.min(dur_a);
    let end_a = to_sec.min(dur_a);
    let span_a = (end_a - start_a).max(1.0);
    let step = span_a / (samples as f64 + 1.0);

    let sample_timestamps: Vec<f64> = (1..=samples).map(|i| start_a + step * i as f64).collect();

    let _ = tx.send(WorkerMsg::Status(format!("Извлечение {} опорных кадров из Релиза А...", samples)));
    let mut points_a = Vec::new();

    for (idx, &ts) in sample_timestamps.iter().enumerate() {
        if let Some((exact_pts_a, sig)) = extract_native_frame(&file_a, ts, &ffmpeg) {
            points_a.push((exact_pts_a, sig));
        }
        let p = (idx + 1) as f32 / samples as f32 * 0.30;
        let _ = tx.send(WorkerMsg::Progress(p));
    }

    if points_a.is_empty() {
        return Err("Не удалось извлечь информативные кадры из Релиза А.".to_string());
    }

    let _ = tx.send(WorkerMsg::Status("Покадровый поиск и высокоточная сверка пар...".to_string()));
    let rate_guess = if dur_a > 0.0 { dur_b / dur_a } else { 1.0 };
    let mut matches: Vec<(f64, f64, f64, usize, u32, f32)> = Vec::new();
    let mut current_delta: Option<f64> = None;

    for (idx, (ts_a, sig_a)) in points_a.iter().enumerate() {
        let primary_center = match current_delta {
            Some(d) => (ts_a + d).clamp(0.0, dur_b),
            None => (ts_a * rate_guess).clamp(0.0, dur_b),
        };

        // Сначала ищем в основном окне вокруг текущего сдвига, при неудаче — в расширенном окне (на случай вырезанных/добавленных сцен)
        let mut search_windows = vec![(primary_center, window_sec)];
        let fallback_center = (ts_a * rate_guess).clamp(0.0, dur_b);
        if (fallback_center - primary_center).abs() > window_sec * 0.5 {
            search_windows.push((fallback_center, window_sec * 1.5));
        }
        search_windows.push((primary_center, (window_sec * 4.0).max(60.0)));

        for (center_b, win) in search_windows {
            let start_b = (center_b - win).max(0.0);
            let end_b = (center_b + win).min(dur_b);
            let span_b = (end_b - start_b).max(0.2);

            log_debug(&format!("Point #{:02} (ts_a={:.4}s) -> searching window {:.2}..{:.2}s (delta={:?})", idx + 1, ts_a, start_b, end_b, current_delta));
            if let Some((approx_ts_b, coarse_hdiff, coarse_zncc, coarse_det, f_cnt)) =
                find_best_match_in_window(&file_b, start_b, span_b, sig_a, &ffmpeg)
            {
                log_debug(&format!("    [coarse HIT] frames={} approx_ts_b={:.4}s hdiff={} zncc={:.4} det={:.4}", f_cnt, approx_ts_b, coarse_hdiff, coarse_zncc, coarse_det));
                if coarse_zncc >= 0.78 && coarse_hdiff <= 64 {
                    if let Some((exact_a, exact_b, win_start_b, best_idx_b, hi_hdiff, hi_zncc, hi_det)) =
                        refine_exact_match(&file_a, *ts_a, &file_b, approx_ts_b, &ffmpeg)
                    {
                        if hi_zncc >= 0.78 && hi_hdiff <= 64 {
                            log_debug(&format!("    >>> ACCEPTED PAIR: A={:.4}s B={:.4}s (idx={}) (delta={:.3}s) hi_hdiff={} hi_zncc={:.4} hi_det={:.4}", exact_a, exact_b, best_idx_b, exact_b - exact_a, hi_hdiff, hi_zncc, hi_det));
                            matches.push((exact_a, exact_b, win_start_b, best_idx_b, hi_hdiff, hi_det));
                            current_delta = Some(exact_b - exact_a);
                            break;
                        } else {
                            log_debug(&format!("    xxx REJECTED BY REFINE THRESHOLD: hi_zncc={:.4} hi_hdiff={}", hi_zncc, hi_hdiff));
                        }
                    }
                }
            }
        }

        let p = 0.30 + (idx + 1) as f32 / points_a.len() as f32 * 0.50;
        let _ = tx.send(WorkerMsg::Progress(p));
        let _ = tx.send(WorkerMsg::Status(format!(
            "Обработано точек: {}/{} | Найдено пар: {}/{}",
            idx + 1,
            points_a.len(),
            matches.len(),
            count
        )));
        if matches.len() >= count {
            log_debug(&format!("Reached target count={} pairs, stopping search early.", count));
            break;
        }
    }

    if matches.is_empty() {
        return Err("Точных совпадений не найдено (непохожие кадры отсеяны). Попробуйте увеличить окно поиска или число точек.".to_string());
    }

    matches.sort_by(|a, b| b.5.partial_cmp(&a.5).unwrap_or(std::cmp::Ordering::Equal));
    matches.truncate(count);
    matches.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    let out_dir = PathBuf::from("matches");
    let _ = std::fs::create_dir_all(&out_dir);
    if let Ok(entries) = std::fs::read_dir(&out_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("png") {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    tx.send(WorkerMsg::Status(format!("Экспорт {} точных пар PNG без сжатия...", matches.len())));
    let mut export_records = Vec::new();

    for (i, &(ts_a, ts_b, win_start_b, best_idx_b, diff, _score)) in matches.iter().enumerate() {
        let pair_id = i + 1;
        let fn_a = out_dir.join(format!("pair_{:02}_A.png", pair_id));
        let fn_b = out_dir.join(format!("pair_{:02}_B.png", pair_id));

        let ok_a = save_frame_png(&file_a, ts_a, &fn_a, &ffmpeg);
        let ok_b = if save_window_frame_png(&file_b, win_start_b, best_idx_b, &fn_b, &ffmpeg) {
            true
        } else {
            log_debug(&format!("Falling back to save_frame_png for pair {} B at ts_b={:.4}", pair_id, ts_b));
            save_frame_png(&file_b, ts_b, &fn_b, &ffmpeg)
        };
        log_debug(&format!("Export pair {:02}: ok_a={} ({:?}) ok_b={} ({:?})", pair_id, ok_a, fn_a, ok_b, fn_b));

        export_records.push(MatchRecord {
            id: pair_id,
            time_a_sec: ts_a,
            time_b_sec: ts_b,
            time_a_formatted: format_time(ts_a),
            time_b_formatted: format_time(ts_b),
            time_delta_sec: ts_b - ts_a,
            hash_diff: diff,
            file_a: fn_a.file_name().unwrap().to_string_lossy().to_string(),
            file_b: fn_b.file_name().unwrap().to_string_lossy().to_string(),
        });

        let p = 0.80 + (i + 1) as f32 / matches.len() as f32 * 0.15;
        tx.send(WorkerMsg::Progress(p));
    }

    let json_content = serde_json::to_string_pretty(&export_records).unwrap_or_else(|_| "[]".to_string());
    std::fs::write(out_dir.join("matches.json"), &json_content);

    let name_a = file_a.file_name().unwrap_or_default().to_string_lossy();
    let name_b = file_b.file_name().unwrap_or_default().to_string_lossy();
    let html_code = generate_html(&json_content, &name_a, &name_b);

    let html_path = out_dir.join("index.html");
    std::fs::write(&html_path, &html_code);
    let _ = std::fs::write(out_dir.join("viewer.html"), &html_code);

    let _ = std::fs::remove_file("matches.zip");
    let _ = tx.send(WorkerMsg::Status("Кадры извлечены. Выберите нужные пары для сборки ZIP.".to_string()));

    Ok((html_path, export_records, name_a.to_string(), name_b.to_string()))
}

fn generate_html(data_json: &str, name_a: &str, name_b: &str) -> String {
    format!(r##"<!DOCTYPE html>
<html lang="ru">
<head>
    <meta charset="UTF-8">
    <title>Инспектор: {name_a} vs {name_b}</title>
    <style>
        :root {{
            --bg: #090a0d;
            --surface: #14171f;
            --surface-active: #10263d;
            --border: #262a36;
            --border-active: #38bdf8;
            --text: #f8fafc;
            --muted: #7c8497;
            --accent-glow: 0 0 24px rgba(56, 189, 248, 0.35);
            --green: #22c55e;
        }}
        * {{ box-sizing: border-box; user-select: none; }}
        body {{
            margin: 0; padding: 12px 20px; background: var(--bg); color: var(--text);
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            display: flex; flex-direction: column; align-items: center; overflow-x: hidden;
        }}
        .app-container {{ width: 100%; max-width: 1380px; display: flex; flex-direction: column; gap: 10px; }}
        .tabs-grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }}
        .release-tab {{
            background: var(--surface); border: 2px solid var(--border); border-radius: 10px;
            padding: 10px 16px; display: flex; flex-direction: column; gap: 4px; cursor: pointer; transition: all 0.15s ease;
        }}
        .release-tab.active {{ background: var(--surface-active); border-color: var(--border-active); box-shadow: var(--accent-glow); }}
        .tab-status {{ display: flex; justify-content: space-between; font-size: 0.8rem; font-weight: 800; text-transform: uppercase; color: var(--muted); }}
        .release-tab.active .tab-status {{ color: var(--border-active); }}
        .file-title {{ font-size: 1rem; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
        .time-badge {{ font-family: "JetBrains Mono", Consolas, monospace; font-size: 1.2rem; font-weight: 800; }}
        .controls-bar {{
            background: var(--surface); border: 1px solid var(--border); border-radius: 10px;
            padding: 8px 14px; display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 10px;
        }}
        .btn {{
            background: #202430; color: var(--text); border: 1px solid var(--border); padding: 7px 15px;
            border-radius: 6px; cursor: pointer; font-size: 0.9rem; font-weight: 600;
        }}
        .btn:hover {{ background: #2d3345; }}
        .btn-toggle {{ background: #15273e; border: 1px solid var(--border-active); color: var(--border-active); font-weight: 700; padding: 8px 20px; }}
        .btn-toggle:hover {{ background: var(--border-active); color: #000; }}
        .zoom-group {{ display: flex; align-items: center; gap: 6px; background: #0d0f14; padding: 4px 8px; border-radius: 6px; border: 1px solid var(--border); }}
        .zoom-label {{ font-size: 0.8rem; color: var(--muted); font-weight: bold; }}
        .btn-zoom {{ padding: 4px 10px; font-size: 0.85rem; border-radius: 4px; border: 1px solid transparent; background: #1a1e28; color: var(--text); cursor: pointer; }}
        .btn-zoom:hover {{ border-color: var(--border-active); }}
        .btn-zoom.active {{ background: var(--border-active); color: #000; font-weight: bold; }}
        select {{ background: #0c0e12; color: var(--text); border: 1px solid var(--border); padding: 7px 10px; border-radius: 6px; font-size: 0.9rem; font-weight: 600; cursor: pointer; }}
        .delta-text {{ color: var(--green); font-family: monospace; font-size: 0.95rem; font-weight: bold; }}
        .viewport-wrapper {{
            position: relative; width: 100%; height: 72vh; min-height: 520px; background: #000;
            border-radius: 10px; border: 2px solid var(--border); overflow: hidden; cursor: grab;
            display: flex; justify-content: center; align-items: center;
        }}
        .viewport-wrapper.dragging {{ cursor: grabbing; }}
        .zoom-stage {{ position: absolute; transform-origin: 0 0; will-change: transform; pointer-events: none; }}
        .zoom-stage img {{ display: block; max-width: none; image-rendering: pixelated; }}
        .frame-indicator {{
            position: absolute; top: 14px; left: 14px; background: rgba(9, 10, 13, 0.85); border: 1px solid var(--border-active);
            padding: 6px 14px; border-radius: 6px; font-size: 0.9rem; font-weight: 800; color: var(--border-active); pointer-events: none; backdrop-filter: blur(6px); z-index: 10;
        }}
        .zoom-badge {{
            position: absolute; bottom: 14px; right: 14px; background: rgba(9, 10, 13, 0.85); border: 1px solid var(--border);
            padding: 4px 10px; border-radius: 6px; font-family: monospace; font-size: 0.85rem; color: var(--muted); pointer-events: none; z-index: 10;
        }}
        .hints {{ text-align: center; font-size: 0.85rem; color: var(--muted); }}
        .key {{ background: #1e222d; border: 1px solid var(--border); padding: 2px 7px; border-radius: 4px; color: var(--text); font-family: monospace; }}
    </style>
</head>
<body>
<div class="app-container">
    <div class="tabs-grid">
        <div class="release-tab active" id="tabA" onclick="setRelease('A')">
            <div class="tab-status"><span>[1] Релиз А</span><span id="txtStateA" style="color: var(--border-active);">● В КАДРЕ</span></div>
            <div class="file-title" title="{name_a}">{name_a}</div>
            <div class="time-badge" id="timeA">--:--:--</div>
        </div>
        <div class="release-tab" id="tabB" onclick="setRelease('B')">
            <div class="tab-status"><span>[2] Релиз Б</span><span id="txtStateB">СКРЫТ</span></div>
            <div class="file-title" title="{name_b}">{name_b}</div>
            <div class="time-badge" id="timeB">--:--:--</div>
        </div>
    </div>
    <div class="controls-bar" onclick="event.stopPropagation()">
        <div style="display: flex; gap: 8px; align-items: center;">
            <button class="btn" onclick="prevPair()">← (A / ←)</button>
            <select id="pairSelect" onchange="loadPair(this.value)"></select>
            <button class="btn" onclick="nextPair()">(D / →) →</button>
            <span class="delta-text" id="deltaLabel">Сдвиг: +0.000s</span>
        </div>
        <div class="zoom-group">
            <span class="zoom-label">ЛУПА:</span>
            <button class="btn-zoom active" id="btnZoom1" onclick="setZoomLevel(1.0)">1x (Fit)</button>
            <button class="btn-zoom" id="btnZoom4" onclick="setZoomLevel(4.0)">4x</button>
            <button class="btn-zoom" id="btnZoom8" onclick="setZoomLevel(8.0)">8x</button>
        </div>
        <button class="btn btn-toggle" onclick="toggleRelease()">Сменить кадр А / Б (Клик / Пробел)</button>
    </div>
    <div class="viewport-wrapper" id="viewport">
        <div class="zoom-stage" id="stage"><img id="mainImg" src="" alt="Frame"></div>
        <div class="frame-indicator" id="indicator">РЕЛИЗ А</div>
        <div class="zoom-badge" id="zoomBadge">1.0x</div>
    </div>
    <div class="hints">
        Управление: <span class="key">Клик</span> или <span class="key">Пробел</span> — блинк А/Б | 
        <span class="key">Колесо мыши</span> — зум | <span class="key">Зажать мышь</span> — перемещение (Pan) | 
        <span class="key">4</span> / <span class="key">8</span> / <span class="key">0</span> — зум 4x / 8x / сброс
    </div>
</div>
<script>
    const matches = {data_json};
    let currentIdx = 0, activeRelease = 'A', scale = 1.0, panX = 0, panY = 0;
    let isDragging = false, startX = 0, startY = 0, hasMoved = false;

    const viewport = document.getElementById("viewport"), stage = document.getElementById("stage");
    const mainImg = document.getElementById("mainImg"), zoomBadge = document.getElementById("zoomBadge"), selectEl = document.getElementById("pairSelect");

    matches.forEach((m, idx) => {{
        const opt = document.createElement("option");
        opt.value = idx;
        opt.textContent = `Пара #${{idx + 1}} (dHash: ${{m.hash_diff}})`;
        selectEl.appendChild(opt);
    }});

    function updateTransform() {{
        stage.style.transform = `translate(${{panX}}px, ${{panY}}px) scale(${{scale}})`;
        zoomBadge.textContent = scale.toFixed(1) + "x";
        document.getElementById("btnZoom1").classList.toggle("active", Math.abs(scale - 1.0) < 0.1);
        document.getElementById("btnZoom4").classList.toggle("active", Math.abs(scale - 4.0) < 0.1);
        document.getElementById("btnZoom8").classList.toggle("active", Math.abs(scale - 8.0) < 0.1);
    }}

    function resetView() {{
        if (!mainImg.naturalWidth) return;
        const vw = viewport.clientWidth, vh = viewport.clientHeight;
        const iw = mainImg.naturalWidth, ih = mainImg.naturalHeight;
        scale = Math.min(vw / iw, vh / ih);
        panX = (vw - iw * scale) / 2;
        panY = (vh - ih * scale) / 2;
        updateTransform();
    }}

    function setZoomLevel(targetZoom) {{
        if (!mainImg.naturalWidth) return;
        if (targetZoom === 1.0) {{ resetView(); return; }}
        const centerX = viewport.clientWidth / 2, centerY = viewport.clientHeight / 2;
        const imgX = (centerX - panX) / scale, imgY = (centerY - panY) / scale;
        scale = targetZoom;
        panX = centerX - imgX * scale;
        panY = centerY - imgY * scale;
        updateTransform();
    }}

    mainImg.onload = () => {{ if (scale <= 1.0 || !panX) resetView(); }};

    function render() {{
        if (!matches || matches.length === 0) return;
        const item = matches[currentIdx];
        selectEl.value = currentIdx;
        document.getElementById("timeA").textContent = item.time_a_formatted;
        document.getElementById("timeB").textContent = item.time_b_formatted;
        const sign = item.time_delta_sec >= 0 ? '+' : '';
        document.getElementById("deltaLabel").textContent = `Сдвиг (Б - А): ${{sign}}${{item.time_delta_sec.toFixed(3)}} сек`;

        const tabA = document.getElementById("tabA"), tabB = document.getElementById("tabB");
        const stateA = document.getElementById("txtStateA"), stateB = document.getElementById("txtStateB");
        const indicator = document.getElementById("indicator");

        if (activeRelease === 'A') {{
            tabA.classList.add("active"); tabB.classList.remove("active");
            stateA.textContent = "● В КАДРЕ"; stateA.style.color = "var(--border-active)";
            stateB.textContent = "СКРЫТ"; stateB.style.color = "var(--muted)";
            indicator.textContent = "РЕЛИЗ А: " + item.time_a_formatted;
            mainImg.src = item.file_a;
        }} else {{
            tabB.classList.add("active"); tabA.classList.remove("active");
            stateB.textContent = "● В КАДРЕ"; stateB.style.color = "var(--border-active)";
            stateA.textContent = "СКРЫТ"; stateA.style.color = "var(--muted)";
            indicator.textContent = "РЕЛИЗ Б: " + item.time_b_formatted;
            mainImg.src = item.file_b;
        }}
    }}

    function loadPair(idx) {{
        currentIdx = parseInt(idx, 10);
        if (isNaN(currentIdx) || currentIdx < 0) currentIdx = 0;
        if (currentIdx >= matches.length) currentIdx = matches.length - 1;
        render();
    }}

    function toggleRelease() {{ activeRelease = activeRelease === 'A' ? 'B' : 'A'; render(); }}
    function setRelease(rel) {{ activeRelease = rel; render(); }}
    function nextPair() {{ if (currentIdx < matches.length - 1) {{ currentIdx++; render(); }} }}
    function prevPair() {{ if (currentIdx > 0) {{ currentIdx--; render(); }} }}

    viewport.addEventListener("wheel", (e) => {{
        e.preventDefault();
        const rect = viewport.getBoundingClientRect();
        const mouseX = e.clientX - rect.left, mouseY = e.clientY - rect.top;
        const zoomFactor = e.deltaY < 0 ? 1.25 : 0.8;
        let newScale = Math.max(0.2, Math.min(scale * zoomFactor, 20.0));
        panX = mouseX - (mouseX - panX) * (newScale / scale);
        panY = mouseY - (mouseY - panY) * (newScale / scale);
        scale = newScale;
        updateTransform();
    }}, {{ passive: false }});

    viewport.addEventListener("mousedown", (e) => {{
        isDragging = true; hasMoved = false;
        startX = e.clientX - panX; startY = e.clientY - panY;
        viewport.classList.add("dragging");
    }});

    window.addEventListener("mousemove", (e) => {{
        if (!isDragging) return;
        const curX = e.clientX - startX, curY = e.clientY - startY;
        if (Math.abs(curX - panX) > 3 || Math.abs(curY - panY) > 3) hasMoved = true;
        panX = curX; panY = curY; updateTransform();
    }});

    window.addEventListener("mouseup", () => {{
        if (isDragging) {{
            isDragging = false;
            viewport.classList.remove("dragging");
            if (!hasMoved) toggleRelease();
        }}
    }});

    window.addEventListener("keydown", (e) => {{
        if (e.target && (e.target.tagName === 'SELECT' || e.target.tagName === 'INPUT')) return;
        if (e.code === "Space") {{ e.preventDefault(); toggleRelease(); }}
        else if (e.code === "ArrowRight" || e.code === "KeyD") nextPair();
        else if (e.code === "ArrowLeft" || e.code === "KeyA") prevPair();
        else if (e.key === "1") setRelease('A');
        else if (e.key === "2") setRelease('B');
        else if (e.key === "4") setZoomLevel(4.0);
        else if (e.key === "8") setZoomLevel(8.0);
        else if (e.key === "0" || e.key === "Escape") resetView();
    }});

    window.addEventListener("resize", () => {{ if (scale <= 1.0) resetView(); }});
    if (matches.length > 0) render();
</script>
</body>
</html>
"##, data_json = data_json, name_a = name_a, name_b = name_b)
}

fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes.as_slice()));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Some(egui::IconData {
        rgba: buf,
        width: info.width,
        height: info.height,
    })
}

fn main() -> eframe::Result<()> {
    cleanup_old_binary();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 480.0])
        .with_min_inner_size([640.0, 420.0])
        .with_drag_and_drop(true);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "ScreenCompare GUI",
        native_options,
        Box::new(|_cc| Box::new(ScreenCompareApp::default())),
    )
}
