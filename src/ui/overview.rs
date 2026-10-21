//! The Overview: live charts for CPU, memory, GPUs, disks and network,
//! plus sensors, battery, system and gaming tiles.

use crate::app::App;
use crate::sample::io::NetKind;
use crate::sample::Snapshot;
use crate::ui_ext::{self as ext, area_chart, legend, meter, stat_line, tile, Series};
use egui::{vec2, RichText, Ui};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::widgets;

const CHART_H: f32 = 96.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    let Some(snap) = app.snap.clone() else {
        ext::wide_page(ui, ICON_MONITORING, "Overview", "", 1240.0, |ui| common::loading(ui, "Reading the system…"));
        return;
    };
    let palette = widgets::palette(ui);
    let p = &*palette;
    let info = app.sampler.info.clone();
    let subtitle = format!("{} · {} · up {}", info.hostname, info.os, ext::duration(snap.uptime));
    let history = &app.history;
    let len = history.len();
    let gaming = app.gaming.value.clone();
    let show_virtual = app.config.show_virtual_interfaces;

    ext::wide_page(ui, ICON_MONITORING, "Overview", &subtitle, 1240.0, |ui| {
        // CPU spans the full width.
        cpu_tile(ui, &snap, &info, history, len);

        let mut tiles: Vec<ext::TileFn> = Vec::new();
        let s = &snap;
        tiles.push(Box::new(move |ui: &mut Ui| memory_tile(ui, s, history, len)));
        for gpu in &snap.gpus {
            tiles.push(Box::new(move |ui: &mut Ui| {
                let busy = gpu.busy.map(|b| format!("{b:.0}%")).unwrap_or_else(|| "—".into());
                tile(ui, ICON_VIDEO_SETTINGS, &gpu.name, &busy, |ui| {
                    if gpu.busy.is_some() {
                        let v = history.values(&format!("gpu:{}", gpu.key));
                        area_chart(ui, vec2(ui.available_width(), CHART_H), &[Series { values: &v, color: p.secondary }], 100.0, len);
                        ui.add_space(6.0);
                    }
                    if let (Some(used), Some(total)) = (gpu.vram_used, gpu.vram_total) {
                        stat_line(ui, "Video memory", &format!("{} of {}", ext::bytes(used), ext::bytes(total)));
                        meter(ui, used as f32 / total as f32, ui.available_width());
                        ui.add_space(4.0);
                    }
                    if let Some(c) = gpu.clock_mhz {
                        let max = gpu.max_clock_mhz.map(|m| format!(" (max {})", ext::mhz(m))).unwrap_or_default();
                        stat_line(ui, "Clock", &format!("{}{max}", ext::mhz(c)));
                    }
                    if let Some(t) = gpu.temp {
                        stat_line(ui, "Temperature", &format!("{t:.0} °C"));
                    }
                    if let Some(w) = gpu.power {
                        stat_line(ui, "Power", &format!("{w:.1} W"));
                    }
                    if let Some(f) = gpu.fan_rpm {
                        stat_line(ui, "Fan", &format!("{f} RPM"));
                    } else if let Some(f) = gpu.fan_percent {
                        stat_line(ui, "Fan", &format!("{f:.0}%"));
                    }
                    if gpu.busy.is_none() {
                        common::note(ui, "Usage isn't exposed by this driver without root; frequency is shown instead.");
                    }
                });
            }));
        }
        // Disks that hold a mounted filesystem, or that did any I/O lately
        // (idle, unmounted disks would only add empty charts).
        let disks = snap.disks.iter().filter(|d| {
            snap.mounts.iter().any(|m| m.device.trim_start_matches("/dev/").starts_with(&d.name))
                || history.max(&format!("disk:{}:r", d.name)) + history.max(&format!("disk:{}:w", d.name)) > 0.0
        });
        for disk in disks {
            tiles.push(Box::new(move |ui: &mut Ui| {
                let title = disk.model.clone().map(|m| format!("{m} ({})", disk.name)).unwrap_or_else(|| disk.name.clone());
                tile(ui, ICON_HARD_DRIVE, &title, &ext::percent(disk.busy), |ui| {
                    let r = history.values(&format!("disk:{}:r", disk.name));
                    let w = history.values(&format!("disk:{}:w", disk.name));
                    let max = ext::nice_max(history.max(&format!("disk:{}:r", disk.name)).max(history.max(&format!("disk:{}:w", disk.name))).max(1e6));
                    area_chart(
                        ui,
                        vec2(ui.available_width(), CHART_H),
                        &[Series { values: &r, color: p.primary }, Series { values: &w, color: p.tertiary }],
                        max,
                        len,
                    );
                    ui.horizontal(|ui| {
                        legend(ui, p.primary, &format!("Read {}", ext::rate(disk.read_rate)));
                        ui.add_space(8.0);
                        legend(ui, p.tertiary, &format!("Write {}", ext::rate(disk.write_rate)));
                    });
                });
            }));
        }
        let nets: Vec<_> = snap
            .nets
            .iter()
            .filter(|n| show_virtual || n.kind != NetKind::Virtual)
            .filter(|n| n.rx_total + n.tx_total > 0)
            .collect();
        for net in nets {
            tiles.push(Box::new(move |ui: &mut Ui| {
                let icon = match net.kind {
                    NetKind::Wireless => ICON_WIFI,
                    NetKind::Wired => ICON_LAN,
                    NetKind::Virtual => ICON_DEVICE_HUB,
                };
                tile(ui, icon, &net.name, &ext::rate(net.rx_rate + net.tx_rate), |ui| {
                    let rx = history.values(&format!("net:{}:rx", net.name));
                    let tx = history.values(&format!("net:{}:tx", net.name));
                    let max = ext::nice_max(history.max(&format!("net:{}:rx", net.name)).max(history.max(&format!("net:{}:tx", net.name))).max(1e5));
                    area_chart(
                        ui,
                        vec2(ui.available_width(), CHART_H),
                        &[Series { values: &rx, color: p.primary }, Series { values: &tx, color: p.tertiary }],
                        max,
                        len,
                    );
                    ui.horizontal(|ui| {
                        legend(ui, p.primary, &format!("Down {}", ext::rate(net.rx_rate)));
                        ui.add_space(8.0);
                        legend(ui, p.tertiary, &format!("Up {}", ext::rate(net.tx_rate)));
                    });
                    stat_line(ui, "Since boot", &format!("↓ {}  ↑ {}", ext::bytes(net.rx_total), ext::bytes(net.tx_total)));
                });
            }));
        }
        if !snap.mounts.is_empty() {
            tiles.push(Box::new(move |ui: &mut Ui| {
                let used: u64 = s.mounts.iter().map(|m| m.total - m.available).sum();
                let total: u64 = s.mounts.iter().map(|m| m.total).sum();
                tile(ui, ICON_STORAGE, "Storage", &ext::percent(used as f32 / total.max(1) as f32), |ui| {
                    for m in &s.mounts {
                        let used = m.total.saturating_sub(m.available);
                        stat_line(ui, &format!("{} ({})", m.mount_point, m.fs), &format!("{} free of {}", ext::bytes(m.available), ext::bytes(m.total)));
                        meter(ui, used as f32 / m.total.max(1) as f32, ui.available_width());
                        ui.add_space(6.0);
                    }
                });
            }));
        }
        if !snap.temps.is_empty() || !snap.fans.is_empty() {
            tiles.push(Box::new(move |ui: &mut Ui| sensors_tile(ui, s)));
        }
        for bat in &snap.batteries {
            let ac = snap.ac;
            tiles.push(Box::new(move |ui: &mut Ui| {
                let icon = if bat.status == "Charging" { ICON_BATTERY_CHARGING_FULL } else { ICON_BATTERY_5_BAR };
                let title = if bat.model.is_empty() { "Battery".to_string() } else { format!("Battery · {}", bat.model) };
                tile(ui, icon, &title, &format!("{:.0}%", bat.percent), |ui| {
                    meter(ui, bat.percent / 100.0, ui.available_width());
                    ui.add_space(6.0);
                    let mut status = bat.status.clone();
                    if ac == Some(true) && status == "Discharging" {
                        status = "On AC".into();
                    }
                    stat_line(ui, "Status", &status);
                    if let Some(h) = bat.hours_left {
                        let what = if bat.status == "Charging" { "Until full" } else { "Remaining" };
                        stat_line(ui, what, &ext::duration(h as f64 * 3600.0));
                    }
                    if let Some(w) = bat.power {
                        stat_line(ui, "Power", &format!("{w:.1} W"));
                    }
                    if let Some(h) = bat.health {
                        stat_line(ui, "Health", &ext::percent(h));
                    }
                    if let Some(c) = bat.cycles {
                        stat_line(ui, "Cycles", &c.to_string());
                    }
                });
            }));
        }
        tiles.push(Box::new(move |ui: &mut Ui| {
            tile(ui, ICON_SPORTS_ESPORTS, "Gaming", "", |ui| gaming_lines(ui, s, gaming.as_ref()));
        }));
        tiles.push(Box::new(move |ui: &mut Ui| {
            tile(ui, ICON_COMPUTER, "System", "", |ui| {
                stat_line(ui, "Kernel", &info.kernel);
                stat_line(ui, "Uptime", &ext::duration(s.uptime));
                stat_line(ui, "Load average", &format!("{:.2}  {:.2}  {:.2}", s.load.one, s.load.five, s.load.fifteen));
                stat_line(ui, "Tasks", &s.load.tasks.to_string());
                stat_line(ui, "Context switches", &format!("{:.0}/s", s.context_switch_rate));
                let pressure = |v: Option<f32>| v.map(|v| format!("{v:.1}%")).unwrap_or_else(|| "—".into());
                stat_line(
                    ui,
                    "Pressure (cpu / mem / io)",
                    &format!("{} / {} / {}", pressure(s.pressure.cpu), pressure(s.pressure.memory), pressure(s.pressure.io)),
                );
            });
        }));
        ext::tile_grid(ui, 380.0, tiles);
    });
}

