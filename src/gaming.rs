//! Gaming state from the rest of the desktop: Material 3 Games' Game Mode
//! and running game, the sched-ext scheduler (scxctl), and the power
//! profile.

use material_3_ui::sys::cmd;
use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct RunningGame {
    pub id: String,
    pub name: String,
    pub pid: u32,
    #[serde(default)]
    pub started: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct GamesStatus {
    #[serde(default)]
    pub game_mode: bool,
    #[serde(default)]
    pub session_ready: bool,
    #[serde(default)]
    pub running_game: Option<RunningGame>,
}

pub fn parse_games_status(json: &str) -> Option<GamesStatus> {
    serde_json::from_str(json).ok()
}

/// The running sched-ext scheduler and mode, from `scxctl get`
/// ("running Lavd in Gaming mode"). None when none is running.
pub fn parse_scx(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.trim().strip_prefix("running ")?;
    match rest.split_once(" in ") {
        Some((sched, mode)) => Some((sched.trim().to_string(), Some(mode.trim().trim_end_matches(" mode").to_string()))),
        None => Some((rest.trim().to_string(), None)),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gaming {
    /// None when m3-games isn't installed.
    pub games: Option<GamesStatus>,
    /// Some(None) when scxctl is installed but nothing runs.
    pub scheduler: Option<Option<(String, Option<String>)>>,
    pub power_profile: Option<String>,
    pub gamemode_active: Option<bool>,
}

/// Ask everything. Runs on a background thread; each part is skipped when
/// its tool isn't installed.
pub fn query() -> Gaming {
    let games = cmd::exists("m3-games")
        .then(|| cmd::run("m3-games", &["status", "--json"]).ok())
        .flatten()
        .and_then(|j| parse_games_status(&j));
    let scheduler = cmd::exists("scxctl").then(|| cmd::run("scxctl", &["get"]).ok().and_then(|t| parse_scx(&t)));
    let power_profile = cmd::exists("powerprofilesctl")
        .then(|| cmd::run("powerprofilesctl", &["get"]).ok())
        .flatten()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    // gamemoded answers "gamemode is active" / "gamemode is inactive".
    let gamemode_active = cmd::exists("gamemoded")
        .then(|| cmd::run("gamemoded", &["-s"]).ok())
        .flatten()
        .map(|s| s.contains("is active"));
    Gaming { games, scheduler, power_profile, gamemode_active }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scx_output() {
        assert_eq!(parse_scx("running Lavd in Gaming mode\n"), Some(("Lavd".into(), Some("Gaming".into()))));
        assert_eq!(parse_scx("running Bpfland\n"), Some(("Bpfland".into(), None)));
        assert_eq!(parse_scx("no scx scheduler running\n"), None);
    }

    #[test]
    fn games_status() {
        let s = parse_games_status(
            r#"{"game_mode":true,"session_ready":true,"running_game":{"id":"steam:570","name":"Dota 2","pid":4242,"started":1700000000}}"#,
        )
        .unwrap();
        assert!(s.game_mode);
        assert_eq!(s.running_game.unwrap().name, "Dota 2");
        assert_eq!(parse_games_status(r#"{"game_mode":false,"session_ready":false,"running_game":null}"#).unwrap().running_game, None);
    }
}
