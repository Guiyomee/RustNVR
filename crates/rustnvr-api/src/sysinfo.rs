//! Process and host resource usage, read from `/proc` (Linux only; `None` elsewhere).

use std::time::{Duration, Instant};

/// Clock ticks per second used by `/proc/<pid>/stat` (`USER_HZ`), which the
/// kernel fixes at 100 for user-space interfaces on mainstream architectures.
const USER_HZ: f64 = 100.0;

/// A reading of this process's resource usage.
#[derive(Clone, Copy, Debug)]
pub struct ProcSample {
    at: Instant,
    cpu_ticks: u64,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
}

impl ProcSample {
    /// Reads the current values.
    pub fn read() -> Option<Self> {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        // Fields after the parenthesized command name, which may contain spaces.
        let rest = stat.rsplit_once(')')?.1;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // utime and stime are fields 14 and 15 of the full line, i.e. 11 and 12 here.
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let rss_kib = kib_field(&status, "VmRSS:")?;
        Some(Self {
            at: Instant::now(),
            cpu_ticks: utime + stime,
            rss_bytes: rss_kib * 1024,
        })
    }

    /// CPU usage between `earlier` and `self`, in percent of one core.
    pub fn cpu_percent_since(&self, earlier: &ProcSample) -> f64 {
        let wall = self
            .at
            .duration_since(earlier.at)
            .max(Duration::from_millis(1));
        let cpu = self.cpu_ticks.saturating_sub(earlier.cpu_ticks) as f64 / USER_HZ;
        100.0 * cpu / wall.as_secs_f64()
    }
}

fn kib_field(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix(key))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()
}

/// What [`GroupSample`] measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageScope {
    /// The whole container (cgroup root of a container): the figures of `docker stats`.
    Container,
    /// The control group of the service (systemd unit), which holds only RustNVR and its children.
    Service,
    /// RustNVR and the programs it started, added up from `/proc`.
    Processes,
}

impl UsageScope {
    /// Name used by the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Service => "service",
            Self::Processes => "processes",
        }
    }
}

/// Resource use of RustNVR together with the programs it starts (FFmpeg).
///
/// Read from the process's cgroup v2 when that group holds nothing else
/// (container or systemd service), the way `docker stats` computes it:
/// memory is `memory.current` minus `inactive_file`, CPU is `usage_usec`.
/// Otherwise the process and its descendants are added up from `/proc`.
#[derive(Clone, Copy, Debug)]
pub struct GroupSample {
    at: Instant,
    cpu_us: u64,
    /// Memory in use in bytes.
    pub memory_bytes: u64,
    /// Memory limit of the cgroup in bytes, when one is set.
    pub memory_limit_bytes: Option<u64>,
    /// What was measured.
    pub scope: UsageScope,
}

impl GroupSample {
    /// Reads the current values.
    pub fn read() -> Option<Self> {
        Self::from_cgroup().or_else(Self::from_process_tree)
    }

    /// CPU usage between `earlier` and `self`, in percent of one core (as `docker stats`).
    pub fn cpu_percent_since(&self, earlier: &GroupSample) -> Option<f64> {
        if self.scope != earlier.scope {
            return None;
        }
        let wall = self
            .at
            .duration_since(earlier.at)
            .max(Duration::from_millis(1));
        let cpu = self.cpu_us.saturating_sub(earlier.cpu_us) as f64 / 1e6;
        Some(100.0 * cpu / wall.as_secs_f64())
    }

    fn from_cgroup() -> Option<Self> {
        let groups = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let path = groups.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
        let dir = std::path::Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok();
        // Absent at the real root of the hierarchy (no container, no service).
        let current: u64 = read("memory.current")?.trim().parse().ok()?;
        let cpu_us = value_field(&read("cpu.stat")?, "usage_usec")?;
        let inactive = read("memory.stat")
            .and_then(|s| value_field(&s, "inactive_file"))
            .unwrap_or(0);
        let limit = read("memory.max").and_then(|s| s.trim().parse().ok());
        if !only_related_processes(&read("cgroup.procs")?) {
            return None;
        }
        Some(Self {
            at: Instant::now(),
            cpu_us,
            memory_bytes: current.saturating_sub(inactive),
            memory_limit_bytes: limit,
            scope: if path == "/" {
                UsageScope::Container
            } else {
                UsageScope::Service
            },
        })
    }

    fn from_process_tree() -> Option<Self> {
        let me = std::process::id();
        let mut children: std::collections::HashMap<u32, Vec<u32>> = Default::default();
        let mut stats = std::collections::HashMap::new();
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(s) = proc_stat(pid) {
                children.entry(s.ppid).or_default().push(pid);
                stats.insert(pid, s);
            }
        }
        let mine = stats.get(&me)?;
        // Children already ended count through this process's `cutime`/`cstime`.
        let mut ticks = mine.ticks + mine.children_ticks;
        let mut memory = rss_bytes(me).unwrap_or(0);
        let mut todo = children.get(&me).cloned().unwrap_or_default();
        while let Some(pid) = todo.pop() {
            if let Some(s) = stats.get(&pid) {
                ticks += s.ticks;
            }
            memory += rss_bytes(pid).unwrap_or(0);
            if let Some(c) = children.get(&pid) {
                todo.extend(c);
            }
        }
        Some(Self {
            at: Instant::now(),
            cpu_us: (ticks as f64 * 1e6 / USER_HZ) as u64,
            memory_bytes: memory,
            memory_limit_bytes: None,
            scope: UsageScope::Processes,
        })
    }
}

