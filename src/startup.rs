//! What starts with your session: XDG autostart entries, Hyprland's
//! startup commands (read-only), and enabled user services.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct AutostartEntry {
    /// "nm-applet.desktop"; a file in ~/.config/autostart overrides the
    /// system one with the same name.
    pub file_name: String,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub enabled: bool,
    /// The file the entry was read from.
    pub path: PathBuf,
    /// Whether that file is in the user's own autostart folder.
    pub user: bool,
}

/// Look up `key` in the `[Desktop Entry]` group.
pub fn desktop_value(text: &str, key: &str) -> Option<String> {
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Set `key=value` in the `[Desktop Entry]` group, adding it if missing.
pub fn set_desktop_value(text: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let mut in_entry = false;
    let mut done = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_entry && !done {
                out.push(format!("{key}={value}"));
                done = true;
            }
            in_entry = trimmed == "[Desktop Entry]";
        } else if in_entry && !done && trimmed.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            out.push(format!("{key}={value}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if !in_entry {
            out.push("[Desktop Entry]".into());
        }
        out.push(format!("{key}={value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

pub fn parse_entry(path: &Path, text: &str, user: bool) -> AutostartEntry {
    let hidden = desktop_value(text, "Hidden").is_some_and(|v| v == "true");
    let gnome_off = desktop_value(text, "X-GNOME-Autostart-enabled").is_some_and(|v| v == "false");
    let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    AutostartEntry {
        name: desktop_value(text, "Name").unwrap_or_else(|| file_name.trim_end_matches(".desktop").to_string()),
        comment: desktop_value(text, "Comment").unwrap_or_default(),
        exec: desktop_value(text, "Exec").unwrap_or_default(),
        enabled: !hidden && !gnome_off,
        file_name,
        path: path.to_path_buf(),
        user,
    }
}

/// Autostart folders, user first ($XDG_CONFIG_HOME, then $XDG_CONFIG_DIRS).
pub fn autostart_dirs() -> Vec<(PathBuf, bool)> {
    let mut dirs = vec![(material_3_ui::sys::paths::config_home().join("autostart"), true)];
    let system = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    for d in system.split(':').filter(|d| !d.is_empty()) {
        dirs.push((Path::new(d).join("autostart"), false));
    }
    dirs
}

pub fn autostart_in(dirs: &[(PathBuf, bool)]) -> Vec<AutostartEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (dir, user) in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "desktop"))
            .collect();
        files.sort();
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if !seen.insert(name) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push(parse_entry(&path, &text, *user));
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

pub fn autostart() -> Vec<AutostartEntry> {
    autostart_in(&autostart_dirs())
}

/// Turn an entry on or off by writing `Hidden=` into the user's copy
/// (copying the system file first when there's no user copy yet).
pub fn set_enabled(entry: &AutostartEntry, enabled: bool, user_dir: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(&entry.path).map_err(|e| format!("{}: {e}", entry.path.display()))?;
    let mut text = set_desktop_value(&text, "Hidden", if enabled { "false" } else { "true" });
    if desktop_value(&text, "X-GNOME-Autostart-enabled").is_some() {
        text = set_desktop_value(&text, "X-GNOME-Autostart-enabled", if enabled { "true" } else { "false" });
    }
    std::fs::create_dir_all(user_dir).map_err(|e| format!("{}: {e}", user_dir.display()))?;
    let target = user_dir.join(&entry.file_name);
    std::fs::write(&target, text).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target)
}

/// A command Hyprland runs at startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyprExec {
    pub command: String,
    pub file: PathBuf,
    pub line: usize,
}

fn strip_lua_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut long = false;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for (i, &(pos, c)) in chars.iter().enumerate() {
        let next = chars.get(i + 1).map(|(_, n)| *n);
        if long {
            if c == ']' && next == Some(']') {
                long = false;
            }
            continue;
        }
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '[' && next == Some('[') => long = true,
            None if c == '-' && next == Some('-') => return &line[..pos],
            None => {}
        }
    }
    line
}

