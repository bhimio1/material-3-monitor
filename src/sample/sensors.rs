//! Temperatures and fans from `/sys/class/hwmon`, batteries and AC from
//! `/sys/class/power_supply`.

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Temp {
    /// The chip ("k10temp", "amdgpu", "nvme").
    pub chip: String,
    /// The sensor's label ("Tctl", "edge", "Composite"), or tempN.
    pub label: String,
    pub celsius: f32,
    /// The critical (else max) limit, when the driver gives one.
    pub critical: Option<f32>,
}

impl Temp {
    pub fn display_name(&self) -> String {
        let chip = chip_name(&self.chip);
        if self.label.starts_with("temp") {
            chip.to_string()
        } else {
            format!("{chip} · {}", self.label)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub chip: String,
    pub label: String,
    pub rpm: u32,
}

/// A friendlier name for well-known hwmon drivers.
pub fn chip_name(chip: &str) -> &str {
    match chip {
        "k10temp" | "zenpower" | "coretemp" | "cpu_thermal" | "x86_pkg_temp" => "CPU",
        "amdgpu" | "nouveau" | "radeon" | "i915" | "xe" => "GPU",
        "nvme" => "NVMe",
        "acpitz" => "ACPI",
        "iwlwifi_1" | "iwlwifi" | "mt7921_phy0" | "ath11k_hwmon" => "Wi-Fi",
        "spd5118" | "jc42" => "Memory",
        "thinkpad" => "ThinkPad",
        "asus" | "asus_wmi_sensors" => "ASUS",
        "dell_smm" => "Dell",
        "BAT0" | "BAT1" => "Battery",
        other => other,
    }
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_num(path: &Path) -> Option<f64> {
    read_trim(path)?.parse().ok()
}

/// Every temperature and fan under `<sys>/class/hwmon`.
pub fn hwmon(sys: &Path) -> (Vec<Temp>, Vec<Fan>) {
    let mut temps = Vec::new();
    let mut fans = Vec::new();
    let Ok(entries) = std::fs::read_dir(sys.join("class/hwmon")) else {
        return (temps, fans);
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort_by_key(|d| {
        let n = d.file_name().unwrap_or_default().to_string_lossy().into_owned();
        n.trim_start_matches("hwmon").parse::<u32>().unwrap_or(u32::MAX)
    });
    for dir in dirs {
        let chip = read_trim(&dir.join("name")).unwrap_or_else(|| "hwmon".into());
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        let mut names: Vec<String> = files.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
        names.sort_by_key(|n| (n.len(), n.clone()));
        for name in names {
            if let Some(idx) = name.strip_prefix("temp").and_then(|n| n.strip_suffix("_input")) {
                let Some(milli) = read_num(&dir.join(&name)) else { continue };
                let celsius = (milli / 1000.0) as f32;
                // Disconnected probes read -127 or absurd values.
                if !(-40.0..=150.0).contains(&celsius) {
                    continue;
                }
                let label = read_trim(&dir.join(format!("temp{idx}_label"))).unwrap_or_else(|| format!("temp{idx}"));
                let critical = read_num(&dir.join(format!("temp{idx}_crit")))
                    .or_else(|| read_num(&dir.join(format!("temp{idx}_max"))))
                    .map(|v| (v / 1000.0) as f32)
                    .filter(|v| *v > 0.0 && *v < 200.0);
                temps.push(Temp { chip: chip.clone(), label, celsius, critical });
            } else if let Some(idx) = name.strip_prefix("fan").and_then(|n| n.strip_suffix("_input")) {
                let Some(rpm) = read_num(&dir.join(&name)) else { continue };
                let label = read_trim(&dir.join(format!("fan{idx}_label"))).unwrap_or_else(|| format!("Fan {idx}"));
                fans.push(Fan { chip: chip.clone(), label, rpm: rpm as u32 });
            }
        }
    }
    (temps, fans)
}

/// The temperature that best represents the CPU package.
pub fn cpu_temp(temps: &[Temp]) -> Option<f32> {
    const PREFERRED: [(&str, &str); 6] = [
        ("k10temp", "Tctl"),
        ("k10temp", "Tdie"),
        ("zenpower", "Tdie"),
        ("coretemp", "Package id 0"),
        ("x86_pkg_temp", ""),
        ("cpu_thermal", ""),
    ];
    PREFERRED
        .iter()
        .find_map(|(chip, label)| {
            temps.iter().find(|t| t.chip == *chip && (label.is_empty() || t.label == *label))
        })
        .or_else(|| temps.iter().find(|t| chip_name(&t.chip) == "CPU"))
        .map(|t| t.celsius)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub name: String,
    pub model: String,
    pub percent: f32,
    /// "Charging", "Discharging", "Full", "Not charging".
    pub status: String,
    /// Watts going in or out, when known.
    pub power: Option<f32>,
    /// Full capacity as a fraction of the design capacity.
    pub health: Option<f32>,
    pub cycles: Option<u32>,
    /// Hours to empty (discharging) or to full (charging).
    pub hours_left: Option<f32>,
}

/// Batteries and whether AC power is connected.
pub fn power_supplies(sys: &Path) -> (Vec<Battery>, Option<bool>) {
    let mut batteries = Vec::new();
    let mut ac = None;
    let Ok(entries) = std::fs::read_dir(sys.join("class/power_supply")) else {
        return (batteries, ac);
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let kind = read_trim(&dir.join("type")).unwrap_or_default();
        if kind == "Mains" {
            ac = Some(ac.unwrap_or(false) || read_trim(&dir.join("online")).as_deref() == Some("1"));
            continue;
        }
        // Mouse and controller batteries have scope=Device.
        if kind != "Battery" || read_trim(&dir.join("scope")).as_deref() == Some("Device") {
            continue;
        }
        let num = |f: &str| read_num(&dir.join(f));
        // energy_* (µWh, µW) or charge_* (µAh, µA) with voltage (µV).
        let volts = num("voltage_now").map(|v| v / 1e6);
        let (now, full, design, rate) = if num("energy_full").is_some() {
            (num("energy_now"), num("energy_full"), num("energy_full_design"), num("power_now"))
        } else {
            let v = volts.unwrap_or(0.0);
            (
                num("charge_now").map(|c| c * v),
                num("charge_full").map(|c| c * v),
                num("charge_full_design").map(|c| c * v),
                num("current_now").map(|c| c * v),
            )
        };
        let status = read_trim(&dir.join("status")).unwrap_or_else(|| "Unknown".into());
        let percent = num("capacity")
            .or_else(|| Some(now? / full? * 100.0))
            .unwrap_or(0.0) as f32;
        let power = rate.map(|r| (r / 1e6) as f32).filter(|w| *w > 0.0);
        let hours_left = match (status.as_str(), now, full, rate) {
            ("Discharging", Some(now), _, Some(rate)) if rate > 0.0 => Some((now / rate) as f32),
            ("Charging", Some(now), Some(full), Some(rate)) if rate > 0.0 => Some(((full - now).max(0.0) / rate) as f32),
            _ => None,
        };
        batteries.push(Battery {
            name: dir.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            model: read_trim(&dir.join("model_name")).unwrap_or_default(),
            percent,
            status,
            power,
            health: match (full, design) {
                (Some(f), Some(d)) if d > 0.0 => Some((f / d) as f32),
                _ => None,
            },
            cycles: num("cycle_count").map(|c| c as u32).filter(|c| *c > 0),
            hours_left,
        });
    }
    (batteries, ac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, value: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), value).unwrap();
    }

    #[test]
    fn hwmon_tree() {
        let root = tempfile::tempdir().unwrap();
        let cpu = root.path().join("class/hwmon/hwmon2");
        write(&cpu, "name", "k10temp\n");
        write(&cpu, "temp1_input", "61250\n");
        write(&cpu, "temp1_label", "Tctl\n");
        write(&cpu, "temp3_input", "55000\n");
        write(&cpu, "temp3_label", "Tccd1\n");
        let gpu = root.path().join("class/hwmon/hwmon10");
        write(&gpu, "name", "amdgpu\n");
        write(&gpu, "temp1_input", "48000\n");
        write(&gpu, "temp1_label", "edge\n");
        write(&gpu, "temp1_crit", "100000\n");
        write(&gpu, "fan1_input", "1450\n");
        let acpi = root.path().join("class/hwmon/hwmon0");
        write(&acpi, "name", "acpitz\n");
        write(&acpi, "temp1_input", "-127000\n");

        let (temps, fans) = hwmon(root.path());
        assert_eq!(temps.len(), 3, "the disconnected probe is skipped: {temps:?}");
        assert_eq!(temps[0].label, "Tctl", "hwmon2 sorts before hwmon10");
        assert_eq!(temps[2].critical, Some(100.0));
        assert_eq!(temps[2].display_name(), "GPU · edge");
        assert_eq!(cpu_temp(&temps), Some(61.25));
        assert_eq!(fans, vec![Fan { chip: "amdgpu".into(), label: "Fan 1".into(), rpm: 1450 }]);
    }

    #[test]
    fn coretemp_package() {
        let t = |chip: &str, label: &str, c: f32| Temp { chip: chip.into(), label: label.into(), celsius: c, critical: None };
        let temps = vec![t("coretemp", "Core 0", 50.0), t("coretemp", "Package id 0", 57.0)];
        assert_eq!(cpu_temp(&temps), Some(57.0));
        assert_eq!(cpu_temp(&[t("nvme", "Composite", 40.0)]), None);
    }

    #[test]
    fn energy_battery() {
        let root = tempfile::tempdir().unwrap();
        let bat = root.path().join("class/power_supply/BAT0");
        write(&bat, "type", "Battery\n");
        write(&bat, "status", "Discharging\n");
        write(&bat, "capacity", "80\n");
        write(&bat, "energy_now", "40000000\n");
        write(&bat, "energy_full", "50000000\n");
        write(&bat, "energy_full_design", "55000000\n");
        write(&bat, "power_now", "10000000\n");
        write(&bat, "cycle_count", "120\n");
        write(&bat, "model_name", "5B10W51867\n");
        let ac = root.path().join("class/power_supply/AC");
        write(&ac, "type", "Mains\n");
        write(&ac, "online", "0\n");
        let mouse = root.path().join("class/power_supply/hidpp_battery_0");
        write(&mouse, "type", "Battery\n");
        write(&mouse, "scope", "Device\n");

        let (bats, ac) = power_supplies(root.path());
        assert_eq!(ac, Some(false));
        assert_eq!(bats.len(), 1);
        let b = &bats[0];
        assert_eq!(b.percent, 80.0);
        assert_eq!(b.power, Some(10.0));
        assert_eq!(b.hours_left, Some(4.0));
        assert!((b.health.unwrap() - 0.909).abs() < 0.01);
        assert_eq!(b.cycles, Some(120));
    }

    #[test]
    fn charge_battery() {
        let root = tempfile::tempdir().unwrap();
        let bat = root.path().join("class/power_supply/BAT1");
        write(&bat, "type", "Battery\n");
        write(&bat, "status", "Charging\n");
        write(&bat, "charge_now", "2000000\n");
        write(&bat, "charge_full", "4000000\n");
        write(&bat, "current_now", "1000000\n");
        write(&bat, "voltage_now", "12000000\n");
        let (bats, _) = power_supplies(root.path());
        assert_eq!(bats[0].percent, 50.0);
        assert_eq!(bats[0].power, Some(12.0));
        assert_eq!(bats[0].hours_left, Some(2.0));
    }
}