fn cpu_tile(ui: &mut Ui, snap: &Snapshot, info: &crate::sample::StaticInfo, history: &crate::sample::history::History, len: usize) {
    let p = widgets::palette(ui);
    let title = format!("{} · {} threads", info.cpu_model, info.cores);
    tile(ui, ICON_MEMORY, &title, &ext::percent(snap.cpu), |ui| {
        let v = history.values("cpu");
        area_chart(ui, vec2(ui.available_width(), 130.0), &[Series { values: &v, color: p.primary }], 100.0, len);
        ui.add_space(8.0);
        ext::bars(ui, &snap.cores, 40.0).on_hover_text(
            snap.cores
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let f = snap.freqs.get(i).filter(|f| **f > 0.0).map(|f| format!(" · {}", ext::mhz(*f))).unwrap_or_default();
                    format!("CPU {i}: {}{f}", ext::percent(*c))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let mut chip = |label: &str, value: String| {
                ui.label(RichText::new(label).size(12.5).color(p.on_surface_variant));
                ui.label(RichText::new(value).size(12.5).color(p.on_surface));
                ui.add_space(14.0);
            };
            let active: Vec<f32> = snap.freqs.iter().copied().filter(|f| *f > 0.0).collect();
            if !active.is_empty() {
                let avg = active.iter().sum::<f32>() / active.len() as f32;
                let top = active.iter().copied().fold(0.0, f32::max);
                chip("Frequency", format!("{} avg · {} top", ext::mhz(avg), ext::mhz(top)));
            }
            if let Some(max) = info.max_freq {
                chip("Max", ext::mhz(max));
            }
            if let Some(t) = snap.cpu_temp {
                chip("Temperature", format!("{t:.0} °C"));
            }
            chip("Load", format!("{:.2}", snap.load.one));
            if let Some(g) = &snap.governor {
                chip("Governor", g.clone());
            }
        });
    });
}

