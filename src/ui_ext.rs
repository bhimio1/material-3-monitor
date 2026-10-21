//! General pieces that aren't in material-3-ui yet (candidates for the
//! crate): number formatting, live area charts, meters, and a wide page /
//! responsive tile grid.

use egui::{pos2, vec2, Color32, CornerRadius, Frame, Margin, Mesh, Rect, RichText, Sense, Shape, Stroke, Ui};
use material_3_ui::icons::MaterialIcon;
use material_3_ui::widgets;

/// "1.4 GiB", "512 KiB", "0 B".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// "3.2 MB/s" (decimal, like download speeds).
pub fn rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 5] = ["B/s", "kB/s", "MB/s", "GB/s", "TB/s"];
    let mut v = bytes_per_sec.max(0.0);
    let mut unit = 0;
    while v >= 1000.0 && unit + 1 < UNITS.len() {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 || v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// "2 d 4 h", "3 h 12 min", "45 s".
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (d, h, m) = (s / 86400, (s / 3600) % 24, (s / 60) % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}

/// "2.4 GHz" or "800 MHz".
pub fn mhz(v: f32) -> String {
    if v >= 1000.0 {
        format!("{:.2} GHz", v / 1000.0)
    } else {
        format!("{v:.0} MHz")
    }
}

pub fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).clamp(0.0, 100.0))
}

/// A rounded "nice" maximum for an auto-scaled chart (1, 2, 5 × 10ⁿ).
pub fn nice_max(v: f32) -> f32 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let exp = v.log10().floor();
    let base = 10f32.powf(exp);
    let m = v / base;
    let step = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * base
}

/// One line of a chart.
pub struct Series<'a> {
    pub values: &'a [f32],
    pub color: Color32,
}

/// A live area chart: newest value at the right edge, `capacity` samples
/// across, gradient fill under each line, faint grid lines.
pub fn area_chart(ui: &mut Ui, size: egui::Vec2, series: &[Series], max: f32, capacity: usize) -> egui::Response {
    let p = widgets::palette(ui);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(12), p.surface_container_high);
    let plot = rect.shrink2(vec2(2.0, 6.0));
    for i in 1..4 {
        let y = plot.bottom() - plot.height() * i as f32 / 4.0;
        painter.line_segment(
            [pos2(plot.left() + 6.0, y), pos2(plot.right() - 6.0, y)],
            Stroke::new(1.0, p.outline_variant.gamma_multiply(0.35)),
        );
    }
    let max = if max > 0.0 { max } else { 1.0 };
    let capacity = capacity.max(2);
    let step = plot.width() / (capacity - 1) as f32;
    for s in series {
        let n = s.values.len().min(capacity);
        if n < 2 {
            continue;
        }
        let values = &s.values[s.values.len() - n..];
        let points: Vec<egui::Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = plot.right() - (n - 1 - i) as f32 * step;
                let y = plot.bottom() - (v / max).clamp(0.0, 1.0) * plot.height();
                pos2(x, y)
            })
            .collect();
        let mut mesh = Mesh::default();
        let top = s.color.gamma_multiply(0.30);
        let bottom = s.color.gamma_multiply(0.04);
        for w in points.windows(2) {
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(w[0], top);
            mesh.colored_vertex(w[1], top);
            mesh.colored_vertex(pos2(w[1].x, plot.bottom()), bottom);
            mesh.colored_vertex(pos2(w[0].x, plot.bottom()), bottom);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        painter.add(Shape::mesh(mesh));
        painter.add(Shape::line(points, Stroke::new(1.8, s.color)));
    }
    response
}

/// A horizontal usage meter; turns to the error color above 90%.
pub fn meter(ui: &mut Ui, fraction: f32, width: f32) -> egui::Response {
    let p = widgets::palette(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let r = CornerRadius::same(4);
    ui.painter().rect_filled(rect, r, p.secondary_container);
    if f > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(8.0), rect.height()));
        ui.painter().rect_filled(fill, r, if f > 0.9 { p.error } else { p.primary });
    }
    response
}

