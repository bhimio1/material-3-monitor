//! Background sampling. A thread reads `/proc`, `/sys` and sysinfo about
//! once a second and sends a [`Snapshot`] to the UI; NVIDIA GPUs are
//! polled on a thread of their own because `nvidia-smi` is slow to start.

pub mod cpu;
pub mod gpu;
pub mod history;
pub mod io;
pub mod mem;
pub mod procs;
pub mod sensors;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where `/proc` and `/sys` are. `M3_MONITOR_SYSROOT` points `sys` at a
/// fixture tree (used for screenshots and on machines without sensors).
#[derive(Clone, Debug)]
pub struct Roots {
    pub proc: PathBuf,
    pub sys: PathBuf,
}

impl Roots {
    pub fn from_env() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
            sys: std::env::var_os("M3_MONITOR_SYSROOT").map(PathBuf::from).unwrap_or_else(|| "/sys".into()),
        }
    }
}

/// Things that don't change while the app runs.
#[derive(Clone, Debug, Default)]
pub struct StaticInfo {
    pub cpu_model: String,
    pub cores: usize,
    pub max_freq: Option<f32>,
    pub hostname: String,
    pub kernel: String,
    pub os: String,
}

#[derive(Clone, Debug)]
pub struct DiskIo {
    pub name: String,
    pub model: Option<String>,
    pub read_rate: f64,
    pub write_rate: f64,
    /// Fraction of the interval the disk was busy.
    pub busy: f32,
}

#[derive(Clone, Debug)]
pub struct Mount {
    pub mount_point: String,
    pub device: String,
    pub fs: String,
    pub total: u64,
    pub available: u64,
}

#[derive(Clone, Debug)]
pub struct NetIo {
    pub name: String,
    pub kind: io::NetKind,
    pub rx_rate: f64,
    pub tx_rate: f64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub freqs: Vec<f32>,
    pub governor: Option<String>,
    pub load: cpu::LoadAvg,
    pub uptime: f64,
    pub context_switch_rate: f64,
    pub mem: mem::MemInfo,
    pub zram: Vec<mem::Zram>,
    pub pressure: mem::Pressure,
    pub disks: Vec<DiskIo>,
    pub mounts: Vec<Mount>,
    pub nets: Vec<NetIo>,
    pub temps: Vec<sensors::Temp>,
    pub fans: Vec<sensors::Fan>,
    pub cpu_temp: Option<f32>,
    pub batteries: Vec<sensors::Battery>,
    pub ac: Option<bool>,
    pub gpus: Vec<gpu::Gpu>,
    /// Only filled while the Processes page (or something else) wants it.
    pub processes: Option<Arc<Vec<procs::ProcRow>>>,
}

/// Shared switches the UI flips.
#[derive(Default)]
pub struct Control {
    pub paused: AtomicBool,
    pub want_processes: AtomicBool,
    pub interval_ms: AtomicU64,
}

pub struct Sampler {
    pub control: Arc<Control>,
    pub info: StaticInfo,
    rx: flume::Receiver<Snapshot>,
    wake: flume::Sender<()>,
}