/// The first Lua string literal at the start of `s` ("…", '…' or [[…]]).
fn lua_string(s: &str) -> Option<String> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix("[[") {
        return rest.find("]]").map(|end| rest[..end].to_string());
    }
    let q = s.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut out = String::new();
    let mut chars = s[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            c if c == q => return Some(out),
            c => out.push(c),
        }
    }
    None
}

fn words(code: &str) -> impl Iterator<Item = &str> {
    code.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty())
}

/// Commands run by `hl.on("hyprland.start", function() … end)` blocks,
/// and the modules `require`d by the file.
pub fn parse_lua(text: &str) -> (Vec<(usize, String)>, Vec<String>) {
    let mut commands = Vec::new();
    let mut requires = Vec::new();
    let mut depth: i32 = 0;
    let mut in_block = false;
    for (n, raw) in text.lines().enumerate() {
        let code = strip_lua_comment(raw);
        let mut search = code;
        while let Some(pos) = search.find("require") {
            let after = search[pos + 7..].trim_start().trim_start_matches('(');
            if let Some(module) = lua_string(after) {
                requires.push(module);
            }
            search = &search[pos + 7..];
        }
        let mut segment = code;
        if !in_block {
            match code.find("hyprland.start") {
                Some(pos) if code[..pos].contains("hl.on") => {
                    in_block = true;
                    depth = 0;
                    segment = &code[pos..];
                }
                _ => continue,
            }
        }
        let mut s = segment;
        while let Some(pos) = s.find("exec_cmd") {
            let after = s[pos + 8..].trim_start().trim_start_matches('(');
            if let Some(cmd) = lua_string(after) {
                commands.push((n + 1, cmd));
            }
            s = &s[pos + 8..];
        }
        for w in words(segment) {
            match w {
                "function" | "if" | "do" | "repeat" => depth += 1,
                "end" | "until" => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 && segment.contains("end") {
            in_block = false;
        }
    }
    (commands, requires)
}

/// `exec-once = …` lines and `source = …` files of a hyprlang config.
pub fn parse_conf(text: &str) -> (Vec<(usize, String)>, Vec<String>) {
    let mut commands = Vec::new();
    let mut sources = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "exec-once" => commands.push((n + 1, v.trim().to_string())),
            "source" => sources.push(v.trim().to_string()),
            _ => {}
        }
    }
    (commands, sources)
}

/// Every startup command in the Hyprland config under `hypr_dir`,
/// following `require` (Lua) and `source` (hyprlang).
pub fn hyprland_in(hypr_dir: &Path) -> Vec<HyprExec> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let lua = hypr_dir.join("hyprland.lua");
    let mut queue: Vec<PathBuf> = if lua.is_file() { vec![lua] } else { vec![hypr_dir.join("hyprland.conf")] };
    while let Some(file) = queue.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let is_lua = file.extension().is_some_and(|e| e == "lua");
        let (commands, includes) = if is_lua { parse_lua(&text) } else { parse_conf(&text) };
        out.extend(commands.into_iter().map(|(line, command)| HyprExec { command, file: file.clone(), line }));
        for inc in includes {
            if is_lua {
                queue.push(hypr_dir.join(inc.replace('.', "/")).with_extension("lua"));
            } else {
                let p = material_3_ui::sys::paths::expand(&inc);
                let p = if p.is_relative() { file.parent().unwrap_or(hypr_dir).join(p) } else { p };
                queue.push(p);
            }
        }
    }
    out
}

pub fn hyprland() -> Vec<HyprExec> {
    hyprland_in(&material_3_ui::sys::paths::config_home().join("hypr"))
}

