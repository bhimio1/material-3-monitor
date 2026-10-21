//! App state, the sidebar, and running actions in the background.

use crate::config::Config;
use crate::gaming::Gaming;
use crate::sample::history::History;
use crate::sample::{Roots, Sampler, Snapshot};
use crate::services::{Scope, Unit};
use crate::startup::{AutostartEntry, HyprExec};
use crate::ui;
use material_3_ui::charts;
use material_3_ui::common;
use material_3_ui::format;
use material_3_ui::icons::icons::*;
use material_3_ui::icons::MaterialIcon;
use material_3_ui::sys::task::Task;
use material_3_ui::theme::ThemeManager;
use material_3_ui::toasts::Toasts;
use material_3_ui::widgets;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Processes,
    Services,
    Startup,
    Preferences,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Overview, Page::Processes, Page::Services, Page::Startup, Page::Preferences];

    pub fn parse(s: &str) -> Option<Page> {
        Some(match s.to_ascii_lowercase().as_str() {
            "overview" | "performance" | "resources" => Page::Overview,
            "processes" | "apps" => Page::Processes,
            "services" => Page::Services,
            "startup" | "autostart" => Page::Startup,
            "preferences" | "settings" | "prefs" => Page::Preferences,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Processes => "Processes",
            Page::Services => "Services",
            Page::Startup => "Startup",
            Page::Preferences => "Preferences",
        }
    }

    pub fn icon(self) -> MaterialIcon {
        match self {
            Page::Overview => ICON_MONITORING,
            Page::Processes => ICON_APPS,
            Page::Services => ICON_DNS,
            Page::Startup => ICON_ROCKET_LAUNCH,
            Page::Preferences => ICON_SETTINGS,
        }
    }
}

/// A question to confirm before acting on processes.
#[derive(Clone, Debug)]
pub struct ConfirmEnd {
    pub pids: Vec<u32>,
    pub name: String,
    pub signal: crate::sample::procs::Signal,
}

#[derive(Clone, Debug)]
pub struct StartupData {
    pub autostart: Vec<AutostartEntry>,
    pub hyprland: Vec<HyprExec>,
    pub user_services: Result<Vec<String>, String>,
}

pub struct App {
    pub config: Config,
    pub theme: ThemeManager,
    pub toasts: Toasts<()>,
    pub sampler: Sampler,
    pub snap: Option<Snapshot>,
    pub history: History,
    pub page: Page,
    pub samples: u64,

    // Processes
    pub proc_query: String,
    /// The selected process, and whether it stands for its whole app group.
    pub selected: Option<(u32, bool)>,
    pub expanded: HashSet<u32>,
    pub collapsed: HashSet<u32>,
    pub confirm: Option<ConfirmEnd>,
    pub details: Option<u32>,
    pub renice: Option<(u32, i32, String)>,

    // Services
    pub services: Task<Result<Vec<Unit>, String>>,
    pub services_loaded: Option<Scope>,
    pub svc_query: String,
    pub svc_filter: ui::services::Filter,
    pub logs: Option<(String, Task<Result<String, String>>)>,

    // Startup
    pub startup: Task<StartupData>,

    // Overview
    pub gaming: Task<Gaming>,
    gaming_at: Option<Instant>,

