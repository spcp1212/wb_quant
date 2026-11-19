use crate::analysis::{self as an, BaselineMode, GrayImage, InvertMode, Lane, LaneDir, Params, Rect};
use eframe::egui::{self, Align2, Color32, CursorIcon, FontId, Key, Pos2, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use egui_plot::{Bar, BarChart, Line, Plot, PlotPoints, VLine};
use std::path::PathBuf;

const IMG_EXT: [&str; 7] = ["tif", "tiff", "png", "jpg", "jpeg", "bmp", "gif"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    AddLane,
    Strip,
}

enum Drag {
    None,
    NewRect { start: (f32, f32), cur: (f32, f32), strip: bool },
    Move { idx: usize, off: (f32, f32) },
    Resize { idx: usize, edges: Edges, orig: Rect, start: (f32, f32) },
}

/// Which edges of a lane rectangle are being dragged.
#[derive(Clone, Copy, Default)]
struct Edges {
    l: bool,
    r: bool,
    t: bool,
    b: bool,
}

impl Edges {
    fn cursor(self) -> CursorIcon {
        match (self.l, self.r, self.t, self.b) {
            (true, _, true, _) | (_, true, _, true) => CursorIcon::ResizeNwSe,
            (_, true, true, _) | (true, _, _, true) => CursorIcon::ResizeNeSw,
            (true, _, _, _) | (_, true, _, _) => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::ResizeVertical,
        }
    }
}

struct Slot {
    title: &'static str,
    path: Option<PathBuf>,
    /// Image as loaded (before rotation).
    orig: Option<GrayImage>,
    /// Image actually displayed and measured (rotated by params.rotation).
    img: Option<GrayImage>,
    tex: Option<TextureHandle>,
    show_grid: bool,
    invert_mode: InvertMode,
    params: Params,
    lanes: Vec<Lane>,
    selected: Option<usize>,
    strip: Option<Rect>,
    n_lanes: usize,
    tool: Tool,
    zoom: f32,
    drag: Drag,
    plot_bound: Option<bool>,
}

impl Slot {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            path: None,
            orig: None,
            img: None,
            tex: None,
            show_grid: false,
            invert_mode: InvertMode::Auto,
            params: Params::default(),
            lanes: Vec::new(),
            selected: None,
            strip: None,
            n_lanes: 6,
            tool: Tool::Strip,
            zoom: 1.0,
            drag: Drag::None,
            plot_bound: None,
        }
    }

    fn load(&mut self, ctx: &egui::Context, path: PathBuf) -> Result<(), String> {
        let img = GrayImage::open(&path)?;
        self.params.invert = self.invert_mode.resolve(&img);
        self.params.rotation = 0.0;
        self.orig = Some(img.clone());
        self.set_image(ctx, img);
        self.path = Some(path);
        Ok(())
    }

    fn set_image(&mut self, ctx: &egui::Context, img: GrayImage) {
        let disp = img.display_u8();
        let ci = egui::ColorImage::from_gray([img.width, img.height], &disp);
        match &mut self.tex {
            Some(t) => t.set(ci, TextureOptions::LINEAR),
            None => self.tex = Some(ctx.load_texture(format!("img-{}", self.title), ci, TextureOptions::LINEAR)),
        }
        self.img = Some(img);
        self.recompute_all();
    }

    /// Re-rotate the original image by params.rotation; lanes keep their positions.
    fn apply_rotation(&mut self, ctx: &egui::Context) {
        self.params.rotation = (self.params.rotation * 100.0).round() / 100.0;
        if let Some(o) = &self.orig {
            let img = o.rotated(self.params.rotation);
            self.set_image(ctx, img);
        }
    }

    /// Lane edge / corner under (x, y) — the selected lane is checked first.
    fn edge_hit(&self, x: f32, y: f32, tol: f32) -> Option<(usize, Edges)> {
        let mut order: Vec<usize> = (0..self.lanes.len()).rev().collect();
        if let Some(s) = self.selected.filter(|&s| s < self.lanes.len()) {
            order.retain(|&i| i != s);
            order.insert(0, s);
        }
        for i in order {
            let r = self.lanes[i].rect;
            let (x0, x1, y0, y1) = (r.x, r.x + r.w, r.y, r.y + r.h);
            if x < x0 - tol || x > x1 + tol || y < y0 - tol || y > y1 + tol {
                continue;
            }
            let th = tol.min(r.w / 3.0);
            let tv = tol.min(r.h / 3.0);
            let e = Edges {
                l: (x - x0).abs() <= th,
                r: (x - x1).abs() <= th,
                t: (y - y0).abs() <= tv,
                b: (y - y1).abs() <= tv,
            };
            if e.l || e.r || e.t || e.b {
                return Some((i, e));
            }
        }
        None
    }

    fn nudge_selected(&mut self, dx: f32, dy: f32) {
        let Some((w, h)) = self.dims() else { return };
        if let Some(i) = self.selected.filter(|&i| i < self.lanes.len()) {
            self.lanes[i].rect.x += dx;
            self.lanes[i].rect.y += dy;
            self.lanes[i].rect.clamp_to(w, h);
            self.recompute(i);
            self.sort_lanes();
        }
    }

    fn dims(&self) -> Option<(usize, usize)> {
        self.img.as_ref().map(|i| (i.width, i.height))
    }

    fn recompute_all(&mut self) {
        if let Some(img) = &self.img {
            for l in &mut self.lanes {
                l.rect.clamp_to(img.width, img.height);
                l.update(img, &self.params);
            }
        }
    }

    fn recompute(&mut self, i: usize) {
        if let (Some(img), Some(l)) = (&self.img, self.lanes.get_mut(i)) {
            l.update(img, &self.params);
        }
    }

    /// Number lanes left→right (or top→bottom) and keep the selection on the same lane.
    fn sort_lanes(&mut self) {
        let sel = self.selected.and_then(|i| self.lanes.get(i)).map(|l| l.rect);
        let dir = self.params.dir;
        self.lanes
            .sort_by(|a, b| a.across_center(dir).partial_cmp(&b.across_center(dir)).unwrap_or(std::cmp::Ordering::Equal));
        self.selected = sel.and_then(|r| self.lanes.iter().position(|l| l.rect == r));
    }

    fn add_lane(&mut self, mut r: Rect) -> usize {
        if let Some((w, h)) = self.dims() {
            r.clamp_to(w, h);
        }
        self.lanes.push(Lane::new(r));
        let i = self.lanes.len() - 1;
        self.recompute(i);
        self.selected = Some(i);
        self.sort_lanes();
        self.selected.unwrap_or(0)
    }

    /// New lane with the same size as the selected (or last) lane, centred at (x, y) — ImageJ "Select Next Lane".
    fn add_lane_at(&mut self, x: f32, y: f32) -> usize {
        let t = self
            .selected
            .and_then(|i| self.lanes.get(i))
            .or(self.lanes.last())
            .map(|l| l.rect)
            .unwrap_or(Rect::new(0.0, 0.0, 40.0, 60.0));
        self.add_lane(Rect::new(x - t.w / 2.0, y - t.h / 2.0, t.w, t.h))
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.lanes.iter().rposition(|l| l.rect.contains(x, y))
    }

    fn set_lanes(&mut self, rects: Vec<Rect>) {
        self.lanes = rects.into_iter().map(Lane::new).collect();
        self.selected = if self.lanes.is_empty() { None } else { Some(0) };
        self.recompute_all();
        self.sort_lanes();
    }
}

