//! Graphics cards seen by RustNVR (Linux: `/sys/class/drm`,
//! `/proc/driver/nvidia`), whether this process (the container under Docker)
//! may use them, and how busy they are.
//!
//! RustNVR computes nothing on them itself: they are shown so that one knows
//! whether a GPU reaches the container, and how busy the card is (all its
//! users, CodeProject.AI included). Busy figures: AMD `gpu_busy_percent`;
//! Intel, the share of time out of its idle state (RC6); NVIDIA, unknown
//! (would need NVML).

use crate::dto::GpuDto;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the system files are: `/sys`, `/dev`, `/proc` (a fake tree in tests).
#[derive(Clone, Debug)]
pub(crate) struct Roots {
    pub sys: PathBuf,
    pub dev: PathBuf,
    pub proc: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            sys: "/sys".into(),
            dev: "/dev".into(),
            proc: "/proc".into(),
        }
    }
}

/// Files listing the names of PCI devices (Debian / Ubuntu `pci.ids` package).
const PCI_IDS: [&str; 3] = [
    "/usr/share/misc/pci.ids",
    "/usr/share/hwdata/pci.ids",
    "/usr/share/pci.ids",
];

/// Vendors whose cards can compute (the others only display: virtual VM
/// adapters, server management chips…).
fn vendor(id: u16) -> Option<(&'static str, &'static str)> {
    match id {
        0x8086 => Some(("intel", "Intel")),
        0x10de => Some(("nvidia", "NVIDIA")),
        0x1002 => Some(("amd", "AMD")),
        _ => None,
    }
}

/// Name of the usual display-only adapters, without `pci.ids`.
fn display_vendor(id: u16) -> &'static str {
    match id {
        0x1af4 => "Virtio (VM)",
        0x1234 => "QEMU (VM)",
        0x1b36 => "QXL (VM)",
        0x15ad => "VMware (VM)",
        0x80ee => "VirtualBox (VM)",
        0x1414 => "Hyper-V (VM)",
        0x1a03 => "ASPEED",
        0x102b => "Matrox",
        _ => "Adaptateur graphique",
    }
}