struct ProcStat {
    ppid: u32,
    /// utime + stime.
    ticks: u64,
    /// cutime + cstime (children waited for).
    children_ticks: u64,
}

fn proc_stat(pid: u32) -> Option<ProcStat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let num = |i: usize| fields.get(i).and_then(|f| f.parse::<i64>().ok());
    Some(ProcStat {
        ppid: u32::try_from(num(1)?).ok()?,
        ticks: (num(11)? + num(12)?).max(0) as u64,
        children_ticks: (num(13)? + num(14)?).max(0) as u64,
    })
}

fn rss_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    Some(kib_field(&status, "VmRSS:")? * 1024)
}

/// Whether every process of `procs` (a `cgroup.procs` list) is this process,
/// one of its ancestors (`docker-init`) or one of its descendants (FFmpeg).
fn only_related_processes(procs: &str) -> bool {
    let me = std::process::id();
    let parent = |pid: u32| proc_stat(pid).map(|s| s.ppid);
    let mut ancestors = Vec::new();
    let mut p = me;
    while let Some(up) = parent(p).filter(|&up| up != 0) {
        ancestors.push(up);
        p = up;
        if ancestors.len() > 64 {
            break;
        }
    }
    procs
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .all(|pid| {
            if pid == me || ancestors.contains(&pid) {
                return true;
            }
            let mut p = pid;
            for _ in 0..64 {
                match parent(p) {
                    Some(up) if up == me => return true,
                    Some(up) if up != 0 => p = up,
                    // Ended meanwhile: not counted against the group.
                    None => return true,
                    _ => return false,
                }
            }
            false
        })
}

/// Value of `key value` in a cgroup flat-keyed file (`cpu.stat`, `memory.stat`).
fn value_field(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(' ')?;
        (k == key).then(|| v.trim().parse().ok()).flatten()
    })
}

/// Aggregate host CPU time counters from the first line of `/proc/stat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuTimes {
    busy: u64,
    total: u64,
}

impl CpuTimes {
    /// Reads the counters.
    pub fn read() -> Option<Self> {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        Self::parse(stat.lines().next()?)
    }

    fn parse(line: &str) -> Option<Self> {
        let mut it = line.split_whitespace();
        if it.next()? != "cpu" {
            return None;
        }
        // user nice system idle iowait irq softirq steal (guest time is already in user).
        let v: Vec<u64> = it.take(8).map(|f| f.parse().ok()).collect::<Option<_>>()?;
        if v.len() < 5 {
            return None;
        }
        let total: u64 = v.iter().sum();
        let idle = v[3] + v[4];
        Some(Self {
            busy: total - idle,
            total,
        })
    }

    /// Host CPU usage between `earlier` and `self`, in percent of all cores.
    pub fn percent_since(&self, earlier: &CpuTimes) -> Option<f64> {
        let total = self.total.checked_sub(earlier.total)?;
        let busy = self.busy.checked_sub(earlier.busy)?;
        (total > 0).then(|| 100.0 * busy as f64 / total as f64)
    }
}

/// Host memory from `/proc/meminfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemInfo {
    /// Total RAM in bytes.
    pub total_bytes: u64,
    /// RAM in use (total minus `MemAvailable`) in bytes.
    pub used_bytes: u64,
}

impl MemInfo {
    /// Reads the values.
    pub fn read() -> Option<Self> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let total = kib_field(&text, "MemTotal:")? * 1024;
        let available = kib_field(&text, "MemAvailable:")? * 1024;
        Some(Self {
            total_bytes: total,
            used_bytes: total.saturating_sub(available),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_line() {
        let a = CpuTimes::parse("cpu  100 0 50 800 50 0 0 0 0 0").expect("valid");
        // +150 busy ticks, +850 idle ticks.
        let b = CpuTimes::parse("cpu  250 0 50 1650 50 0 0 0 0 0").expect("valid");
        assert_eq!(b.percent_since(&a), Some(15.0));
        assert!(CpuTimes::parse("cpu0 1 2 3 4 5").is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn reads_proc() {
        assert!(ProcSample::read().expect("proc").rss_bytes > 0);
        assert!(CpuTimes::read().is_some());
        let m = MemInfo::read().expect("meminfo");
        assert!(m.total_bytes >= m.used_bytes && m.total_bytes > 0);
        let g = GroupSample::read().expect("group");
        assert!(g.memory_bytes > 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn counts_child_processes() {
        // A busy child process must show in the group CPU and memory.
        let before = GroupSample::read().expect("group");
        let status = std::process::Command::new("sh")
            .args(["-c", "i=0; while [ $i -lt 1000000 ]; do i=$((i+1)); done"])
            .status()
            .expect("sh");
        assert!(status.success());
        let after = GroupSample::read().expect("group");
        let cpu = after.cpu_percent_since(&before).expect("same scope");
        assert!(
            cpu > 50.0,
            "child CPU not counted: {cpu:.1} % ({:?})",
            after.scope
        );
    }

    #[test]
    fn reads_flat_keyed_files() {
        let s = "usage_usec 1500\nuser_usec 1000\n";
        assert_eq!(value_field(s, "usage_usec"), Some(1500));
        assert_eq!(value_field(s, "user_usec"), Some(1000));
        assert_eq!(value_field(s, "usage"), None);
    }
}
