//! CPU: `/proc/stat` times, per-core frequencies, load average, uptime and
//! the model name.

use std::path::Path;

/// Jiffies spent in each state, from one `cpu` line of `/proc/stat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

impl CpuTimes {
    /// Busy fraction (0–1) between an earlier sample and this one.
    pub fn usage_since(&self, earlier: &CpuTimes) -> f32 {
        let total = self.total.saturating_sub(earlier.total);
        if total == 0 {
            return 0.0;
        }
        (self.busy.saturating_sub(earlier.busy) as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

/// The aggregate line and one entry per core, plus context switches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stat {
    pub total: CpuTimes,
    pub cores: Vec<CpuTimes>,
    pub context_switches: u64,
    pub processes_running: u32,
}

fn parse_times(fields: &[&str]) -> Option<CpuTimes> {
    // user nice system idle iowait irq softirq steal guest guest_nice
    let n: Vec<u64> = fields.iter().map(|f| f.parse().ok()).collect::<Option<_>>()?;
    if n.len() < 4 {
        return None;
    }
    let get = |i: usize| n.get(i).copied().unwrap_or(0);
    // guest time is already counted in user/nice.
    let total = (0..8).map(get).sum::<u64>();
    let idle = get(3) + get(4);
    Some(CpuTimes { busy: total.saturating_sub(idle), total })
}

pub fn parse_stat(text: &str) -> Stat {
    let mut stat = Stat::default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let rest: Vec<&str> = parts.collect();
        if key == "cpu" {
            stat.total = parse_times(&rest).unwrap_or_default();
        } else if let Some(idx) = key.strip_prefix("cpu").and_then(|i| i.parse::<usize>().ok()) {
            if let Some(t) = parse_times(&rest) {
                if stat.cores.len() <= idx {
                    stat.cores.resize(idx + 1, CpuTimes::default());
                }
                stat.cores[idx] = t;
            }
        } else if key == "ctxt" {
            stat.context_switches = rest.first().and_then(|v| v.parse().ok()).unwrap_or(0);
        } else if key == "procs_running" {
            stat.processes_running = rest.first().and_then(|v| v.parse().ok()).unwrap_or(0);
        }
    }
    stat
}

/// 1, 5 and 15 minute load averages, and running/total tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadAvg {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
    pub tasks: u32,
}

pub fn parse_loadavg(text: &str) -> LoadAvg {
    let f: Vec<&str> = text.split_whitespace().collect();
    let num = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    LoadAvg {
        one: num(0),
        five: num(1),
        fifteen: num(2),
        tasks: f
            .get(3)
            .and_then(|v| v.split_once('/'))
            .and_then(|(_, total)| total.parse().ok())
            .unwrap_or(0),
    }
}

/// Seconds since boot.
pub fn parse_uptime(text: &str) -> f64 {
    text.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// "AMD Ryzen 7 7840U w/ Radeon 780M Graphics", from `/proc/cpuinfo`.
pub fn parse_model(cpuinfo: &str) -> Option<String> {
    cpuinfo
        .lines()
        .find(|l| l.starts_with("model name") || l.starts_with("Model") || l.starts_with("Hardware"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Current frequency of each core in MHz (`scaling_cur_freq` is in kHz).
pub fn frequencies(sys: &Path, cores: usize) -> Vec<f32> {
    (0..cores)
        .map(|i| {
            std::fs::read_to_string(sys.join(format!("devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq")))
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .map(|khz| khz / 1000.0)
                .unwrap_or(0.0)
        })
        .collect()
}

/// Highest `cpuinfo_max_freq` across cores, in MHz.
pub fn max_frequency(sys: &Path) -> Option<f32> {
    std::fs::read_to_string(sys.join("devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"))
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|khz| khz / 1000.0)
}

/// The cpufreq governor of cpu0 ("powersave", "performance", …).
pub fn governor(sys: &Path) -> Option<String> {
    std::fs::read_to_string(sys.join("devices/system/cpu/cpu0/cpufreq/scaling_governor"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0
cpu0 60 0 20 400 20 0 0 0 0 0
cpu1 40 0 30 400 30 0 0 0 0 0
intr 12345 0 0
ctxt 987654
btime 1700000000
processes 4242
procs_running 3
procs_blocked 0
";

    #[test]
    fn stat_lines() {
        let s = parse_stat(STAT);
        assert_eq!(s.total, CpuTimes { busy: 150, total: 1000 });
        assert_eq!(s.cores.len(), 2);
        assert_eq!(s.cores[1], CpuTimes { busy: 70, total: 500 });
        assert_eq!(s.context_switches, 987654);
        assert_eq!(s.processes_running, 3);
    }

    #[test]
    fn usage_between_samples() {
        let a = CpuTimes { busy: 100, total: 1000 };
        let b = CpuTimes { busy: 150, total: 1100 };
        assert!((b.usage_since(&a) - 0.5).abs() < 1e-6);
        // Counter reset or no time passed.
        assert_eq!(a.usage_since(&b), 0.0);
        assert_eq!(a.usage_since(&a), 0.0);
    }

    #[test]
    fn loadavg_uptime_model() {
        let l = parse_loadavg("0.52 0.58 0.59 2/1234 56789\n");
        assert_eq!((l.one, l.five, l.fifteen, l.tasks), (0.52, 0.58, 0.59, 1234));
        assert_eq!(parse_uptime("3600.25 7000.00\n"), 3600.25);
        let info = "processor\t: 0\nmodel name\t: AMD Ryzen 7  7840U\nflags\t: fpu\n";
        assert_eq!(parse_model(info).as_deref(), Some("AMD Ryzen 7 7840U"));
    }

    #[test]
    fn frequencies_from_sysfs() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("devices/system/cpu/cpu0/cpufreq");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("scaling_cur_freq"), "3800000\n").unwrap();
        std::fs::write(f.join("cpuinfo_max_freq"), "5100000\n").unwrap();
        std::fs::write(f.join("scaling_governor"), "powersave\n").unwrap();
        assert_eq!(frequencies(dir.path(), 2), vec![3800.0, 0.0]);
        assert_eq!(max_frequency(dir.path()), Some(5100.0));
        assert_eq!(governor(dir.path()).as_deref(), Some("powersave"));
    }
}
