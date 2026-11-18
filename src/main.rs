#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod app;
mod cli;

use eframe::egui;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s == "--cli").unwrap_or(false) {
        // release builds on Windows have no console of their own: reuse the calling one
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn AttachConsole(pid: u32) -> i32;
            }
            unsafe {
                AttachConsole(0xFFFF_FFFF);
            }
        }
        std::process::exit(cli::run(&args[2..]));
    }
    // a file passed on the command line (or dropped onto the .exe) opens as the target blot
    let initial = args.get(1).filter(|a| !a.starts_with("--")).map(std::path::PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("WB Band Quantifier")
            .with_drag_and_drop(true),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "WB Band Quantifier",
        options,
        Box::new(move |cc| Ok(Box::new(app::WbApp::new(cc, initial)))),
    ) {
        eprintln!("GUI error: {e}");
        std::process::exit(1);
    }
}
