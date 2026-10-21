//! Preferences, stored in `~/.config/m3-monitor/config.toml`.

use crate::sample::procs::SortKey;
use crate::services::Scope;
use material_3_ui::theme::ThemeSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessView {
    /// Processes grouped into the apps they belong to.
    #[default]
    Apps,
    /// Every process, flat.
    List,
    /// Parent / child tree.
    Tree,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub appearance: ThemeSettings,
    /// Milliseconds between samples.
    pub interval_ms: u64,
    /// Seconds of history the charts show.
    pub history_seconds: u32,
    /// Stop sampling while the window isn't focused.
    pub pause_when_unfocused: bool,
    pub show_virtual_interfaces: bool,
    pub process_view: ProcessView,
    pub sort: SortKey,
    pub descending: bool,
    pub only_mine: bool,
    pub show_kernel_threads: bool,
    pub confirm_end: bool,
    pub services_scope: Scope,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: ThemeSettings::default(),
            interval_ms: 1000,
            history_seconds: 60,
            pause_when_unfocused: true,
            show_virtual_interfaces: false,
            process_view: ProcessView::Apps,
            sort: SortKey::Cpu,
            descending: true,
            only_mine: false,
            show_kernel_threads: false,
            confirm_end: true,
            services_scope: Scope::User,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        material_3_ui::sys::paths::config_home().join("m3-monitor/config.toml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// How many samples the charts keep.
    pub fn history_len(&self) -> usize {
        ((self.history_seconds as u64 * 1000) / self.interval_ms.max(250)).max(10) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let c: Config = toml::from_str("interval_ms = 2000\n[appearance]\nseed_color = \"#FF0000\"\n").unwrap();
        assert_eq!(c.interval_ms, 2000);
        assert_eq!(c.history_len(), 30);
        assert!(c.pause_when_unfocused);
        assert_eq!(c.appearance.seed_color, "#FF0000");
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
