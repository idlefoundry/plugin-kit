//! The production standalone's adapter driven as `plugin-kit-standalone` drives it, with no
//! audio device, window or host (PATCHES.md, changes 20 to 22).
use super::*;
use crate::prelude::*;
use std::num::NonZeroU32;

#[derive(Default)]
struct ProbeParams;
// SAFETY: this fixture exposes no parameters or persistent fields.
unsafe impl Params for ProbeParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        Vec::new()
    }
}

/// What the instrument saw in each block: the panic's token, read first as an instrument
/// must, then whether the block had a panic.
type Seen = Arc<Mutex<Vec<(Option<u64>, bool)>>>;

#[derive(Default)]
struct Probe {
    params: Arc<ProbeParams>,
    seen: Seen,
}

impl Plugin for Probe {
    const NAME: &'static str = "production adapter test";
    const VENDOR: &'static str = "test";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }
    fn process(
        &mut self,
        _: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let token = context.standalone_panic_token();
        let panic = context.take_standalone_panic();
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((token, panic));
        ProcessStatus::Normal
    }
}

/// A block of `frames` (the right channel `right` long) with or without a panic.
fn run(processor: &mut Processor<Probe>, panic: bool, frames: usize, right: usize) {
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; right]);
    processor.process(AudioBlock {
        left: &mut left,
        right: &mut right,
        input_left: &[],
        input_right: &[],
        midi: &[],
        panic,
    });
}

/// The token captured with a panic reaches the instrument with that block alone: not a block
/// without the panic, not the next block, and not past a block the adapter rejected (the
/// kit's K-PANIC-TOKEN; the CA-72's e43d39c).
#[test]
fn a_panics_token_reaches_the_instrument_with_its_own_block_alone() {
    let seen = Seen::default();
    let probe = Probe {
        seen: Arc::clone(&seen),
        ..Probe::default()
    };
    let (mut processor, _session) = Processor::new(probe, &[]).expect("the adapter");
    processor
        .prepare(ProcessConfig {
            sample_rate: 48_000.0,
            max_buffer_frames: 64,
        })
        .expect("prepared");

    processor.panic_token(Some(7));
    run(&mut processor, true, 64, 64);
    // The next block, no token given for it.
    run(&mut processor, true, 64, 64);
    // A token with no panic in its block.
    processor.panic_token(Some(8));
    run(&mut processor, false, 64, 64);
    // A token whose block is rejected (its channels of two lengths): not kept for the next.
    processor.panic_token(Some(9));
    run(&mut processor, true, 64, 32);
    run(&mut processor, true, 64, 64);

    let seen = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        seen,
        [
            (Some(7), true),
            (None, true),
            (None, false),
            (None, true)
        ]
    );
}

/// An editor that takes the scale it is told, or refuses it, and notes what it was asked.
struct Scaled {
    takes: bool,
    asked: Mutex<Vec<String>>,
}

impl Scaled {
    fn new(takes: bool) -> Self {
        Self {
            takes,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, what: String) {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(what);
    }

    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Editor for Scaled {
    fn spawn(&self, _: ParentWindowHandle, _: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        Box::new(())
    }
    fn size(&self) -> (u32, u32) {
        self.note("size".into());
        (600, 400)
    }
    fn set_scale_factor(&self, factor: f32) -> bool {
        self.note(format!("scale {factor}"));
        self.takes
    }
    fn param_value_changed(&self, _: &str, _: f32) {}
    fn param_modulation_changed(&self, _: &str, _: f32) {}
    fn param_values_changed(&self) {}
}

/// The app's window opens at the scale its editor was told before it was first asked its size,
/// and at the system's where there is none or the editor refuses it (PATCHES.md, change 21; the
/// CA-72's R-WIN-STANDALONE).
#[test]
fn the_apps_window_opens_at_the_scale_its_editor_was_told_first() {
    let editor = Scaled::new(true);
    let options = app_window::options("App".into(), &editor, Some(1.5));
    assert_eq!(editor.asked(), ["scale 1.5", "size"]);
    assert_eq!(options.title, "App");
    assert_eq!(options.size, baseview::Size::new(600.0, 400.0));
    assert_eq!(options.scale, baseview::WindowScalePolicy::ScaleFactor(1.5));

    let refusing = Scaled::new(false);
    let options = app_window::options("App".into(), &refusing, Some(1.25));
    assert_eq!(refusing.asked(), ["scale 1.25", "size"]);
    assert_eq!(options.scale, baseview::WindowScalePolicy::SystemScaleFactor);

    let untold = Scaled::new(true);
    let options = app_window::options("App".into(), &untold, None);
    assert_eq!(untold.asked(), ["size"], "no scale to tell");
    assert_eq!(options.scale, baseview::WindowScalePolicy::SystemScaleFactor);
}

/// A resize from outside fits the window to its editor again on Windows alone, where Windows
/// scales a window whose monitor's scale changes; elsewhere a window manager's size is kept.
#[test]
fn a_resize_refits_the_apps_window_on_windows_alone() {
    let info = baseview::WindowInfo::from_logical_size(baseview::Size::new(600.0, 400.0), 1.0);
    let resized = Event::Window(baseview::WindowEvent::Resized(info));
    assert_eq!(app_window::refits(&resized), cfg!(target_os = "windows"));
    assert!(!app_window::refits(&Event::Window(
        baseview::WindowEvent::Focused
    )));
}
