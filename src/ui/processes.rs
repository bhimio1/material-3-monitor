//! The Processes page: a sortable table of apps (grouped), every process,
//! or the process tree, with end / kill / stop / continue / renice actions.

use crate::app::{App, ConfirmEnd, Refresh};
use crate::config::ProcessView;
use crate::sample::procs::{self, AppGroup, ProcRow, Signal, SortKey};
use crate::ui_ext as ext;
use egui::{pos2, vec2, Align2, CornerRadius, FontId, Rect, RichText, Sense, Ui};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::icons::MaterialIcon;
use material_3_ui::widgets::{self, ButtonKind};
use std::collections::HashMap;
use std::sync::Arc;

const ROW_H: f32 = 36.0;

/// Columns after the name: key, header, width, right-aligned.
const COLUMNS: [(SortKey, &str, f32, bool); 6] = [
    (SortKey::Pid, "PID", 72.0, true),
    (SortKey::User, "User", 96.0, false),
    (SortKey::Cpu, "CPU", 72.0, true),
    (SortKey::Memory, "Memory", 92.0, true),
    (SortKey::Io, "Disk", 96.0, true),
    (SortKey::State, "State", 104.0, false),
];

/// A Material icon for a process, guessed from its name.
pub fn icon_for(row: &ProcRow) -> MaterialIcon {
    if row.kernel_thread {
        return ICON_MEMORY;
    }
    let n = row.name.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|k| n.contains(k));
    if has(&["firefox", "zen", "chrom", "brave", "vivaldi", "librewolf", "m3-browser", "web co", "webkit"]) {
        ICON_LANGUAGE
    } else if has(&["kitty", "foot", "alacritty", "ghostty", "wezterm", "konsole", "terminal", "bash", "fish", "zsh", "tmux"]) {
        ICON_TERMINAL
    } else if has(&["steam", "gamescope", "wine", "proton", ".exe", "lutris", "heroic", "m3-games", "mangohud"]) {
        ICON_SPORTS_ESPORTS
    } else if has(&["code", "zed", "nvim", "vim", "helix", "emacs", "cargo", "rustc", "rust-analyzer", "m3-text"]) {
        ICON_CODE
    } else if has(&["discord", "vesktop", "telegram", "signal", "element", "slack"]) {
        ICON_CHAT
    } else if has(&["pipewire", "wireplumber", "pulse", "spotify", "m3-music", "mpv", "vlc", "easyeffects"]) {
        ICON_GRAPHIC_EQ
    } else if has(&["m3fm", "nautilus", "thunar", "dolphin", "nemo", "yazi"]) {
        ICON_FOLDER
    } else if has(&["hyprland", "quickshell", "qs", "waybar", "xwayland", "sddm", "gdm"]) {
        ICON_DESKTOP_WINDOWS
    } else if row.uid == Some(0) || n.starts_with("systemd") || n.ends_with('d') && n.len() <= 16 {
        ICON_SETTINGS
    } else {
        ICON_APPS
    }
}

enum Item<'a> {
    Group(&'a AppGroup, bool),
    Proc { row: &'a ProcRow, depth: usize, has_children: bool, collapsed: bool },
}

fn my_uid() -> u32 {
    // SAFETY: getuid() cannot fail.
    unsafe { libc::getuid() }
}

fn visible_rows<'a>(app: &App, rows: &'a [ProcRow]) -> Vec<&'a ProcRow> {
    let me = my_uid();
    rows.iter()
        .filter(|r| app.config.show_kernel_threads || !r.kernel_thread)
        .filter(|r| !app.config.only_mine || r.uid == Some(me))
        .collect()
}

