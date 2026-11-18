//! Core analysis — a re-implementation of the ImageJ "Analyze > Gels" workflow.
//!
//! ImageJ procedure                         | this module
//! -----------------------------------------|----------------------------------------------
//! Image > Type > 8-bit                     | `GrayImage` (unweighted (R+G+B)/3, 16-bit kept)
//! Gels > Select First/Next Lane            | `Rect` lanes (same size, user placed / auto)
//! Gels > Plot Lanes (column-average plot)  | `lane_profile`
//! Straight-line tool closing each peak     | `BaselineMode::Line` between peak start/end
//! Wand tool inside the closed peak         | `peak_area`  (Σ signal − baseline)
//! Relative density / loading control       | `summarize`

use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use std::path::Path;

// ---------------------------------------------------------------- image

#[derive(Clone)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    /// Gray values in the native range (0..=255 for 8-bit, 0..=65535 for 16-bit).
    pub data: Vec<f32>,
    pub max_val: f32,
    pub bit_depth: u8,
}

impl GrayImage {
    pub fn open(path: &Path) -> Result<Self, String> {
        use image::ImageDecoder;
        let err = |e: &dyn std::fmt::Display| format!("Cannot open {}: {e}", path.display());
        let mut dec = image::ImageReader::open(path)
            .map_err(|e| err(&e))?
            .with_guessed_format()
            .map_err(|e| err(&e))?
            .into_decoder()
            .map_err(|e| err(&e))?;
        // camera / ChemiDoc JPGs often store pixels rotated + an EXIF orientation tag
        let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut img = DynamicImage::from_decoder(dec).map_err(|e| err(&e))?;
        img.apply_orientation(orientation);
        Ok(Self::from_dynamic(&img))
    }

    pub fn from_dynamic(img: &DynamicImage) -> Self {
        use image::ColorType as C;
        let (w, h) = img.dimensions();
        let (bit_depth, max_val) = match img.color() {
            C::L16 | C::La16 | C::Rgb16 | C::Rgba16 => (16u8, 65535.0f32),
            C::Rgb32F | C::Rgba32F => (32u8, 1.0f32),
            _ => (8u8, 255.0f32),
        };
        let data: Vec<f32> = match img {
            DynamicImage::ImageLuma8(b) => b.as_raw().iter().map(|&v| v as f32).collect(),
            DynamicImage::ImageLuma16(b) => b.as_raw().iter().map(|&v| v as f32).collect(),
            // ImageJ default RGB -> 8-bit conversion is the unweighted mean (R+G+B)/3
            _ => img
                .to_rgb32f()
                .pixels()
                .map(|p| ((p[0] + p[1] + p[2]) / 3.0 * max_val).round())
                .collect(),
        };
        Self { width: w as usize, height: h as usize, data, max_val, bit_depth }
    }

