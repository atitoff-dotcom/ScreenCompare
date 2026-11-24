use crate::{generate_html, MatchRecord};
use eframe::egui;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

pub const HTMLKIN_MAX_BYTES: u64 = 150 * 1024 * 1024;
const HTML_OVERHEAD_BYTES: u64 = 64 * 1024;

#[derive(Clone)]
pub struct PairItem {
    pub record: MatchRecord,
    pub selected: bool,
    pub size_bytes: u64,
}

pub struct ZipProgress {
    pub progress: f32,
    pub label: String,
    pub done: bool,
    pub error: Option<String>,
    pub kept_items: Vec<PairItem>,
    pub stamp: String,
    pub human: String,
    pub zip_dest: String,
}

pub struct ZipDoneInfo {
    pub pairs_count: usize,
    pub total_bytes: u64,
    pub stamp: String,
    pub human: String,
    pub zip_dest: String,
}

#[derive(Default)]
pub struct PickerState {
    pub is_open: bool,
    pub pairs: Vec<PairItem>,
    pub current_idx: usize,
    pub show_b: bool,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub name_a: String,
    pub name_b: String,
    pub textures: HashMap<String, egui::TextureHandle>,
    pub failed_loads: HashMap<String, String>,
    pub status_msg: String,
    pub zip_created: bool,
    pub zip_job: Option<Arc<Mutex<ZipProgress>>>,
}

pub fn pair_files_size(out_dir: &Path, rec: &MatchRecord) -> u64 {
    let sa = std::fs::metadata(out_dir.join(&rec.file_a)).map(|m| m.len()).unwrap_or(0);
    let sb = std::fs::metadata(out_dir.join(&rec.file_b)).map(|m| m.len()).unwrap_or(0);
    sa + sb
}

pub fn build_pair_items(records: Vec<MatchRecord>) -> Vec<PairItem> {
    let out_dir = PathBuf::from("matches");
    let mut used = HTML_OVERHEAD_BYTES;
    records.into_iter().map(|record| {
        let sz = pair_files_size(&out_dir, &record);
        let fits = used.saturating_add(sz) <= HTMLKIN_MAX_BYTES;
        if fits { used = used.saturating_add(sz); }
        PairItem { record, selected: fits, size_bytes: sz }
    }).collect()
}

pub fn selected_total_bytes(pairs: &[PairItem]) -> u64 {
    let imgs: u64 = pairs.iter().filter(|p| p.selected).map(|p| p.size_bytes).sum();
    if imgs == 0 { 0 } else { imgs + HTML_OVERHEAD_BYTES }
}

pub fn format_mb(bytes: u64) -> String {
    format!("{:.2} МБ", bytes as f64 / (1024.0 * 1024.0))
}

