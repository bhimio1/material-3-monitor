//! The process table: rows from sysinfo, grouping into apps, the parent /
//! child tree, sorting, and the signal / renice actions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Sleeping,
    DiskSleep,
    Stopped,
    Zombie,
    Idle,
    Other,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Running => "Running",
            State::Sleeping => "Sleeping",
            State::DiskSleep => "Waiting (I/O)",
            State::Stopped => "Stopped",
            State::Zombie => "Zombie",
            State::Idle => "Idle",
            State::Other => "Other",
        }
    }
}

impl From<sysinfo::ProcessStatus> for State {
    fn from(s: sysinfo::ProcessStatus) -> Self {
        use sysinfo::ProcessStatus as P;
        match s {
            P::Run => State::Running,
            P::Sleep => State::Sleeping,
            P::UninterruptibleDiskSleep => State::DiskSleep,
            P::Stop | P::Tracing => State::Stopped,
            P::Zombie | P::Dead => State::Zombie,
            P::Idle => State::Idle,
            _ => State::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcRow {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub cmd: String,
    pub exe: Option<PathBuf>,
    pub uid: Option<u32>,
    pub user: String,
    /// Percent of the whole machine (all cores = 100).
    pub cpu: f32,
    pub memory: u64,
    /// Bytes per second.
    pub read_rate: f64,
    pub write_rate: f64,
    pub state: State,
    /// Unix seconds.
    pub start_time: u64,
    pub kernel_thread: bool,
}

/// Processes that start whole sessions: an app is the topmost ancestor
/// below one of these.
const SESSION_ROOTS: &[&str] = &[
    "systemd",
    "init",
    "kthreadd",
    "Hyprland",
    ".Hyprland-wrapped",
    "start-hyprland",
    "uwsm",
    "sway",
    "niri",
    "kwin_wayland",
    "gnome-shell",
    "sddm",
    "sddm-helper",
    "gdm",
    "gdm-session-wor",
    "login",
    "greetd",
    "agreety",
    "tuigreet",
    "sshd",
    "sshd-session",
    "quickshell",
    "qs",
    "dbus-daemon",
    "dbus-broker",
];

pub fn is_session_root(name: &str) -> bool {
    SESSION_ROOTS.contains(&name)
}

/// For each pid, the pid of the app it belongs to: its topmost ancestor
/// whose parent is a session root (or that has no parent). Session roots
/// are their own app.
pub fn app_roots(rows: &[ProcRow]) -> HashMap<u32, u32> {
    let by_pid: HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        let mut current = row;
        let mut seen = HashSet::new();
        loop {
            if is_session_root(&current.name) || current.kernel_thread || !seen.insert(current.pid) {
                break;
            }
            match current.parent.and_then(|p| by_pid.get(&p)) {
                Some(parent) if !is_session_root(&parent.name) && parent.pid > 1 => current = parent,
                _ => break,
            }
        }
        out.insert(row.pid, current.pid);
    }
    out
}

/// One app: its root process and the totals of all its processes.
#[derive(Clone, Debug, PartialEq)]
pub struct AppGroup {
    pub root: u32,
    pub name: String,
    pub pids: Vec<u32>,
    pub cpu: f32,
    pub memory: u64,
    pub read_rate: f64,
    pub write_rate: f64,
    pub user: String,
}

pub fn group(rows: &[ProcRow]) -> Vec<AppGroup> {
    let roots = app_roots(rows);
    let by_pid: HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut groups: HashMap<u32, AppGroup> = HashMap::new();
    for row in rows {
        let root = roots.get(&row.pid).copied().unwrap_or(row.pid);
        let g = groups.entry(root).or_insert_with(|| {
            let r = by_pid.get(&root).copied().unwrap_or(row);
            AppGroup {
                root,
                name: r.name.clone(),
                pids: Vec::new(),
                cpu: 0.0,
                memory: 0,
                read_rate: 0.0,
                write_rate: 0.0,
                user: r.user.clone(),
            }
        });
        g.pids.push(row.pid);
        g.cpu += row.cpu;
        g.memory += row.memory;
        g.read_rate += row.read_rate;
        g.write_rate += row.write_rate;
    }
    let mut out: Vec<AppGroup> = groups.into_values().collect();
    for g in &mut out {
        g.pids.sort_unstable();
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Name,
    Pid,
    User,
    #[default]
    Cpu,
    Memory,
    Io,
    State,
}

pub fn compare(a: &ProcRow, b: &ProcRow, key: SortKey) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let f = |x: f64, y: f64| x.partial_cmp(&y).unwrap_or(Ordering::Equal);
    match key {
        SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortKey::Pid => a.pid.cmp(&b.pid),
        SortKey::User => a.user.cmp(&b.user),
        SortKey::Cpu => f(a.cpu as f64, b.cpu as f64),
        SortKey::Memory => a.memory.cmp(&b.memory),
        SortKey::Io => f(a.read_rate + a.write_rate, b.read_rate + b.write_rate),
        SortKey::State => a.state.label().cmp(b.state.label()),
    }
    .then(a.pid.cmp(&b.pid))
}

/// Rows in tree order with their depth, siblings sorted by `key`. The
/// children of pids in `collapsed` are left out.
pub fn tree(rows: &[ProcRow], key: SortKey, descending: bool, collapsed: &HashSet<u32>) -> Vec<(usize, usize, bool)> {
    let index: HashMap<u32, usize> = rows.iter().enumerate().map(|(i, r)| (r.pid, i)).collect();
    let mut children: HashMap<Option<u32>, Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        let parent = r.parent.filter(|p| index.contains_key(p));
        children.entry(parent).or_default().push(i);
    }
    for list in children.values_mut() {
        list.sort_by(|&a, &b| {
            let o = compare(&rows[a], &rows[b], key);
            if descending {
                o.reverse()
            } else {
                o
            }
        });
    }
    // (row index, depth, has children)
    let mut out = Vec::with_capacity(rows.len());
    let mut stack: Vec<(usize, usize)> = children.get(&None).map(|v| v.iter().rev().map(|&i| (i, 0)).collect()).unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some((i, depth)) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        let kids = children.get(&Some(rows[i].pid));
        out.push((i, depth, kids.is_some_and(|k| !k.is_empty())));
        if collapsed.contains(&rows[i].pid) {
            continue;
        }
        if let Some(kids) = kids {
            stack.extend(kids.iter().rev().map(|&k| (k, depth + 1)));
        }
    }
    out
}