    pub fn from_raw(width: usize, height: usize, data: Vec<f32>, max_val: f32) -> Self {
        let bit_depth = if max_val > 255.0 { 16 } else { 8 };
        Self { width, height, data, max_val, bit_depth }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Approximate percentile (q in 0..=1) from a subsample.
    pub fn percentile(&self, q: f64) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let stride = (self.data.len() / 200_000).max(1);
        let mut s: Vec<f32> = self.data.iter().step_by(stride).copied().collect();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((s.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
        s[idx]
    }

    /// Light background (typical scanned blot with dark bands) -> invert.
    pub fn auto_invert(&self) -> bool {
        self.percentile(0.5) > self.max_val * 0.5
    }

    /// 8-bit buffer for display only (16-bit images are contrast-stretched).
    pub fn display_u8(&self) -> Vec<u8> {
        let (lo, hi) = if self.bit_depth == 8 {
            (0.0, 255.0)
        } else {
            (self.percentile(0.001), self.percentile(0.999))
        };
        let span = (hi - lo).max(1e-6);
        self.data
            .iter()
            .map(|&v| (((v - lo) / span).clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }
}

// ---------------------------------------------------------------- geometry & settings

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        Self { x: a.0.min(b.0), y: a.1.min(b.1), w: (a.0 - b.0).abs(), h: (a.1 - b.1).abs() }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    /// Keep size (if possible) and move the rectangle inside the image.
    pub fn clamp_to(&mut self, w: usize, h: usize) {
        let (w, h) = (w as f32, h as f32);
        self.w = self.w.clamp(1.0, w);
        self.h = self.h.clamp(1.0, h);
        self.x = self.x.clamp(0.0, w - self.w);
        self.y = self.y.clamp(0.0, h - self.h);
    }
    /// Intersection with the image (used for freshly drawn rectangles).
    pub fn intersect_image(&self, w: usize, h: usize) -> Rect {
        let x0 = self.x.max(0.0);
        let y0 = self.y.max(0.0);
        let x1 = (self.x + self.w).min(w as f32);
        let y1 = (self.y + self.h).min(h as f32);
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
    /// Integer pixel bounds [x0, x1) × [y0, y1), clipped to the image.
    pub fn px_bounds(&self, w: usize, h: usize) -> (usize, usize, usize, usize) {
        let c = |v: f32, max: usize| (v.round().max(0.0) as usize).min(max);
        (c(self.x, w), c(self.y, h), c(self.x + self.w, w), c(self.y + self.h, h))
    }
}

/// Orientation of the lanes.
/// `Vertical`: lanes run top→bottom and sit side by side (bands are horizontal stripes).
/// This is the usual western-blot layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneDir {
    Vertical,
    Horizontal,
}
impl LaneDir {
    pub fn other(self) -> Self {
        match self {
            LaneDir::Vertical => LaneDir::Horizontal,
            LaneDir::Horizontal => LaneDir::Vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineMode {
    /// Straight line between the profile values at peak start and end (ImageJ line tool).
    Line,
    /// Horizontal line at the lower of the two end values.
    MinHorizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvertMode {
    Auto,
    Yes,
    No,
}
impl InvertMode {
    pub fn resolve(self, img: &GrayImage) -> bool {
        match self {
            InvertMode::Auto => img.auto_invert(),
            InvertMode::Yes => true,
            InvertMode::No => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub dir: LaneDir,
    pub invert: bool,
    pub baseline: BaselineMode,
    /// Moving-average window used only for automatic peak-boundary detection.
    pub smooth_win: usize,
}
impl Default for Params {
    fn default() -> Self {
        Self { dir: LaneDir::Vertical, invert: true, baseline: BaselineMode::Line, smooth_win: 5 }
    }
}

// ---------------------------------------------------------------- profile & peak

/// Column-average profile along the lane (ImageJ "Plot Lanes").
/// For `Vertical` lanes: one value per row = mean over the lane width.
pub fn lane_profile(img: &GrayImage, r: &Rect, dir: LaneDir, invert: bool) -> Vec<f64> {
    let (x0, y0, x1, y1) = r.px_bounds(img.width, img.height);
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let m = img.max_val as f64;
    let sig = |v: f32| if invert { m - v as f64 } else { v as f64 };
    match dir {
        LaneDir::Vertical => (y0..y1)
            .map(|y| (x0..x1).map(|x| sig(img.get(x, y))).sum::<f64>() / (x1 - x0) as f64)
            .collect(),
        LaneDir::Horizontal => (x0..x1)
            .map(|x| (y0..y1).map(|y| sig(img.get(x, y))).sum::<f64>() / (y1 - y0) as f64)
            .collect(),
    }
}

/// Centered moving average (edge-truncated).
pub fn smooth(p: &[f64], win: usize) -> Vec<f64> {
    let n = p.len();
    if n == 0 || win <= 1 {
        return p.to_vec();
    }
    let half = win / 2;
    let mut pre = vec![0.0; n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + p[i];
    }
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half).min(n - 1);
            (pre[hi + 1] - pre[lo]) / (hi + 1 - lo) as f64
        })
        .collect()
}

/// Profile value at an end point, averaged over ±1 sample to reduce noise.
fn end_value(p: &[f64], i: usize) -> f64 {
    let lo = i.saturating_sub(1);
    let hi = (i + 1).min(p.len() - 1);
    p[lo..=hi].iter().sum::<f64>() / (hi + 1 - lo) as f64
}

pub fn baseline_at(p: &[f64], a: usize, b: usize, mode: BaselineMode, i: usize) -> f64 {
    let (va, vb) = (end_value(p, a), end_value(p, b));
    match mode {
        BaselineMode::Line => {
            if b == a {
                va
            } else {
                va + (vb - va) * (i as f64 - a as f64) / (b - a) as f64
            }
        }
        BaselineMode::MinHorizontal => va.min(vb),
    }
}

/// Area enclosed between the profile and the baseline in [start, end]
/// (what the ImageJ wand tool measures inside a closed peak).
pub fn peak_area(p: &[f64], start: usize, end: usize, mode: BaselineMode) -> f64 {
    if p.is_empty() {
        return 0.0;
    }
    let last = p.len() - 1;
    let a = start.min(end).min(last);
    let b = start.max(end).min(last);
    (a..=b).map(|i| (p[i] - baseline_at(p, a, b, mode, i)).max(0.0)).sum()
}

/// Automatic peak boundaries: from the highest point walk outward to the valley
/// (stops when the smoothed profile rises again or reaches the background level).
pub fn auto_peak_range(profile: &[f64], smooth_win: usize) -> (usize, usize) {
    let n = profile.len();
    if n < 3 {
        return (0, n.saturating_sub(1));
    }
    let s = smooth(profile, smooth_win);
    let mut pk = 0;
    for i in 1..n {
        if s[i] > s[pk] {
            pk = i;
        }
    }
    let pmin = s.iter().cloned().fold(f64::MAX, f64::min);
    let range = (s[pk] - pmin).max(1e-12);
    let tol = 0.02 * range;
    let floor = pmin + 0.03 * range;
    let look = smooth_win.max(3);
    let walk = |step: isize| -> usize {
        let mut best = pk;
        let mut i = pk as isize;
        loop {
            let j = i + step;
            if j < 0 || j >= n as isize {
                break;
            }
            let ju = j as usize;
            if s[ju] > s[best] + tol {
                break; // rising again -> neighbouring band
            }
            i = j;
            if s[ju] < s[best] {
                best = ju;
            }
            if s[best] <= floor {
                break; // reached background
            }
            // tail has flattened (e.g. a smear / high background below the band):
            // total further drop over the next `look` samples is < 1 % of the peak height
            let ahead = (i + step * look as isize).clamp(0, n as isize - 1) as usize;
            if s[best] - s[ahead] < 0.01 * range && (s[pk] - s[best]) > 0.5 * range {
                break;
            }
        }
        best
    };
    (walk(-1), walk(1))
}

// ---------------------------------------------------------------- lanes

#[derive(Clone, Debug)]
pub struct Lane {
    pub rect: Rect,
    pub start: usize,
    pub end: usize,
    /// If true the peak boundaries are re-detected automatically whenever the lane changes.
    pub auto_range: bool,
    pub profile: Vec<f64>,
    pub area: f64,
}

impl Lane {
    pub fn new(rect: Rect) -> Self {
        Self { rect, start: 0, end: 0, auto_range: true, profile: Vec::new(), area: 0.0 }
    }

    pub fn update(&mut self, img: &GrayImage, p: &Params) {
        self.profile = lane_profile(img, &self.rect, p.dir, p.invert);
        let n = self.profile.len();
        if n == 0 {
            self.start = 0;
            self.end = 0;
            self.area = 0.0;
            return;
        }
        if self.auto_range {
            let (a, b) = auto_peak_range(&self.profile, p.smooth_win);
            self.start = a;
            self.end = b;
        } else {
            self.end = self.end.min(n - 1);
            self.start = self.start.min(self.end);
        }
        self.area = peak_area(&self.profile, self.start, self.end, p.baseline);
    }

    /// Position of the lane centre across the lane direction (used for numbering left→right).
    pub fn across_center(&self, dir: LaneDir) -> f32 {
        let (cx, cy) = self.rect.center();
        match dir {
            LaneDir::Vertical => cx,
            LaneDir::Horizontal => cy,
        }
    }
}

fn lane_rect_in_strip(strip: &Rect, dir: LaneDir, start: f32, width: f32) -> Rect {
    match dir {
        LaneDir::Vertical => Rect::new(strip.x + start, strip.y, width, strip.h),
        LaneDir::Horizontal => Rect::new(strip.x, strip.y + start, strip.w, width),
    }
}

/// Split the strip into `n` equal lanes (80 % of the pitch, centred).
pub fn equal_lanes(strip: &Rect, n: usize, dir: LaneDir) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let len = match dir {
        LaneDir::Vertical => strip.w,
        LaneDir::Horizontal => strip.h,
    };
    let pitch = len / n as f32;
    let width = 0.8 * pitch;
    (0..n)
        .map(|k| lane_rect_in_strip(strip, dir, k as f32 * pitch + 0.1 * pitch, width))
        .collect()
}

/// Detect `n` evenly spaced lanes inside `strip`.
/// 1. Fit a regular grid (pitch, offset) to the cross-lane projection, maximising
///    mean signal inside each lane window minus the signal in the gaps between lanes
///    (gaps are the most reliable feature: bands are often flat-topped or saturated).
/// 2. Fine-tune each lane individually by shifting its window ±pitch/6.
pub fn auto_lanes(img: &GrayImage, strip: &Rect, n: usize, dir: LaneDir, invert: bool) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![*strip];
    }
    let proj = lane_profile(img, strip, dir.other(), invert);
    let len = proj.len();
    if len < 4 * n {
        return equal_lanes(strip, n, dir);
    }
    let nominal = len as f64 / n as f64;
    let s = smooth(&proj, ((nominal / 15.0) as usize).max(3));
    let mut pre = vec![0.0; len + 1];
    for i in 0..len {
        pre[i + 1] = pre[i] + s[i];
    }
    // mean of s over [a, b] (clipped)
    let mean = |a: f64, b: f64| -> f64 {
        let lo = a.round().max(0.0) as usize;
        let hi = (b.round() as isize).min(len as isize - 1);
        if hi < lo as isize {
            return 0.0;
        }
        let hi = hi as usize;
        (pre[hi + 1] - pre[lo]) / (hi + 1 - lo) as f64
    };
    // minimum of s within ±r of x
    let gap_min = |x: f64, r: f64| -> f64 {
        let lo = (x - r).round().max(0.0) as usize;
        let hi = ((x + r).round() as usize).min(len - 1);
        s[lo..=hi.max(lo)].iter().cloned().fold(f64::MAX, f64::min)
    };

    let mut best = (f64::MIN, nominal, nominal / 2.0);
    let mut pitch = 0.5 * nominal;
    while pitch <= nominal + 1e-9 {
        let half = 0.3 * pitch;
        let r = (0.06 * pitch).max(1.0);
        let span = pitch * (n as f64 - 1.0);
        let mut c0 = half;
        while c0 + span + half <= len as f64 - 1.0 {
            let on: f64 = (0..n).map(|k| {
                let c = c0 + k as f64 * pitch;
                mean(c - half, c + half)
            }).sum::<f64>() / n as f64;
            let off: f64 = (0..n - 1)
                .map(|k| gap_min(c0 + (k as f64 + 0.5) * pitch, r))
                .sum::<f64>() / (n - 1) as f64;
            let score = on - off;
            if score > best.0 {
                best = (score, pitch, c0);
            }
            c0 += 0.5;
        }
        pitch += 0.25;
    }
    let (_, pitch, c0) = best;
    let half = 0.4 * pitch; // final lane = 80 % of pitch
    let shift = (pitch / 6.0).max(1.0);

    let centers: Vec<f64> = (0..n)
        .map(|k| {
            let c = c0 + k as f64 * pitch;
            let mut bc = c;
            let mut bv = f64::MIN;
            let mut d = -shift;
            while d <= shift {
                let cc = c + d;
                if cc - half >= 0.0 && cc + half <= len as f64 - 1.0 {
                    let v = mean(cc - half, cc + half);
                    if v > bv + 1e-9 {
                        bv = v;
                        bc = cc;
                    }
                }
                d += 0.5;
            }
            bc
        })
        .collect();

    let width = (2.0 * half) as f32;
    let (x0, y0, _, _) = strip.px_bounds(img.width, img.height);
    let off = match dir {
        LaneDir::Vertical => x0 as f32 - strip.x,
        LaneDir::Horizontal => y0 as f32 - strip.y,
    };
    let strip_len = len as f32;
    centers
        .iter()
        .map(|&c| {
            let start = (off + c as f32 + 0.5 - width / 2.0).clamp(0.0, (strip_len - width).max(0.0));
            lane_rect_in_strip(strip, dir, start, width)
        })
        .collect()
}

// ---------------------------------------------------------------- results

#[derive(Clone, Debug, Default)]
pub struct ResultRow {
    pub lane: usize,
    pub t_area: Option<f64>,
    pub t_pct: Option<f64>,
    pub t_rel: Option<f64>,
    pub lc_area: Option<f64>,
    pub lc_rel: Option<f64>,
    pub norm: Option<f64>,
    pub norm_rel: Option<f64>,
}

fn ratio(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) if b > 0.0 => Some(a / b),
        _ => None,
    }
}

/// `t` = target band areas, `lc` = loading-control areas (may be empty),
/// `r` = 0-based reference lane.
/// t_rel  = T_i / T_ref               (ImageJ "relative density")
/// norm   = T_i / LC_i
/// norm_rel = (T_i/LC_i)/(T_ref/LC_ref)  (= "adjusted relative density")
pub fn summarize(t: &[f64], lc: &[f64], r: usize) -> Vec<ResultRow> {
    let n = t.len().max(lc.len());
    let t_sum: f64 = t.iter().sum();
    let norm = |i: usize| ratio(t.get(i).copied(), lc.get(i).copied());
    (0..n)
        .map(|i| {
            let ta = t.get(i).copied();
            let la = lc.get(i).copied();
            ResultRow {
                lane: i + 1,
                t_area: ta,
                t_pct: if t_sum > 0.0 { ta.map(|a| 100.0 * a / t_sum) } else { None },
                t_rel: ratio(ta, t.get(r).copied()),
                lc_area: la,
                lc_rel: ratio(la, lc.get(r).copied()),
                norm: norm(i),
                norm_rel: ratio(norm(i), norm(r)),
            }
        })
        .collect()
}

pub struct CsvInput<'a> {
    pub t_path: Option<String>,
    pub t_lanes: &'a [Lane],
    pub t_params: Params,
    pub lc_path: Option<String>,
    pub lc_lanes: &'a [Lane],
    pub lc_params: Params,
    pub ref_idx: usize,
}