fn memory_tile(ui: &mut Ui, snap: &Snapshot, history: &crate::sample::history::History, len: usize) {
    let p = widgets::palette(ui);
    let m = &snap.mem;
    let frac = if m.total > 0 { m.used() as f32 / m.total as f32 } else { 0.0 };
    tile(ui, ICON_DEVELOPER_BOARD, "Memory", &ext::percent(frac), |ui| {
        let v = history.values("mem");
        let swap = history.values("swap");
        let mut series = vec![Series { values: &v, color: p.tertiary }];
        if m.swap_total > 0 {
            series.push(Series { values: &swap, color: p.secondary });
        }
        area_chart(ui, vec2(ui.available_width(), CHART_H), &series, 100.0, len);
        ui.horizontal(|ui| {
            legend(ui, p.tertiary, &format!("Used {} of {}", ext::bytes(m.used()), ext::bytes(m.total)));
            if m.swap_total > 0 {
                ui.add_space(8.0);
                legend(ui, p.secondary, &format!("Swap {} of {}", ext::bytes(m.swap_used()), ext::bytes(m.swap_total)));
            }
        });
        stat_line(ui, "Cached", &ext::bytes(m.cached + m.buffers));
        stat_line(ui, "Shared", &ext::bytes(m.shared));
        for z in &snap.zram {
            stat_line(
                ui,
                &format!("{} ({})", z.name, z.algorithm),
                &format!("{} in {} · {:.1}×", ext::bytes(z.original), ext::bytes(z.memory_used), z.ratio()),
            );
        }
    });
}