/// One card found, before its busy figure.
#[derive(Clone, Debug, PartialEq)]
struct Found {
    id: String,
    pci: Option<String>,
    vendor_id: u16,
    device_id: u16,
    driver: Option<String>,
    /// `/dev/dri/renderD…` or `/dev/nvidia…`, as seen from this process.
    node: Option<PathBuf>,
    /// Model name given by the NVIDIA driver.
    model: Option<String>,
    /// `/sys/class/drm/cardN` (busy figures).
    sys: Option<PathBuf>,
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

fn read_hex(p: &Path) -> Option<u16> {
    let s = read_trim(p)?;
    u16::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn link_name(p: &Path) -> Option<String> {
    std::fs::read_link(p)
        .ok()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Cards of `/sys/class/drm` (`cardN`, not their connectors), then NVIDIA
/// cards known only to the NVIDIA driver.
fn discover(roots: &Roots) -> Vec<Found> {
    let mut found = Vec::new();
    let drm = roots.sys.join("class/drm");
    let mut cards: Vec<(u32, PathBuf)> = std::fs::read_dir(&drm)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix("card")?.parse::<u32>().ok()?;
            Some((n, e.path()))
        })
        .collect();
    cards.sort();
    for (n, card) in cards {
        let device = card.join("device");
        let (Some(vendor_id), Some(device_id)) = (
            read_hex(&device.join("vendor")),
            read_hex(&device.join("device")),
        ) else {
            continue;
        };
        let driver = link_name(&device.join("driver"));
        let render = std::fs::read_dir(device.join("drm"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("renderD"));
        let node = if vendor_id == 0x10de && driver.as_deref() == Some("nvidia") {
            Some(roots.dev.join(format!("nvidia{n}")))
        } else {
            render.map(|r| roots.dev.join("dri").join(r))
        };
        found.push(Found {
            id: format!("card{n}"),
            pci: link_name(&device),
            vendor_id,
            device_id,
            driver,
            node,
            model: None,
            sys: Some(card),
        });
    }
    // NVIDIA: the driver lists its cards (model name included), even without
    // a DRM device.
    let gpus = roots.proc.join("driver/nvidia/gpus");
    let mut nvidia: Vec<PathBuf> = std::fs::read_dir(&gpus)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    nvidia.sort();
    for (i, dir) in nvidia.iter().enumerate() {
        let info = std::fs::read_to_string(dir.join("information")).unwrap_or_default();
        let field = |key: &str| {
            info.lines()
                .find_map(|l| l.strip_prefix(key))
                .map(|v| v.trim().to_owned())
        };
        let pci = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase());
        let minor = field("Device Minor:").and_then(|m| m.parse::<u32>().ok());
        let node = roots
            .dev
            .join(format!("nvidia{}", minor.unwrap_or(i as u32)));
        let model = field("Model:");
        if let Some(existing) = found
            .iter_mut()
            .find(|f| f.pci.is_some() && f.pci.as_deref().map(str::to_ascii_lowercase) == pci)
        {
            existing.model = model;
            existing.node = Some(node);
            continue;
        }
        found.push(Found {
            id: format!("nvidia{}", minor.unwrap_or(i as u32)),
            pci,
            vendor_id: 0x10de,
            device_id: 0,
            driver: Some("nvidia".into()),
            node: Some(node),
            model,
            sys: None,
        });
    }
    found
}

/// Whether this process may open `node` for reading and writing.
fn can_open(node: &Path) -> bool {
    #[cfg(unix)]
    {
        use rustix::fs::{Access, access};
        access(node, Access::READ_OK | Access::WRITE_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        node.exists()
    }
}

/// `usable`, `not_passed` (the card exists but its device is not in this
/// container), `no_permission`, or `display_only` (cannot compute).
fn status(f: &Found) -> &'static str {
    if vendor(f.vendor_id).is_none() {
        return "display_only";
    }
    match &f.node {
        Some(node) if node.exists() => {
            if can_open(node) {
                "usable"
            } else {
                "no_permission"
            }
        }
        _ => "not_passed",
    }
}

/// Names from a `pci.ids` file: vendor name, and device name if listed.
fn pci_names(ids: &str, vendor_id: u16, device_id: u16) -> (Option<String>, Option<String>) {
    let vendor_key = format!("{vendor_id:04x}  ");
    let device_key = format!("\t{device_id:04x}  ");
    let mut vendor = None;
    for line in ids.lines() {
        if vendor.is_none() {
            if let Some(name) = line.strip_prefix(&vendor_key) {
                vendor = Some(name.trim().to_owned());
            }
            continue;
        }
        // Next vendor: the device is not listed.
        if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            break;
        }
        if let Some(name) = line.strip_prefix(&device_key) {
            return (vendor, Some(name.trim().to_owned()));
        }
    }
    (vendor, None)
}

/// Sum and number of the idle-time counters (ms) found in `files`.
fn sum_counters(files: impl Iterator<Item = PathBuf>) -> Option<(u64, u32)> {
    let values: Vec<u64> = files
        .filter_map(|p| read_trim(&p).and_then(|s| s.parse::<u64>().ok()))
        .collect();
    (!values.is_empty()).then(|| (values.iter().sum(), values.len() as u32))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect()
}

/// Idle time of an Intel card: total ms and number of counters.
fn idle_ms(sys: &Path) -> Option<(u64, u32)> {
    // i915: one RC6 counter per GT; older kernels: a single one.
    sum_counters(
        subdirs(&sys.join("gt"))
            .into_iter()
            .map(|gt| gt.join("rc6_residency_ms")),
    )
    .or_else(|| sum_counters(std::iter::once(sys.join("power/rc6_residency_ms"))))
    // xe: idle time of each GT of each tile.
    .or_else(|| {
        sum_counters(
            subdirs(&sys.join("device"))
                .into_iter()
                .filter(|t| {
                    t.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("tile"))
                })
                .flat_map(|tile| subdirs(&tile))
                .map(|gt| gt.join("gtidle/idle_residency_ms")),
        )
    })
}

/// Discovers the cards and measures how busy they are, at each call.
#[derive(Default)]
pub(crate) struct GpuSampler {
    roots: Roots,
    /// Previous idle counters per card: when, total idle ms, counters.
    idle: HashMap<String, (Instant, u64, u32)>,
    /// Names already looked up in `pci.ids`.
    names: HashMap<(u16, u16), String>,
    /// Contents of `pci.ids`, read once when a name is needed.
    pci_ids: Option<Option<String>>,
}

