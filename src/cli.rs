//! Headless batch mode:
//!   wb_quant --cli <target.tif> --lanes 6 [--strip x,y,w,h] [--lc <actin.tif>] [--lc-strip x,y,w,h]
//!            [--dir v|h] [--invert auto|yes|no] [--baseline line|min] [--smooth 5] [--ref 1]
//!            [--out results.csv] [--overlay overlay.png]

use crate::analysis::*;
use std::path::PathBuf;

const USAGE: &str = "usage: wb_quant --cli <target_image> --lanes N [--strip x,y,w,h] \
[--lc <loading_control_image>] [--lc-strip x,y,w,h] [--dir v|h] [--invert auto|yes|no] \
[--baseline line|min] [--smooth N] [--ref N] [--out results.csv] [--overlay overlay.png]";

fn parse_rect(s: &str) -> Result<Rect, String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|t| t.trim().parse::<f32>().map_err(|_| format!("bad rectangle '{s}'")))
        .collect::<Result<_, _>>()?;
    if v.len() != 4 {
        return Err(format!("rectangle needs x,y,w,h: '{s}'"));
    }
    Ok(Rect::new(v[0], v[1], v[2], v[3]))
}

fn measure(path: &PathBuf, strip: Option<Rect>, n: usize, mut p: Params, inv: InvertMode) -> Result<(GrayImage, Vec<Lane>, Params), String> {
    let img = GrayImage::open(path)?;
    p.invert = inv.resolve(&img);
    let strip = strip
        .unwrap_or(Rect::new(0.0, 0.0, img.width as f32, img.height as f32))
        .intersect_image(img.width, img.height);
    let lanes = auto_lanes(&img, &strip, n, p.dir, p.invert)
        .into_iter()
        .map(|r| {
            let mut l = Lane::new(r);
            l.update(&img, &p);
            l
        })
        .collect();
    Ok((img, lanes, p))
}

pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            2
        }
    }
}

fn run_inner(args: &[String]) -> Result<(), String> {
    // --demo out.png : write a synthetic 6-lane blot (target row + loading-control row) for testing
    if args.first().map(|a| a == "--demo").unwrap_or(false) {
        let out = args.get(1).ok_or("--demo needs an output file")?;
        let img = synthetic_blot(&[100.0, 80.0, 60.0, 40.0, 20.0, 120.0], &[90.0, 90.0, 70.0, 90.0, 90.0, 90.0]);
        let buf: Vec<u8> = img.data.iter().map(|&v| v as u8).collect();
        image::GrayImage::from_raw(img.width as u32, img.height as u32, buf)
            .ok_or("internal error")?
            .save(out)
            .map_err(|e| e.to_string())?;
        println!("demo blot written to {out}: target row y≈40-100, loading-control row y≈130-190, 6 lanes");
        println!("expected target relative density: 1.00 0.80 0.60 0.40 0.20 1.20");
        return Ok(());
    }
    let mut target: Option<PathBuf> = None;
    let mut lc: Option<PathBuf> = None;
    let (mut strip, mut lc_strip) = (None, None);
    let mut n = 0usize;
    let mut p = Params::default();
    let mut inv = InvertMode::Auto;
    let mut ref_lane = 1usize;
    let (mut out, mut overlay) = (None::<PathBuf>, None::<PathBuf>);

    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("missing value for {a}"));
        match a.as_str() {
            "--lanes" => n = val()?.parse().map_err(|_| "bad --lanes")?,
            "--strip" => strip = Some(parse_rect(&val()?)?),
            "--lc" => lc = Some(PathBuf::from(val()?)),
            "--lc-strip" => lc_strip = Some(parse_rect(&val()?)?),
            "--dir" => {
                p.dir = match val()?.as_str() {
                    "v" | "vertical" => LaneDir::Vertical,
                    "h" | "horizontal" => LaneDir::Horizontal,
                    o => return Err(format!("bad --dir {o}")),
                }
            }
            "--invert" => {
                inv = match val()?.as_str() {
                    "auto" => InvertMode::Auto,
                    "yes" => InvertMode::Yes,
                    "no" => InvertMode::No,
                    o => return Err(format!("bad --invert {o}")),
                }
            }
            "--baseline" => {
                p.baseline = match val()?.as_str() {
                    "line" => BaselineMode::Line,
                    "min" => BaselineMode::MinHorizontal,
                    o => return Err(format!("bad --baseline {o}")),
                }
            }
            "--smooth" => p.smooth_win = val()?.parse().map_err(|_| "bad --smooth")?,
            "--ref" => ref_lane = val()?.parse().map_err(|_| "bad --ref")?,
            "--out" => out = Some(PathBuf::from(val()?)),
            "--overlay" => overlay = Some(PathBuf::from(val()?)),
            s if !s.starts_with("--") && target.is_none() => target = Some(PathBuf::from(s)),
            o => return Err(format!("unknown argument {o}")),
        }
    }
    let target = target.ok_or("no target image given")?;
    if n == 0 {
        return Err("--lanes N is required".into());
    }
    let (timg, tl, tp) = measure(&target, strip, n, p, inv)?;
    let (ll, lp) = match &lc {
        Some(path) => {
            let (_, l, pp) = measure(path, lc_strip, n, p, inv)?;
            (l, pp)
        }
        None => (Vec::new(), p),
    };
    let csv = build_csv(&CsvInput {
        t_path: Some(target.display().to_string()),
        t_lanes: &tl,
        t_params: tp,
        lc_path: lc.as_ref().map(|p| p.display().to_string()),
        lc_lanes: &ll,
        lc_params: lp,
        ref_idx: ref_lane.saturating_sub(1),
    });
    match out {
        Some(o) => std::fs::write(&o, csv).map_err(|e| format!("cannot write {}: {e}", o.display()))?,
        None => print!("{}", csv.trim_start_matches('\u{FEFF}')),
    }
    if let Some(o) = overlay {
        render_overlay(&timg, &tl, tp.dir)
            .save(&o)
            .map_err(|e| format!("cannot write {}: {e}", o.display()))?;
    }
    Ok(())
}