/// Enabled user services (they start with your session).
pub fn enabled_user_services() -> Result<Vec<String>, String> {
    let json = crate::services::unit_files(crate::services::Scope::User)?;
    let mut names: Vec<String> = crate::services::parse_unit_files(&json)?
        .into_iter()
        .filter(|(name, state)| state == "enabled" && !name.contains("@."))
        .map(|(name, _)| name)
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_values() {
        let text = "[Desktop Entry]\nName=Thing\nExec=thing --tray\n[Desktop Action x]\nName=Other\n";
        assert_eq!(desktop_value(text, "Name").as_deref(), Some("Thing"));
        assert_eq!(desktop_value(text, "Hidden"), None);
        let hidden = set_desktop_value(text, "Hidden", "true");
        assert!(hidden.contains("Exec=thing --tray\nHidden=true\n[Desktop Action x]"));
        assert_eq!(desktop_value(&set_desktop_value(&hidden, "Hidden", "false"), "Hidden").as_deref(), Some("false"));
    }

    #[test]
    fn user_entries_override_system_ones() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let system = root.path().join("system");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(system.join("a.desktop"), "[Desktop Entry]\nName=Applet\nExec=applet\n").unwrap();
        std::fs::write(system.join("b.desktop"), "[Desktop Entry]\nName=Bee\nExec=bee\nX-GNOME-Autostart-enabled=false\n").unwrap();
        let dirs = vec![(user.clone(), true), (system.clone(), false)];
        let list = autostart_in(&dirs);
        assert_eq!(list.len(), 2);
        assert!(list[0].enabled && !list[0].user);
        assert!(!list[1].enabled);

        set_enabled(&list[0], false, &user).unwrap();
        set_enabled(&list[1], true, &user).unwrap();
        let list = autostart_in(&dirs);
        assert!(!list[0].enabled && list[0].user);
        assert!(list[1].enabled, "Hidden=false and X-GNOME-Autostart-enabled=true");
        assert!(std::fs::read_to_string(system.join("a.desktop")).unwrap().find("Hidden").is_none());
    }

    #[test]
    fn lua_start_block() {
        let text = r#"require("custom.general")
hl.bind("SUPER + T", hl.dsp.exec_cmd("kitty")) -- not at startup
hl.on("hyprland.start", function()
    hl.exec_cmd("qs -c ii") -- the shell
    if true then
        hl.exec_cmd('hypridle')
    end
    -- hl.exec_cmd("commented")
end)
hl.exec_cmd("after")
hl.on("hyprland.start", function() hl.exec_cmd([[nm-applet --indicator]]) end)
"#;
        let (cmds, requires) = parse_lua(text);
        let cmds: Vec<&str> = cmds.iter().map(|(_, c)| c.as_str()).collect();
        assert_eq!(cmds, vec!["qs -c ii", "hypridle", "nm-applet --indicator"]);
        assert_eq!(requires, vec!["custom.general"]);
    }

    #[test]
    fn conf_and_includes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("hyprland.conf"), "exec-once = waybar\nsource = extra.conf\n# exec-once = no\n").unwrap();
        std::fs::write(root.path().join("extra.conf"), "exec-once=swww-daemon\nsource = hyprland.conf\n").unwrap();
        let list = hyprland_in(root.path());
        let cmds: Vec<&str> = list.iter().map(|e| e.command.as_str()).collect();
        assert_eq!(cmds, vec!["waybar", "swww-daemon"]);
        assert_eq!(list[1].line, 1);
    }

    #[test]
    fn lua_requires_are_followed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("custom")).unwrap();
        std::fs::write(root.path().join("hyprland.lua"), "require(\"custom.execs\")\n").unwrap();
        std::fs::write(
            root.path().join("custom/execs.lua"),
            "hl.on(\"hyprland.start\", function()\n  hl.exec_cmd(\"easyeffects --gapplication-service\")\nend)\n",
        )
        .unwrap();
        let list = hyprland_in(root.path());
        assert_eq!(list.len(), 1);
        assert!(list[0].file.ends_with("custom/execs.lua"));
        assert_eq!(list[0].line, 2);
    }
}
