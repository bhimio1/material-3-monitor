//! GPUs: AMD through amdgpu's sysfs files, NVIDIA through `nvidia-smi`, and
//! Intel's frequencies through i915/xe sysfs.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Amd,
    Nvidia,
    Intel,
    Other,
}

impl Vendor {
    pub fn from_pci(id: &str) -> Self {
        match id.trim().to_ascii_lowercase().as_str() {
            "0x1002" => Vendor::Amd,
            "0x10de" => Vendor::Nvidia,
            "0x8086" => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

}

#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    /// Stable key for the history ("card1", "nvidia0").
    pub key: String,
    pub name: String,
    pub vendor: Vendor,
    /// Busy percentage (0–100).
    pub busy: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub temp: Option<f32>,
    /// Watts.
    pub power: Option<f32>,
    pub clock_mhz: Option<f32>,
    pub max_clock_mhz: Option<f32>,
    pub fan_rpm: Option<u32>,
    pub fan_percent: Option<f32>,
}

impl Gpu {
    fn new(key: String, name: String, vendor: Vendor) -> Self {
        Self {
            key,
            name,
            vendor,
            busy: None,
            vram_used: None,
            vram_total: None,
            temp: None,
            power: None,
            clock_mhz: None,
            max_clock_mhz: None,
            fan_rpm: None,
            fan_percent: None,
        }
    }
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn read_num(path: &Path) -> Option<f64> {
    read_trim(path)?.parse().ok()
}

/// The `drm/cardN` directories that are GPUs (not connectors like
/// `card1-DP-1`), with their vendor.
pub fn cards(sys: &Path) -> Vec<(String, PathBuf, Vendor)> {
    let Ok(entries) = std::fs::read_dir(sys.join("class/drm")) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix("card")?;
            if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let dir = e.path();
            let vendor = Vendor::from_pci(&read_trim(&dir.join("device/vendor"))?);
            Some((name, dir, vendor))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The current clock in a `pp_dpm_sclk` table (the line marked `*`), and
/// the highest one.
pub fn parse_dpm(text: &str) -> (Option<f32>, Option<f32>) {
    let mhz = |line: &str| -> Option<f32> {
        let v = line.split_once(':')?.1.split_whitespace().next()?;
        let v = v.to_ascii_lowercase();
        v.strip_suffix("mhz")?.parse().ok()
    };
    let current = text.lines().find(|l| l.trim_end().ends_with('*')).and_then(mhz);
    let max = text.lines().filter_map(mhz).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))));
    (current, max)
}