fn f(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.4}")).unwrap_or_default()
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn params_desc(p: &Params) -> String {
    format!(
        "lanes={:?}; inverted={}; baseline={}; smoothing={}",
        p.dir,
        p.invert,
        match p.baseline {
            BaselineMode::Line => "straight line",
            BaselineMode::MinHorizontal => "horizontal (min)",
        },
        p.smooth_win
    )
}

pub fn build_csv(c: &CsvInput) -> String {
    let t: Vec<f64> = c.t_lanes.iter().map(|l| l.area).collect();
    let lc: Vec<f64> = c.lc_lanes.iter().map(|l| l.area).collect();
    let rows = summarize(&t, &lc, c.ref_idx);
    let mut s = String::from("\u{FEFF}");
    s += &format!("# Target image,{}\n", q(c.t_path.as_deref().unwrap_or("")));
    s += &format!("# Target settings,{}\n", q(&params_desc(&c.t_params)));
    if !c.lc_lanes.is_empty() {
        s += &format!("# Loading control image,{}\n", q(c.lc_path.as_deref().unwrap_or("")));
        s += &format!("# Loading control settings,{}\n", q(&params_desc(&c.lc_params)));
    }
    s += &format!("# Reference lane,{}\n", c.ref_idx + 1);
    s += "Lane,Target_Area,Target_Percent,Target_RelToRef,LC_Area,LC_RelToRef,Target_over_LC,Normalized_RelToRef,\
Target_x,Target_y,Target_w,Target_h,Target_PeakStart,Target_PeakEnd,LC_x,LC_y,LC_w,LC_h,LC_PeakStart,LC_PeakEnd\n";
    let geo = |l: Option<&Lane>| match l {
        Some(l) => format!(
            "{:.1},{:.1},{:.1},{:.1},{},{}",
            l.rect.x, l.rect.y, l.rect.w, l.rect.h, l.start, l.end
        ),
        None => ",,,,,".to_string(),
    };
    for r in &rows {
        s += &format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            r.lane,
            f(r.t_area),
            f(r.t_pct),
            f(r.t_rel),
            f(r.lc_area),
            f(r.lc_rel),
            f(r.norm),
            f(r.norm_rel),
            geo(c.t_lanes.get(r.lane - 1)),
            geo(c.lc_lanes.get(r.lane - 1)),
        );
    }
    s
}

