//! The plug-ins' stereo and DRIVE's AUTO GAIN, from the CA-74 (its decisions.md R25 and R27 to
//! R30, R41), as the CA-72 took them (its R-STEREO), with INNER (K7): [`place`], where the
//! voices sit and how loud, and [`auto_gain`], DRIVE's loudness measured and taken back. What is an instrument's own
//! stays with it: its voices, the controls that make a sound and its key, and the curve its
//! presets average.

pub mod auto_gain;
pub mod place;
