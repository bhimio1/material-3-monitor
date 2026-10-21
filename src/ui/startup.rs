//! The Startup page: XDG autostart entries (toggle), Hyprland's startup
//! commands (read-only) and enabled user services.

use crate::app::{App, Page, Refresh, StartupData};
use crate::startup;
use egui::Ui;
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::sys::paths;
use material_3_ui::widgets::{self, ButtonKind};

pub fn show(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    app.startup.ensure(&ctx, || StartupData {
        autostart: startup::autostart(),
        hyprland: startup::hyprland(),
        user_services: startup::enabled_user_services(),
    });
    let mut toggle: Option<(startup::AutostartEntry, bool)> = None;
    let mut open: Option<std::path::PathBuf> = None;
    let mut disable: Option<String> = None;
    let mut go_services = false;

    common::page(ui, ICON_ROCKET_LAUNCH, "Startup", "What runs when you log in.", |ui| {
        let Some(data) = &app.startup.value else {
            common::loading(ui, "Looking…");
            return;
        };
        common::card(ui, "Autostart apps", |ui| {
            common::note(
                ui,
                "XDG autostart entries. Hyprland runs them when your session uses uwsm or systemd's autostart generator (dex works too). Turning one off writes Hidden=true to your own copy in ~/.config/autostart.",
            );
            if data.autostart.is_empty() {
                common::empty_hint(ui, "No autostart entries");
            }
            for e in &data.autostart {
                let sub = if e.comment.is_empty() { e.exec.clone() } else { format!("{} · {}", e.comment, e.exec) };
                common::list_row(ui, ICON_APPS, e.enabled, &e.name, &sub, |ui| {
                    let mut on = e.enabled;
                    if widgets::switch(ui, &mut on).clicked() {
                        toggle = Some((e.clone(), on));
                    }
                    if e.user && widgets::icon_button(ui, ICON_EDIT_NOTE, "Open the file", true).clicked() {
                        open = Some(e.path.clone());
                    }
                });
            }
        });

        common::card(ui, "Hyprland", |ui| {
            common::note(ui, "Commands your Hyprland config runs at startup (hl.on(\"hyprland.start\", …) on Lua configs, exec-once on hyprland.conf). Edit them in the config file.");
            if data.hyprland.is_empty() {
                common::empty_hint(ui, "No startup commands found in ~/.config/hypr");
            }
            for h in &data.hyprland {
                let sub = format!("{}:{}", paths::tilde(&h.file), h.line);
                common::list_row(ui, ICON_TERMINAL, true, &h.command, &sub, |ui| {
                    if widgets::icon_button(ui, ICON_OPEN_IN_NEW, "Open the file", true).clicked() {
                        open = Some(h.file.clone());
                    }
                });
            }
        });

        common::card(ui, "User services", |ui| {
            match &data.user_services {
                Err(e) => common::note(ui, e),
                Ok(list) if list.is_empty() => common::empty_hint(ui, "No enabled user services"),
                Ok(list) => {
                    common::note(ui, "systemd user services that start with your session.");
                    for name in list {
                        common::list_row(ui, ICON_DNS, true, name, "enabled", |ui| {
                            if widgets::button(ui, ButtonKind::Text, None, "Disable").clicked() {
                                disable = Some(name.clone());
                            }
                        });
                    }
                }
            }
            ui.add_space(4.0);
            if widgets::button(ui, ButtonKind::Tonal, Some(ICON_DNS), "All services").clicked() {
                go_services = true;
            }
            ui.add_space(8.0);
        });
    });

    if go_services {
        app.page = Page::Services;
    }
    if let Some((entry, on)) = toggle {
        let dir = paths::config_home().join("autostart");
        app.run_job(&ctx, Refresh::Startup, move || {
            startup::set_enabled(&entry, on, &dir)
                .map(|_| format!("{} {}", entry.name, if on { "starts at login" } else { "won't start at login" }))
        });
    }
    if let Some(name) = disable {
        app.run_job(&ctx, Refresh::Startup, move || {
            crate::services::act(crate::services::Scope::User, crate::services::Verb::Disable, &name).map(|_| format!("Disabled {name}"))
        });
    }
    if let Some(path) = open {
        let p = path.to_string_lossy().into_owned();
        let opener = if material_3_ui::sys::cmd::exists("m3-text") { "m3-text" } else { "xdg-open" };
        if let Err(e) = material_3_ui::sys::cmd::spawn_detached(opener, &[&p]) {
            app.toasts.error(e);
        }
    }
}