/// Case-insensitive match on name, command line, user or pid.
pub fn matches(row: &ProcRow, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    row.name.to_lowercase().contains(&q)
        || row.cmd.to_lowercase().contains(&q)
        || row.user.to_lowercase().contains(&q)
        || row.pid.to_string() == q
}

/// Fields from `/proc/PID/stat` that sysinfo doesn't expose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatExtra {
    pub nice: i32,
    pub threads: u32,
}

/// The comm field can contain spaces and parentheses, so split after the
/// last ')'.
pub fn parse_pid_stat(text: &str) -> Option<StatExtra> {
    let rest = &text[text.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After comm: state(0) ppid(1) … priority(15) nice(16) num_threads(17)
    Some(StatExtra { nice: f.get(16)?.parse().ok()?, threads: f.get(17)?.parse().ok()? })
}

pub fn stat_extra(pid: u32) -> Option<StatExtra> {
    parse_pid_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
    Stop,
    Cont,
}

impl Signal {
    fn number(self) -> i32 {
        match self {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
            Signal::Stop => libc::SIGSTOP,
            Signal::Cont => libc::SIGCONT,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Signal::Term => "TERM",
            Signal::Kill => "KILL",
            Signal::Stop => "STOP",
            Signal::Cont => "CONT",
        }
    }
}

/// Send a signal; processes of other users go through pkexec.
pub fn send_signal(pids: &[u32], signal: Signal) -> Result<(), String> {
    let mut denied = Vec::new();
    for &pid in pids {
        // SAFETY: kill() has no memory-safety preconditions.
        if unsafe { libc::kill(pid as i32, signal.number()) } != 0 {
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EPERM) => denied.push(pid.to_string()),
                Some(libc::ESRCH) => {}
                _ => return Err(format!("Couldn't signal {pid}: {err}")),
            }
        }
    }
    if denied.is_empty() {
        return Ok(());
    }
    let mut args = vec!["kill", "-s", signal.name()];
    args.extend(denied.iter().map(String::as_str));
    material_3_ui::sys::cmd::run("pkexec", &args).map(|_| ())
}

/// Change a process's niceness (-20..19). Raising priority needs root.
pub fn renice(pid: u32, nice: i32) -> Result<(), String> {
    // SAFETY: setpriority() has no memory-safety preconditions.
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EPERM) | Some(libc::EACCES) => {
            let n = nice.to_string();
            let p = pid.to_string();
            material_3_ui::sys::cmd::run("pkexec", &["renice", "-n", &n, "-p", &p]).map(|_| ())
        }
        _ => Err(format!("Couldn't renice {pid}: {err}")),
    }
}

