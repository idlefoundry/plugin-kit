//! The plug-ins' voice workers: the CA-72's thread model (its decisions.md R11, R21, R23,
//! R-THREADS, R-SPIN, R-LO-WORKERS and R-DROPOUTS), generic over the voices a plug-in gives it
//! (the kit's K-POOL).
//!
//! - **Workers only while needed.** An instance's [`Crew`] holds a [`Pool`] only while the
//!   plug-in wants workers (more than one voice to share). Its audio thread asks for one and
//!   lets it go ([`Crew::follow`]); its [`Helper`] thread starts and stops them
//!   ([`Crew::serve`]). Nothing is started, joined, allocated or freed on the audio thread.
//! - **One budget for the machine** ([`Budget`], [`rules`]): B = max(floor, N − 2 − A) workers
//!   for the A instances holding them, in every process of the user's ([`seats`]); an
//!   instance at most C = max(floor, F − 2) and its voices less one; shares even, and a pool
//!   started again at its new share when the instances come and go.
//! - **Short spins**: a worker spins [`SPIN`] (50 µs) for the next run, and parks at once when
//!   the block is over ([`Pool::rest`]).
//! - **The same samples** with any number of workers: every voice played whole by one thread,
//!   summed by the plug-in in its own order.

mod budget;
mod crew;
mod helper;
pub mod machine;
mod pool;
pub mod rules;
pub mod seats;

pub use budget::{Budget, LOOK};
pub use crew::{AUTO, Ask, Crew};
pub use helper::{Helper, SERVE};
pub use machine::{Cores, cores};
pub use pool::{Deadlines, MAX, Play, Pool, Run, SPIN, Shared, Threads, threads_here};

/// The machine's budget ([`Budget::machine`]), for every instance of the plug-in in the process,
/// its seats shared with every process of the user's.
pub static BUDGET: Budget = Budget::machine();