fn first_hwmon(device: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(device.join("hwmon")).ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// A readable name: the PCI database isn't parsed, so prefer what the
/// driver says and fall back to the vendor and device id.
fn card_name(device: &Path, vendor: Vendor) -> String {
    if let Some(name) = read_trim(&device.join("product_name")) {
        return name;
    }
    let id = read_trim(&device.join("device")).unwrap_or_default();
    let id = id.trim_start_matches("0x");
    match vendor {
        Vendor::Amd => format!("AMD Radeon ({id})"),
        Vendor::Intel => format!("Intel Graphics ({id})"),
        Vendor::Nvidia => format!("NVIDIA ({id})"),
        Vendor::Other => format!("GPU ({id})"),
    }
}

pub fn amd(key: &str, card: &Path) -> Gpu {
    let device = card.join("device");
    let mut gpu = Gpu::new(key.to_string(), card_name(&device, Vendor::Amd), Vendor::Amd);
    gpu.busy = read_num(&device.join("gpu_busy_percent")).map(|v| v as f32);
    gpu.vram_used = read_num(&device.join("mem_info_vram_used")).map(|v| v as u64);
    gpu.vram_total = read_num(&device.join("mem_info_vram_total")).map(|v| v as u64).filter(|v| *v > 0);
    let (clock, max) = read_trim(&device.join("pp_dpm_sclk")).map(|t| parse_dpm(&t)).unwrap_or_default();
    gpu.clock_mhz = clock;
    gpu.max_clock_mhz = max;
    if let Some(hw) = first_hwmon(&device) {
        gpu.temp = read_num(&hw.join("temp1_input")).map(|v| (v / 1000.0) as f32);
        gpu.power = read_num(&hw.join("power1_average"))
            .or_else(|| read_num(&hw.join("power1_input")))
            .map(|uw| (uw / 1e6) as f32);
        gpu.fan_rpm = read_num(&hw.join("fan1_input")).map(|v| v as u32);
        if gpu.clock_mhz.is_none() {
            gpu.clock_mhz = read_num(&hw.join("freq1_input")).map(|hz| (hz / 1e6) as f32);
        }
    }
    gpu
}

pub fn intel(key: &str, card: &Path) -> Gpu {
    let device = card.join("device");
    let mut gpu = Gpu::new(key.to_string(), card_name(&device, Vendor::Intel), Vendor::Intel);
    // i915 exposes gt_* on the card; xe under tile0/gt0/freq0.
    gpu.clock_mhz = read_num(&card.join("gt_act_freq_mhz"))
        .or_else(|| read_num(&card.join("gt_cur_freq_mhz")))
        .or_else(|| read_num(&device.join("tile0/gt0/freq0/act_freq")))
        .map(|v| v as f32);
    gpu.max_clock_mhz = read_num(&card.join("gt_max_freq_mhz"))
        .or_else(|| read_num(&device.join("tile0/gt0/freq0/max_freq")))
        .map(|v| v as f32);
    if let Some(hw) = first_hwmon(&device) {
        gpu.temp = read_num(&hw.join("temp1_input")).map(|v| (v / 1000.0) as f32);
    }
    gpu
}

/// The fields asked of `nvidia-smi`, in order.
pub const NVIDIA_QUERY: &str =
    "index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,clocks.gr,clocks.max.gr,fan.speed";

/// Parse `nvidia-smi --query-gpu=NVIDIA_QUERY --format=csv,noheader,nounits`.
/// Unsupported values ("[N/A]", "[Not Supported]") become None.
pub fn parse_nvidia(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            if f.len() < 10 {
                return None;
            }
            let num = |i: usize| f[i].parse::<f64>().ok();
            let mut gpu = Gpu::new(format!("nvidia{}", f[0]), f[1].to_string(), Vendor::Nvidia);
            gpu.busy = num(2).map(|v| v as f32);
            gpu.vram_used = num(3).map(|mib| (mib * 1024.0 * 1024.0) as u64);
            gpu.vram_total = num(4).map(|mib| (mib * 1024.0 * 1024.0) as u64);
            gpu.temp = num(5).map(|v| v as f32);
            gpu.power = num(6).map(|v| v as f32);
            gpu.clock_mhz = num(7).map(|v| v as f32);
            gpu.max_clock_mhz = num(8).map(|v| v as f32);
            gpu.fan_percent = num(9).map(|v| v as f32);
            Some(gpu)
        })
        .collect()
}

/// Run `nvidia-smi` (or read the fixture named by `M3_MONITOR_NVIDIA_SMI`).
pub fn query_nvidia() -> Option<Vec<Gpu>> {
    let text = match std::env::var_os("M3_MONITOR_NVIDIA_SMI") {
        Some(path) => std::fs::read_to_string(path).ok()?,
        None => {
            let query = format!("--query-gpu={NVIDIA_QUERY}");
            material_3_ui::sys::cmd::run("nvidia-smi", &[&query, "--format=csv,noheader,nounits"]).ok()?
        }
    };
    Some(parse_nvidia(&text))
}

/// Whether NVIDIA should be polled at all.
pub fn has_nvidia(sys: &Path) -> bool {
    std::env::var_os("M3_MONITOR_NVIDIA_SMI").is_some()
        || (cards(sys).iter().any(|(_, _, v)| *v == Vendor::Nvidia) && material_3_ui::sys::cmd::exists("nvidia-smi"))
}