#[cfg(test)]
pub fn row(pid: u32, parent: Option<u32>, name: &str) -> ProcRow {
    ProcRow {
        pid,
        parent,
        name: name.into(),
        cmd: name.into(),
        exe: None,
        uid: Some(1000),
        user: "user".into(),
        cpu: 1.0,
        memory: 100,
        read_rate: 0.0,
        write_rate: 0.0,
        state: State::Sleeping,
        start_time: 0,
        kernel_thread: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<ProcRow> {
        vec![
            row(1, None, "systemd"),
            row(2, None, "kthreadd"),
            row(900, Some(1), "systemd"),
            row(1000, Some(900), "Hyprland"),
            row(1100, Some(1000), "kitty"),
            row(1101, Some(1100), "fish"),
            row(1102, Some(1101), "cargo"),
            row(1200, Some(1), "firefox"),
            row(1201, Some(1200), "Isolated Web Co"),
            row(1202, Some(1200), "Isolated Web Co"),
            row(1300, Some(900), "pipewire"),
        ]
    }

    #[test]
    fn apps_are_topmost_ancestors_below_the_session() {
        let roots = app_roots(&sample());
        assert_eq!(roots[&1102], 1100, "cargo in fish in kitty belongs to kitty");
        assert_eq!(roots[&1202], 1200);
        assert_eq!(roots[&1300], 1300, "services under the user manager are their own app");
        assert_eq!(roots[&1000], 1000);
    }

    #[test]
    fn groups_sum_their_processes() {
        let groups = group(&sample());
        let firefox = groups.iter().find(|g| g.name == "firefox").unwrap();
        assert_eq!(firefox.pids, vec![1200, 1201, 1202]);
        assert_eq!(firefox.memory, 300);
        assert!((firefox.cpu - 3.0).abs() < 1e-6);
        let kitty = groups.iter().find(|g| g.name == "kitty").unwrap();
        assert_eq!(kitty.pids.len(), 3);
    }

    #[test]
    fn cycles_dont_hang() {
        let rows = vec![row(10, Some(11), "a"), row(11, Some(10), "b")];
        assert_eq!(app_roots(&rows).len(), 2);
        assert_eq!(tree(&rows, SortKey::Pid, false, &HashSet::new()).len(), 0, "a pure cycle has no root");
    }

    #[test]
    fn tree_order_and_collapse() {
        let rows = sample();
        let t = tree(&rows, SortKey::Pid, false, &HashSet::new());
        let order: Vec<(u32, usize)> = t.iter().map(|&(i, d, _)| (rows[i].pid, d)).collect();
        assert_eq!(&order[..4], &[(1, 0), (900, 1), (1000, 2), (1100, 3)]);
        assert_eq!(order.len(), rows.len());
        let collapsed: HashSet<u32> = [1100].into();
        let t = tree(&rows, SortKey::Pid, false, &collapsed);
        assert_eq!(t.len(), rows.len() - 2);
        let kitty = t.iter().find(|(i, _, _)| rows[*i].pid == 1100).unwrap();
        assert!(kitty.2, "kitty still shows it has children");
    }

    #[test]
    fn sorting_and_search() {
        let mut a = row(5, None, "Zed");
        let mut b = row(6, None, "alacritty");
        a.memory = 10;
        b.memory = 20;
        assert_eq!(compare(&a, &b, SortKey::Name), std::cmp::Ordering::Greater);
        assert_eq!(compare(&a, &b, SortKey::Memory), std::cmp::Ordering::Less);
        assert!(matches(&a, "zE"));
        assert!(matches(&a, "5"));
        assert!(!matches(&a, "55"));
    }

    fn state_char(pid: u32) -> char {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        text[text.rfind(')').unwrap() + 1..].trim_start().chars().next().unwrap()
    }

    #[test]
    fn signals_and_renice_on_a_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        send_signal(&[pid], Signal::Stop).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(state_char(pid), 'T');
        send_signal(&[pid], Signal::Cont).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_ne!(state_char(pid), 'T');
        renice(pid, 7).unwrap();
        assert_eq!(stat_extra(pid).unwrap().nice, 7);
        send_signal(&[pid], Signal::Term).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
        // Already gone: not an error.
        send_signal(&[pid], Signal::Term).unwrap();
    }

    #[test]
    fn pid_stat_fields() {
        let text = "1234 (Web Content (x)) S 1200 1234 1234 0 -1 4194560 100 0 0 0 50 10 0 0 20 5 42 0 12345 0 0";
        assert_eq!(parse_pid_stat(text), Some(StatExtra { nice: 5, threads: 42 }));
        assert_eq!(parse_pid_stat("garbage"), None);
    }
}
