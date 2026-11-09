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
            GetLogicalProcessorInformationEx, PROCESSOR_RELATIONSHIP, RelationProcessorCore,
            SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        };
        // (The offsets `classes` reads by, as the system's types have them.)
        const _: () = {
            assert!(std::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Size) == SIZE_AT);
            assert!(
                std::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous) == DATA_AT
            );
            assert!(std::mem::offset_of!(PROCESSOR_RELATIONSHIP, EfficiencyClass) == CLASS_AT);
            assert!(DATA_AT + std::mem::size_of::<PROCESSOR_RELATIONSHIP>() == CORE_RECORD);
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
        // SAFETY: a buffer of at least `len` bytes.
        if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, p, &mut len) } == 0 {
            return None;
        }
        let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_ne_bytes()).collect();
        let classes = classes(&bytes[..(len as usize).min(bytes.len())]);
        let top = *classes.iter().max()?;
        let mixed = classes.iter().any(|&c| c != top);
        mixed.then(|| classes.iter().filter(|&&c| c == top).count())
    }

    /// Where a `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` record keeps its size and its
    /// relationship's data, where a core's `PROCESSOR_RELATIONSHIP` keeps its efficiency class,
    /// and the size of a core's record (the type itself is larger: its union holds the largest
    /// relationship, 80 bytes against a core's 48).
    #[cfg(any(windows, test))]
    const SIZE_AT: usize = 4;
    #[cfg(any(windows, test))]
    const DATA_AT: usize = 8;
    #[cfg(any(windows, test))]
    const CLASS_AT: usize = 1;
    #[cfg(any(windows, test))]
    const CORE_RECORD: usize = 48;

    /// Each core's efficiency class from `GetLogicalProcessorInformationEx`'s records for
    /// `RelationProcessorCore`, each read by its own size; a record shorter than a core's, or
    /// running past the end, ends the reading.
    #[cfg(any(windows, test))]
    pub(crate) fn classes(bytes: &[u8]) -> Vec<u8> {
        let mut classes = Vec::new();
        let mut off = 0usize;
        while let Some(head) = bytes.get(off..off + DATA_AT) {
            let size = u32::from_ne_bytes([
                head[SIZE_AT],
                head[SIZE_AT + 1],
                head[SIZE_AT + 2],
                head[SIZE_AT + 3],
            ]) as usize;
            if size < CORE_RECORD || off + size > bytes.len() {
                break;
            }
            classes.push(bytes[off + DATA_AT + CLASS_AT]);
            off += size;
        }
        classes
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

    /// Every core's record is read, the last too: a record is a core's 48 bytes, not the
    /// type's 80 (the reading once stopped 80 bytes before the end and dropped the last core);
    /// a short or overrunning record ends it.
    #[test]
    fn every_cores_record_is_read() {
        let record = |class: u8| {
            let mut r = vec![0u8; 48];
            r[4..8].copy_from_slice(&48u32.to_ne_bytes());
            r[9] = class;
            r
        };
        let mut bytes: Vec<u8> = [1u8, 1, 0, 0, 1].iter().flat_map(|&c| record(c)).collect();
        assert_eq!(sys::classes(&bytes), [1, 1, 0, 0, 1]);
        bytes.truncate(4 * 48 + 20);
        assert_eq!(
            sys::classes(&bytes),
            [1, 1, 0, 0],
            "a record running past the end read"
        );
        let mut short = record(1);
        short[4..8].copy_from_slice(&8u32.to_ne_bytes());
        assert_eq!(
            sys::classes(&short),
            Vec::<u8>::new(),
            "a short record read"
        );
    }

    /// A kernel CPU list read as the kernel writes it.
    #[test]
    fn a_cpu_list_is_read() {
        assert_eq!(sys::list("0-3,8,10-11\n"), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(sys::list(""), Vec::<usize>::new());
    }
}