/// Vertical bars, one per value (0–1), filling the width: per-core load.
pub fn bars(ui: &mut Ui, values: &[f32], height: f32) -> egui::Response {
    let p = widgets::palette(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if values.is_empty() || !ui.is_rect_visible(rect) {
        return response;
    }
    let gap = if values.len() > 32 { 2.0 } else { 4.0 };
    let w = ((rect.width() + gap) / values.len() as f32 - gap).max(1.0);
    for (i, v) in values.iter().enumerate() {
        let x = rect.left() + i as f32 * (w + gap);
        let col = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        let radius = CornerRadius::same((w / 2.0).min(4.0) as u8);
        ui.painter().rect_filled(col, radius, p.secondary_container);
        let h = (v.clamp(0.0, 1.0) * rect.height()).max(2.0);
        let fill = Rect::from_min_max(pos2(col.left(), col.bottom() - h), col.max);
        ui.painter().rect_filled(fill, radius, if *v > 0.9 { p.tertiary } else { p.primary });
    }
    response
}

/// A scrolling page like `common::page`, but wider for dashboards.
pub fn wide_page(ui: &mut Ui, icon: MaterialIcon, title: &str, subtitle: &str, max_width: f32, body: impl FnOnce(&mut Ui)) {
    let p = widgets::palette(ui);
    egui::ScrollArea::vertical()
        .id_salt(("wide-page", title))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::symmetric(28, 22)).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(max_width));
                ui.horizontal(|ui| {
                    widgets::icon(ui, icon, 30.0, p.primary);
                    ui.label(RichText::new(title).size(28.0).color(p.on_surface));
                });
                if !subtitle.is_empty() {
                    ui.add(egui::Label::new(RichText::new(subtitle).color(p.on_surface_variant)).wrap());
                }
                ui.add_space(12.0);
                body(ui);
                ui.add_space(24.0);
            });
        });
}

/// A tile: rounded surface with an icon + title header and a value on the
/// right of the header.
pub fn tile(ui: &mut Ui, icon: MaterialIcon, title: &str, value: &str, body: impl FnOnce(&mut Ui)) {
    let p = widgets::palette(ui);
    Frame::new()
        .fill(p.surface_container)
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::icon(ui, icon, 20.0, p.primary);
                ui.label(widgets::title_text(title, 15.0).color(p.on_surface));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(widgets::title_text(value, 18.0).color(p.on_surface));
                });
            });
            ui.add_space(8.0);
            body(ui);
        });
    ui.add_space(12.0);
}

/// A tile's contents, for [`tile_grid`].
pub type TileFn<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

/// Lay boxed tiles out in as many columns as fit `min_width`, in order.
pub fn tile_grid<'a>(ui: &mut Ui, min_width: f32, tiles: Vec<TileFn<'a>>) {
    let cols = ((ui.available_width() + 12.0) / (min_width + 12.0)).floor().clamp(1.0, 3.0) as usize;
    if cols == 1 {
        for t in tiles {
            t(ui);
        }
        return;
    }
    let mut buckets: Vec<Vec<TileFn<'a>>> = (0..cols).map(|_| Vec::new()).collect();
    for (i, t) in tiles.into_iter().enumerate() {
        buckets[i % cols].push(t);
    }
    ui.columns(cols, |columns| {
        for (col, bucket) in columns.iter_mut().zip(buckets) {
            for t in bucket {
                t(col);
            }
        }
    });
}

/// A small label / value line inside a tile.
pub fn stat_line(ui: &mut Ui, label: &str, value: &str) {
    let p = widgets::palette(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.5).color(p.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.5).color(p.on_surface));
        });
    });
}

/// A colored dot and label, for chart legends.
pub fn legend(ui: &mut Ui, color: Color32, text: &str) {
    let p = widgets::palette(ui);
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
    ui.label(RichText::new(text).size(12.5).color(p.on_surface_variant));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(16 * 1024 * 1024 * 1024), "16.0 GiB");
        assert_eq!(bytes(500 * 1024 * 1024), "500 MiB");
        assert_eq!(rate(999.0), "999 B/s");
        assert_eq!(rate(2_500_000.0), "2.5 MB/s");
        assert_eq!(duration(45.0), "45 s");
        assert_eq!(duration(3.0 * 3600.0 + 720.0), "3 h 12 min");
        assert_eq!(duration(2.0 * 86400.0 + 4.0 * 3600.0), "2 d 4 h");
        assert_eq!(mhz(3800.0), "3.80 GHz");
        assert_eq!(mhz(800.0), "800 MHz");
        assert_eq!(percent(0.256), "26%");
    }

    #[test]
    fn nice_maxima() {
        assert_eq!(nice_max(0.0), 1.0);
        assert_eq!(nice_max(130.0), 200.0);
        assert_eq!(nice_max(3.2e6), 5e6);
        assert_eq!(nice_max(1000.0), 1000.0);
        assert_eq!(nice_max(7.0), 10.0);
    }
}
