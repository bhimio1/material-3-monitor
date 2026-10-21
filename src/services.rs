//! systemd services through `systemctl --output=json`, and their logs
//! through `journalctl`.

use material_3_ui::sys::cmd;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    User,
    System,
}

impl Scope {
    fn flag(self) -> &'static str {
        match self {
            Scope::User => "--user",
            Scope::System => "--system",
        }
    }

}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub description: String,
    /// loaded, not-found, masked, …
    pub load: String,
    /// active, inactive, failed, activating, …
    pub active: String,
    /// running, exited, dead, …
    pub sub: String,
    /// enabled, disabled, static, masked, … (from list-unit-files)
    pub file_state: Option<String>,
}

impl Unit {
    pub fn is_running(&self) -> bool {
        self.active == "active" || self.active == "activating" || self.active == "reloading"
    }

    pub fn is_failed(&self) -> bool {
        self.active == "failed"
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self.file_state.as_deref(), Some("enabled" | "enabled-runtime" | "linked"))
    }

    /// Whether enable/disable makes sense (not static, generated or masked).
    pub fn can_toggle(&self) -> bool {
        matches!(self.file_state.as_deref(), Some("enabled" | "disabled" | "enabled-runtime"))
    }
}

#[derive(Deserialize)]
struct ListUnit {
    unit: String,
    #[serde(default)]
    load: String,
    #[serde(default)]
    active: String,
    #[serde(default)]
    sub: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct UnitFile {
    unit_file: String,
    #[serde(default)]
    state: String,
}

pub fn parse_list_units(json: &str) -> Result<Vec<Unit>, String> {
    let list: Vec<ListUnit> = serde_json::from_str(json).map_err(|e| format!("Couldn't read systemctl's output: {e}"))?;
    Ok(list
        .into_iter()
        .map(|u| Unit {
            name: u.unit,
            description: u.description,
            load: u.load,
            active: u.active,
            sub: u.sub,
            file_state: None,
        })
        .collect())
}

pub fn parse_unit_files(json: &str) -> Result<HashMap<String, String>, String> {
    let list: Vec<UnitFile> = serde_json::from_str(json).map_err(|e| format!("Couldn't read systemctl's output: {e}"))?;
    Ok(list
        .into_iter()
        .map(|f| {
            let name = f.unit_file.rsplit('/').next().unwrap_or(&f.unit_file).to_string();
            (name, f.state)
        })
        .collect())
}

/// Loaded units, plus installed ones that aren't loaded (disabled services
/// that never ran), with their enablement state.
pub fn merge(mut units: Vec<Unit>, files: HashMap<String, String>) -> Vec<Unit> {
    for u in &mut units {
        u.file_state = files.get(&u.name).cloned();
    }
    let known: std::collections::HashSet<String> = units.iter().map(|u| u.name.clone()).collect();
    for (name, state) in files {
        // Templates (foo@.service) can't be started without an instance.
        if known.contains(&name) || name.contains("@.") {
            continue;
        }
        units.push(Unit {
            name,
            description: String::new(),
            load: "not-loaded".into(),
            active: "inactive".into(),
            sub: "dead".into(),
            file_state: Some(state),
        });
    }
    units.sort_by_key(|a| a.name.to_lowercase());
    units
}

/// Development: `M3_MONITOR_SERVICES_FIXTURE=dir` reads `{user,system}-units.json`
/// and `{user,system}-unit-files.json` from `dir` instead of asking systemd
/// (for screenshots on machines without it).
fn fixture(scope: Scope, what: &str) -> Option<Result<String, String>> {
    let dir = std::env::var_os("M3_MONITOR_SERVICES_FIXTURE")?;
    let name = format!("{}-{what}.json", if scope == Scope::User { "user" } else { "system" });
    Some(std::fs::read_to_string(std::path::Path::new(&dir).join(name)).map_err(|e| e.to_string()))
}

/// `systemctl list-unit-files --type=service --output=json`.
pub fn unit_files(scope: Scope) -> Result<String, String> {
    if let Some(f) = fixture(scope, "unit-files") {
        return f;
    }
    cmd::run("systemctl", &[scope.flag(), "list-unit-files", "--type=service", "--output=json", "--no-pager"])
}

pub fn list(scope: Scope) -> Result<Vec<Unit>, String> {
    if let Some(units) = fixture(scope, "units") {
        let files = unit_files(scope).ok().and_then(|f| parse_unit_files(&f).ok()).unwrap_or_default();
        return Ok(merge(parse_list_units(&units?)?, files));
    }
    let units = cmd::run(
        "systemctl",
        &[scope.flag(), "list-units", "--type=service", "--all", "--output=json", "--no-pager"],
    )?;
    let files = unit_files(scope)
        .ok()
    .and_then(|f| parse_unit_files(&f).ok())
    .unwrap_or_default();
    Ok(merge(parse_list_units(&units)?, files))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

impl Verb {
    pub fn command(self) -> &'static str {
        match self {
            Verb::Start => "start",
            Verb::Stop => "stop",
            Verb::Restart => "restart",
            Verb::Enable => "enable",
            Verb::Disable => "disable",
        }
    }

    pub fn past(self) -> &'static str {
        match self {
            Verb::Start => "Started",
            Verb::Stop => "Stopped",
            Verb::Restart => "Restarted",
            Verb::Enable => "Enabled",
            Verb::Disable => "Disabled",
        }
    }
}

/// Run a systemctl verb. System units ask for authentication through
/// polkit (your desktop's agent shows the prompt).
pub fn act(scope: Scope, verb: Verb, unit: &str) -> Result<(), String> {
    cmd::run("systemctl", &[scope.flag(), verb.command(), unit]).map(|_| ())
}

/// The last 200 lines of a unit's journal.
pub fn logs(scope: Scope, unit: &str) -> Result<String, String> {
    let selector = match scope {
        Scope::User => format!("--user-unit={unit}"),
        Scope::System => format!("--unit={unit}"),
    };
    cmd::run("journalctl", &[&selector, "-n", "200", "--output=short-iso", "--no-pager"])
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNITS: &str = r#"[
      {"unit":"pipewire.service","load":"loaded","active":"active","sub":"running","description":"PipeWire Multimedia Service"},
      {"unit":"broken.service","load":"loaded","active":"failed","sub":"failed","description":"Broken"}
    ]"#;
    const FILES: &str = r#"[
      {"unit_file":"/usr/lib/systemd/user/pipewire.service","state":"enabled","preset":"enabled"},
      {"unit_file":"/home/u/.config/systemd/user/sync.service","state":"disabled","preset":null},
      {"unit_file":"/usr/lib/systemd/user/foo@.service","state":"static","preset":null}
    ]"#;

    #[test]
    fn merges_units_and_files() {
        let units = merge(parse_list_units(UNITS).unwrap(), parse_unit_files(FILES).unwrap());
        let names: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["broken.service", "pipewire.service", "sync.service"]);
        let pw = &units[1];
        assert!(pw.is_running() && pw.is_enabled() && pw.can_toggle());
        assert!(units[0].is_failed());
        assert!(!units[0].can_toggle(), "no unit file state known");
        assert_eq!(units[2].active, "inactive");
        assert_eq!(units[2].file_state.as_deref(), Some("disabled"));
    }

    #[test]
    fn bad_json() {
        assert!(parse_list_units("not json").is_err());
    }
}