impl Sampler {
    pub fn start(ctx: egui::Context, roots: Roots, interval: Duration) -> Self {
        let control = Arc::new(Control::default());
        control.interval_ms.store(interval.as_millis() as u64, Ordering::Relaxed);
        let (tx, rx) = flume::bounded(2);
        let (wake, wake_rx) = flume::unbounded();
        let info = static_info(&roots);

        let nvidia: Arc<Mutex<Vec<gpu::Gpu>>> = Arc::default();
        if gpu::has_nvidia(&roots.sys) {
            let out = nvidia.clone();
            let control = control.clone();
            std::thread::Builder::new()
                .name("m3-monitor-nvidia".into())
                .spawn(move || loop {
                    if !control.paused.load(Ordering::Relaxed) {
                        match gpu::query_nvidia() {
                            Some(g) => *out.lock().unwrap() = g,
                            None => break,
                        }
                    }
                    std::thread::sleep(Duration::from_millis(control.interval_ms.load(Ordering::Relaxed).max(500)));
                    if Arc::strong_count(&out) == 1 {
                        break;
                    }
                })
                .ok();
        }

        let thread_control = control.clone();
        std::thread::Builder::new()
            .name("m3-monitor-sampler".into())
            .spawn(move || {
                let mut state = State::new(roots, info_cores(&ctx));
                loop {
                    if !thread_control.paused.load(Ordering::Relaxed) {
                        let want = thread_control.want_processes.load(Ordering::Relaxed);
                        let mut snap = state.sample(want);
                        snap.gpus.extend(nvidia.lock().unwrap().iter().cloned());
                        if tx.send(snap).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    let wait = Duration::from_millis(thread_control.interval_ms.load(Ordering::Relaxed).max(250));
                    match wake_rx.recv_timeout(wait) {
                        Ok(()) | Err(flume::RecvTimeoutError::Timeout) => {}
                        Err(flume::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn sampler thread");

        Self { control, info, rx, wake }
    }

    /// The newest snapshot, if one arrived.
    pub fn poll(&self) -> Option<Snapshot> {
        self.rx.try_iter().last()
    }

    /// Sample again now (after an action, or when resuming).
    pub fn refresh(&self) {
        let _ = self.wake.send(());
    }
}

fn info_cores(_ctx: &egui::Context) -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn static_info(roots: &Roots) -> StaticInfo {
    let cpuinfo = std::fs::read_to_string(roots.proc.join("cpuinfo")).unwrap_or_default();
    let stat = cpu::parse_stat(&std::fs::read_to_string(roots.proc.join("stat")).unwrap_or_default());
    let os = material_3_ui::sys::arch::OsRelease::load();
    StaticInfo {
        cpu_model: cpu::parse_model(&cpuinfo).unwrap_or_else(|| "CPU".into()),
        cores: stat.cores.len().max(1),
        max_freq: cpu::max_frequency(&roots.sys),
        hostname: sysinfo::System::host_name().unwrap_or_default(),
        kernel: sysinfo::System::kernel_version().unwrap_or_default(),
        os: if os.pretty_name.is_empty() { os.id } else { os.pretty_name },
    }
}

/// The sampler thread's memory of the previous sample, for rates.
struct State {
    roots: Roots,
    cores: usize,
    last: Option<Instant>,
    stat: cpu::Stat,
    disks: HashMap<String, io::DiskCounters>,
    nets: HashMap<String, io::NetCounters>,
    net_kinds: HashMap<String, io::NetKind>,
    disk_models: HashMap<String, Option<String>>,
    system: sysinfo::System,
    users: sysinfo::Users,
    mounts: sysinfo::Disks,
    ticks: u64,
    process_read: HashMap<u32, (u64, u64)>,
}

impl State {
    fn new(roots: Roots, cores: usize) -> Self {
        Self {
            roots,
            cores,
            last: None,
            stat: cpu::Stat::default(),
            disks: HashMap::new(),
            nets: HashMap::new(),
            net_kinds: HashMap::new(),
            disk_models: HashMap::new(),
            system: sysinfo::System::new(),
            users: sysinfo::Users::new_with_refreshed_list(),
            mounts: sysinfo::Disks::new_with_refreshed_list(),
            ticks: 0,
            process_read: HashMap::new(),
        }
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.roots.proc.join(path)).unwrap_or_default()
    }

    fn sample(&mut self, want_processes: bool) -> Snapshot {
        let now = Instant::now();
        let dt = self.last.map(|l| (now - l).as_secs_f64()).filter(|d| *d > 0.05);
        self.last = Some(now);
        self.ticks += 1;
        let sys = self.roots.sys.clone();

        let stat = cpu::parse_stat(&self.read("stat"));
        let cpu_total = stat.total.usage_since(&self.stat.total);
        let cores: Vec<f32> = stat
            .cores
            .iter()
            .enumerate()
            .map(|(i, c)| self.stat.cores.get(i).map(|p| c.usage_since(p)).unwrap_or(0.0))
            .collect();
        let ctx_rate = dt
            .map(|dt| stat.context_switches.saturating_sub(self.stat.context_switches) as f64 / dt)
            .unwrap_or(0.0);
        self.stat = stat;

        let rate = |now: u64, before: Option<u64>| match (dt, before) {
            (Some(dt), Some(b)) => now.saturating_sub(b) as f64 / dt,
            _ => 0.0,
        };

        let diskstats = io::parse_diskstats(&self.read("diskstats"));
        let mut disks: Vec<DiskIo> = diskstats
            .iter()
            .map(|(name, c)| {
                let prev = self.disks.get(name);
                let model = self
                    .disk_models
                    .entry(name.clone())
                    .or_insert_with(|| io::disk_model(&sys, name))
                    .clone();
                DiskIo {
                    name: name.clone(),
                    model,
                    read_rate: rate(c.read, prev.map(|p| p.read)),
                    write_rate: rate(c.written, prev.map(|p| p.written)),
                    busy: match (dt, prev) {
                        (Some(dt), Some(p)) => (c.io_ms.saturating_sub(p.io_ms) as f64 / (dt * 1000.0)).min(1.0) as f32,
                        _ => 0.0,
                    },
                }
            })
            .collect();
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        self.disks = diskstats;

        let netdev = io::parse_net_dev(&self.read("net/dev"));
        let mut nets = Vec::new();
        let mut counters = HashMap::new();
        for (name, c) in netdev {
            let prev = self.nets.get(&name);
            let kind = *self.net_kinds.entry(name.clone()).or_insert_with(|| io::net_kind(&sys, &name));
            nets.push(NetIo {
                name: name.clone(),
                kind,
                rx_rate: rate(c.rx, prev.map(|p| p.rx)),
                tx_rate: rate(c.tx, prev.map(|p| p.tx)),
                rx_total: c.rx,
                tx_total: c.tx,
            });
            counters.insert(name, c);
        }
        self.nets = counters;

        // Mounts change rarely; re-list every 10 samples, refresh sizes every time.
        if self.ticks.is_multiple_of(10) {
            self.mounts.refresh(true);
        } else {
            self.mounts.refresh(false);
        }
        let mounts = collect_mounts(&self.mounts);

        let (temps, fans) = sensors::hwmon(&sys);
        let (batteries, ac) = sensors::power_supplies(&sys);

        let processes = want_processes.then(|| Arc::new(self.processes(dt)));
        if !want_processes {
            self.process_read.clear();
        }

        Snapshot {
            cpu: cpu_total,
            cores,
            freqs: cpu::frequencies(&sys, self.cores),
            governor: cpu::governor(&sys),
            load: cpu::parse_loadavg(&self.read("loadavg")),
            uptime: cpu::parse_uptime(&self.read("uptime")),
            context_switch_rate: ctx_rate,
            mem: mem::parse_meminfo(&self.read("meminfo")),
            zram: mem::zram_devices(&sys),
            pressure: mem::pressure(&self.roots.proc),
            disks,
            mounts,
            nets,
            cpu_temp: sensors::cpu_temp(&temps),
            temps,
            fans,
            batteries,
            ac,
            gpus: gpu::sysfs_gpus(&sys),
            processes,
        }
    }

    fn processes(&mut self, dt: Option<f64>) -> Vec<procs::ProcRow> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage()
                .with_user(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        );
        if self.ticks.is_multiple_of(30) {
            self.users.refresh();
        }
        let cores = self.cores.max(1) as f32;
        let mut seen = HashMap::with_capacity(self.system.processes().len());
        let rows = self
            .system
            .processes()
            .values()
            .filter(|p| p.thread_kind() != Some(sysinfo::ThreadKind::Userland))
            .map(|p| {
                let pid = p.pid().as_u32();
                let usage = p.disk_usage();
                // sysinfo's per-refresh deltas assume a regular interval;
                // compute rates from the totals instead.
                let prev = self.process_read.get(&pid).copied();
                seen.insert(pid, (usage.total_read_bytes, usage.total_written_bytes));
                let (read_rate, write_rate) = match (dt, prev) {
                    (Some(dt), Some((r, w))) => (
                        usage.total_read_bytes.saturating_sub(r) as f64 / dt,
                        usage.total_written_bytes.saturating_sub(w) as f64 / dt,
                    ),
                    _ => (0.0, 0.0),
                };
                let uid = p.user_id().map(|u| **u);
                let user = p
                    .user_id()
                    .and_then(|u| self.users.get_user_by_id(u))
                    .map(|u| u.name().to_string())
                    .or_else(|| uid.map(|u| u.to_string()))
                    .unwrap_or_default();
                let cmd = p.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ");
                let parent = p.parent().map(|p| p.as_u32());
                procs::ProcRow {
                    pid,
                    parent,
                    name: p.name().to_string_lossy().into_owned(),
                    kernel_thread: cmd.is_empty() && (parent == Some(2) || pid == 2),
                    cmd,
                    exe: p.exe().map(Path::to_path_buf),
                    uid,
                    user,
                    cpu: p.cpu_usage() / cores,
                    memory: p.memory(),
                    read_rate,
                    write_rate,
                    state: p.status().into(),
                    start_time: p.start_time(),
                }
            })
            .collect();
        self.process_read = seen;
        rows
    }
}

/// Real filesystems, one entry per device (btrfs subvolumes share one).
fn collect_mounts(disks: &sysinfo::Disks) -> Vec<Mount> {
    const VIRTUAL: &[&str] = &["tmpfs", "devtmpfs", "overlay", "squashfs", "ramfs", "efivarfs", "proc", "sysfs", "fuse.portal"];
    let mut by_device: HashMap<String, Mount> = HashMap::new();
    for d in disks.list() {
        let fs = d.file_system().to_string_lossy().into_owned();
        if VIRTUAL.contains(&fs.as_str()) || d.total_space() == 0 {
            continue;
        }
        let m = Mount {
            mount_point: d.mount_point().to_string_lossy().into_owned(),
            device: d.name().to_string_lossy().into_owned(),
            fs,
            total: d.total_space(),
            available: d.available_space(),
        };
        match by_device.get(&m.device) {
            Some(existing) if existing.mount_point.len() <= m.mount_point.len() => {}
            _ => {
                by_device.insert(m.device.clone(), m);
            }
        }
    }
    let mut out: Vec<Mount> = by_device.into_values().collect();
    out.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture tree used for screenshots parses into every tile.
    #[test]
    fn fixture_tree() {
        let sys = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sys");
        let (temps, fans) = sensors::hwmon(&sys);
        assert_eq!(temps.len(), 5);
        assert_eq!(sensors::cpu_temp(&temps), Some(63.5));
        assert_eq!(fans.len(), 1);
        let (bats, ac) = sensors::power_supplies(&sys);
        assert_eq!((bats.len(), ac), (1, Some(false)));
        let gpus = gpu::sysfs_gpus(&sys);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "AMD Radeon 780M");
        assert_eq!(mem::zram_devices(&sys).len(), 1);
        assert_eq!(cpu::frequencies(&sys, 2), vec![2800.0, 3200.0]);
        let nvidia = std::fs::read_to_string(sys.parent().unwrap().join("nvidia-smi.csv")).unwrap();
        assert_eq!(gpu::parse_nvidia(&nvidia).len(), 1);
    }
}