pub fn start_zip_archive_job(st: &mut PickerState) -> Result<(), String> {
    if st.zip_job.is_some() { return Ok(()); }
    let selected: Vec<PairItem> = st.pairs.iter().filter(|p| p.selected).cloned().collect();
    if selected.is_empty() { return Err("Выберите хотя бы одну пару!".to_string()); }
    let total_sz = selected_total_bytes(&st.pairs);
    if total_sz > HTMLKIN_MAX_BYTES {
        return Err(format!("Превышен лимит ({} > 150.00 МБ).", format_mb(total_sz)));
    }
    let job = Arc::new(Mutex::new(ZipProgress {
        progress: 0.02,
        label: "Подготовка файлов отчёта...".to_string(),
        done: false,
        error: None,
        kept_items: Vec::new(),
        stamp: String::new(),
        human: String::new(),
        zip_dest: String::new(),
    }));
    st.zip_job = Some(Arc::clone(&job));
    st.zip_created = false;
    let name_a = st.name_a.clone();
    let name_b = st.name_b.clone();

    std::thread::spawn(move || {
        let res = (|| -> Result<(Vec<PairItem>, String, String, String), String> {
            let out_dir = PathBuf::from("matches");
            let mut recs = Vec::new();
            let mut items = Vec::new();
            let mut pic_recs = Vec::new();
            for (i, item) in selected.iter().enumerate() {
                let id = i + 1;
                let (fa, fb) = (format!("pair_{:02}_A.png", id), format!("pair_{:02}_B.png", id));
                let mut rec = item.record.clone();
                rec.id = id;
                rec.file_a = fa.clone();
                rec.file_b = fb.clone();
                recs.push(rec.clone());
                items.push(PairItem { record: rec.clone(), selected: true, size_bytes: item.size_bytes });
                rec.file_a = format!("pic/{}", fa);
                rec.file_b = format!("pic/{}", fb);
                pic_recs.push(rec);
            }
            let json_plain = serde_json::to_string_pretty(&recs).unwrap_or_else(|_| "[]".to_string());
            let json_pic = serde_json::to_string_pretty(&pic_recs).unwrap_or_else(|_| "[]".to_string());
            let html_zip = generate_html(&json_pic, &name_a, &name_b);

            let f = File::create("matches.zip").map_err(|e| format!("Не удалось создать matches.zip: {}", e))?;
            let mut zip = ZipWriter::new(BufWriter::with_capacity(256 * 1024, f));
            let opts_deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(1));
            let opts_store = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("index.html", opts_deflate).map_err(|e| e.to_string())?;
            zip.write_all(html_zip.as_bytes()).map_err(|e| e.to_string())?;
            zip.start_file("pic/matches.json", opts_deflate).map_err(|e| e.to_string())?;
            zip.write_all(json_plain.as_bytes()).map_err(|e| e.to_string())?;
            zip.start_file("pic/viewer.html", opts_deflate).map_err(|e| e.to_string())?;
            zip.write_all(html_zip.as_bytes()).map_err(|e| e.to_string())?;

            let total_steps = selected.len().max(1) as f32;
            for (i, item) in selected.iter().enumerate() {
                let id = i + 1;
                if let Ok(mut g) = job.lock() {
                    g.progress = 0.08 + 0.84 * (i as f32 / total_steps);
                    g.label = format!("Упаковка пары {}/{}...", id, selected.len());
                }
                let src_a = out_dir.join(&item.record.file_a);
                let src_b = out_dir.join(&item.record.file_b);
                if src_a.exists() {
                    let bytes = std::fs::read(&src_a).map_err(|e| e.to_string())?;
                    zip.start_file(format!("pic/pair_{:02}_A.png", id), opts_store).map_err(|e| e.to_string())?;
                    zip.write_all(&bytes).map_err(|e| e.to_string())?;
                }
                if src_b.exists() {
                    let bytes = std::fs::read(&src_b).map_err(|e| e.to_string())?;
                    zip.start_file(format!("pic/pair_{:02}_B.png", id), opts_store).map_err(|e| e.to_string())?;
                    zip.write_all(&bytes).map_err(|e| e.to_string())?;
                }
            }
            if let Ok(mut g) = job.lock() {
                g.progress = 0.95;
                g.label = "Финализация архива и очистка временных файлов...".to_string();
            }
            let mut bw = zip.finish().map_err(|e| e.to_string())?;
            let _ = bw.flush();
            if let Ok(mut g) = job.lock() {
                g.progress = 0.96;
                g.label = "Сохранение копии в историю...".to_string();
            }
            let _ = std::fs::create_dir_all("history");
            let (stamp, human) = crate::history::now_timestamp();
            let zip_dest = format!("history/report_{}.zip", stamp);
            let _ = std::fs::copy("matches.zip", &zip_dest);
            let _ = std::fs::remove_dir_all(&out_dir);
            Ok((items, stamp, human, zip_dest))
        })();

        if let Ok(mut g) = job.lock() {
            match res {
                Ok((items, stamp, human, zip_dest)) => {
                    g.progress = 1.0; g.kept_items = items; g.stamp = stamp;
                    g.human = human; g.zip_dest = zip_dest; g.done = true;
                }
                Err(err) => { g.error = Some(err); g.done = true; }
            }
        }
    });
    Ok(())
}