fn group_cmp(a: &AppGroup, b: &AppGroup, key: SortKey) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let f = |x: f64, y: f64| x.partial_cmp(&y).unwrap_or(Ordering::Equal);
    match key {
        SortKey::Name | SortKey::State => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortKey::Pid => a.root.cmp(&b.root),
        SortKey::User => a.user.cmp(&b.user),
        SortKey::Cpu => f(a.cpu as f64, b.cpu as f64),
        SortKey::Memory => a.memory.cmp(&b.memory),
        SortKey::Io => f(a.read_rate + a.write_rate, b.read_rate + b.write_rate),
    }
    .then(a.root.cmp(&b.root))
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = widgets::palette(ui);
    let Some(all) = app.snap.as_ref().and_then(|s| s.processes.clone()) else {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(28, 22)).show(ui, |ui| {
            header(ui, "");
            common::loading(ui, "Reading processes…");
        });
        return;
    };
    let ctx = ui.ctx().clone();
    let rows: Vec<ProcRow> = visible_rows(app, &all).into_iter().cloned().collect();
    let key = app.config.sort;
    let desc = app.config.descending;
    let ord = |o: std::cmp::Ordering| if desc { o.reverse() } else { o };
    let query = app.proc_query.trim().to_string();

    let groups = procs::group(&rows);
    let by_pid: HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut items: Vec<Item> = Vec::new();
    match app.config.process_view {
        ProcessView::Apps => {
            let mut gs: Vec<&AppGroup> = groups
                .iter()
                .filter(|g| query.is_empty() || g.pids.iter().any(|pid| by_pid.get(pid).is_some_and(|r| procs::matches(r, &query))))
                .collect();
            gs.sort_by(|a, b| ord(group_cmp(a, b, key)));
            for g in gs {
                let expanded = app.expanded.contains(&g.root);
                items.push(Item::Group(g, expanded));
                if expanded {
                    let mut members: Vec<&ProcRow> = g.pids.iter().filter_map(|pid| by_pid.get(pid).copied()).collect();
                    members.sort_by(|a, b| ord(procs::compare(a, b, key)));
                    items.extend(members.into_iter().map(|row| Item::Proc { row, depth: 1, has_children: false, collapsed: false }));
                }
            }
        }
        ProcessView::List => {
            let mut list: Vec<&ProcRow> = rows.iter().filter(|r| procs::matches(r, &query)).collect();
            list.sort_by(|a, b| ord(procs::compare(a, b, key)));
            items.extend(list.into_iter().map(|row| Item::Proc { row, depth: 0, has_children: false, collapsed: false }));
        }
        ProcessView::Tree => {
            if query.is_empty() {
                for (i, depth, has_children) in procs::tree(&rows, key, desc, &app.collapsed) {
                    let row = &rows[i];
                    items.push(Item::Proc { row, depth, has_children, collapsed: app.collapsed.contains(&row.pid) });
                }
            } else {
                // While searching, show matches flat: the tree would hide them.
                let mut list: Vec<&ProcRow> = rows.iter().filter(|r| procs::matches(r, &query)).collect();
                list.sort_by(|a, b| ord(procs::compare(a, b, key)));
                items.extend(list.into_iter().map(|row| Item::Proc { row, depth: 0, has_children: false, collapsed: false }));
            }
        }
    }

    // Drop a selection whose process is gone.
    if let Some((pid, _)) = app.selected {
        if !by_pid.contains_key(&pid) {
            app.selected = None;
        }
    }

    let total_cpu: f32 = rows.iter().map(|r| r.cpu).sum();
    let summary = format!(
        "{} processes · {} apps · CPU {:.0}% · {} in use",
        rows.len(),
        groups.len(),
        total_cpu,
        ext::bytes(app.snap.as_ref().map(|s| s.mem.used()).unwrap_or(0))
    );

    let mut toggle_expand: Option<u32> = None;
    let mut toggle_collapse: Option<u32> = None;
    let mut new_sort: Option<SortKey> = None;
    let mut select: Option<(u32, bool)> = None;
    let mut open_details: Option<u32> = None;
    let mut menu_action: Option<(Vec<u32>, String, MenuAction)> = None;

    egui::Frame::new().inner_margin(egui::Margin { left: 28, right: 28, top: 22, bottom: 12 }).show(ui, |ui| {
        header(ui, &summary);
        ui.add_space(12.0);
        // Toolbar.
        ui.horizontal(|ui| {
            common::search_field(ui, &mut app.proc_query, "Search name, command, user or PID", 300.0);
            ui.add_space(8.0);
            let mut view = app.config.process_view;
            if widgets::segmented(
                ui,
                &mut view,
                &[(ProcessView::Apps, ICON_APPS, "Apps"), (ProcessView::List, ICON_VIEW_LIST, "All"), (ProcessView::Tree, ICON_ACCOUNT_TREE, "Tree")],
            ) {
                app.config.process_view = view;
                app.save_config();
            }
            ui.add_space(8.0);
            if widgets::filter_chip(ui, "Only mine", app.config.only_mine, Some(ICON_PERSON)).clicked() {
                app.config.only_mine = !app.config.only_mine;
                app.save_config();
            }
            if widgets::filter_chip(ui, "Kernel threads", app.config.show_kernel_threads, None).clicked() {
                app.config.show_kernel_threads = !app.config.show_kernel_threads;
                app.save_config();
            }
        });
        ui.add_space(10.0);

        let has_selection = app.selected.is_some();
        let bar_h = if has_selection { 56.0 } else { 0.0 };
        let table_h = (ui.available_height() - bar_h).max(120.0);
        ui.allocate_ui(vec2(ui.available_width(), table_h), |ui| {
            // Header row.
            let width = ui.available_width();
            let (hrect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
            let name_w = (width - COLUMNS.iter().map(|c| c.2).sum::<f32>()).max(160.0);
            let mut x = hrect.left();
            let mut header_cell = |ui: &mut Ui, k: SortKey, label: &str, w: f32, right: bool, x: f32| {
                let r = Rect::from_min_size(pos2(x, hrect.top()), vec2(w, hrect.height()));
                let resp = ui.interact(r, egui::Id::unique(("proc-hdr", label)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                let active = k == key;
                let color = if active { p.primary } else { p.on_surface_variant };
                let (pos, align) = if right { (pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER) } else { (pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER) };
                let text_r = ui.painter().text(pos, align, label, widgets::title_font(12.5), color);
                if active {
                    let icon = if desc { ICON_ARROW_DOWNWARD } else { ICON_ARROW_UPWARD };
                    let ix = if right { text_r.left() - 9.0 } else { text_r.right() + 9.0 };
                    widgets::paint_icon(ui, icon, pos2(ix, r.center().y), 14.0, color);
                }
                if resp.clicked() {
                    new_sort = Some(k);
                }
            };
            header_cell(ui, SortKey::Name, "Name", name_w, false, x);
            x += name_w;
            for (k, label, w, right) in COLUMNS {
                header_cell(ui, k, label, w, right, x);
                x += w;
            }
            ui.painter().line_segment(
                [pos2(hrect.left(), hrect.bottom()), pos2(hrect.right(), hrect.bottom())],
                egui::Stroke::new(1.0, p.outline_variant),
            );

            if items.is_empty() {
                common::empty_hint(ui, if query.is_empty() { "No processes" } else { "Nothing matches your search" });
                return;
            }
            egui::ScrollArea::vertical().id_salt("proc-table").auto_shrink([false, false]).show_rows(ui, ROW_H, items.len(), |ui, range| {
                for item in &items[range] {
                    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
                    let (pid, is_group) = match item {
                        Item::Group(g, _) => (g.root, true),
                        Item::Proc { row, .. } => (row.pid, false),
                    };
                    let selected = app.selected == Some((pid, is_group));
                    let fill = if selected {
                        p.secondary_container
                    } else if resp.hovered() {
                        p.on_surface.gamma_multiply(0.06)
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    if fill != egui::Color32::TRANSPARENT {
                        ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
                    }
                    let fg = if selected { p.on_secondary_container } else { p.on_surface };
                    let sub = if selected { p.on_secondary_container } else { p.on_surface_variant };

                    // Name cell.
                    let (depth, chevron, icon, name, count) = match item {
                        Item::Group(g, expanded) => {
                            let root = by_pid.get(&g.root).copied();
                            let chev = (g.pids.len() > 1).then_some(if *expanded { ICON_EXPAND_MORE } else { ICON_CHEVRON_RIGHT });
                            (0, chev, root.map(icon_for).unwrap_or(ICON_APPS), g.name.clone(), (g.pids.len() > 1).then_some(g.pids.len()))
                        }
                        Item::Proc { row, depth, has_children, collapsed } => {
                            let chev = has_children.then_some(if *collapsed { ICON_CHEVRON_RIGHT } else { ICON_EXPAND_MORE });
                            (*depth, chev, icon_for(row), row.name.clone(), None)
                        }
                    };
                    let mut nx = rect.left() + 8.0 + depth as f32 * 18.0;
                    if let Some(ch) = chevron {
                        let cr = Rect::from_center_size(pos2(nx + 10.0, rect.center().y), vec2(22.0, 22.0));
                        let cresp = ui.interact(cr, egui::Id::unique(("proc-chev", pid, is_group)), Sense::click());
                        if cresp.hovered() {
                            ui.painter().circle_filled(cr.center(), 11.0, p.on_surface.gamma_multiply(0.08));
                        }
                        widgets::paint_icon(ui, ch, cr.center(), 18.0, sub);
                        if cresp.clicked() {
                            if is_group {
                                toggle_expand = Some(pid);
                            } else {
                                toggle_collapse = Some(pid);
                            }
                        }
                    }
                    nx += 24.0;
                    let ic = pos2(nx + 12.0, rect.center().y);
                    ui.painter().circle_filled(ic, 13.0, if selected { p.primary_container } else { p.surface_container_highest });
                    widgets::paint_icon(ui, icon, ic, 16.0, if selected { p.on_primary_container } else { p.on_surface_variant });
                    nx += 32.0;
                    let name_w = (width - COLUMNS.iter().map(|c| c.2).sum::<f32>()).max(160.0);
                    let mut label = name;
                    if let Some(n) = count {
                        label = format!("{label} ({n})");
                    }
                    let g = widgets::truncated_galley(ui, &label, FontId::proportional(14.0), fg, rect.left() + name_w - nx - 8.0);
                    ui.painter().galley(pos2(nx, rect.center().y - g.size().y / 2.0), g, fg);

                    // Other cells.
                    let (pid_s, user, cpu, mem, io, state) = match item {
                        Item::Group(g, _) => (
                            g.root.to_string(),
                            g.user.clone(),
                            g.cpu,
                            g.memory,
                            g.read_rate + g.write_rate,
                            if g.pids.len() > 1 { format!("{} processes", g.pids.len()) } else { by_pid.get(&g.root).map(|r| r.state.label().to_string()).unwrap_or_default() },
                        ),
                        Item::Proc { row, .. } => (
                            row.pid.to_string(),
                            row.user.clone(),
                            row.cpu,
                            row.memory,
                            row.read_rate + row.write_rate,
                            row.state.label().to_string(),
                        ),
                    };
                    let cells = [
                        pid_s,
                        user,
                        format!("{cpu:.1}%"),
                        ext::bytes(mem),
                        if io > 0.0 { ext::rate(io) } else { "—".into() },
                        state,
                    ];
                    let mut x = rect.left() + name_w;
                    for (i, (_, _, w, right)) in COLUMNS.iter().enumerate() {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*w, rect.height()));
                        // CPU heat: emphasise busy processes.
                        let color = if i == 2 && cpu >= 25.0 && !selected { p.primary } else if i == 0 || i == 5 { sub } else { fg };
                        let g = widgets::truncated_galley(ui, &cells[i], FontId::proportional(13.0), color, w - 16.0);
                        let gx = if *right { r.right() - 10.0 - g.size().x } else { r.left() + 10.0 };
                        ui.painter().galley(pos2(gx, r.center().y - g.size().y / 2.0), g, color);
                        x += w;
                    }

                    if resp.clicked() {
                        select = Some((pid, is_group));
                    }
                    if resp.double_clicked() {
                        open_details = Some(pid);
                    }
                    let group_pids: Vec<u32> = match item {
                        Item::Group(g, _) => g.pids.clone(),
                        Item::Proc { row, .. } => vec![row.pid],
                    };
                    let label = match item {
                        Item::Group(g, _) => g.name.clone(),
                        Item::Proc { row, .. } => row.name.clone(),
                    };
                    let stopped = matches!(item, Item::Proc { row, .. } if row.state == procs::State::Stopped)
                        || matches!(item, Item::Group(g, _) if by_pid.get(&g.root).is_some_and(|r| r.state == procs::State::Stopped));
                    resp.context_menu(|ui| {
                        select = Some((pid, is_group));
                        if let Some(a) = context_menu(ui, is_group, stopped) {
                            menu_action = Some((group_pids.clone(), label.clone(), a));
                            ui.close();
                        }
                    });
                }
            });
        });

        if let Some((pid, is_group)) = app.selected {
            let (pids, name, stopped) = if is_group {
                match groups.iter().find(|g| g.root == pid) {
                    Some(g) => (g.pids.clone(), g.name.clone(), by_pid.get(&pid).is_some_and(|r| r.state == procs::State::Stopped)),
                    None => (vec![pid], String::new(), false),
                }
            } else {
                let row = by_pid.get(&pid);
                (vec![pid], row.map(|r| r.name.clone()).unwrap_or_default(), row.is_some_and(|r| r.state == procs::State::Stopped))
            };
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if pids.len() > 1 { format!("{name} · {} processes", pids.len()) } else { format!("{name} · PID {pid}") };
                ui.label(widgets::title_text(label, 14.0).color(p.on_surface));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, ButtonKind::Danger, Some(ICON_CLOSE), if is_group { "End app" } else { "End" }).clicked() {
                        menu_action = Some((pids.clone(), name.clone(), MenuAction::End));
                    }
                    let (icon, text, action) = if stopped { (ICON_PLAY_ARROW, "Continue", MenuAction::Continue) } else { (ICON_PAUSE, "Stop", MenuAction::Stop) };
                    if widgets::button(ui, ButtonKind::Tonal, Some(icon), text).clicked() {
                        menu_action = Some((pids.clone(), name.clone(), action));
                    }
                    if widgets::icon_button(ui, ICON_LOW_PRIORITY, "Change priority", true).clicked() {
                        menu_action = Some((pids.clone(), name.clone(), MenuAction::Renice));
                    }
                    if widgets::icon_button(ui, ICON_FOLDER_OPEN, "Open file location", true).clicked() {
                        menu_action = Some((pids.clone(), name.clone(), MenuAction::OpenLocation));
                    }
                    if widgets::icon_button(ui, ICON_INFO, "Details", true).clicked() {
                        open_details = Some(pid);
                    }
                });
            });
        }
    });

    if let Some(k) = new_sort {
        if app.config.sort == k {
            app.config.descending = !app.config.descending;
        } else {
            app.config.sort = k;
            app.config.descending = matches!(k, SortKey::Cpu | SortKey::Memory | SortKey::Io);
        }
        app.save_config();
    }
    if let Some(pid) = toggle_expand {
        if !app.expanded.remove(&pid) {
            app.expanded.insert(pid);
        }
    }
    if let Some(pid) = toggle_collapse {
        if !app.collapsed.remove(&pid) {
            app.collapsed.insert(pid);
        }
    }
    if let Some(s) = select {
        app.selected = Some(s);
    }
    if let Some(pid) = open_details {
        app.details = Some(pid);
    }
    if let Some((pids, name, action)) = menu_action {
        perform(app, &ctx, &all, pids, name, action);
    }
}

