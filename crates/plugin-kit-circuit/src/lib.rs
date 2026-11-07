//! A transcribed circuit in real time (K-CIRCUIT): an instrument's typed parts ([`part`]) and
//! device cards ([`devices`]) become node equations ([`net`]), an operating point ([`dc`]), a
//! state space with its exact modal steps and its reduction ([`ss`], [`red`]), and a stepper
//! ([`run`]) that advances the linear network exactly in discrete time and solves each
//! nonlinear device by ngspice's equations ([`dev`]) with SPICE's junction limiting; [`live`]
//! steps the same equations for speed (the TR-808's live engine, K-CIRCUIT-LIVE) and [`fast`]
//! a reduced system with only its moving devices; [`over`] runs a model at twice the host's
//! rate. The TR-808's engine's foundation, moved from its
//! plug-in (`olifant`) unchanged but for its names and its boundary with the circuit, which an
//! instrument now supplies: its parts, its [`devices::Library`] and its rails. Each instrument
//! builds its own voice on it (the TR-808's strike and ring stay in the TR-808).

// The solvers index several arrays by one index, as their equations do (a mode's state, its
// weights, its increments); iterators zipped four ways read worse and change nothing (the
// TR-808's workspace allows the lint for the same reason).
#![allow(clippy::needless_range_loop)]

pub mod cancel;
pub mod dc;
pub mod dev;
pub mod devices;
pub mod fast;
pub mod la;
pub mod live;
pub mod net;
pub mod over;
pub mod part;
pub mod red;
pub mod run;
pub mod serial;
pub mod sim;
pub mod ss;

#[cfg(test)]
mod generic_tests;
