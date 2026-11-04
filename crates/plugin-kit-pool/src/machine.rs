//! What the machine is, for the budget's rules ([`crate::rules`]): its hardware threads, and
//! the fast ones among them (the CA-72's decisions.md R-THREADS, item A). Read once, off the
//! audio thread (`available_parallelism` allocates on Linux and reads the cgroup's files).

/// The hardware threads the process may use (`available_parallelism`, which follows its
/// affinity and quotas), and the fast ones among them: the performance cores, counting their
/// second hardware threads only on a processor with one kind of core (on a hybrid processor
/// the threads after its performance cores' first go to its efficiency cores before their
/// second threads, and a voice there is slower: the CA-72's R-THREADS, item 2). Read once.
///
/// - macOS: `hw.perflevel0.physicalcpu` where `hw.nperflevels` is more than 1;
/// - Windows: `GetLogicalProcessorInformationEx`, the cores of the highest `EfficiencyClass`
///   where the classes differ;
/// - Linux: the cores among `/sys/devices/cpu_core/cpus` (Intel's hybrid processors), else
///   those of the largest `cpu_capacity` where it differs (ARM's);
/// - elsewhere, or where the system does not say, every hardware thread.
pub fn cores() -> Cores {
    static CORES: std::sync::OnceLock<Cores> = std::sync::OnceLock::new();
    *CORES.get_or_init(|| {
        let all = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let fast = sys::fast().map_or(all, |f| f.clamp(1, all));
        Cores { all, fast }
    })
}

/// [`cores`]' answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cores {
    /// Every hardware thread the process may use (N).
    pub all: usize,
    /// The fast ones (F).
    pub fast: usize,
}

#[allow(unsafe_code)]
mod sys {
    /// The fast cores where the processor has more than one kind (None: one kind, or not
    /// known).
    #[cfg(target_os = "macos")]
    pub fn fast() -> Option<usize> {
        let levels = sysctl("hw.nperflevels")?;
        (levels > 1)
            .then(|| sysctl("hw.perflevel0.physicalcpu"))
            .flatten()
    }

    #[cfg(target_os = "macos")]
    fn sysctl(name: &str) -> Option<usize> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut v: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: a NUL-terminated name, an int's buffer and its length.
        let r = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&raw mut v).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (r == 0 && v > 0).then_some(v as usize)
    }

    #[cfg(windows)]
    pub fn fast() -> Option<usize> {
        use windows_sys::Win32::System::SystemInformation::{
            GetLogicalProcessorInformationEx, RelationProcessorCore,
            SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        };
        let mut len = 0u32;
        // SAFETY: asked for the length only.
        unsafe {
            GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len)
        };
        if len == 0 {
            return None;
        }
        // (u64s: the records' alignment.)
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let p = buf
            .as_mut_ptr()
            .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
        // SAFETY: a buffer of `len` bytes.
        if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, p, &mut len) } == 0 {
            return None;
        }
        let mut classes = Vec::new();
        let mut off = 0usize;
        while off + std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= len as usize {
            // SAFETY: within the buffer the system filled, one record of `Size` bytes at a time.
            let r = unsafe {
                &*buf
                    .as_ptr()
                    .cast::<u8>()
                    .add(off)
                    .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()
            };
            if r.Size == 0 {
                break;
            }
            // SAFETY: the record is a processor core's (the relation asked for).
            classes.push(unsafe { r.Anonymous.Processor.EfficiencyClass });
            off += r.Size as usize;
        }
        let top = *classes.iter().max()?;
        let mixed = classes.iter().any(|&c| c != top);
        mixed.then(|| classes.iter().filter(|&&c| c == top).count())
    }

    #[cfg(target_os = "linux")]
    pub fn fast() -> Option<usize> {
        let mine = affinity()?;
        let read = |p: String| std::fs::read_to_string(p).ok();
        // The physical cores among `cpus` (each counted once, by its first hardware thread).
        let cores_of = |cpus: &[usize]| {
            let mut firsts: Vec<usize> = cpus
                .iter()
                .filter_map(|c| {
                    read(format!(
                        "/sys/devices/system/cpu/cpu{c}/topology/core_cpus_list"
                    ))
                    .or_else(|| {
                        read(format!(
                            "/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list"
                        ))
                    })
                    .and_then(|l| list(&l).first().copied())
                })
                .collect();
            firsts.sort_unstable();
            firsts.dedup();
            firsts.len()
        };
        if let Some(l) = read("/sys/devices/cpu_core/cpus".into()) {
            let p: Vec<usize> = list(&l).into_iter().filter(|c| mine.contains(c)).collect();
            if !p.is_empty() {
                return Some(cores_of(&p));
            }
        }
        let caps: Vec<(usize, u64)> = mine
            .iter()
            .filter_map(|&c| {
                read(format!("/sys/devices/system/cpu/cpu{c}/cpu_capacity"))
                    .and_then(|x| x.trim().parse().ok())
                    .map(|x| (c, x))
            })
            .collect();
        let top = caps.iter().map(|&(_, x)| x).max()?;
        if caps.len() != mine.len() || caps.iter().all(|&(_, x)| x == top) {
            return None;
        }
        let fast: Vec<usize> = caps
            .iter()
            .filter(|&&(_, x)| x == top)
            .map(|&(c, _)| c)
            .collect();
        Some(cores_of(&fast))
    }

    /// The CPUs the process may run on.
    #[cfg(target_os = "linux")]
    fn affinity() -> Option<Vec<usize>> {
        // SAFETY: a zeroed cpu_set_t is a valid empty set, filled for pid 0 (this thread).
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
                return None;
            }
            Some(
                (0..libc::CPU_SETSIZE as usize)
                    .filter(|&c| libc::CPU_ISSET(c, &set))
                    .collect(),
            )
        }
    }

    /// A kernel CPU list ("0-3,8,10-11").
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn list(s: &str) -> Vec<usize> {
        let mut v = Vec::new();
        for part in s.trim().split(',').filter(|p| !p.is_empty()) {
            let mut ends = part.splitn(2, '-').map(|x| x.trim().parse::<usize>());
            match (ends.next(), ends.next()) {
                (Some(Ok(a)), None) => v.push(a),
                (Some(Ok(a)), Some(Ok(b))) if a <= b => v.extend(a..=b),
                _ => {}
            }
        }
        v
    }

    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    pub fn fast() -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fast cores are among the machine's, at least one.
    #[test]
    fn the_fast_cores_are_among_all_of_them() {
        let c = cores();
        assert!(c.fast >= 1 && c.fast <= c.all, "{c:?}");
    }

    /// A kernel CPU list read as the kernel writes it.
    #[test]
    fn a_cpu_list_is_read() {
        assert_eq!(sys::list("0-3,8,10-11\n"), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(sys::list(""), Vec::<usize>::new());
    }
}