impl GpuSampler {
    #[cfg(test)]
    fn with_roots(roots: Roots) -> Self {
        Self {
            roots,
            pci_ids: Some(None),
            ..Self::default()
        }
    }

    fn name(&mut self, f: &Found) -> String {
        if let Some(model) = &f.model {
            return model.clone();
        }
        if let Some(n) = self.names.get(&(f.vendor_id, f.device_id)) {
            return n.clone();
        }
        let ids = self
            .pci_ids
            .get_or_insert_with(|| PCI_IDS.iter().find_map(|p| std::fs::read_to_string(p).ok()));
        let (pci_vendor, pci_device) = match ids {
            Some(ids) => pci_names(ids, f.vendor_id, f.device_id),
            None => (None, None),
        };
        let vendor_name = vendor(f.vendor_id)
            .map(|(_, n)| n.to_owned())
            .or(pci_vendor)
            .unwrap_or_else(|| display_vendor(f.vendor_id).to_owned());
        let name = match pci_device {
            Some(device) => format!("{vendor_name} {device}"),
            None => format!("{vendor_name} ({:04x}:{:04x})", f.vendor_id, f.device_id),
        };
        self.names.insert((f.vendor_id, f.device_id), name.clone());
        name
    }

    /// Cards now, with how busy each was since the previous call.
    pub(crate) fn sample(&mut self) -> Vec<GpuDto> {
        let now = Instant::now();
        let found = discover(&self.roots);
        let mut out = Vec::with_capacity(found.len());
        let mut idle = HashMap::new();
        for f in found {
            let busy_percent = f.sys.as_deref().and_then(|sys| {
                if f.vendor_id == 0x1002 {
                    return read_trim(&sys.join("device/gpu_busy_percent"))
                        .and_then(|s| s.parse::<f32>().ok());
                }
                if f.vendor_id != 0x8086 {
                    return None;
                }
                let (total, count) = idle_ms(sys)?;
                idle.insert(f.id.clone(), (now, total, count));
                let (then, before, before_count) = self.idle.get(&f.id)?;
                let wall = now.duration_since(*then).as_secs_f64() * 1000.0;
                if *before_count != count || wall < 100.0 {
                    return None;
                }
                let idle_share = total.saturating_sub(*before) as f64 / (wall * f64::from(count));
                Some(((1.0 - idle_share) * 100.0).clamp(0.0, 100.0) as f32)
            });
            out.push(GpuDto {
                id: f.id.clone(),
                name: self.name(&f),
                vendor: vendor(f.vendor_id).map_or("other", |(v, _)| v),
                driver: f.driver.clone(),
                device: f.node.as_ref().map(|n| {
                    // Shown as in the container, whatever the roots.
                    let rel = n.strip_prefix(&self.roots.dev).unwrap_or(n);
                    format!("/dev/{}", rel.display())
                }),
                status: status(&f),
                busy_percent,
            });
        }
        self.idle = idle;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        fs::write(path, text).expect("write");
    }

