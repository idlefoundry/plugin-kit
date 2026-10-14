use plugin_kit_standalone::*;
use std::io::{self, BufRead, Write};
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
#[derive(Default)]
struct Stats {
    calls: AtomicU64,
    frames: AtomicU64,
    input_nonzero: AtomicU64,
    input_matches: AtomicU64,
    invalid: AtomicU64,
    prepares: AtomicU64,
    deactivates: AtomicU64,
    drops: AtomicU64,
    rate: AtomicU32,
}
struct Probe(Arc<Stats>);
impl Drop for Probe {
    fn drop(&mut self) {
        self.0.drops.fetch_add(1, Ordering::Relaxed);
    }
}
impl AudioProcessor for Probe {
    fn prepare(&mut self, c: ProcessConfig) -> Result<(), String> {
        self.0.prepares.fetch_add(1, Ordering::Relaxed);
        self.0.rate.store(c.sample_rate as u32, Ordering::Relaxed);
        Ok(())
    }
    fn deactivate(&mut self) {
        self.0.deactivates.fetch_add(1, Ordering::Relaxed);
    }
    fn process(&mut self, b: AudioBlock<'_>) {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        self.0
            .frames
            .fetch_add(b.left.len() as u64, Ordering::Relaxed);
        for (input, expected) in b
            .input_left
            .iter()
            .map(|v| (v, 0.125))
            .chain(b.input_right.iter().map(|v| (v, -0.25)))
        {
            if !input.is_finite() {
                self.0.invalid.fetch_add(1, Ordering::Relaxed);
            }
            if input.abs() > 0.01 {
                self.0.input_nonzero.fetch_add(1, Ordering::Relaxed);
                if (input - expected).abs() < 0.001 {
                    self.0.input_matches.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        b.left.fill(0.125);
        b.right.fill(-0.25);
    }
}
fn emit(handle: &AppHandle, stats: &Stats) {
    let s = handle.snapshot();
    println!(
        "{}",
        serde_json::json!({
            "phase":format!("{:?}",s.phase), "error":s.error, "warning":s.warning,
            "rate":s.actual_sample_rate, "period":s.actual_buffer_frames,
            "requested_rate":s.config.sample_rate, "requested_period":s.config.buffer_frames,
            "device_count":s.devices.audio.len(),
            "devices":format!("{:?}",s.devices.audio.iter().map(|d|&d.id).collect::<Vec<_>>()),
            "calls":stats.calls.load(Ordering::Relaxed), "frames":stats.frames.load(Ordering::Relaxed),
            "input_nonzero":stats.input_nonzero.load(Ordering::Relaxed),
            "input_matches":stats.input_matches.load(Ordering::Relaxed),
            "invalid":stats.invalid.load(Ordering::Relaxed), "prepares":stats.prepares.load(Ordering::Relaxed),
            "deactivates":stats.deactivates.load(Ordering::Relaxed), "drops":stats.drops.load(Ordering::Relaxed)
        })
    );
    io::stdout().flush().expect("probe stdout");
}
fn main() {
    if !cfg!(target_os = "linux")
        || std::env::var("PLUGIN_KIT_PRIVATE_PIPEWIRE").as_deref() != Ok("1")
        || std::env::var_os("ALSA_CONFIG_PATH").is_none()
        || std::env::var_os("PIPEWIRE_RUNTIME_DIR").is_none()
    {
        eprintln!(
            "Run only through scripts/standalone-virtual/verify.py, which isolates audio access."
        );
        std::process::exit(2);
    }
    let stats = Arc::new(Stats::default());
    let mut runtime = spawn(
        Box::new(Probe(Arc::clone(&stats))),
        None,
        AppOptions {
            app_id: "PrivateProbe".into(),
            state_dir: std::env::args()
                .nth(1)
                .expect("private state directory")
                .into(),
            device_access: true,
        },
    )
    .expect("private runtime");
    runtime
        .wait_ready(Duration::from_secs(2))
        .expect("startup readiness");
    let handle = runtime.handle();
    println!("READY");
    io::stdout().flush().expect("probe stdout");
    for line in io::stdin().lock().lines() {
        let line = line.expect("probe command");
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.as_slice() {
            ["status"] => emit(&handle, &stats),
            ["apply", rate, period, input] => {
                let mut c = handle.snapshot().config;
                c.sample_rate = rate.parse().expect("requested rate");
                c.buffer_frames = period.parse().expect("requested period");
                c.input_channels = [0, 1];
                c.input = if *input == "on" {
                    handle.snapshot().devices.default_output
                } else {
                    None
                };
                println!("APPLY {:?}", handle.apply(c));
                io::stdout().flush().expect("probe stdout");
            }
            ["refresh"] => {
                println!("REFRESH {:?}", handle.refresh());
                io::stdout().flush().expect("probe stdout");
            }
            ["close"] => {
                let start = Instant::now();
                let result = runtime.shutdown();
                println!("CLOSED {:?} {}ms", result, start.elapsed().as_millis());
                emit(&handle, &stats);
                break;
            }
            _ => {}
        }
    }
}
