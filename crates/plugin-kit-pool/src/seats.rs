//! One budget for every process of the user's (the CA-72's decisions.md R-THREADS): a host may
//! run each plug-in in a process of its own (Bitwig's "individually", REAPER's like option),
//! and then every instance would count itself alone and take a lone instance's share. So each
//! instance that holds workers also holds a **seat**: an exclusive advisory lock on one of
//! [`SEATS`] files in a folder of the user's own (`flock` on macOS and Linux, `LockFileEx` on
//! Windows, through `std::fs::File::try_lock`). The kernel lets a process's locks go when it
//! ends, however it ends, so no seat outlives its holder. Each process counts the seats the
//! others hold by trying their locks and letting go at once ([`SeatFiles::others`]), and the
//! budget's shares follow every process's instances ([`crate::Budget`]). Every plug-in on the
//! kit sits in the same folder, so that two different plug-ins share the machine too.
//!
//! Only the helper thread and the threads that start or stop pools touch the files, never an
//! audio thread; each thread's file operations are counted ([`io_here`]) for the tests to
//! check so.

use std::cell::Cell;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The seats: the most instances, across every process of the user, that share the budget
/// by their files (one more takes none, and counts in its own process only).
pub const SEATS: usize = 64;

thread_local! {
    /// File operations on the seats made on this thread.
    static IO: Cell<u64> = const { Cell::new(0) };
}

/// The seats' file operations made on the calling thread so far (the tests': none on an audio
/// thread).
#[doc(hidden)]
pub fn io_here() -> u64 {
    IO.with(Cell::get)
}

fn count(n: u64) {
    IO.with(|c| c.set(c.get() + n));
}

/// The seats' folder for this user: macOS's and Windows' own temporary folder (`$TMPDIR`,
/// `%TEMP%`, the user's); on Linux `$XDG_RUNTIME_DIR` (the user's, made for this), else a
/// folder of the user's in `/tmp`, made private to them. None if there is none to have.
/// `PLUGIN_KIT_SEATS_DIR` puts it elsewhere (the tests' and measurements', so that they do not
/// share seats with a plug-in playing in a host). The folder's name carries the seats' format,
/// so that kits that count differently never share one.
pub fn user_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("PLUGIN_KIT_SEATS_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    // The library's own tests: a folder of theirs (the processes they start included), so
    // that a plug-in playing in a host on this computer does not leave them without workers
    // (the CA-72's tests failed so on 2026-10-17 while Live held the user's seats).
    if cfg!(test) {
        return Some(std::env::temp_dir().join("plugin-kit-pool-seats-tests"));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(run).join("idlefoundry-seats-1"));
        }
        uid().map(|u| std::env::temp_dir().join(format!("idlefoundry-seats-1-{u}")))
    }
    #[cfg(not(target_os = "linux"))]
    Some(std::env::temp_dir().join("Idle Foundry seats 1"))
}

/// The user this process runs as: Linux's from `/proc/self/status` (its effective id), macOS's
/// as the owner of its own temporary folder.
#[cfg(unix)]
fn uid() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with("Uid:"))?;
        line.split_whitespace().nth(2)?.parse().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(std::env::temp_dir())
            .ok()
            .map(|m| m.uid())
    }
}

/// The seats' files, opened once, and which of them this process holds.
#[derive(Debug)]
pub struct SeatFiles {
    files: Vec<File>,
    mine: Mutex<u64>,
}

