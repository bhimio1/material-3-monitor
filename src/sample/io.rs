//! Disk and network counters: `/proc/diskstats` and `/proc/net/dev`.

use std::collections::HashMap;
use std::path::Path;

/// Cumulative bytes moved by one block device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskCounters {
    pub read: u64,
    pub written: u64,
    /// Milliseconds spent doing I/O (for the busy percentage).
    pub io_ms: u64,
}

/// Whole disks only: no partitions, loop, RAM or zram devices.
pub fn is_whole_disk(name: &str) -> bool {
    if ["loop", "ram", "zram", "sr", "fd", "dm-", "md"].iter().any(|p| name.starts_with(p)) {
        return false;
    }
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        // nvme0n1 / mmcblk0 are disks; nvme0n1p2 / mmcblk0p1 are partitions.
        let after = name.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        return !after.contains('p');
    }
    // sda, vda, xvda, hda: partitions end in a digit.
    !name.ends_with(|c: char| c.is_ascii_digit())
}

pub fn parse_diskstats(text: &str) -> HashMap<String, DiskCounters> {
    const SECTOR: u64 = 512;
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let name = *f.get(2)?;
            if !is_whole_disk(name) {
                return None;
            }
            let n = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            Some((
                name.to_string(),
                DiskCounters { read: n(5) * SECTOR, written: n(9) * SECTOR, io_ms: n(12) },
            ))
        })
        .collect()
}

/// The model of a block device, from sysfs ("Samsung SSD 980 PRO 1TB").
pub fn disk_model(sys: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(sys.join("block").join(name).join("device/model"))
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// Cumulative bytes through one network interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetCounters {
    pub rx: u64,
    pub tx: u64,
}

pub fn parse_net_dev(text: &str) -> Vec<(String, NetCounters)> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name.contains('|') || name == "lo" {
                return None;
            }
            let f: Vec<u64> = rest.split_whitespace().map(|v| v.parse().unwrap_or(0)).collect();
            Some((name.to_string(), NetCounters { rx: *f.first()?, tx: *f.get(8)? }))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    Wired,
    Wireless,
    Virtual,
}

pub fn net_kind(sys: &Path, name: &str) -> NetKind {
    let dir = sys.join("class/net").join(name);
    if dir.join("wireless").exists() || dir.join("phy80211").exists() {
        NetKind::Wireless
    } else if std::fs::read_link(&dir).is_ok_and(|l| l.to_string_lossy().contains("/virtual/"))
        || sys.join("devices/virtual/net").join(name).exists()
    {
        NetKind::Virtual
    } else {
        NetKind::Wired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_disks() {
        for d in ["sda", "vda", "nvme0n1", "nvme1n1", "mmcblk0", "xvdb"] {
            assert!(is_whole_disk(d), "{d}");
        }
        for p in ["sda1", "nvme0n1p2", "mmcblk0p1", "loop0", "zram0", "dm-0", "sr0", "ram3"] {
            assert!(!is_whole_disk(p), "{p}");
        }
    }

    #[test]
    fn diskstats() {
        let text = " 259       0 nvme0n1 1000 10 2048 300 500 20 4096 200 0 400 500 0 0 0 0
 259       1 nvme0n1p1 10 0 20 3 0 0 0 0 0 3 3 0 0 0 0
   7       0 loop0 5 0 10 1 0 0 0 0 0 1 1 0 0 0 0
";
        let d = parse_diskstats(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d["nvme0n1"], DiskCounters { read: 2048 * 512, written: 4096 * 512, io_ms: 400 });
    }

    #[test]
    fn net_dev() {
        let text = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  5000      50    0    0    0     0          0         0     5000      50    0    0    0     0       0          0
wlan0: 123456     100    0    0    0     0          0         0    65432      80    0    0    0     0       0          0
";
        let n = parse_net_dev(text);
        assert_eq!(n, vec![("wlan0".to_string(), NetCounters { rx: 123456, tx: 65432 })]);
    }

    #[test]
    fn interface_kinds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("class/net/wlan0/wireless")).unwrap();
        std::fs::create_dir_all(dir.path().join("class/net/enp3s0")).unwrap();
        std::fs::create_dir_all(dir.path().join("devices/virtual/net/docker0")).unwrap();
        assert_eq!(net_kind(dir.path(), "wlan0"), NetKind::Wireless);
        assert_eq!(net_kind(dir.path(), "enp3s0"), NetKind::Wired);
        assert_eq!(net_kind(dir.path(), "docker0"), NetKind::Virtual);
    }
}