// ---------------------------------------------------------------- overlay

fn put(img: &mut RgbImage, x: i64, y: i64, c: Rgb<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn hline(img: &mut RgbImage, x0: i64, x1: i64, y: i64, c: Rgb<u8>) {
    for x in x0..=x1 {
        put(img, x, y, c);
    }
}
fn vline(img: &mut RgbImage, x: i64, y0: i64, y1: i64, c: Rgb<u8>) {
    for y in y0..=y1 {
        put(img, x, y, c);
    }
}

/// 3×5 pixel digits for lane numbers on the overlay.
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn draw_number(img: &mut RgbImage, n: usize, cx: i64, top: i64, scale: i64, c: Rgb<u8>) {
    let s = n.to_string();
    let total_w = s.len() as i64 * 4 * scale - scale;
    let mut x = cx - total_w / 2;
    for ch in s.chars() {
        let d = ch.to_digit(10).unwrap_or(0) as usize;
        for (row, bits) in DIGITS[d].iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(img, x + col * scale + dx, top + row as i64 * scale + dy, c);
                        }
                    }
                }
            }
        }
        x += 4 * scale;
    }
}

/// Image with lane rectangles (yellow), peak boundaries (green) and lane numbers.
pub fn render_overlay(img: &GrayImage, lanes: &[Lane], dir: LaneDir) -> RgbImage {
    let disp = img.display_u8();
    let mut out = RgbImage::from_fn(img.width as u32, img.height as u32, |x, y| {
        let v = disp[y as usize * img.width + x as usize];
        Rgb([v, v, v])
    });
    let yellow = Rgb([255, 220, 0]);
    let green = Rgb([0, 200, 60]);
    let thick = ((img.width.max(img.height) as f32 / 800.0).round() as i64).max(1);
    let digit_scale = ((img.width.max(img.height) as f32 / 400.0).round() as i64).max(1);
    for (i, l) in lanes.iter().enumerate() {
        let (x0, y0, x1, y1) = l.rect.px_bounds(img.width, img.height);
        let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64 - 1, y1 as i64 - 1);
        for t in 0..thick {
            hline(&mut out, x0, x1, y0 + t, yellow);
            hline(&mut out, x0, x1, y1 - t, yellow);
            vline(&mut out, x0 + t, y0, y1, yellow);
            vline(&mut out, x1 - t, y0, y1, yellow);
        }
        if !l.profile.is_empty() {
            for k in [l.start, l.end] {
                match dir {
                    LaneDir::Vertical => hline(&mut out, x0, x1, y0 + k as i64, green),
                    LaneDir::Horizontal => vline(&mut out, x0 + k as i64, y0, y1, green),
                }
            }
        }
        draw_number(&mut out, i + 1, (x0 + x1) / 2, y0 + thick + 2, digit_scale, yellow);
    }
    out
}