fn header(ui: &mut Ui, summary: &str) {
    let p = widgets::palette(ui);
    ui.horizontal(|ui| {
        widgets::icon(ui, ICON_APPS, 30.0, p.primary);
        ui.label(RichText::new("Processes").size(28.0).color(p.on_surface));
    });
    if !summary.is_empty() {
        ui.label(RichText::new(summary).color(p.on_surface_variant));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    End,
    Kill,
    Stop,
    Continue,
    Renice,
    OpenLocation,
    Details,
}

fn context_menu(ui: &mut Ui, group: bool, stopped: bool) -> Option<MenuAction> {
    let mut out = None;
    let mut item = |ui: &mut Ui, text: &str, a: MenuAction| {
        if ui.button(text).clicked() {
            out = Some(a);
        }
    };
    item(ui, "Details", MenuAction::Details);
    item(ui, "Open file location", MenuAction::OpenLocation);
    item(ui, "Change priority…", MenuAction::Renice);
    ui.separator();
    if stopped {
        item(ui, "Continue", MenuAction::Continue);
    } else {
        item(ui, "Stop", MenuAction::Stop);
    }
    item(ui, if group { "End app" } else { "End" }, MenuAction::End);
    item(ui, if group { "Kill app" } else { "Kill" }, MenuAction::Kill);
    out
}

fn perform(app: &mut App, ctx: &egui::Context, all: &Arc<Vec<ProcRow>>, pids: Vec<u32>, name: String, action: MenuAction) {
    match action {
        MenuAction::End | MenuAction::Kill => {
            let signal = if action == MenuAction::Kill { Signal::Kill } else { Signal::Term };
            if app.config.confirm_end {
                app.confirm = Some(ConfirmEnd { pids, name, signal });
            } else {
                send(app, ctx, pids, name, signal);
            }
        }
        MenuAction::Stop => send(app, ctx, pids, name, Signal::Stop),
        MenuAction::Continue => send(app, ctx, pids, name, Signal::Cont),
        MenuAction::Renice => {
            let pid = pids[0];
            let nice = procs::stat_extra(pid).map(|s| s.nice).unwrap_or(0);
            app.renice = Some((pid, nice, name));
        }
        MenuAction::Details => app.details = Some(pids[0]),
        MenuAction::OpenLocation => {
            let exe = all.iter().find(|r| r.pid == pids[0]).and_then(|r| r.exe.clone());
            match exe.as_deref().and_then(|e| e.parent()) {
                Some(dir) => {
                    let dir = dir.to_string_lossy().into_owned();
                    let opener = if material_3_ui::sys::cmd::exists("m3fm") { "m3fm" } else { "xdg-open" };
                    if let Err(e) = material_3_ui::sys::cmd::spawn_detached(opener, &[&dir]) {
                        app.toasts.error(e);
                    }
                }
                None => app.toasts.error(format!("Can't see where {name} was started from (it may belong to another user).")),
            }
        }
    }
}

fn send(app: &mut App, ctx: &egui::Context, pids: Vec<u32>, name: String, signal: Signal) {
    let verb = match signal {
        Signal::Term => "Ended",
        Signal::Kill => "Killed",
        Signal::Stop => "Stopped",
        Signal::Cont => "Continued",
    };
    if matches!(signal, Signal::Term | Signal::Kill) {
        app.selected = None;
    }
    app.run_job(ctx, Refresh::Processes, move || procs::send_signal(&pids, signal).map(|_| format!("{verb} {name}")));
}

/// Confirm, details and priority dialogs.
pub fn dialogs(app: &mut App, ctx: &egui::Context) {
    let p = app.theme.palette().clone();
    if let Some(c) = app.confirm.clone() {
        let kill = c.signal == Signal::Kill;
        let title = if kill { format!("Kill {}?", c.name) } else { format!("End {}?", c.name) };
        let mut close = false;
        let mut go = false;
        let resp = common::modal(ctx, &p, "confirm-end", if kill { ICON_DANGEROUS } else { ICON_CLOSE }, &title, |ui| {
            ui.set_max_width(420.0);
            let text = match (kill, c.pids.len()) {
                (false, 1) => "It's asked to close. Unsaved work may be lost.".to_string(),
                (false, n) => format!("All {n} of its processes are asked to close. Unsaved work may be lost."),
                (true, 1) => "It's stopped at once, without a chance to save. Use this when End doesn't work.".to_string(),
                (true, n) => format!("All {n} of its processes are stopped at once, without a chance to save."),
            };
            ui.add(egui::Label::new(RichText::new(text).color(p.on_surface_variant)).wrap());
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Danger, None, if kill { "Kill" } else { "End" }).clicked() {
                    go = true;
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() {
            close = true;
        }
        if go {
            send(app, ctx, c.pids, c.name, c.signal);
            close = true;
        }
        if close {
            app.confirm = None;
        }
    }

    if let Some(pid) = app.details {
        let row = app.snap.as_ref().and_then(|s| s.processes.as_ref()).and_then(|ps| ps.iter().find(|r| r.pid == pid).cloned());
        let mut close = false;
        let title = row.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| format!("Process {pid}"));
        let resp = common::modal(ctx, &p, "details", ICON_INFO, &title, |ui| {
            ui.set_width(520.0);
            match &row {
                None => common::note(ui, "This process has ended."),
                Some(r) => {
                    let extra = procs::stat_extra(pid).unwrap_or_default();
                    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map(|c| c.display().to_string()).unwrap_or_else(|_| "—".into());
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    let started = if r.start_time > 0 { format!("{} ago", ext::duration(now.saturating_sub(r.start_time) as f64)) } else { "—".into() };
                    common::facts(
                        ui,
                        "proc-facts",
                        &[
                            ("PID", r.pid.to_string()),
                            ("Parent", r.parent.map(|p| p.to_string()).unwrap_or_else(|| "—".into())),
                            ("User", r.user.clone()),
                            ("State", r.state.label().into()),
                            ("Started", started),
                            ("CPU", format!("{:.1}%", r.cpu)),
                            ("Memory", ext::bytes(r.memory)),
                            ("Disk", format!("read {} · write {}", ext::rate(r.read_rate), ext::rate(r.write_rate))),
                            ("Priority (nice)", extra.nice.to_string()),
                            ("Threads", extra.threads.to_string()),
                            ("Executable", r.exe.as_ref().map(|e| e.display().to_string()).unwrap_or_else(|| "—".into())),
                            ("Working folder", cwd),
                        ],
                    );
                    ui.label(widgets::title_text("Command line", 13.0).color(p.primary));
                    let mut cmd = if r.cmd.is_empty() { format!("[{}]", r.name) } else { r.cmd.clone() };
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut cmd).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).interactive(true));
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
            app.details = None;
        }
    }

    if let Some((pid, mut nice, name)) = app.renice.clone() {
        let mut close = false;
        let mut apply = false;
        let resp = common::modal(ctx, &p, "renice", ICON_LOW_PRIORITY, &format!("Priority of {name}"), |ui| {
            ui.set_width(420.0);
            ui.add(
                egui::Label::new(
                    RichText::new("Lower values get more CPU time. Raising priority (below 0) asks for your password.").color(p.on_surface_variant),
                )
                .wrap(),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                for (label, v) in [("Very high", -15), ("High", -5), ("Normal", 0), ("Low", 5), ("Very low", 15)] {
                    if widgets::filter_chip(ui, label, nice == v, None).clicked() {
                        nice = v;
                    }
                }
            });
            ui.add_space(8.0);
            ui.add(egui::Slider::new(&mut nice, -20..=19).text("nice"));
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Filled, None, "Apply").clicked() {
                    apply = true;
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if apply {
            let n = name.clone();
            app.run_job(ctx, Refresh::Processes, move || procs::renice(pid, nice).map(|_| format!("{n} now runs at priority {nice}")));
            close = true;
        }
        if close || resp.should_close() {
            app.renice = None;
        } else {
            app.renice = Some((pid, nice, name));
        }
    }
}