/// AMD and Intel GPUs read from sysfs (NVIDIA comes from [`query_nvidia`]).
pub fn sysfs_gpus(sys: &Path) -> Vec<Gpu> {
    cards(sys)
        .into_iter()
        .filter_map(|(key, dir, vendor)| match vendor {
            Vendor::Amd => Some(amd(&key, &dir)),
            Vendor::Intel => Some(intel(&key, &dir)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dpm_table() {
        let t = "0: 500Mhz\n1: 1800Mhz *\n2: 2700Mhz\n";
        assert_eq!(parse_dpm(t), (Some(1800.0), Some(2700.0)));
        assert_eq!(parse_dpm(""), (None, None));
    }

    #[test]
    fn nvidia_csv() {
        let text = "0, NVIDIA GeForce RTX 4070, 37, 2048, 12282, 58, 112.45, 2475, 3105, 41\n\
                    1, NVIDIA GeForce GTX 1050, 0, 5, 4096, 35, [N/A], 139, 1911, [Not Supported]\n";
        let gpus = parse_nvidia(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].key, "nvidia0");
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(gpus[0].busy, Some(37.0));
        assert_eq!(gpus[0].vram_total, Some(12282 * 1024 * 1024));
        assert_eq!(gpus[0].power, Some(112.45));
        assert_eq!(gpus[0].fan_percent, Some(41.0));
        assert_eq!(gpus[1].power, None);
        assert_eq!(gpus[1].fan_percent, None);
        assert!(parse_nvidia("garbage\n").is_empty());
    }

    #[test]
    fn amd_sysfs() {
        let root = tempfile::tempdir().unwrap();
        let dev = root.path().join("class/drm/card1/device");
        let hw = dev.join("hwmon/hwmon5");
        fs::create_dir_all(&hw).unwrap();
        fs::create_dir_all(root.path().join("class/drm/card1-DP-1")).unwrap();
        fs::write(dev.join("vendor"), "0x1002\n").unwrap();
        fs::write(dev.join("device"), "0x15bf\n").unwrap();
        fs::write(dev.join("gpu_busy_percent"), "23\n").unwrap();
        fs::write(dev.join("mem_info_vram_used"), "536870912\n").unwrap();
        fs::write(dev.join("mem_info_vram_total"), "2147483648\n").unwrap();
        fs::write(dev.join("pp_dpm_sclk"), "0: 800Mhz\n1: 2700Mhz *\n").unwrap();
        fs::write(hw.join("temp1_input"), "52000\n").unwrap();
        fs::write(hw.join("power1_input"), "15000000\n").unwrap();

        let gpus = sysfs_gpus(root.path());
        assert_eq!(gpus.len(), 1, "connectors aren't cards");
        let g = &gpus[0];
        assert_eq!(g.name, "AMD Radeon (15bf)");
        assert_eq!(g.busy, Some(23.0));
        assert_eq!(g.vram_used, Some(512 * 1024 * 1024));
        assert_eq!(g.clock_mhz, Some(2700.0));
        assert_eq!(g.temp, Some(52.0));
        assert_eq!(g.power, Some(15.0));
    }

    #[test]
    fn intel_sysfs() {
        let root = tempfile::tempdir().unwrap();
        let card = root.path().join("class/drm/card0");
        fs::create_dir_all(card.join("device")).unwrap();
        fs::write(card.join("device/vendor"), "0x8086\n").unwrap();
        fs::write(card.join("gt_cur_freq_mhz"), "350\n").unwrap();
        fs::write(card.join("gt_max_freq_mhz"), "1300\n").unwrap();
        let g = &sysfs_gpus(root.path())[0];
        assert_eq!(g.vendor, Vendor::Intel);
        assert_eq!((g.clock_mhz, g.max_clock_mhz), (Some(350.0), Some(1300.0)));
        assert_eq!(g.busy, None);
    }
}