fn load_png_via_ffmpeg(png_path: &Path) -> Result<egui::ColorImage, String> {
    let file = File::open(png_path).map_err(|e| format!("Не открыт файл: {}", e))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("Ошибка PNG: {}", e))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("Ошибка кадра PNG: {}", e))?;
    let raw = &buf[..info.buffer_size()];
    let (w, h) = (info.width as usize, info.height as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(raw),
        png::ColorType::Rgb => { for px in raw.chunks_exact(3) { if let [r, g, b] = *px { rgba.extend_from_slice(&[r, g, b, 255]); } } }
        png::ColorType::GrayscaleAlpha => { for px in raw.chunks_exact(2) { if let [g, a] = *px { rgba.extend_from_slice(&[g, g, g, a]); } } }
        png::ColorType::Grayscale => { for &g in raw { rgba.extend_from_slice(&[g, g, g, 255]); } }
        _ => return Err(format!("Неподдерживаемый формат PNG: {:?}", info.color_type)),
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

pub fn render_pair_picker_viewport(ctx: &egui::Context, picker_arc: &Arc<Mutex<PickerState>>) {
    let mut st = match picker_arc.lock() { Ok(g) => g, Err(_) => return };
    if ctx.input(|i| i.viewport().close_requested()) && st.zip_job.is_none() { st.is_open = false; return; }
    if st.pairs.is_empty() {
        egui::CentralPanel::default().show(ctx, |ui| { ui.centered_and_justified(|ui| { ui.label("Нет пар."); }); });
        return;
    }
    if st.current_idx >= st.pairs.len() { st.current_idx = st.pairs.len() - 1; }

    let busy = st.zip_job.is_some();
    if !busy {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) { st.is_open = false; }
            if i.key_pressed(egui::Key::ArrowRight) && st.current_idx + 1 < st.pairs.len() { st.current_idx += 1; }
            if i.key_pressed(egui::Key::ArrowLeft) && st.current_idx > 0 { st.current_idx -= 1; }
            if i.key_pressed(egui::Key::Space) { st.show_b = !st.show_b; }
            if i.key_pressed(egui::Key::Enter) {
                let idx = st.current_idx;
                if st.pairs[idx].selected { st.pairs[idx].selected = false; }
                else {
                    let cur = selected_total_bytes(&st.pairs);
                    let add = st.pairs[idx].size_bytes + if cur == 0 { HTML_OVERHEAD_BYTES } else { 0 };
                    if cur + add <= HTMLKIN_MAX_BYTES { st.pairs[idx].selected = true; }
                    else { st.status_msg = format!("Лимит 150 МБ! Пара #{} ({}) не помещается.", st.pairs[idx].record.id, format_mb(st.pairs[idx].size_bytes)); }
                }
            }
        });
    }
    if !st.is_open { return; }

    let sel_cnt = st.pairs.iter().filter(|p| p.selected).count();
    let tot_cnt = st.pairs.len();
    let total_bytes = selected_total_bytes(&st.pairs);
    egui::TopBottomPanel::top("picker_top").show(ctx, |ui| {
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("Пара {}/{}", st.current_idx + 1, tot_cnt));
                ui.separator();
                let idx = st.current_idx;
                let mut sel = st.pairs[idx].selected;
                let psz = st.pairs[idx].size_bytes;
                let add = psz + if total_bytes == 0 { HTML_OVERHEAD_BYTES } else { 0 };
                let can_toggle = sel || (total_bytes + add <= HTMLKIN_MAX_BYTES);
                let lbl = if sel { egui::RichText::new(format!(" ✅ В ОТЧЁТЕ ({}) ", format_mb(psz))).strong().color(egui::Color32::from_rgb(100, 230, 120)) }
                          else if can_toggle { egui::RichText::new(format!(" ❌ ИСКЛЮЧЕНА ({}) ", format_mb(psz))).strong().color(egui::Color32::from_rgb(240, 110, 110)) }
                          else { egui::RichText::new(format!(" ⛔ ЛИМИТ 150 МБ ({}) ", format_mb(psz))).strong().color(egui::Color32::from_rgb(245, 160, 80)) };
                if ui.add_enabled(can_toggle, egui::Checkbox::new(&mut sel, lbl)).changed() { st.pairs[idx].selected = sel; }
                ui.separator();
                let col = if total_bytes > HTMLKIN_MAX_BYTES * 9 / 10 { egui::Color32::from_rgb(255, 200, 90) } else { egui::Color32::from_rgb(150, 220, 255) };
                ui.label(egui::RichText::new(format!("Вес: {}", format_mb(total_bytes))).strong().color(col));
                ui.separator();
                if ui.selectable_label(!st.show_b, egui::RichText::new(" Кадр А ").strong()).clicked() { st.show_b = false; }
                if ui.selectable_label(st.show_b, egui::RichText::new(" Кадр Б ").strong()).clicked() { st.show_b = true; }
                ui.separator();
                ui.label(format!("Зум: {:.0}%", st.zoom * 100.0));
                if ui.button("100%").clicked() { st.zoom = 1.0; st.pan = egui::Vec2::ZERO; }
            });
        });
    });

    egui::SidePanel::left("picker_list").resizable(true).default_width(290.0).show(ctx, |ui| {
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                if ui.button("☑ Выбрать все").clicked() {
                    let (mut used, mut skipped) = (HTML_OVERHEAD_BYTES, 0usize);
                    for p in &mut st.pairs {
                        if used.saturating_add(p.size_bytes) <= HTMLKIN_MAX_BYTES { p.selected = true; used += p.size_bytes; }
                        else { p.selected = false; skipped += 1; }
                    }
                    if skipped > 0 { st.status_msg = format!("Выбрано максимум в пределах 150 МБ (пропущено: {}).", skipped); }
                }
                if ui.button("☐ Ни одной").clicked() { for p in &mut st.pairs { p.selected = false; } }
                ui.label(format!("{}/{}", sel_cnt, tot_cnt));
            });
            ui.separator();

            let list_h = (ui.available_height() - 52.0).max(120.0);
            egui::ScrollArea::vertical().max_height(list_h).show(ui, |ui| {
                let cur = st.current_idx;
                let mut next_idx = None;
                let mut run_sz = selected_total_bytes(&st.pairs);
                for (i, item) in st.pairs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let add = item.size_bytes + if run_sz == 0 { HTML_OVERHEAD_BYTES } else { 0 };
                        let can = item.selected || (run_sz + add <= HTMLKIN_MAX_BYTES);
                        let was = item.selected;
                        let cb = ui.add_enabled(can, egui::Checkbox::new(&mut item.selected, ""));
                        if !can { cb.on_disabled_hover_text(format!("Превысит лимит 150 МБ (+{})", format_mb(item.size_bytes))); }
                        else if cb.changed() {
                            if item.selected && !was { run_sz += add; }
                            else if !item.selected && was { run_sz = run_sz.saturating_sub(item.size_bytes); }
                        }
                        let mb = item.size_bytes as f64 / (1024.0 * 1024.0);
                        let mut t = egui::RichText::new(format!("#{:02} | {} | {:.1}МБ | d:{}", item.record.id, item.record.time_a_formatted, mb, item.record.hash_diff));
                        if !item.selected { t = if can { t.weak().strikethrough() } else { t.weak().color(egui::Color32::from_rgb(160, 120, 100)) }; }
                        if ui.selectable_label(i == cur, t).clicked() { next_idx = Some(i); }
                    });
                }
                if let Some(i) = next_idx { st.current_idx = i; }
            });

            ui.separator();
            let can_save = sel_cnt > 0 && total_bytes <= HTMLKIN_MAX_BYTES && !busy;
            let btn = egui::Button::new(
                egui::RichText::new(format!(" 📦 Сделать архив и закрыть ({}/{}) ", sel_cnt, tot_cnt))
                    .strong().size(14.0).color(egui::Color32::BLACK)
            ).fill(egui::Color32::from_rgb(95, 215, 130)).min_size(egui::vec2(ui.available_width(), 36.0));
            if ui.add_enabled(can_save, btn).clicked() {
                match start_zip_archive_job(&mut st) {
                    Ok(()) => {
                        st.is_open = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => { st.status_msg = format!("Ошибка: {}", e); }
                }
            }
        });
    });
    egui::TopBottomPanel::bottom("picker_bot").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let r = &st.pairs[st.current_idx].record;
            ui.label(format!("А: {} | Б: {} | Δt: {:+.3}с | diff: {}", r.time_a_formatted, r.time_b_formatted, r.time_delta_sec, r.hash_diff));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(&st.status_msg).color(egui::Color32::from_rgb(150, 220, 170)));
            });
        });
    });

    egui::CentralPanel::default().show(ctx, |ui| {
        let file = if st.show_b { st.pairs[st.current_idx].record.file_b.clone() } else { st.pairs[st.current_idx].record.file_a.clone() };
        if !st.textures.contains_key(&file) && !st.failed_loads.contains_key(&file) {
            let full_path = std::env::current_dir().unwrap_or_default().join("matches").join(&file);
            match load_png_via_ffmpeg(&full_path) {
                Ok(img) => { st.textures.insert(file.clone(), ctx.load_texture(&file, img, egui::TextureOptions::LINEAR)); }
                Err(err) => { st.failed_loads.insert(file.clone(), err); }
            }
        }
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if !busy {
            if resp.hovered() {
                let sc = ctx.input(|i| i.smooth_scroll_delta.y);
                if sc.abs() > 0.1 {
                    st.zoom = (st.zoom * if sc > 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(0.5, 8.0);
                    if (st.zoom - 1.0).abs() < 0.05 { st.zoom = 1.0; st.pan = egui::Vec2::ZERO; }
                }
            }
            if resp.dragged() { st.pan += resp.drag_delta(); }
            if resp.double_clicked() { st.zoom = 1.0; st.pan = egui::Vec2::ZERO; }
            else if resp.clicked() && !resp.double_clicked() { st.show_b = !st.show_b; }
        }

        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, egui::Color32::from_rgb(16, 18, 22));
        if let Some(tex) = st.textures.get(&file) {
            let ts = tex.size_vec2();
            if ts.x > 0.0 && ts.y > 0.0 {
                let s = (rect.width() / ts.x).min(rect.height() / ts.y) * st.zoom;
                let ir = egui::Rect::from_center_size(rect.center() + st.pan, ts * s);
                p.image(tex.id(), ir, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
        } else if let Some(err) = st.failed_loads.get(&file) {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, format!("Ошибка загрузки {}: {}", file, err), egui::FontId::proportional(15.0), egui::Color32::LIGHT_RED);
        }
        let br = egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 12.0), egui::vec2(260.0, 28.0));
        p.rect_filled(br, 6.0, if st.show_b { egui::Color32::from_rgba_unmultiplied(210, 120, 30, 220) } else { egui::Color32::from_rgba_unmultiplied(35, 130, 210, 220) });
        p.text(br.center(), egui::Align2::CENTER_CENTER, if st.show_b { format!("КАДР Б ({})", st.name_b) } else { format!("КАДР А ({})", st.name_a) }, egui::FontId::proportional(14.0), egui::Color32::WHITE);
    });
}

pub fn poll_zip_job(st: &mut PickerState) -> (Option<(f32, String)>, Option<Result<ZipDoneInfo, String>>) {
    let Some(job_arc) = st.zip_job.clone() else { return (None, None); };
    let (prog, lbl, done, err, kept, stamp, human, zip_dest) = {
        let mut g = job_arc.lock().unwrap();
        (g.progress, g.label.clone(), g.done, g.error.clone(),
         std::mem::take(&mut g.kept_items), std::mem::take(&mut g.stamp),
         std::mem::take(&mut g.human), std::mem::take(&mut g.zip_dest))
    };
    if !done { return (Some((prog, lbl)), None); }
    st.zip_job = None;
    if let Some(e) = err {
        st.status_msg = format!("Ошибка сборки ZIP: {}", e);
        return (None, Some(Err(e)));
    }
    st.pairs = kept; st.current_idx = 0; st.textures.clear(); st.failed_loads.clear(); st.zip_created = true;
    let pairs_count = st.pairs.iter().filter(|p| p.selected).count();
    let total_bytes = selected_total_bytes(&st.pairs);
    (None, Some(Ok(ZipDoneInfo { pairs_count, total_bytes, stamp, human, zip_dest })))
}