    /// A fake `/sys` + `/dev` + `/proc`: an Intel iGPU, a virtual VM adapter,
    /// an NVIDIA card known to its driver only, and an AMD card.
    fn tree() -> (tempfile::TempDir, Roots) {
        let t = tempfile::tempdir().expect("tmp");
        let r = Roots {
            sys: t.path().join("sys"),
            dev: t.path().join("dev"),
            proc: t.path().join("proc"),
        };
        let pci = r.sys.join("devices/pci0000:00");
        let card =
            |n: u32, addr: &str, vendor: &str, device: &str, driver: &str, render: Option<&str>| {
                let dev = pci.join(addr);
                write(&dev.join("vendor"), &format!("0x{vendor}\n"));
                write(&dev.join("device"), &format!("0x{device}\n"));
                let drivers = r.sys.join("bus/pci/drivers").join(driver);
                fs::create_dir_all(&drivers).expect("driver");
                #[cfg(unix)]
                std::os::unix::fs::symlink(&drivers, dev.join("driver")).expect("link");
                if let Some(render) = render {
                    fs::create_dir_all(dev.join("drm").join(render)).expect("render");
                }
                let c = r.sys.join("class/drm").join(format!("card{n}"));
                fs::create_dir_all(&c).expect("card");
                #[cfg(unix)]
                std::os::unix::fs::symlink(&dev, c.join("device")).expect("link");
                fs::create_dir_all(r.sys.join("class/drm").join(format!("card{n}-HDMI-A-1")))
                    .expect("connector");
                c
            };
        let intel = card(
            0,
            "0000:00:02.0",
            "8086",
            "46d1",
            "i915",
            Some("renderD128"),
        );
        write(&intel.join("gt/gt0/rc6_residency_ms"), "1000\n");
        card(1, "0000:00:01.0", "1234", "1111", "bochs-drm", None);
        let amd = card(
            2,
            "0000:03:00.0",
            "1002",
            "73bf",
            "amdgpu",
            Some("renderD129"),
        );
        write(&amd.join("device/gpu_busy_percent"), "37\n");
        write(&r.dev.join("dri/renderD128"), "");
        write(
            &r.proc.join("driver/nvidia/gpus/0000:01:00.0/information"),
            "Model: \t\t NVIDIA GeForce RTX 3060\nIRQ:   42\nDevice Minor: \t 0\n",
        );
        (t, r)
    }

    #[cfg(unix)]
    #[test]
    fn finds_cards_and_tells_which_reach_this_process() {
        let (_t, roots) = tree();
        let mut s = GpuSampler::with_roots(roots.clone());
        let first = s.sample();
        let ids: Vec<_> = first.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["card0", "card1", "card2", "nvidia0"]);
        let intel = &first[0];
        assert_eq!(
            (intel.vendor, intel.name.as_str(), intel.driver.as_deref()),
            ("intel", "Intel (8086:46d1)", Some("i915"))
        );
        assert_eq!(intel.device.as_deref(), Some("/dev/dri/renderD128"));
        assert_eq!(intel.status, "usable");
        // First sample: no busy figure for Intel yet.
        assert_eq!(intel.busy_percent, None);
        assert_eq!(
            (first[1].vendor, first[1].status),
            ("other", "display_only")
        );
        assert_eq!(first[1].name, "QEMU (VM) (1234:1111)");
        // AMD: its device is not in this "container".
        assert_eq!(
            (first[2].status, first[2].busy_percent),
            ("not_passed", Some(37.0))
        );
        let nvidia = &first[3];
        assert_eq!(
            (
                nvidia.vendor,
                nvidia.name.as_str(),
                nvidia.device.as_deref(),
                nvidia.status
            ),
            (
                "nvidia",
                "NVIDIA GeForce RTX 3060",
                Some("/dev/nvidia0"),
                "not_passed"
            )
        );

        // Intel: busy = time out of RC6 since the previous sample. The
        // previous sample is moved 10 s back, of which 2.5 s idle: 75 %
        // busy, whatever the speed of the machine running the test.
        let previous = &mut s.idle.get_mut("card0").expect("previous sample").0;
        *previous = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(10))
            .expect("10 s ago");
        write(
            &roots.sys.join("class/drm/card0/gt/gt0/rc6_residency_ms"),
            "3500\n",
        );
        let busy = s.sample()[0].busy_percent.expect("busy figure");
        assert!((74.0..=76.0).contains(&busy), "{busy}");
    }

    #[test]
    fn names_cards_from_pci_ids() {
        let ids = "# comment\n8086  Intel Corporation\n\t46a6  Alder Lake-P GT2 [Iris Xe Graphics]\n\t46d1  Alder Lake-N [UHD Graphics]\n\t\t1028 0b1a  sub\n8087  Intel Corporation\n\t46d2  other\n";
        assert_eq!(
            pci_names(ids, 0x8086, 0x46d1),
            (
                Some("Intel Corporation".into()),
                Some("Alder Lake-N [UHD Graphics]".into())
            )
        );
        assert_eq!(
            pci_names(ids, 0x8086, 0x46d2),
            (Some("Intel Corporation".into()), None)
        );
        assert_eq!(pci_names(ids, 0x10de, 0x2504), (None, None));
    }
}