impl SeatFiles {
    /// The seats in `dir`, made if need be (private to the user where the system has such);
    /// None if they cannot be had (the budget is then the process's alone, as before).
    pub fn open(dir: &Path) -> Option<SeatFiles> {
        let mut make = std::fs::DirBuilder::new();
        make.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            make.mode(0o700);
        }
        make.create(dir).ok()?;
        #[cfg(unix)]
        {
            // (Another user's folder in a shared /tmp is not ours to count by.)
            use std::os::unix::fs::MetadataExt;
            if Some(std::fs::metadata(dir).ok()?.uid()) != uid() {
                return None;
            }
        }
        let files = (0..SEATS)
            .map(|k| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(dir.join(format!("seat-{k:02}")))
            })
            .collect::<std::io::Result<Vec<File>>>()
            .ok()?;
        Some(SeatFiles {
            files,
            mine: Mutex::new(0),
        })
    }

    fn mine(&self) -> std::sync::MutexGuard<'_, u64> {
        self.mine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A free seat taken, its number; None if every one is held.
    pub fn take(&self) -> Option<usize> {
        let mut mine = self.mine();
        for (k, f) in self.files.iter().enumerate() {
            if *mine & (1 << k) != 0 {
                continue;
            }
            count(1);
            if f.try_lock().is_ok() {
                *mine |= 1 << k;
                return Some(k);
            }
        }
        None
    }

    /// Seat `k`, this process's, let go.
    pub fn give_back(&self, k: usize) {
        let mut mine = self.mine();
        if *mine & (1 << k) != 0 {
            count(1);
            let _ = self.files[k].unlock();
            *mine &= !(1 << k);
        }
    }

    /// The seats this process holds.
    pub fn held_here(&self) -> u64 {
        *self.mine()
    }

    /// The seats other processes hold: each one tried and let go at once if it was free; the
    /// look made twice and only seats held both times counted (another process's look holds
    /// a seat for microseconds).
    pub fn others(&self) -> u64 {
        let mine = self.mine();
        let look = || {
            let mut held = 0u64;
            for (k, f) in self.files.iter().enumerate() {
                if *mine & (1 << k) != 0 {
                    continue;
                }
                count(1);
                match f.try_lock() {
                    Ok(()) => {
                        count(1);
                        let _ = f.unlock();
                    }
                    // (A seat that cannot be tried counts as held: a share may then be
                    // smaller, never larger, than it should.)
                    Err(TryLockError::WouldBlock | TryLockError::Error(_)) => held |= 1 << k,
                }
            }
            held
        };
        let first = look();
        if first == 0 {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
        first & look()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A concurrent spawn can inherit these file descriptions between fork and exec,
    /// even though their descriptors are close-on-exec. Closing this process's copy
    /// need not release its flock until that child reaches exec. Persistent leaks
    /// must still fail rather than being hidden by an unbounded wait.
    fn assert_others_released(seats: &SeatFiles) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let held = seats.others();
            if held == 0 {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the seats outlived their files and concurrent spawns: {held:#x}"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// A seat taken in one set of the files is held to another (another process's, as the
    /// kernel counts locks by open file), and free again once given back.
    #[test]
    fn a_seat_taken_is_held_to_others_and_free_once_given_back() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (
            SeatFiles::open(dir.path()).unwrap(),
            SeatFiles::open(dir.path()).unwrap(),
        );
        let k = a.take().unwrap();
        let j = a.take().unwrap();
        assert_ne!(k, j);
        assert_eq!(b.others(), (1 << k) | (1 << j));
        assert_eq!(a.others(), 0, "its own seats counted as others'");
        // (The other takes the next free seat.)
        let m = b.take().unwrap();
        assert!(m != k && m != j);
        a.give_back(k);
        assert_eq!(b.others(), 1 << j);
        drop(a);
        assert_others_released(&b);
        assert_eq!(b.held_here(), 1 << m);
    }

    /// Hold a child before exec to reproduce the exact lock lifetime that can
    /// overlap another harness test's Command::spawn. The child never calls Rust
    /// code that allocates or locks between fork and exec.
    #[cfg(target_os = "linux")]
    #[test]
    #[allow(unsafe_code)] // Only the regression's bounded pre-exec POSIX handshake.
    fn inherited_seat_locks_release_when_a_concurrent_spawn_reaches_exec() {
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::os::unix::process::CommandExt;

        let dir = tempfile::tempdir().unwrap();
        let a = SeatFiles::open(dir.path()).unwrap();
        let b = SeatFiles::open(dir.path()).unwrap();
        let k = a.take().unwrap();
        let (mut parent, child_signal) = UnixStream::pair().unwrap();
        // Bound a failed handshake so a broken regression cannot hang the suite.
        parent
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let spawn = std::thread::spawn(move || {
            let fd = child_signal.as_raw_fd();
            let mut command = std::process::Command::new("/usr/bin/true");
            // SAFETY: the closure uses only POSIX async-signal-safe I/O;
            // its socket is captured alive across spawn and fd is valid there.
            unsafe {
                command.pre_exec(move || {
                    let ready = [1u8];
                    if libc::write(fd, ready.as_ptr().cast(), 1) != 1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let mut wait = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    if libc::poll(&mut wait, 1, 5_000) <= 0 {
                        return Err(std::io::Error::from_raw_os_error(libc::ETIMEDOUT));
                    }
                    let mut go = [0u8];
                    if libc::read(fd, go.as_mut_ptr().cast(), 1) != 1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command.spawn().unwrap()
        });
        parent.read_exact(&mut [0]).unwrap();
        drop(a);
        let held_before_exec = b.others();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            parent.write_all(&[1]).unwrap();
            let mut child = spawn.join().unwrap();
            assert!(child.wait().unwrap().success());
        });
        assert_others_released(&b);
        release.join().unwrap();
        assert_eq!(held_before_exec, 1 << k, "child did not inherit the seat");
    }

    /// What a child process does for the tests: takes `PLUGIN_KIT_HOLD_SEATS` seats in
    /// `PLUGIN_KIT_HOLD_DIR`, says so, and holds them until it is killed.
    #[test]
    #[ignore = "a child process of the tests"]
    fn hold_seats_for_a_test() {
        let Some(dir) = std::env::var_os("PLUGIN_KIT_HOLD_DIR") else {
            return;
        };
        let n: usize = std::env::var("PLUGIN_KIT_HOLD_SEATS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(1);
        let seats = SeatFiles::open(Path::new(&dir)).unwrap();
        for _ in 0..n {
            seats.take().unwrap();
        }
        println!("HELD {n}");
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    /// Another process holding `n` seats in `dir` ([`hold_seats_for_a_test`]), once it holds
    /// them.
    pub(crate) fn holder(dir: &Path, n: usize) -> std::process::Child {
        use std::io::BufRead;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "seats::tests::hold_seats_for_a_test",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("PLUGIN_KIT_HOLD_DIR", dir)
            .env("PLUGIN_KIT_HOLD_SEATS", n.to_string())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let held = std::io::BufReader::new(out)
            .lines()
            .map_while(Result::ok)
            .any(|l| l.contains("HELD"));
        assert!(held, "the holder never held its seats");
        child
    }

    /// A process killed with seats held (as a host's plug-in process may crash) leaves none
    /// held: the kernel lets its locks go.
    #[test]
    fn a_killed_holder_s_seats_are_free_again() {
        let dir = tempfile::tempdir().unwrap();
        let seats = SeatFiles::open(dir.path()).unwrap();
        let mut child = holder(dir.path(), 3);
        assert_eq!(seats.others().count_ones(), 3);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(seats.others(), 0, "a killed process's seats still held");
    }

    /// The seats' file operations are counted on the thread that makes them, and on no other.
    #[test]
    fn the_seats_file_operations_are_counted_where_they_are_made() {
        let dir = tempfile::tempdir().unwrap();
        let seats = SeatFiles::open(dir.path()).unwrap();
        let before = io_here();
        let elsewhere = std::thread::spawn(io_here).join().unwrap();
        seats.take();
        assert!(io_here() > before);
        assert_eq!(elsewhere, 0);
    }
}
