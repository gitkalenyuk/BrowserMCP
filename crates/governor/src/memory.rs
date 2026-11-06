//! Reading memory: total/available RAM (for the profile and memory pressure), the working
//! set of our own process tree (browser + renderers + GPU + utility processes), and the
//! recommended parallelism for mass runs.
//!
//! Everything goes through [`MemorySource`], so tests can fake the machine.

use schemars::JsonSchema;
use serde::Serialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const MIB: u64 = 1024 * 1024;

/// System RAM in bytes. `total == 0` means "could not be read".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
}

impl MemInfo {
    /// Available RAM in percent of total (100 when unknown, so it never looks like pressure).
    pub fn free_pct(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            self.available as f64 * 100.0 / self.total as f64
        }
    }

    /// True when less than `pct` percent of RAM is available.
    pub fn under_pressure(&self, pct: u8) -> bool {
        self.free_pct() < f64::from(pct)
    }
}

/// One process of our tree.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ProcInfo {
    pub pid: u32,
    /// `browser` (us), or Chromium's `--type=` value: `renderer`, `gpu-process`, `utility`, …
    pub kind: String,
    /// Working set (resident memory) in MB.
    pub mb: f64,
}

/// Where the governor reads memory from.
pub trait MemorySource: Send + Sync + 'static {
    /// Total and available RAM.
    fn read(&self) -> MemInfo;
    /// Our process and all its descendants. Can take ~100 ms (lists every process).
    fn processes(&self) -> Vec<ProcInfo>;
}

/// The real machine, via `sysinfo`.
pub struct SystemMemory;

impl MemorySource for SystemMemory {
    fn read(&self) -> MemInfo {
        // If memory cannot be read, report "unknown" (no pressure, normal profile).
        System::new().map_or(MemInfo::default(), |mut sys| {
            sys.refresh_memory();
            MemInfo {
                total: sys.total_memory(),
                available: sys.available_memory(),
            }
        })
    }

    fn processes(&self) -> Vec<ProcInfo> {
        let Ok(mut sys) = System::new() else {
            return Vec::new();
        };
        let Ok(me) = sysinfo::get_current_pid() else {
            return Vec::new();
        };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cmd(UpdateKind::Always),
        );
        // Walk down from our pid: CEF starts every helper process as our direct child, but
        // walking the whole tree also catches grandchildren (e.g. crashpad helpers).
        let mut tree = vec![me];
        let mut i = 0;
        while i < tree.len() {
            let parent = tree[i];
            for (pid, p) in sys.processes() {
                if p.parent() == Some(parent) && !tree.contains(pid) {
                    tree.push(*pid);
                }
            }
            i += 1;
        }
        tree.into_iter()
            .filter_map(|pid| {
                let p = sys.process(pid)?;
                Some(ProcInfo {
                    pid: pid.as_u32(),
                    kind: if pid == me {
                        "browser".into()
                    } else {
                        process_kind(p.cmd())
                    },
                    mb: to_mb(p.memory()),
                })
            })
            .collect()
    }
}

/// Chromium's `--type=renderer` → `renderer`; no `--type` → `other`.
fn process_kind(cmd: &[std::ffi::OsString]) -> String {
    cmd.iter()
        .filter_map(|a| a.to_str())
        .find_map(|a| a.strip_prefix("--type="))
        .unwrap_or("other")
        .to_string()
}

pub fn to_mb(bytes: u64) -> f64 {
    // One decimal is plenty for a status page.
    (bytes as f64 / MIB as f64 * 10.0).round() / 10.0
}

/// How many parallel slots (script hosts + their tabs) this machine can carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Recommendation {
    /// `min(by_cpu, by_ram)`, at least 1.
    pub parallel: usize,
    /// Logical CPUs.
    pub cpus: usize,
    /// Half the CPUs (the browser needs the rest), at least 1.
    pub by_cpu: usize,
    /// Available RAM / per-slot memory; null when RAM could not be read.
    pub by_ram: Option<usize>,
    /// Available RAM in MB (0 when unknown).
    pub available_mb: u64,
}

/// The rule, without touching the machine: `min(cpus / 2, available / per_slot)`, at least 1.
pub fn recommend(cpus: usize, mem: MemInfo, per_slot_bytes: u64) -> Recommendation {
    let by_cpu = (cpus / 2).max(1);
    let by_ram = (mem.total > 0).then(|| (mem.available / per_slot_bytes.max(1)) as usize);
    Recommendation {
        parallel: by_cpu.min(by_ram.unwrap_or(usize::MAX)).max(1),
        cpus,
        by_cpu,
        by_ram,
        available_mb: mem.available / MIB,
    }
}

/// [`recommend`] for this machine right now. Used by `governor.recommend_parallel` and by the
/// jobs runner's auto-limit.
pub fn recommend_parallel(per_slot_bytes: u64) -> Recommendation {
    let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
    recommend(cpus, SystemMemory.read(), per_slot_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn recommendation_takes_the_smaller_limit() {
        let mem = MemInfo {
            total: 16 * GIB,
            available: 2 * GIB,
        };
        // 2 GB / 400 MB = 5, 16 CPUs / 2 = 8 → 5.
        let r = recommend(16, mem, 400 * MIB);
        assert_eq!((r.parallel, r.by_cpu, r.by_ram), (5, 8, Some(5)));
        // Few CPUs win.
        assert_eq!(recommend(2, mem, 400 * MIB).parallel, 1);
        // No RAM left still gives one slot.
        let empty = MemInfo {
            total: 16 * GIB,
            available: 0,
        };
        assert_eq!(recommend(16, empty, 400 * MIB).parallel, 1);
        // Unknown RAM: only the CPU rule.
        let r = recommend(8, MemInfo::default(), 400 * MIB);
        assert_eq!((r.parallel, r.by_ram), (4, None));
    }

    #[test]
    fn pressure_and_kinds() {
        let mem = MemInfo {
            total: 100,
            available: 14,
        };
        assert!(mem.under_pressure(15));
        assert!(!mem.under_pressure(10));
        assert!(!MemInfo::default().under_pressure(50));
        let cmd: Vec<std::ffi::OsString> = vec!["x.exe".into(), "--type=renderer".into()];
        assert_eq!(process_kind(&cmd), "renderer");
        assert_eq!(process_kind(&[]), "other");
        // The real machine: our own test process is always in the tree.
        let procs = SystemMemory.processes();
        assert!(procs.iter().any(|p| p.kind == "browser"));
    }
}