pub struct WbApp {
    slots: [Slot; 2],
    active: usize,
    ref_lane: usize,
    status: String,
    /// Id of the image canvas, so keyboard shortcuts work when it (or nothing) has focus.
    canvas_id: Option<egui::Id>,
}

impl WbApp {
    pub fn new(cc: &eframe::CreationContext, initial: Option<PathBuf>) -> Self {
        let mut app = Self {
            slots: [Slot::new("Target protein"), Slot::new("Loading control")],
            active: 0,
            ref_lane: 1,
            canvas_id: None,
            status: "Open a blot image (button above or drag & drop).".into(),
        };
        if let Some(p) = initial {
            app.load_into(&cc.egui_ctx, 0, p);
        }
        app
    }

    fn load_into(&mut self, ctx: &egui::Context, slot: usize, path: PathBuf) {
        match self.slots[slot].load(ctx, path.clone()) {
            Ok(()) => {
                self.active = slot;
                let s = &self.slots[slot];
                self.status = format!(
                    "Loaded {} as {} (inverted: {}). Draw a strip over the band row, then 'Auto-detect lanes'.",
                    path.display(),
                    s.title,
                    s.params.invert
                );
            }
            Err(e) => self.status = e,
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context, slot: usize) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Images", &IMG_EXT).pick_file() {
            self.load_into(ctx, slot, p);
        }
    }

    fn csv_text(&self) -> String {
        let (t, l) = (&self.slots[0], &self.slots[1]);
        an::build_csv(&an::CsvInput {
            t_path: t.path.as_ref().map(|p| p.display().to_string()),
            t_lanes: &t.lanes,
            t_params: t.params,
            lc_path: l.path.as_ref().map(|p| p.display().to_string()),
            lc_lanes: &l.lanes,
            lc_params: l.params,
            ref_idx: self.ref_lane.saturating_sub(1),
        })
    }

    fn export_csv(&mut self) {
        if self.slots[0].lanes.is_empty() && self.slots[1].lanes.is_empty() {
            self.status = "Nothing to export — no lanes yet.".into();
            return;
        }
        if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("wb_quant_results.csv").save_file() {
            self.status = match std::fs::write(&p, self.csv_text()) {
                Ok(()) => format!("Saved {}", p.display()),
                Err(e) => format!("Cannot save: {e}"),
            };
        }
    }

    fn save_overlay(&mut self) {
        let s = &self.slots[self.active];
        let Some(img) = &s.img else {
            self.status = "No image in the current tab.".into();
            return;
        };
        let name = format!("{}_lanes.png", if self.active == 0 { "target" } else { "loading_control" });
        if let Some(p) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(&name).save_file() {
            self.status = match an::render_overlay(img, &s.lanes, s.params.dir).save(&p) {
                Ok(()) => format!("Saved {}", p.display()),
                Err(e) => format!("Cannot save: {e}"),
            };
        }
    }

    // ------------------------------------------------------------ panels

    fn controls(&mut self, ui: &mut egui::Ui) {
        let other = 1 - self.active;
        let other_title = self.slots[other].title;
        let other_rects: Vec<Rect> = self.slots[other].lanes.iter().map(|l| l.rect).collect();
        let s = &mut self.slots[self.active];

        ui.heading(s.title);
        match (&s.path, &s.img) {
            (Some(p), Some(img)) => {
                ui.label(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
                ui.weak(format!("{} × {} px, {}-bit", img.width, img.height, img.bit_depth));
            }
            _ => {
                ui.weak("No image loaded");
            }
        }
        ui.separator();

        let ctx = ui.ctx().clone();
        let mut changed = false;
        let mut rot_changed = false;
        egui::Grid::new("params").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Rotation");
            ui.vertical(|ui| {
                rot_changed |= ui
                    .add(
                        egui::Slider::new(&mut s.params.rotation, -15.0..=15.0)
                            .step_by(0.05)
                            .max_decimals(2)
                            .suffix("°"),
                    )
                    .on_hover_text("Positive = clockwise. Type an exact value in the box, or drag.")
                    .changed();
                ui.horizontal(|ui| {
                    for (label, d) in [("-1°", -1.0f32), ("-0.1°", -0.1), ("+0.1°", 0.1), ("+1°", 1.0)] {
                        if ui.small_button(label).clicked() {
                            s.params.rotation += d;
                            rot_changed = true;
                        }
                    }
                    if ui.small_button("Reset").clicked() {
                        s.params.rotation = 0.0;
                        rot_changed = true;
                    }
                    ui.checkbox(&mut s.show_grid, "Grid").on_hover_text("Guide lines to level the bands");
                });
            });
            ui.end_row();

            ui.label("Invert");
            let label = |m: InvertMode| match m {
                InvertMode::Auto => "Auto",
                InvertMode::Yes => "Yes (dark bands on light)",
                InvertMode::No => "No (light bands on dark)",
            };
            egui::ComboBox::from_id_salt("invert")
                .selected_text(format!("{}  ({})", label(s.invert_mode), if s.params.invert { "inverted" } else { "raw" }))
                .show_ui(ui, |ui| {
                    for m in [InvertMode::Auto, InvertMode::Yes, InvertMode::No] {
                        if ui.selectable_value(&mut s.invert_mode, m, label(m)).changed() {
                            changed = true;
                        }
                    }
                });
            ui.end_row();

            ui.label("Lane direction");
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut s.params.dir, LaneDir::Vertical, "Vertical ↕").changed();
                changed |= ui.radio_value(&mut s.params.dir, LaneDir::Horizontal, "Horizontal ↔").changed();
            });
            ui.end_row();

            ui.label("Baseline");
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut s.params.baseline, BaselineMode::Line, "Straight line (ImageJ)").changed();
                changed |= ui.radio_value(&mut s.params.baseline, BaselineMode::MinHorizontal, "Horizontal").changed();
            });
            ui.end_row();

            ui.label("Peak smoothing");
            changed |= ui
                .add(egui::DragValue::new(&mut s.params.smooth_win).range(1..=51).suffix(" px"))
                .on_hover_text("Moving-average window used only to find peak boundaries automatically")
                .changed();
            ui.end_row();

            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut s.zoom, 0.25..=8.0).logarithmic(true).suffix("×"));
            ui.end_row();
        });
        if rot_changed {
            s.apply_rotation(&ctx);
        }
        if changed {
            if let Some(img) = &s.orig {
                s.params.invert = s.invert_mode.resolve(img);
            }
            s.recompute_all();
            s.sort_lanes();
        }

        ui.separator();
        ui.strong("Mouse tool");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.tool, Tool::Strip, "1. Draw strip");
            ui.selectable_value(&mut s.tool, Tool::AddLane, "Add lane");
            ui.selectable_value(&mut s.tool, Tool::Select, "Select / move");
        });
        ui.weak(match s.tool {
            Tool::Strip => "Drag a rectangle over one band row (all lanes), then auto-detect or split.",
            Tool::AddLane => "No lanes: drag the first lane. Then click to add same-size lanes (ImageJ 'Next Lane'). Drag a lane edge to resize.",
            Tool::Select => "Click a lane to select. Drag inside = move, drag an edge/corner = resize. Arrow keys nudge (Shift = 10 px). Delete removes.",
        });

        ui.horizontal(|ui| {
            ui.label("Lanes:");
            ui.add(egui::DragValue::new(&mut s.n_lanes).range(1..=60));
            let has_strip = s.strip.is_some() && s.img.is_some();
            if ui.add_enabled(has_strip, egui::Button::new("2. Auto-detect lanes")).clicked() {
                let img = s.img.as_ref().unwrap();
                let rects = an::auto_lanes(img, &s.strip.unwrap(), s.n_lanes, s.params.dir, s.params.invert);
                s.set_lanes(rects);
                s.tool = Tool::Select;
            }
            if ui.add_enabled(has_strip, egui::Button::new("Equal split")).clicked() {
                let rects = an::equal_lanes(&s.strip.unwrap(), s.n_lanes, s.params.dir);
                s.set_lanes(rects);
                s.tool = Tool::Select;
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!other_rects.is_empty() && s.img.is_some(), egui::Button::new(format!("Copy lanes from {other_title}")))
                .clicked()
            {
                s.set_lanes(other_rects.clone());
            }
            if ui.button("Clear lanes").clicked() {
                s.lanes.clear();
                s.selected = None;
            }
        });
        if ui.button("Auto peak range for all lanes").clicked() {
            for l in &mut s.lanes {
                l.auto_range = true;
            }
            s.recompute_all();
        }

        // ---- selected lane
        ui.separator();
        let Some(i) = s.selected.filter(|&i| i < s.lanes.len()) else {
            ui.weak("Select a lane to see its profile.");
            return;
        };
        let mut lane_changed = false;
        ui.horizontal(|ui| {
            ui.strong(format!("Lane {}", i + 1));
            let l = &mut s.lanes[i];
            ui.label("x");
            lane_changed |= ui.add(egui::DragValue::new(&mut l.rect.x).speed(0.5).max_decimals(1)).changed();
            ui.label("y");
            lane_changed |= ui.add(egui::DragValue::new(&mut l.rect.y).speed(0.5).max_decimals(1)).changed();
            ui.label("w");
            lane_changed |= ui.add(egui::DragValue::new(&mut l.rect.w).range(2.0..=10000.0).speed(0.5).max_decimals(1)).changed();
            ui.label("h");
            lane_changed |= ui.add(egui::DragValue::new(&mut l.rect.h).range(2.0..=10000.0).speed(0.5).max_decimals(1)).changed();
        });
        ui.horizontal(|ui| {
            if ui.button("Apply this size to all lanes").clicked() {
                let r = s.lanes[i].rect;
                for l in &mut s.lanes {
                    let (cx, cy) = l.rect.center();
                    l.rect = Rect::new(cx - r.w / 2.0, cy - r.h / 2.0, r.w, r.h);
                }
                s.recompute_all();
            }
            if ui.button("Delete lane").clicked() {
                s.lanes.remove(i);
                s.selected = None;
                return;
            }
        });
        if s.selected.is_none() {
            return;
        }
        if lane_changed {
            if let Some((w, h)) = s.dims() {
                s.lanes[i].rect.clamp_to(w, h);
            }
            s.recompute(i);
        }

        let n = s.lanes[i].profile.len();
        if n == 0 {
            return;
        }
        let mode = s.params.baseline;
        let (a, b) = {
            let l = &s.lanes[i];
            (l.start.min(n - 1), l.end.min(n - 1))
        };
        let p = s.lanes[i].profile.clone();
        ui.label(egui::RichText::new(format!("Peak area = {:.1}", s.lanes[i].area)).size(16.0).strong());

        let line_pts: PlotPoints = (0..n).map(|k| [k as f64, p[k]]).collect();
        let base_pts: PlotPoints = (a..=b).map(|k| [k as f64, an::baseline_at(&p, a, b, mode, k)]).collect();
        let fill = Color32::from_rgba_unmultiplied(255, 170, 0, 110);
        let bars: Vec<Bar> = (a..=b)
            .filter_map(|k| {
                let base = an::baseline_at(&p, a, b, mode, k);
                let v = p[k] - base;
                (v > 0.0).then(|| Bar::new(k as f64, v).base_offset(base).width(1.0).fill(fill).stroke(Stroke::NONE))
            })
            .collect();
        let pr = Plot::new(("profile", self.active, i))
            .height(260.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .x_axis_label("position along lane (px)")
            .y_axis_label("signal")
            .show(ui, |pui| {
                pui.bar_chart(BarChart::new(bars).color(fill));
                pui.line(Line::new(line_pts).color(Color32::from_rgb(60, 140, 255)).width(1.5_f32));
                pui.line(Line::new(base_pts).color(Color32::from_rgb(230, 50, 50)).width(1.2_f32));
                pui.vline(VLine::new(a as f64).color(Color32::from_rgb(0, 180, 60)));
                pui.vline(VLine::new(b as f64).color(Color32::from_rgb(0, 180, 60)));
                pui.pointer_coordinate()
            });
        let resp = pr.response;
        let mut range_changed = false;
        if let Some(c) = pr.inner {
            let x = c.x.round().clamp(0.0, (n - 1) as f64) as usize;
            if resp.drag_started() || resp.clicked() {
                let l = &s.lanes[i];
                s.plot_bound = Some((x as isize - l.start as isize).abs() <= (x as isize - l.end as isize).abs());
            }
            if resp.dragged() || resp.clicked() {
                if let Some(is_start) = s.plot_bound {
                    let l = &mut s.lanes[i];
                    if is_start { l.start = x } else { l.end = x }
                    if l.start > l.end {
                        std::mem::swap(&mut l.start, &mut l.end);
                        s.plot_bound = Some(!is_start);
                    }
                    l.auto_range = false;
                    range_changed = true;
                }
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            s.plot_bound = None;
        }
        ui.weak("Click / drag on the plot to move the nearest peak boundary (green). Red = baseline.");
        ui.horizontal(|ui| {
            let l = &mut s.lanes[i];
            ui.label("Start");
            range_changed |= ui.add(egui::DragValue::new(&mut l.start).range(0..=n - 1)).changed();
            ui.label("End");
            range_changed |= ui.add(egui::DragValue::new(&mut l.end).range(0..=n - 1)).changed();
            if ui.checkbox(&mut l.auto_range, "auto").changed() {
                range_changed = true;
            }
        });
        if range_changed {
            if s.lanes[i].start > s.lanes[i].end {
                let l = &mut s.lanes[i];
                std::mem::swap(&mut l.start, &mut l.end);
            }
            s.recompute(i);
        }
    }

    fn image_view(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.slots[self.active];
        let (Some(tex), Some((iw, ih))) = (s.tex.clone(), s.dims()) else {
            ui.centered_and_justified(|ui| {
                ui.label(format!("{}: open an image or drag & drop a file here", s.title));
            });
            return;
        };
        let (fw, fh) = (iw as f32, ih as f32);
        let avail = ui.available_size();
        let fit = (avail.x / fw).min(avail.y / fh).max(0.01);
        let scale = fit * s.zoom;

        let mut canvas_id = None;
        egui::ScrollArea::both().auto_shrink([false, false]).drag_to_scroll(false).show(ui, |ui| {
            let (resp, painter) = ui.allocate_painter(Vec2::new(fw, fh) * scale, Sense::click_and_drag());
            canvas_id = Some(resp.id);
            if resp.clicked() || resp.drag_started() {
                resp.request_focus(); // take keyboard focus from any previously clicked button
            }
            if resp.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        resp.id,
                        egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() },
                    )
                });
            }
            let origin = resp.rect.min;
            let to_img = |p: Pos2| ((p.x - origin.x) / scale, (p.y - origin.y) / scale);
            let to_scr = |r: &Rect| {
                egui::Rect::from_min_size(Pos2::new(origin.x + r.x * scale, origin.y + r.y * scale), Vec2::new(r.w * scale, r.h * scale))
            };
            painter.image(tex.id(), resp.rect, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

            // ctrl + wheel zoom
            if resp.hovered() {
                let z = ui.input(|i| i.zoom_delta());
                if z != 1.0 {
                    s.zoom = (s.zoom * z).clamp(0.25, 8.0);
                }
            }

            // ---- mouse interaction
            let tol = 6.0 / scale; // edge grab tolerance: 6 screen px, in image px
            if let Drag::Resize { edges, .. } = &s.drag {
                ui.ctx().set_cursor_icon(edges.cursor());
            } else if s.tool != Tool::Strip && matches!(s.drag, Drag::None) {
                if let Some(hp) = resp.hover_pos() {
                    let (x, y) = to_img(hp);
                    if let Some((_, e)) = s.edge_hit(x, y, tol) {
                        ui.ctx().set_cursor_icon(e.cursor());
                    } else if s.tool == Tool::Select && s.hit(x, y).is_some() {
                        ui.ctx().set_cursor_icon(CursorIcon::Grab);
                    }
                }
            }
            if resp.drag_started() {
                if let Some(p0) = ui.input(|i| i.pointer.press_origin()) {
                    let (x, y) = to_img(p0);
                    let edge = if s.tool == Tool::Strip { None } else { s.edge_hit(x, y, tol) };
                    s.drag = if let Some((idx, edges)) = edge {
                        s.selected = Some(idx);
                        Drag::Resize { idx, edges, orig: s.lanes[idx].rect, start: (x, y) }
                    } else {
                        match s.tool {
                        Tool::Strip => Drag::NewRect { start: (x, y), cur: (x, y), strip: true },
                        Tool::AddLane if s.lanes.is_empty() => Drag::NewRect { start: (x, y), cur: (x, y), strip: false },
                        Tool::AddLane => {
                            let idx = s.add_lane_at(x, y);
                            let r = s.lanes[idx].rect;
                            Drag::Move { idx, off: (x - r.x, y - r.y) }
                        }
                        Tool::Select => match s.hit(x, y) {
                            Some(i) => {
                                s.selected = Some(i);
                                let r = s.lanes[i].rect;
                                Drag::Move { idx: i, off: (x - r.x, y - r.y) }
                            }
                            None => Drag::None,
                        },
                        }
                    };
                }
            }
            if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let (x, y) = to_img(p);
                    match &mut s.drag {
                        Drag::NewRect { cur, .. } => *cur = (x, y),
                        Drag::Move { idx, off } => {
                            let (idx, off) = (*idx, *off);
                            if let Some(l) = s.lanes.get_mut(idx) {
                                l.rect.x = x - off.0;
                                l.rect.y = y - off.1;
                                l.rect.clamp_to(iw, ih);
                            }
                            s.recompute(idx);
                        }
                        Drag::Resize { idx, edges, orig, start } => {
                            let (idx, e, o) = (*idx, *edges, *orig);
                            let (dx, dy) = (x - start.0, y - start.1);
                            let min = 3.0;
                            let mut r = o;
                            if e.l {
                                let nx = (o.x + dx).min(o.x + o.w - min);
                                r.w = o.x + o.w - nx;
                                r.x = nx;
                            }
                            if e.r {
                                r.w = (o.w + dx).max(min);
                            }
                            if e.t {
                                let ny = (o.y + dy).min(o.y + o.h - min);
                                r.h = o.y + o.h - ny;
                                r.y = ny;
                            }
                            if e.b {
                                r.h = (o.h + dy).max(min);
                            }
                            let r = r.intersect_image(iw, ih);
                            if let Some(l) = s.lanes.get_mut(idx) {
                                if r.w >= min && r.h >= min {
                                    l.rect = r;
                                }
                            }
                            s.recompute(idx);
                        }
                        Drag::None => {}
                    }
                }
            }
            if resp.drag_stopped() {
                match std::mem::replace(&mut s.drag, Drag::None) {
                    Drag::NewRect { start, cur, strip } => {
                        let r = Rect::from_corners(start, cur).intersect_image(iw, ih);
                        if r.w >= 2.0 && r.h >= 2.0 {
                            if strip {
                                s.strip = Some(r);
                            } else {
                                s.add_lane(r);
                                s.tool = Tool::AddLane;
                            }
                        }
                    }
                    Drag::Move { .. } | Drag::Resize { .. } => s.sort_lanes(),
                    Drag::None => {}
                }
            }
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let (x, y) = to_img(p);
                    match s.tool {
                        Tool::Select => s.selected = s.hit(x, y),
                        Tool::AddLane if !s.lanes.is_empty() => {
                            s.add_lane_at(x, y);
                        }
                        _ => {}
                    }
                }
            }

            // ---- drawing
            if s.show_grid {
                let gc = Color32::from_rgba_unmultiplied(0, 170, 255, 90);
                let step = (fh / 24.0).max(10.0);
                let mut gy = step;
                while gy < fh {
                    let yy = origin.y + gy * scale;
                    painter.line_segment([Pos2::new(resp.rect.left(), yy), Pos2::new(resp.rect.right(), yy)], Stroke::new(1.0_f32, gc));
                    gy += step;
                }
                let mut gx = step;
                while gx < fw {
                    let xx = origin.x + gx * scale;
                    painter.line_segment([Pos2::new(xx, resp.rect.top()), Pos2::new(xx, resp.rect.bottom())], Stroke::new(1.0_f32, gc.gamma_multiply(0.5)));
                    gx += step;
                }
            }
            if let Some(st) = s.strip {
                painter.rect_stroke(to_scr(&st), 0.0, Stroke::new(1.5_f32, Color32::from_rgb(0, 200, 255)));
            }
            let green = Color32::from_rgb(0, 220, 80);
            for (i, l) in s.lanes.iter().enumerate() {
                let r = to_scr(&l.rect);
                let sel = s.selected == Some(i);
                let col = if sel { Color32::from_rgb(255, 70, 40) } else { Color32::from_rgb(255, 215, 0) };
                painter.rect_stroke(r, 0.0, Stroke::new(if sel { 2.5_f32 } else { 1.5 }, col));
                painter.text(r.center_top() + Vec2::new(0.0, 2.0), Align2::CENTER_TOP, format!("{}", i + 1), FontId::proportional(14.0), col);
                if !l.profile.is_empty() {
                    for k in [l.start, l.end] {
                        let off = (k as f32 + 0.5) * scale;
                        let seg = match s.params.dir {
                            LaneDir::Vertical => [Pos2::new(r.left(), r.top() + off), Pos2::new(r.right(), r.top() + off)],
                            LaneDir::Horizontal => [Pos2::new(r.left() + off, r.top()), Pos2::new(r.left() + off, r.bottom())],
                        };
                        painter.line_segment(seg, Stroke::new(1.0_f32, green));
                    }
                }
            }
            if let Drag::NewRect { start, cur, strip } = &s.drag {
                let r = Rect::from_corners(*start, *cur);
                let c = if *strip { Color32::from_rgb(0, 200, 255) } else { Color32::from_rgb(255, 215, 0) };
                painter.rect_stroke(to_scr(&r), 0.0, Stroke::new(1.0_f32, c));
            }
        });
        if canvas_id.is_some() {
            self.canvas_id = canvas_id;
        }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let t: Vec<f64> = self.slots[0].lanes.iter().map(|l| l.area).collect();
        let lc: Vec<f64> = self.slots[1].lanes.iter().map(|l| l.area).collect();
        let n = t.len().max(lc.len()).max(1);
        self.ref_lane = self.ref_lane.clamp(1, n);
        ui.horizontal(|ui| {
            ui.strong("Results");
            ui.separator();
            ui.label("Reference (control) lane:");
            ui.add(egui::DragValue::new(&mut self.ref_lane).range(1..=n));
            if !t.is_empty() && !lc.is_empty() && t.len() != lc.len() {
                ui.colored_label(Color32::from_rgb(230, 120, 0), format!("! lane count differs: target {} vs loading control {}", t.len(), lc.len()));
            }
            ui.separator();
            if ui.button("Copy table").clicked() {
                let txt = self.csv_text().replace(',', "\t");
                ui.ctx().copy_text(txt.trim_start_matches('\u{FEFF}').to_string());
                self.status = "Table copied to clipboard (tab-separated, paste into Excel/Prism).".into();
            }
        });
        let rows = an::summarize(&t, &lc, self.ref_lane - 1);
        let f = |v: Option<f64>, d: usize| v.map(|x| format!("{x:.d$}")).unwrap_or_else(|| "—".into());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("results").striped(true).num_columns(8).min_col_width(70.0).show(ui, |ui| {
                for h in [
                    "Lane",
                    "Target area",
                    "% of total",
                    "Target / ref",
                    "LC area",
                    "LC / ref",
                    "Target ÷ LC",
                    "Normalized / ref",
                ] {
                    ui.strong(h);
                }
                ui.end_row();
                for r in &rows {
                    let is_ref = r.lane == self.ref_lane;
                    ui.label(if is_ref { format!("{} (ref)", r.lane) } else { r.lane.to_string() });
                    ui.label(f(r.t_area, 1));
                    ui.label(f(r.t_pct, 2));
                    ui.label(f(r.t_rel, 3));
                    ui.label(f(r.lc_area, 1));
                    ui.label(f(r.lc_rel, 3));
                    ui.label(f(r.norm, 4));
                    ui.label(egui::RichText::new(f(r.norm_rel, 3)).strong());
                    ui.end_row();
                }
            });
        });
    }
}