fn temp_color(p: &material_3_ui::theme::M3Palette, c: f32, critical: Option<f32>) -> egui::Color32 {
    let hot = critical.map(|cr| cr - 10.0).unwrap_or(85.0);
    if c >= hot {
        p.error
    } else if c >= hot - 15.0 {
        p.tertiary
    } else {
        p.primary
    }
}

fn sensors_tile(ui: &mut Ui, snap: &Snapshot) {
    let p = widgets::palette(ui);
    let hottest = snap.temps.iter().map(|t| t.celsius).fold(f32::MIN, f32::max);
    let value = if hottest > f32::MIN { format!("{hottest:.0} °C") } else { String::new() };
    tile(ui, ICON_THERMOSTAT, "Sensors", &value, |ui| {
        for t in &snap.temps {
            ui.horizontal(|ui| {
                ui.label(RichText::new(t.display_name()).size(12.5).color(p.on_surface_variant));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0} °C", t.celsius)).size(12.5).color(temp_color(&p, t.celsius, t.critical)));
                    let scale = t.critical.unwrap_or(100.0).max(t.celsius);
                    meter(ui, t.celsius / scale, 90.0);
                });
            });
        }
        for f in &snap.fans {
            stat_line(ui, &format!("{} · {}", crate::sample::sensors::chip_name(&f.chip), f.label), &format!("{} RPM", f.rpm));
        }
    });
}

fn gaming_lines(ui: &mut Ui, snap: &Snapshot, gaming: Option<&crate::gaming::Gaming>) {
    let Some(g) = gaming else {
        common::loading(ui, "Checking…");
        return;
    };
    match &g.games {
        Some(s) => {
            stat_line(ui, "Game Mode", if s.game_mode { "On" } else { "Off" });
            let running = s.running_game.as_ref().map(|r| format!("{} (PID {})", r.name, r.pid)).unwrap_or_else(|| "None".into());
            stat_line(ui, "Running game", &running);
        }
        None => stat_line(ui, "Material 3 Games", "Not installed"),
    }
    match &g.scheduler {
        Some(Some((sched, mode))) => {
            let m = mode.as_ref().map(|m| format!(" · {m}")).unwrap_or_default();
            stat_line(ui, "CPU scheduler", &format!("scx_{}{m}", sched.to_lowercase()));
        }
        Some(None) => stat_line(ui, "CPU scheduler", "Kernel default"),
        None => {}
    }
    if let Some(gov) = &snap.governor {
        stat_line(ui, "Governor", gov);
    }
    if let Some(pp) = &g.power_profile {
        stat_line(ui, "Power profile", pp);
    }
    if let Some(active) = g.gamemode_active {
        stat_line(ui, "GameMode", if active { "Active" } else { "Idle" });
    }
}
