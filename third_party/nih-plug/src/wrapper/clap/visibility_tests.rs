//! The host capability is queried only during init and reaches the real GUI context.
//! No native editor, host application, audio device or processing thread is opened.
use super::*;
use crate::prelude::*;
use clap_sys::version::CLAP_VERSION;
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct ProbeParams;
// SAFETY: this fixture exposes no parameters or persistent fields.
unsafe impl Params for ProbeParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        Vec::new()
    }
}

#[derive(Default)]
struct ProbePlugin {
    params: Arc<ProbeParams>,
}
impl Plugin for ProbePlugin {
    const NAME: &'static str = "visibility propagation test";
    const VENDOR: &'static str = "test";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[];
    type SysExMessage = ();
    type BackgroundTask = ();
    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }
    fn process(
        &mut self,
        _: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}
impl ClapPlugin for ProbePlugin {
    const CLAP_ID: &'static str = "com.idlefoundry.visibility-propagation-test";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[];
}

struct HostState {
    remote: bool,
    may_query: AtomicBool,
    early_queries: AtomicUsize,
    marker_queries: AtomicUsize,
}
unsafe extern "C" fn extension(host: *const clap_host, name: *const c_char) -> *const c_void {
    // SAFETY: the fixture's host and state outlive the wrapper; names follow CLAP's contract.
    let state = unsafe { &*(*host).host_data.cast::<HostState>() };
    if !state.may_query.load(Ordering::Relaxed) {
        state.early_queries.fetch_add(1, Ordering::Relaxed);
    }
    if unsafe { CStr::from_ptr(name) } == HOST_NATIVE_WINDOW_OCCLUSION_UNRELIABLE {
        state.marker_queries.fetch_add(1, Ordering::Relaxed);
        if state.remote {
            // Only presence is consumed; the production host returns a static version-1 marker.
            static MARKER: u32 = 1;
            return (&MARKER as *const u32).cast();
        }
    }
    std::ptr::null()
}
unsafe extern "C" fn request(_: *const clap_host) {}

struct Fixture {
    // Drop the context and wrapper before their host and host data.
    gui: Arc<WrapperGuiContext<ProbePlugin>>,
    _wrapper: Arc<Wrapper<ProbePlugin>>,
    _host: Box<clap_host>,
    state: Box<HostState>,
}

fn fixture(remote: bool) -> Fixture {
    let state = Box::new(HostState {
        remote,
        may_query: AtomicBool::new(false),
        early_queries: AtomicUsize::new(0),
        marker_queries: AtomicUsize::new(0),
    });
    let host = Box::new(clap_host {
        clap_version: CLAP_VERSION,
        host_data: (&*state as *const HostState).cast_mut().cast(),
        name: c"unknown host".as_ptr(),
        vendor: c"".as_ptr(),
        url: c"".as_ptr(),
        version: c"0".as_ptr(),
        get_extension: Some(extension),
        request_restart: Some(request),
        request_process: Some(request),
        request_callback: Some(request),
    });
    // SAFETY: the fixture's stable host pointer remains alive until the wrapper is dropped.
    let wrapper = unsafe { Wrapper::<ProbePlugin>::new(&*host) };
    assert_eq!(state.early_queries.load(Ordering::Relaxed), 0);
    assert_eq!(state.marker_queries.load(Ordering::Relaxed), 0);
    state.may_query.store(true, Ordering::Relaxed);
    let plugin = {
        let plugin = wrapper.clap_plugin.borrow();
        (&*plugin) as *const clap_plugin
    };
    assert!(unsafe { Wrapper::<ProbePlugin>::init(plugin) });
    assert_eq!(state.marker_queries.load(Ordering::Relaxed), 1);
    let gui = wrapper.clone().make_gui_context();
    Fixture {
        gui,
        _wrapper: wrapper,
        _host: host,
        state,
    }
}

fn check(remote: bool) {
    let f = fixture(remote);
    assert_eq!(f.gui.plugin_api(), PluginApi::Clap);
    for _ in 0..10 {
        assert_eq!(f.gui.native_window_occlusion_reliable(), !remote);
    }
    assert_eq!(
        f.state.marker_queries.load(Ordering::Relaxed),
        1,
        "the frame path must not query again"
    );
}

#[test]
fn an_unknown_host_without_the_marker_retains_native_visibility() {
    check(false);
}
#[test]
fn a_remote_host_marker_reaches_the_gui_context_without_changing_the_plugin_api() {
    check(true);
}
#[test]
fn remote_and_local_instances_keep_their_own_capability() {
    let remote = fixture(true);
    let local = fixture(false);
    assert!(!remote.gui.native_window_occlusion_reliable());
    assert!(local.gui.native_window_occlusion_reliable());
    assert!(!remote.gui.native_window_occlusion_reliable());
}
