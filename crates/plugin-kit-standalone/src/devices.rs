use crate::Config;
use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId {
    pub name: String,
    pub input_channels: u16,
    pub output_channels: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamCapability {
    pub channels: u16,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    /// Unknown buffer limits are represented by None, not invented limits.
    pub buffer_range: Option<[u32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub input_configs: Vec<StreamCapability>,
    pub output_configs: Vec<StreamCapability>,
    pub sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub backend: String,
    pub audio: Vec<DeviceInfo>,
    pub default_output: Option<DeviceId>,
    pub default_sample_rate: u32,
    pub midi_inputs: Vec<String>,
}

/// MIDI errors keep the last valid MIDI inventory while healthy audio discovery
/// remains available. This is never constructed or read by an audio callback.
pub(crate) struct Enumeration {
    pub devices: Devices,
    pub midi_error: Option<String>,
}

/// A successful MIDI client lives until the owning control thread shuts down.
pub(crate) struct MidiDiscovery<C = midir::MidiInput> {
    client: Option<C>,
}

impl<C> Default for MidiDiscovery<C> {
    fn default() -> Self {
        Self { client: None }
    }
}

impl<C> MidiDiscovery<C> {
    fn poll_with(
        &mut self,
        initialize: impl FnOnce() -> Result<C, String>,
        read: impl FnOnce(&C) -> Result<Vec<String>, String>,
    ) -> Result<Vec<String>, String> {
        if self.client.is_none() {
            self.client = Some(initialize()?);
        }
        read(self.client.as_ref().expect("initialized MIDI discovery"))
    }
}

impl MidiDiscovery {
    fn poll(&mut self, app_id: &str) -> Result<Vec<String>, String> {
        self.poll_with(
            || midir::MidiInput::new(app_id).map_err(|e| format!("Could not initialize MIDI: {e}")),
            |input| {
                let before = input.port_count();
                let ports = input.ports();
                let after = input.port_count();
                midi_names(before, &ports, after, |port| {
                    input.port_name(port).map_err(|e| e.to_string())
                })
            },
        )
    }
}

fn midi_names<P>(
    before: usize,
    ports: &[P],
    after: usize,
    mut name: impl FnMut(&P) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    if before != after || ports.len() != before {
        return Err("The MIDI port inventory changed during discovery.".into());
    }
    ports
        .iter()
        .enumerate()
        .map(|(index, port)| {
            name(port).map_err(|e| format!("Could not read MIDI input {}: {e}", index + 1))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiated {
    pub config: Config,
    pub output: DeviceId,
    pub input: Option<DeviceId>,
    pub sample_rate: u32,
    pub buffer_frames: u32,
    pub output_stream_channels: u16,
    pub input_stream_channels: u16,
}

pub fn resolve_device<'a>(
    devices: &'a [DeviceInfo],
    id: &DeviceId,
) -> Result<&'a DeviceInfo, String> {
    let mut matches = devices.iter().filter(|device| device.id == *id);
    let found = matches.next().ok_or_else(|| {
        format!(
            "Audio device '{}' is unavailable. Refresh or select a device.",
            id.name
        )
    })?;
    if matches.next().is_some() {
        return Err(format!(
            "Audio device '{}' is ambiguous. Rename duplicate devices or select a distinct device.",
            id.name
        ));
    }
    Ok(found)
}

fn choose(
    capabilities: &[StreamCapability],
    channels: [u16; 2],
    rate: u32,
    frames: u32,
) -> Result<u16, String> {
    let needed = channels[0]
        .max(channels[1])
        .checked_add(1)
        .ok_or("Channel index is out of range")?;
    capabilities
        .iter()
        .filter(|c| {
            c.channels >= needed
                && c.min_sample_rate <= rate
                && rate <= c.max_sample_rate
                && c.buffer_range
                    .is_none_or(|[min, max]| min <= frames && frames <= max)
        })
        .map(|c| c.channels)
        .min()
        .ok_or_else(|| {
            "Selected channels, sample rate or buffer size are unsupported by this device.".into()
        })
}

pub fn negotiate(config: &Config, devices: &Devices) -> Result<Negotiated, String> {
    if !(16..=8192).contains(&config.buffer_frames) {
        return Err("Buffer size must be between 16 and 8192 frames.".into());
    }
    if config.midi_channel.is_some_and(|c| !(1..=16).contains(&c)) {
        return Err("MIDI channel must be ALL or 1–16.".into());
    }
    if config.midi_inputs.len() > 64 {
        return Err("At most 64 MIDI inputs may be selected.".into());
    }
    for (index, name) in config.midi_inputs.iter().enumerate() {
        if config.midi_inputs[..index].contains(name) {
            return Err("A MIDI input may only be selected once.".into());
        }
    }
    let output_id = config
        .output
        .as_ref()
        .or(devices.default_output.as_ref())
        .ok_or("No audio output is available. Connect a device and refresh.")?;
    let output = resolve_device(&devices.audio, output_id)?;
    let rate = if config.sample_rate == 0 {
        output.default_sample_rate
    } else {
        config.sample_rate
    };
    if !(8000..=384000).contains(&rate) {
        return Err("Sample rate must be between 8000 and 384000 Hz.".into());
    }
    let mut config = config.clone();
    if config.output.is_none() && output.id.output_channels == 1 && config.output_channels == [0, 1]
    {
        config.output_channels = [0, 0];
    }
    let output_channels = choose(
        &output.output_configs,
        config.output_channels,
        rate,
        config.buffer_frames,
    )?;
    let mut input_channels = 0;
    if let Some(id) = &config.input {
        let input = resolve_device(&devices.audio, id)?;
        input_channels = choose(
            &input.input_configs,
            config.input_channels,
            rate,
            config.buffer_frames,
        )?;
    }
    // MIDI devices are independently optional. Missing/ambiguous ports produce
    // warnings when connecting, while other inputs and audio remain available.
    config.sample_rate = rate;
    let buffer_frames = config.buffer_frames;
    Ok(Negotiated {
        output: output.id.clone(),
        input: config.input.clone(),
        config,
        sample_rate: rate,
        buffer_frames,
        output_stream_channels: output_channels,
        input_stream_channels: input_channels,
    })
}

fn capability(c: cpal::SupportedStreamConfigRange) -> StreamCapability {
    StreamCapability {
        channels: c.channels(),
        min_sample_rate: c.min_sample_rate().0,
        max_sample_rate: c.max_sample_rate().0,
        buffer_range: match c.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => Some([*min, *max]),
            cpal::SupportedBufferSize::Unknown => None,
        },
    }
}

pub(crate) fn describe(device: &cpal::Device) -> Result<DeviceInfo, String> {
    let name = device.name().map_err(|e| e.to_string())?;
    let inputs = device
        .supported_input_configs()
        .map(|c| c.map(capability).collect::<Vec<_>>())
        .unwrap_or_default();
    let outputs = device
        .supported_output_configs()
        .map(|c| c.map(capability).collect::<Vec<_>>())
        .unwrap_or_default();
    let id = DeviceId {
        name,
        input_channels: inputs.iter().map(|c| c.channels).max().unwrap_or(0),
        output_channels: outputs.iter().map(|c| c.channels).max().unwrap_or(0),
    };
    let sample_rates = [44100, 48000, 88200, 96000, 176400, 192000]
        .into_iter()
        .filter(|rate| {
            outputs
                .iter()
                .any(|c| c.min_sample_rate <= *rate && *rate <= c.max_sample_rate)
        })
        .collect();
    Ok(DeviceInfo {
        id,
        input_configs: inputs,
        output_configs: outputs,
        sample_rates,
        default_sample_rate: device
            .default_output_config()
            .map(|c| c.sample_rate().0)
            .unwrap_or(48000),
    })
}

pub(crate) fn enumerate(
    host: &cpal::Host,
    app_id: &str,
    midi: &mut MidiDiscovery,
) -> Result<Enumeration, String> {
    let mut audio = Vec::new();
    for device in host.devices().map_err(|e| e.to_string())? {
        if let Ok(info) = describe(&device) {
            audio.push(info);
        }
    }
    let default = host.default_output_device();
    let default_output = default
        .as_ref()
        .and_then(|d| describe(d).ok())
        .map(|d| d.id);
    let default_sample_rate = default
        .and_then(|d| d.default_output_config().ok())
        .map(|c| c.sample_rate().0)
        .unwrap_or(48000);
    let (midi_inputs, midi_error) = match midi.poll(app_id) {
        Ok(names) => (names, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(Enumeration {
        devices: Devices {
            backend: format!("{:?}", host.id()),
            audio,
            default_output,
            default_sample_rate,
            midi_inputs,
        },
        midi_error,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn midi_discovery_reuses_a_successful_client_and_retries_initialization_errors() {
        use std::cell::Cell;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Client(Arc<AtomicUsize>);
        impl Drop for Client {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let initialized = Cell::new(0);
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut discovery = MidiDiscovery::<Client>::default();
        assert!(
            discovery
                .poll_with(|| Err("initialization failed".into()), |_| unreachable!())
                .is_err()
        );
        assert!(discovery.client.is_none());
        assert!(
            discovery
                .poll_with(
                    || {
                        initialized.set(initialized.get() + 1);
                        Ok(Client(Arc::clone(&dropped)))
                    },
                    |_| Err("port name failed".into())
                )
                .is_err()
        );
        for _ in 0..100 {
            assert_eq!(
                discovery
                    .poll_with(
                        || panic!("successful client must not be recreated"),
                        |_| Ok(vec!["Keys".into()]),
                    )
                    .expect("recovered inventory"),
                ["Keys"]
            );
        }
        assert_eq!(initialized.get(), 1);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        drop(discovery);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn midi_names_reject_partial_or_unstable_results_but_accept_a_real_empty_list() {
        let ports = ["First", "Second"];
        let error = midi_names(2, &ports, 2, |name| {
            if *name == "Second" {
                Err("name query failed".into())
            } else {
                Ok((*name).into())
            }
        })
        .expect_err("one failed name invalidates the entire result");
        assert!(error.contains("input 2") && error.contains("name query failed"));
        assert!(midi_names(2, &ports, 0, |name| Ok((*name).into())).is_err());
        assert!(midi_names(2, &ports[..1], 2, |name| Ok((*name).into())).is_err());
        assert_eq!(
            midi_names(2, &ports, 2, |name| Ok((*name).into())).expect("stable inventory"),
            ports
        );
        let none: [&str; 0] = [];
        assert!(
            midi_names(0, &none, 0, |name| Ok((*name).into()))
                .expect("real unplug")
                .is_empty()
        );
    }

    pub(crate) fn inventory(channels: u16) -> Devices {
        let id = DeviceId {
            name: "Test duplex".into(),
            input_channels: channels,
            output_channels: channels,
        };
        let c = StreamCapability {
            channels,
            min_sample_rate: 44100,
            max_sample_rate: 96000,
            buffer_range: Some([64, 1024]),
        };
        Devices {
            backend: "Fake".into(),
            audio: vec![DeviceInfo {
                id: id.clone(),
                input_configs: vec![c.clone()],
                output_configs: vec![c],
                sample_rates: vec![44100, 48000, 96000],
                default_sample_rate: 48000,
            }],
            default_output: Some(id),
            default_sample_rate: 48000,
            midi_inputs: vec!["Keys".into()],
        }
    }
    #[test]
    fn missing_and_duplicate_identity_fail_closed() {
        let mut devices = inventory(2);
        let id = devices.audio[0].id.clone();
        assert!(resolve_device(&[], &id).is_err());
        devices.audio.push(devices.audio[0].clone());
        assert!(resolve_device(&devices.audio, &id).is_err());
    }
    #[test]
    fn configuration_negotiates_and_rejects_invalid_combinations() {
        let d = inventory(8);
        let mut c = Config::initial();
        c.output_channels = [6, 7];
        c.input = d.default_output.clone();
        c.input_channels = [3, 3];
        let negotiated = negotiate(&c, &d).expect("supported");
        assert_eq!(negotiated.sample_rate, 48000);
        assert_eq!(negotiated.output_stream_channels, 8);
        c.output_channels = [7, 8];
        assert!(negotiate(&c, &d).is_err());
        c.output_channels = [0, 1];
        c.sample_rate = 192000;
        assert!(negotiate(&c, &d).is_err());
        c.sample_rate = 48000;
        c.buffer_frames = 32;
        assert!(negotiate(&c, &d).is_err());
        c.buffer_frames = 256;
        c.midi_channel = Some(17);
        assert!(negotiate(&c, &d).is_err());
        c.midi_channel = None;
        c.midi_inputs = vec!["Missing".into()];
        assert!(negotiate(&c, &d).is_ok());
    }
    #[test]
    fn default_mono_and_independent_capture_endpoints_are_supported() {
        assert_eq!(
            negotiate(&Config::initial(), &inventory(1))
                .expect("mono")
                .config
                .output_channels,
            [0, 0]
        );
        let mut d = inventory(2);
        let mut other = d.audio[0].clone();
        other.id.name = "Other clock".into();
        d.audio.push(other.clone());
        let mut c = Config::initial();
        c.input = Some(other.id);
        assert!(negotiate(&c, &d).is_ok());
    }
}
