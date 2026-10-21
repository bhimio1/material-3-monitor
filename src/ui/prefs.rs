//! Preferences: the shared Appearance card and the monitor's own options.

use crate::app::App;
use egui::Ui;
use material_3_ui::common::{self, card, row};
use material_3_ui::icons::icons::*;
use material_3_ui::{prefs, widgets};

pub fn show(app: &mut App, ui: &mut Ui) {
    let mut changed = false;
    common::page(ui, ICON_SETTINGS, "Preferences", "", |ui| {
        if prefs::appearance_card(ui, &mut app.config.appearance, &app.theme) {
            app.theme.apply(&app.config.appearance);
            changed = true;
        }
        card(ui, "Monitoring", |ui| {
            row(ui, "Update every", "How often everything is sampled.", |ui| {
                let mut v = app.config.interval_ms;
                if widgets::segmented(ui, &mut v, &[(500, ICON_SPEED, "0.5 s"), (1000, ICON_SPEED, "1 s"), (2000, ICON_SPEED, "2 s"), (5000, ICON_SPEED, "5 s")]) {
                    app.config.interval_ms = v;
                    changed = true;
                }
            });
            ui.separator();
            row(ui, "History", "How far back the charts go.", |ui| {
                let mut v = app.config.history_seconds;
                if widgets::segmented(ui, &mut v, &[(60, ICON_SCHEDULE, "1 min"), (120, ICON_SCHEDULE, "2 min"), (300, ICON_SCHEDULE, "5 min")]) {
                    app.config.history_seconds = v;
                    changed = true;
                }
            });
            ui.separator();
            row(ui, "Pause in the background", "Stop sampling while the window isn't focused, so the monitor costs nothing when you're not looking.", |ui| {
                changed |= widgets::switch(ui, &mut app.config.pause_when_unfocused).changed();
            });
            ui.separator();
            row(ui, "Virtual network interfaces", "Show bridges, VPNs and container interfaces on the Overview.", |ui| {
                changed |= widgets::switch(ui, &mut app.config.show_virtual_interfaces).changed();
            });
        });
        card(ui, "Processes", |ui| {
            row(ui, "Confirm before ending", "Ask before ending or killing a process.", |ui| {
                changed |= widgets::switch(ui, &mut app.config.confirm_end).changed();
            });
        });
        card(ui, "About", |ui| {
            common::facts(
                ui,
                "about",
                &[
                    ("Version", env!("CARGO_PKG_VERSION").to_string()),
                    ("Config", material_3_ui::sys::paths::tilde(&crate::config::Config::path())),
                ],
            );
        });
    });
    if changed {
        app.save_config();
    }
}