impl eframe::App for WbApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // drag & drop -> current tab
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.load_into(ctx, self.active, p);
        }
        // Delete key removes the selected lane
        let canvas_id = self.canvas_id;
        let kb_ok = ctx.memory(|m| m.focused().is_none() || m.focused() == canvas_id);
        if kb_ok && ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            let s = &mut self.slots[self.active];
            if let Some(i) = s.selected.filter(|&i| i < s.lanes.len()) {
                s.lanes.remove(i);
                s.selected = None;
            }
        }

        // arrow keys nudge the selected lane (Shift = 10 px)
        if kb_ok {
            let (dx, dy) = ctx.input(|i| {
                let st = if i.modifiers.shift { 10.0 } else { 1.0 };
                let mut d = (0.0f32, 0.0f32);
                d.0 += st * (i.num_presses(Key::ArrowRight) as f32 - i.num_presses(Key::ArrowLeft) as f32);
                d.1 += st * (i.num_presses(Key::ArrowDown) as f32 - i.num_presses(Key::ArrowUp) as f32);
                d
            });
            if dx != 0.0 || dy != 0.0 {
                self.slots[self.active].nudge_selected(dx, dy);
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Open target blot…").clicked() {
                    self.open_dialog(ctx, 0);
                }
                if ui.button("Open loading control…").clicked() {
                    self.open_dialog(ctx, 1);
                }
                ui.separator();
                if ui.button("Export CSV…").clicked() {
                    self.export_csv();
                }
                if ui.button("Save lane overlay PNG…").clicked() {
                    self.save_overlay();
                }
                ui.separator();
                ui.label("Tab:");
                let t0 = format!("Target ({} lanes)", self.slots[0].lanes.len());
                let t1 = format!("Loading control ({} lanes)", self.slots[1].lanes.len());
                ui.selectable_value(&mut self.active, 0, t0);
                ui.selectable_value(&mut self.active, 1, t1);
            });
            ui.weak(&self.status);
            ui.add_space(2.0);
        });

        egui::TopBottomPanel::bottom("results").resizable(true).default_height(230.0).show(ctx, |ui| {
            ui.add_space(4.0);
            self.results(ui);
        });

        egui::SidePanel::right("controls").resizable(true).default_width(470.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
        });

        egui::CentralPanel::default().show(ctx, |ui| self.image_view(ui));
    }
}