// ---------------------------------------------------------------- synthetic test image

/// Synthetic blot: light background, dark Gaussian bands. Used by tests and the demo example.
pub fn synthetic_blot(target_amp: &[f32], lc_amp: &[f32]) -> GrayImage {
    let n = target_amp.len().max(lc_amp.len());
    let (w, h) = (80 * n + 40, 220usize);
    let mut data = vec![0f32; w * h];
    let mut seed: u32 = 12345;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((seed >> 8) % 7) as f32 - 3.0;
            let mut v = 232.0 + noise;
            for (row_y, amps) in [(70.0f32, target_amp), (160.0f32, lc_amp)] {
                for (k, &a) in amps.iter().enumerate() {
                    let cx = 20.0 + 80.0 * k as f32 + 40.0;
                    let dx = (x as f32 - cx) / 26.0;
                    let dy = (y as f32 - row_y) / 6.0;
                    v -= a * (-dx.powi(6)).exp() * (-0.5 * dy * dy).exp();
                }
            }
            data[y * w + x] = v.clamp(0.0, 255.0).round();
        }
    }
    GrayImage::from_raw(w, h, data, 255.0)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_baseline_area_of_triangle() {
        // flat background 10 with a triangular peak of height 10 and base 10
        let mut p = vec![10.0; 30];
        for i in 0..=10 {
            p[10 + i] = 10.0 + 10.0 - (i as f64 - 5.0).abs() * 2.0;
        }
        let a = peak_area(&p, 8, 22, BaselineMode::Line);
        assert!((a - 50.0).abs() < 1.0, "area {a}");
        let (s, e) = auto_peak_range(&p, 1);
        assert!(s <= 10 && e >= 20, "range {s}-{e}");
    }

    #[test]
    fn sloped_background_removed_by_line_baseline() {
        let p: Vec<f64> = (0..100)
            .map(|i| 0.5 * i as f64 + 40.0 * (-0.5 * ((i as f64 - 50.0) / 5.0).powi(2)).exp())
            .collect();
        let (s, e) = auto_peak_range(&p, 3);
        let a = peak_area(&p, s, e, BaselineMode::Line);
        let expected = 40.0 * 5.0 * (2.0 * std::f64::consts::PI).sqrt(); // ≈ 501
        assert!((a - expected).abs() / expected < 0.05, "area {a} vs {expected}");
    }

    #[test]
    fn synthetic_blot_relative_density() {
        let t = [100.0, 80.0, 60.0, 40.0, 20.0, 120.0];
        let lc = [90.0, 90.0, 70.0, 90.0, 90.0, 90.0];
        let img = synthetic_blot(&t, &lc);
        assert!(img.auto_invert());
        let p = Params { invert: true, ..Default::default() };
        let strip_t = Rect::new(0.0, 40.0, img.width as f32, 60.0);
        let strip_lc = Rect::new(0.0, 130.0, img.width as f32, 60.0);
        let lanes_t = auto_lanes(&img, &strip_t, 6, LaneDir::Vertical, true);
        let lanes_lc = auto_lanes(&img, &strip_lc, 6, LaneDir::Vertical, true);
        assert_eq!(lanes_t.len(), 6);
        for (k, r) in lanes_t.iter().enumerate() {
            let expected_cx = 60.0 + 80.0 * k as f32;
            assert!((r.center().0 - expected_cx).abs() < 4.0, "lane {k} at {:?}", r);
        }
        let area = |rects: &[Rect]| -> Vec<f64> {
            rects
                .iter()
                .map(|r| {
                    let mut l = Lane::new(*r);
                    l.update(&img, &p);
                    l.area
                })
                .collect()
        };
        let at = area(&lanes_t);
        let al = area(&lanes_lc);
        let rows = summarize(&at, &al, 0);
        for (i, r) in rows.iter().enumerate() {
            let exp_rel = t[i] as f64 / t[0] as f64;
            let got = r.t_rel.unwrap();
            assert!((got - exp_rel).abs() / exp_rel < 0.04, "lane {i}: {got} vs {exp_rel}");
            let exp_norm = (t[i] / lc[i]) as f64 / (t[0] / lc[0]) as f64;
            let gotn = r.norm_rel.unwrap();
            assert!((gotn - exp_norm).abs() / exp_norm < 0.05, "lane {i}: {gotn} vs {exp_norm}");
        }
    }

    #[test]
    fn rgb_conversion_is_unweighted_mean() {
        let mut im = RgbImage::new(1, 1);
        im.put_pixel(0, 0, Rgb([30, 60, 90]));
        let g = GrayImage::from_dynamic(&DynamicImage::ImageRgb8(im));
        assert_eq!(g.data[0], 60.0);
    }
}
