//! Memory: `/proc/meminfo`, zram devices and pressure stall information.

use std::path::Path;

/// Sizes in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub free: u64,
    pub buffers: u64,
    pub cached: u64,
    pub shared: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub dirty: u64,
}

impl MemInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut m = MemInfo::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else { continue };
        let mut parts = rest.split_whitespace();
        let Some(value) = parts.next().and_then(|v| v.parse::<u64>().ok()) else { continue };
        let bytes = if parts.next() == Some("kB") { value * 1024 } else { value };
        let slot = match key {
            "MemTotal" => &mut m.total,
            "MemAvailable" => &mut m.available,
            "MemFree" => &mut m.free,
            "Buffers" => &mut m.buffers,
            "Cached" => &mut m.cached,
            "Shmem" => &mut m.shared,
            "SwapTotal" => &mut m.swap_total,
            "SwapFree" => &mut m.swap_free,
            "Dirty" => &mut m.dirty,
            _ => continue,
        };
        *slot = bytes;
    }
    // Kernels before 3.14 have no MemAvailable.
    if m.available == 0 && m.total > 0 {
        m.available = m.free + m.buffers + m.cached;
    }
    m
}

/// One zram device: what's stored in it and how much RAM that takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zram {
    pub name: String,
    pub disk_size: u64,
    /// Uncompressed size of the data stored.
    pub original: u64,
    /// Compressed size.
    pub compressed: u64,
    /// RAM used, including allocator overhead.
    pub memory_used: u64,
    pub algorithm: String,
}

impl Zram {
    pub fn ratio(&self) -> f32 {
        if self.compressed == 0 {
            0.0
        } else {
            self.original as f32 / self.compressed as f32
        }
    }
}

/// `mm_stat`: orig_data_size compr_data_size mem_used_total …
pub fn parse_mm_stat(text: &str) -> Option<(u64, u64, u64)> {
    let n: Vec<u64> = text.split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
    (n.len() == 3).then(|| (n[0], n[1], n[2]))
}

/// "lzo [lz4] zstd" → "lz4".
pub fn parse_algorithm(text: &str) -> String {
    text.split_whitespace()
        .find_map(|a| a.strip_prefix('[').and_then(|a| a.strip_suffix(']')))
        .unwrap_or_else(|| text.trim())
        .to_string()
}

pub fn zram_devices(sys: &Path) -> Vec<Zram> {
    let Ok(entries) = std::fs::read_dir(sys.join("block")) else {
        return Vec::new();
    };
    let mut out: Vec<Zram> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("zram") {
                return None;
            }
            let dir = e.path();
            let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
            let disk_size = read("disksize").trim().parse().unwrap_or(0);
            if disk_size == 0 {
                return None;
            }
            let (original, compressed, memory_used) = parse_mm_stat(&read("mm_stat")).unwrap_or_default();
            Some(Zram {
                name,
                disk_size,
                original,
                compressed,
                memory_used,
                algorithm: parse_algorithm(&read("comp_algorithm")),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// "some" average over 10 seconds, in percent, for cpu, memory and io.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pressure {
    pub cpu: Option<f32>,
    pub memory: Option<f32>,
    pub io: Option<f32>,
}

/// The `some avg10=` value of a `/proc/pressure/*` file.
pub fn parse_pressure(text: &str) -> Option<f32> {
    let line = text.lines().find(|l| l.starts_with("some"))?;
    line.split_whitespace()
        .find_map(|f| f.strip_prefix("avg10="))
        .and_then(|v| v.parse().ok())
}

pub fn pressure(proc: &Path) -> Pressure {
    let read = |f: &str| std::fs::read_to_string(proc.join("pressure").join(f)).ok().and_then(|t| parse_pressure(&t));
    Pressure { cpu: read("cpu"), memory: read("memory"), io: read("io") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo() {
        let m = parse_meminfo(
            "MemTotal:       16000000 kB\nMemFree:         2000000 kB\nMemAvailable:    8000000 kB\n\
             Buffers:          100000 kB\nCached:          5000000 kB\nShmem:            300000 kB\n\
             SwapTotal:       4000000 kB\nSwapFree:        3000000 kB\nHugePages_Total:       0\n",
        );
        assert_eq!(m.total, 16_000_000 * 1024);
        assert_eq!(m.used(), 8_000_000 * 1024);
        assert_eq!(m.swap_used(), 1_000_000 * 1024);
        assert_eq!(m.shared, 300_000 * 1024);
    }

    #[test]
    fn meminfo_without_available() {
        let m = parse_meminfo("MemTotal: 100 kB\nMemFree: 10 kB\nBuffers: 5 kB\nCached: 20 kB\n");
        assert_eq!(m.available, 35 * 1024);
    }

    #[test]
    fn zram_files() {
        assert_eq!(parse_mm_stat("4096000 1024000 1200000 0 1500000 10 0 5 0\n"), Some((4096000, 1024000, 1200000)));
        assert_eq!(parse_mm_stat("12"), None);
        assert_eq!(parse_algorithm("lzo lzo-rle lz4 [zstd]\n"), "zstd");
        assert_eq!(parse_algorithm("zstd\n"), "zstd");

        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("block/zram0");
        std::fs::create_dir_all(&z).unwrap();
        std::fs::create_dir_all(dir.path().join("block/zram1")).unwrap();
        std::fs::write(z.join("disksize"), "8589934592\n").unwrap();
        std::fs::write(z.join("mm_stat"), "400 100 120 0 0 0 0 0 0\n").unwrap();
        std::fs::write(z.join("comp_algorithm"), "lz4 [zstd]\n").unwrap();
        let devs = zram_devices(dir.path());
        assert_eq!(devs.len(), 1, "unconfigured zram1 is skipped");
        assert_eq!(devs[0].algorithm, "zstd");
        assert_eq!(devs[0].ratio(), 4.0);
    }

    #[test]
    fn pressure_avg10() {
        let text = "some avg10=1.25 avg60=0.50 avg300=0.10 total=123\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(parse_pressure(text), Some(1.25));
        assert_eq!(parse_pressure(""), None);
    }
}
