//! Material 3 Monitor — a system monitor for Arch Linux and Hyprland,
//! themed to match end4-pC.

mod app;
mod config;
mod gaming;
mod sample;
mod services;
mod startup;
mod ui;
mod ui_ext;

const USAGE: &str = "Usage: m3-monitor [--page PAGE]

Opens the system monitor.

Options:
  --page PAGE   Start on a page: overview, processes, services, startup,
                preferences (the page name alone works too)
  -h, --help    Show this help
  -V, --version";

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut page = app::Page::Overview;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("m3-monitor {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--page" => {
                i += 1;
                match args.get(i).and_then(|p| app::Page::parse(p)) {
                    Some(p) => page = p,
                    None => {
                        eprintln!("m3-monitor: --page needs one of overview, processes, services, startup, preferences\n\n{USAGE}");
                        std::process::exit(2);
                    }
                }
            }
            other => match app::Page::parse(other.trim_start_matches("--")) {
                Some(p) => page = p,
                None => {
                    eprintln!("m3-monitor: unknown argument {other:?}\n\n{USAGE}");
                    std::process::exit(2);
                }
            },
        }
        i += 1;
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Monitor")
        .with_app_id("material_3_monitor")
        .with_inner_size([1180.0, 800.0])
        .with_min_inner_size([720.0, 480.0]);
    // Development: a fixed window size for screenshots ("1240x900").
    if let Some((w, h)) = std::env::var("M3_MONITOR_WINDOW_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
    }) {
        viewport = viewport.with_inner_size([w, h]);
    }
    eframe::run_native(
        "material_3_monitor",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, page)))),
    )
}
