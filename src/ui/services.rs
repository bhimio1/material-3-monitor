//! The Services page: systemd user and system services with start / stop /
//! restart / enable / disable and their logs.

use crate::app::{App, Refresh};
use crate::services::{self, Scope, Unit, Verb};
use egui::{RichText, Ui};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::widgets::{self, ButtonKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Running,
    Failed,
    Enabled,
}

fn ensure_loaded(app: &mut App, ctx: &egui::Context) {
    let scope = app.config.services_scope;
    app.services.poll();
    if app.services_loaded != Some(scope) && !app.services.running() {
        app.services_loaded = Some(scope);
        app.services.spawn(ctx, move || services::list(scope));
    }
}

fn status_text(u: &Unit) -> String {
    let mut s = if u.active == "active" { u.sub.clone() } else { u.active.clone() };
    if let Some(f) = &u.file_state {
        s.push_str(" · ");
        s.push_str(f);
    }
    if !u.description.is_empty() {
        s = format!("{} · {s}", u.description);
    }
    s
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    ensure_loaded(app, &ctx);
    let p = widgets::palette(ui);
    let mut action: Option<(Verb, String)> = None;
    let mut logs: Option<String> = None;
    let mut reload = false;

    common::page(ui, ICON_DNS, "Services", "Background services run by systemd. User services run as you; system services ask for your password to change.", |ui| {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let mut scope = app.config.services_scope;
            if widgets::segmented(ui, &mut scope, &[(Scope::User, ICON_PERSON, "User"), (Scope::System, ICON_COMPUTER, "System")]) {
                app.config.services_scope = scope;
                app.save_config();
            }
            ui.add_space(8.0);
            common::search_field(ui, &mut app.svc_query, "Search services", 260.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if common::refresh_button(ui, app.services.running()) {
                    reload = true;
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (f, label) in [(Filter::All, "All"), (Filter::Running, "Running"), (Filter::Failed, "Failed"), (Filter::Enabled, "Enabled")] {
                if widgets::filter_chip(ui, label, app.svc_filter == f, None).clicked() {
                    app.svc_filter = f;
                }
            }
        });

        match &app.services.value {
            None => common::loading(ui, "Asking systemd…"),
            Some(Err(e)) => common::banner(ui, ICON_ERROR, e, true, |_| {}),
            Some(Ok(units)) => {
                let q = app.svc_query.to_lowercase();
                let shown: Vec<&Unit> = units
                    .iter()
                    .filter(|u| q.is_empty() || u.name.to_lowercase().contains(&q) || u.description.to_lowercase().contains(&q))
                    .filter(|u| match app.svc_filter {
                        Filter::All => true,
                        Filter::Running => u.is_running(),
                        Filter::Failed => u.is_failed(),
                        Filter::Enabled => u.is_enabled(),
                    })
                    .collect();
                let failed = units.iter().filter(|u| u.is_failed()).count();
                if failed > 0 && app.svc_filter != Filter::Failed {
                    let mut show_failed = false;
                    common::banner(ui, ICON_WARNING, &format!("{failed} service{} failed.", if failed == 1 { "" } else { "s" }), true, |ui| {
                        show_failed = widgets::button(ui, ButtonKind::Text, None, "Show").clicked();
                    });
                    if show_failed {
                        app.svc_filter = Filter::Failed;
                    }
                }
                common::card(ui, &format!("{} services", shown.len()), |ui| {
                    if shown.is_empty() {
                        common::empty_hint(ui, "No services match");
                    }
                    let busy = app.job.running();
                    for u in shown.iter().take(400) {
                        let icon = if u.is_failed() { ICON_ERROR } else if u.is_running() { ICON_CHECK_CIRCLE } else { ICON_RADIO_BUTTON_UNCHECKED };
                        let resp = common::list_row(ui, icon, u.is_running(), &u.name, &status_text(u), |ui| {
                            if widgets::icon_button(ui, ICON_RECEIPT_LONG, "Logs", true).clicked() {
                                logs = Some(u.name.clone());
                            }
                            if u.can_toggle() {
                                let mut on = u.is_enabled();
                                if widgets::switch(ui, &mut on).on_hover_text("Start with the session / at boot").clicked() && !busy {
                                    action = Some((if on { Verb::Enable } else { Verb::Disable }, u.name.clone()));
                                }
                            }
                            if u.is_running() {
                                if widgets::icon_button(ui, ICON_RESTART_ALT, "Restart", !busy).clicked() {
                                    action = Some((Verb::Restart, u.name.clone()));
                                }
                                if widgets::icon_button(ui, ICON_STOP, "Stop", !busy).clicked() {
                                    action = Some((Verb::Stop, u.name.clone()));
                                }
                            } else if u.load != "masked" && widgets::icon_button(ui, ICON_PLAY_ARROW, "Start", !busy).clicked() {
                                action = Some((Verb::Start, u.name.clone()));
                            }
                        });
                        if resp.double_clicked() {
                            logs = Some(u.name.clone());
                        }
                    }
                    if shown.len() > 400 {
                        common::note(ui, &format!("{} more — search to narrow the list.", shown.len() - 400));
                    }
                });
            }
        }
        let _ = p;
    });

    if reload {
        app.services_loaded = None;
    }
    let scope = app.config.services_scope;
    if let Some((verb, unit)) = action {
        app.run_job(&ctx, Refresh::Services, move || {
            services::act(scope, verb, &unit).map(|_| format!("{} {unit}", verb.past()))
        });
    }
    if let Some(unit) = logs {
        let mut task = material_3_ui::sys::task::Task::default();
        let u = unit.clone();
        task.spawn(&ctx, move || services::logs(scope, &u));
        app.logs = Some((unit, task));
    }
}

/// The logs dialog.
pub fn dialogs(app: &mut App, ctx: &egui::Context) {
    let p = app.theme.palette().clone();
    let Some((unit, task)) = &mut app.logs else { return };
    task.poll();
    let mut close = false;
    let title = unit.clone();
    let resp = common::modal(ctx, &p, "logs", ICON_RECEIPT_LONG, &title, |ui| {
        let size = ctx.content_rect().size();
        ui.set_width((size.x - 160.0).clamp(360.0, 1000.0));
        match &task.value {
            None => common::loading(ui, "Reading the journal…"),
            Some(Err(e)) => common::banner(ui, ICON_ERROR, e, true, |_| {}),
            Some(Ok(text)) => {
                let text = if text.trim().is_empty() { "No log entries." } else { text.as_str() };
                egui::ScrollArea::both()
                    .max_height((size.y - 260.0).max(200.0))
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().size(12.0).color(p.on_surface));
                    });
            }
        }
        common::dialog_buttons(ui, |ui| {
            if widgets::button(ui, ButtonKind::Text, None, "Close").clicked() {
                close = true;
            }
        });
    });
    if close || resp.should_close() {
        app.logs = None;
    }
}