    /// The running action: a message for the toast, and what to refresh.
    pub job: Task<(Result<String, String>, Refresh)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Processes,
    Services,
    Startup,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, page: Page) -> Self {
        let config = Config::load();
        let theme = ThemeManager::new(&cc.egui_ctx, &config.appearance);
        let sampler = Sampler::start(cc.egui_ctx.clone(), Roots::from_env(), Duration::from_millis(config.interval_ms));
        let history = History::new(config.history_len());
        Self {
            theme,
            toasts: Toasts::default(),
            sampler,
            snap: None,
            history,
            page,
            samples: 0,
            proc_query: String::new(),
            selected: None,
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            confirm: None,
            details: None,
            renice: None,
            services: Task::default(),
            services_loaded: None,
            svc_query: String::new(),
            svc_filter: ui::services::Filter::All,
            logs: None,
            startup: Task::default(),
            gaming: Task::default(),
            gaming_at: None,
            job: Task::default(),
            config,
        }
    }

    pub fn save_config(&mut self) {
        if let Err(e) = self.config.save() {
            self.toasts.error(format!("Couldn't save preferences: {e}"));
        }
        self.sampler.control.interval_ms.store(self.config.interval_ms, Ordering::Relaxed);
        self.history.set_len(self.config.history_len());
    }

    /// Run `f` in the background; its message becomes a toast and `refresh`
    /// says what to reload afterwards.
    pub fn run_job(&mut self, ctx: &egui::Context, refresh: Refresh, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        self.job.spawn(ctx, move || (f(), refresh));
    }

    fn take_snapshot(&mut self, snap: Snapshot) {
        self.samples += 1;
        let h = &mut self.history;
        h.push("cpu", snap.cpu * 100.0);
        for (i, c) in snap.cores.iter().enumerate() {
            h.push(&format!("core{i}"), c * 100.0);
        }
        let freq = if snap.freqs.is_empty() { 0.0 } else { snap.freqs.iter().sum::<f32>() / snap.freqs.len() as f32 };
        h.push("freq", freq);
        if snap.mem.total > 0 {
            h.push("mem", snap.mem.used() as f32 / snap.mem.total as f32 * 100.0);
        }
        if snap.mem.swap_total > 0 {
            h.push("swap", snap.mem.swap_used() as f32 / snap.mem.swap_total as f32 * 100.0);
        }
        for g in &snap.gpus {
            h.push(&format!("gpu:{}", g.key), g.busy.unwrap_or(0.0));
        }
        for d in &snap.disks {
            h.push(&format!("disk:{}:r", d.name), d.read_rate as f32);
            h.push(&format!("disk:{}:w", d.name), d.write_rate as f32);
        }
        for n in &snap.nets {
            h.push(&format!("net:{}:rx", n.name), n.rx_rate as f32);
            h.push(&format!("net:{}:tx", n.name), n.tx_rate as f32);
        }
        if let Some(t) = snap.cpu_temp {
            h.push("temp:cpu", t);
        }
        // Forget unplugged disks and interfaces now and then.
        if self.samples.is_multiple_of(60) {
            let live: HashSet<String> = snap
                .disks
                .iter()
                .map(|d| format!("disk:{}:", d.name))
                .chain(snap.nets.iter().map(|n| format!("net:{}:", n.name)))
                .chain(snap.gpus.iter().map(|g| format!("gpu:{}", g.key)))
                .collect();
            h.retain(|k| {
                !(k.starts_with("disk:") || k.starts_with("net:") || k.starts_with("gpu:"))
                    || live.iter().any(|l| k.starts_with(l.as_str()))
            });
        }
        self.snap = Some(snap);
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some((result, refresh)) = self.job.take() {
            match result {
                Ok(msg) if !msg.is_empty() => self.toasts.success(msg),
                Ok(_) => {}
                Err(e) => self.toasts.error(e),
            }
            match refresh {
                Refresh::Processes => self.sampler.refresh(),
                Refresh::Services => self.services_loaded = None,
                Refresh::Startup => self.startup.value = None,
            }
        }
        // Gaming state on the Overview, every 5 seconds.
        self.gaming.poll();
        if self.page == Page::Overview
            && !self.gaming.running()
            && self.gaming_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            self.gaming_at = Some(Instant::now());
            self.gaming.spawn(ctx, crate::gaming::query);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = widgets::palette(ui);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            widgets::icon(ui, ICON_MONITORING, 26.0, p.primary);
            ui.label(widgets::title_text("Monitor", 20.0).color(p.on_surface));
        });
        ui.add_space(14.0);
        let counts = self.snap.as_ref().map(|s| {
            (
                format::percent(s.cpu),
                if s.mem.total > 0 { format::percent(s.mem.used() as f32 / s.mem.total as f32) } else { String::new() },
            )
        });
        for page in Page::ALL {
            if page == Page::Preferences {
                continue;
            }
            let trailing = match (page, &counts) {
                (Page::Overview, Some((cpu, _))) => Some(cpu.as_str()),
                _ => None,
            };
            if widgets::nav_item(ui, page.icon(), page.label(), self.page == page, trailing).clicked() {
                // Lists that don't update themselves are re-read on each visit.
                if self.page != page {
                    match page {
                        Page::Services => self.services_loaded = None,
                        Page::Startup => self.startup.value = None,
                        _ => {}
                    }
                }
                self.page = page;
            }
        }
        // Small live CPU and memory charts.
        if let Some((cpu, mem)) = &counts {
            ui.add_space(16.0);
            widgets::section_header(ui, "Now");
            let len = self.history.len();
            let w = ui.available_width();
            for (key, label, value, color) in [("cpu", "CPU", cpu, p.primary), ("mem", "Memory", mem, p.tertiary)] {
                common::stat_line(ui, label, value);
                let values = self.history.values(key);
                charts::area_chart(ui, egui::vec2(w, 40.0), &[charts::Series { values: &values, color }], 100.0, len);
                ui.add_space(6.0);
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if widgets::nav_item(ui, ICON_SETTINGS, "Preferences", self.page == Page::Preferences, None).clicked() {
                self.page = Page::Preferences;
            }
            if self.sampler.control.paused.load(Ordering::Relaxed) {
                ui.label(egui::RichText::new("Paused while in the background").size(12.0).color(p.on_surface_variant));
            }
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.theme.poll(&self.config.appearance) {
            ctx.request_repaint();
        }
        if let Some(e) = self.theme.take_error() {
            self.toasts.error(e);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let paused = self.config.pause_when_unfocused && !focused;
        let was = self.sampler.control.paused.swap(paused, Ordering::Relaxed);
        if was && !paused {
            self.sampler.refresh();
        }
        self.sampler
            .control
            .want_processes
            .store(self.page == Page::Processes, Ordering::Relaxed);
        if let Some(snap) = self.sampler.poll() {
            self.take_snapshot(snap);
        }
        self.poll_jobs(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = self.theme.palette().clone();
        let surface = egui::Frame::new().fill(p.surface);
        egui::Panel::left("nav")
            .default_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(surface.inner_margin(egui::Margin::same(12)))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(surface.inner_margin(egui::Margin { left: 0, right: 12, top: 12, bottom: 12 }))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(p.surface_container_low)
                    .corner_radius(egui::CornerRadius::same(24))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        match self.page {
                            Page::Overview => ui::overview::show(self, ui),
                            Page::Processes => ui::processes::show(self, ui),
                            Page::Services => ui::services::show(self, ui),
                            Page::Startup => ui::startup::show(self, ui),
                            Page::Preferences => ui::prefs::show(self, ui),
                        }
                    });
            });
        let ctx = ui.ctx().clone();
        ui::processes::dialogs(self, &ctx);
        ui::services::dialogs(self, &ctx);
        self.toasts.show(&ctx, &p, 20.0);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.palette().surface.to_normalized_gamma_f32()
    }
}
