//! Helpers for safely constructing [`Buffer`]s from a plugin host's audio buffers.

use std::num::NonZeroU32;
use std::ptr::NonNull;

use crate::prelude::{AudioIOLayout, Buffer};

/// Buffers created using [`create_buffers`]. At some point the main `Plugin::process()` should
/// probably also take an argument like this instead of main+aux buffers if we also want to provide
/// access to overflowing input channels for e.g. stereo to mono plugins.
pub struct Buffers<'a, 'buffer: 'a> {
    pub main_buffer: &'a mut Buffer<'buffer>,

    // We can't use `AuxiliaryBuffers` here directly because we need different lifetimes for `'a`
    // and `'buffer` while `AuxiliaryBuffers` uses the same lifetime for both.
    pub aux_inputs: &'a mut [Buffer<'buffer>],
    pub aux_outputs: &'a mut [Buffer<'buffer>],
}

/// A helper for safely creating and initializing [`Buffer`]s based on the host's input and output
/// buffers.
pub struct BufferManager {
    // These are the storage backing the fields in `BufferSource`. The wrapper needs to set these
    // values to match the channel pointers provided by the host. If audio buffers are not provided
    // for a bus, then they should be set to `None`. This helper will then copy data to the buffers
    // or fill them with zeroes if there is no data, while also accounting for in-place main IO
    // buffers.
    main_input_channel_pointers: Option<ChannelPointers>,
    main_output_channel_pointers: Option<ChannelPointers>,
    aux_input_channel_pointers: Vec<Option<ChannelPointers>>,
    aux_output_channel_pointers: Vec<Option<ChannelPointers>>,

    /// The backing buffers that will be filled during `create_buffers`. This `'static` lifetime
    /// will be shortened when returning a reference to these buffers in `create_buffers` to match
    /// the function's lifetime.
    main_buffer: Buffer<'static>,

    aux_input_buffers: Vec<Buffer<'static>>,
    /// Stores the data to back `aux_input_buffers`. We need to copy the host's auxiliary input
    /// buffers to our own first because the `Buffer` API is designed around mutable buffers, and
    /// the host may reuse its input buffers between plugins.
    aux_input_storage: Vec<Vec<Vec<f32>>>,

    aux_output_buffers: Vec<Buffer<'static>>,

    /// Where the plugin writes an output channel the host gave a null pointer for (VST3 hosts do
    /// for a bus they deactivated): one per main output channel and per auxiliary output channel,
    /// discarded after each block.
    main_output_scratch: Vec<Vec<f32>>,
    aux_output_scratch: Vec<Vec<Vec<f32>>>,
}

// SAFETY: The raw pointers in the `ChannelPointers` fields/vectors are only used as scratch storage
//         inside of the `create_buffers()` function.
unsafe impl Send for BufferManager {}
unsafe impl Sync for BufferManager {}

/// Host data that the plugin's [`Buffer`]s should be created from. Leave these fields as `None`
/// values
#[derive(Debug)]
pub struct BufferSource<'a> {
    pub main_input_channel_pointers: &'a mut Option<ChannelPointers>,
    pub main_output_channel_pointers: &'a mut Option<ChannelPointers>,
    pub aux_input_channel_pointers: &'a mut [Option<ChannelPointers>],
    pub aux_output_channel_pointers: &'a mut [Option<ChannelPointers>],
}

/// Pointers to raw multichannel audio data for this port.
#[derive(Debug, Clone, Copy)]
pub struct ChannelPointers {
    /// A raw pointer to an array of f32 arrays, containing one array for each channel. `ptrs` must
    /// contain (at least) `num_channel` `*const f32`s, and each of those inner arrays must contain
    /// (at least) `num_samples` `f32` values, or be a null pointer: an input channel the plugin
    /// then reads as silence, an output channel whose samples are discarded.
    pub ptrs: NonNull<*mut f32>,
    /// The number of audio channels used for this port.
    pub num_channels: usize,
}

impl BufferManager {
    /// Initialize managed buffers for a specific audio IO layout. The actual buffers can be set up
    /// using channel pointer data using [`create_buffers()`][Self::create_buffers()].
    pub fn for_audio_io_layout(max_buffer_size: usize, audio_io_layout: AudioIOLayout) -> Self {
        nih_debug_assert!(
            audio_io_layout
                .main_input_channels
                .map(NonZeroU32::get)
                .unwrap_or(0)
                <= audio_io_layout
                    .main_output_channels
                    .map(NonZeroU32::get)
                    .unwrap_or(0),
            "Stereo-to-mono and other many-to-few audio channel configurations are currently not \
             supported"
        );

        // The buffers are preallocated so that `create_buffers()` can be called without having to
        // allocate
        let mut main_buffer = Buffer::default();
        unsafe {
            main_buffer.set_slices(0, |output_slices| {
                output_slices.resize_with(
                    audio_io_layout
                        .main_output_channels
                        .map(NonZeroU32::get)
                        .unwrap_or(0) as usize,
                    || &mut [],
                );
            })
        };

        let mut aux_input_buffers = Vec::with_capacity(audio_io_layout.aux_input_ports.len());
        let mut aux_input_storage = Vec::with_capacity(audio_io_layout.aux_input_ports.len());
        for num_channels in audio_io_layout.aux_input_ports {
            let mut buffer = Buffer::default();
            unsafe {
                buffer.set_slices(0, |slices| {
                    slices.resize_with(num_channels.get() as usize, || &mut []);
                })
            };

            aux_input_buffers.push(buffer);
            aux_input_storage.push(vec![
                vec![0.0; max_buffer_size];
                num_channels.get() as usize
            ]);
        }

        let mut aux_output_buffers = Vec::with_capacity(audio_io_layout.aux_output_ports.len());
        for num_channels in audio_io_layout.aux_output_ports {
            let mut buffer = Buffer::default();
            unsafe {
                buffer.set_slices(0, |slices| {
                    slices.resize_with(num_channels.get() as usize, || &mut []);
                })
            };

            aux_output_buffers.push(buffer);
        }

        let main_output_scratch = vec![
            vec![0.0; max_buffer_size];
            audio_io_layout
                .main_output_channels
                .map(NonZeroU32::get)
                .unwrap_or(0) as usize
        ];
        let aux_output_scratch = audio_io_layout
            .aux_output_ports
            .iter()
            .map(|num_channels| vec![vec![0.0; max_buffer_size]; num_channels.get() as usize])
            .collect();

        Self {
            main_input_channel_pointers: None,
            main_output_channel_pointers: None,
            aux_input_channel_pointers: vec![None; audio_io_layout.aux_input_ports.len()],
            aux_output_channel_pointers: vec![None; audio_io_layout.aux_output_ports.len()],

            main_buffer,

            aux_input_buffers,
            aux_input_storage,

            aux_output_buffers,

            main_output_scratch,
            aux_output_scratch,
        }
    }

    /// Initialize the buffers using the host provided buffer pointers and return a reference to the
    /// created buffers that can be passed to `Plugin::process()`. This accounts for in-place main
    /// IO, missing channel pointers, null pointers, and mismatching channel counts. All
    /// uninitialized buffer data (aux outputs, and main output channels with no matching input
    /// channel) are filled with zeroes.
    ///
    /// `sample_offset` and `num_samples` can be used to slice a set of host channel pointers for
    /// sample accurate automation. If any of the outputs are missing because the host hasn't
    /// provided enough channels or outputs, then they will be replaced by empty slices.
    ///
    /// An inner channel pointer may be a null pointer, as VST3 hosts give for every channel of a
    /// bus they deactivated (JUCE's hosts among them). Such an input channel is read as silence,
    /// and such an output channel is backed by scratch storage whose samples are discarded, so
    /// that every channel the plugin sees is as long as the block.
    ///
    /// # Safety
    ///
    /// Any provided `ChannelPointers` must point to memory regions that remain valid to read from
    /// or write to for the lifetime of the returned [`Buffers`], apart from null inner pointers.
    pub unsafe fn create_buffers<'a, 'buffer: 'a>(
        &'a mut self,
        sample_offset: usize,
        num_samples: usize,
        set_buffer_sources: impl FnOnce(&mut BufferSource),
    ) -> Buffers<'a, 'buffer> {
        // Make sure the caller can't forget to unset previously set values
        self.main_input_channel_pointers = None;
        self.main_output_channel_pointers = None;
        self.aux_input_channel_pointers.fill(None);
        self.aux_output_channel_pointers.fill(None);
        set_buffer_sources(&mut BufferSource {
            main_input_channel_pointers: &mut self.main_input_channel_pointers,
            main_output_channel_pointers: &mut self.main_output_channel_pointers,
            aux_input_channel_pointers: &mut self.aux_input_channel_pointers,
            aux_output_channel_pointers: &mut self.aux_output_channel_pointers,
        });

        // The main buffer points directly to the main output pointers
        self.main_buffer.set_slices(num_samples, |output_slices| {
            match self.main_output_channel_pointers {
                Some(output_channel_pointers) => {
                    nih_debug_assert_eq!(output_slices.len(), output_channel_pointers.num_channels);
                    for (channel_idx, (output_slice, scratch)) in output_slices
                        .iter_mut()
                        .zip(self.main_output_scratch.iter_mut())
                        .enumerate()
                        .take(output_channel_pointers.num_channels)
                    {
                        *output_slice = match channel_pointer(&output_channel_pointers, channel_idx)
                        {
                            Some(output_channel_pointer) => std::slice::from_raw_parts_mut(
                                output_channel_pointer.as_ptr().add(sample_offset),
                                num_samples,
                            ),
                            None => scratch_slice(scratch, num_samples),
                        };
                    }

                    // If the caller/host should have provided buffer pointers but didn't then we
                    // must get rid of any dangling slices
                    output_slices[output_channel_pointers.num_channels..].fill_with(|| &mut [])
                }
                None => {
                    nih_debug_assert_eq!(output_slices.len(), 0);

                    // Same as above
                    output_slices.fill_with(|| &mut [])
                }
            }
        });

        // Since NIH-plug processes audio in-place, main input data needs to be copied to the main
        // output buffers
        if let (Some(input_channel_pointers), Some(output_channel_pointers)) = (
            self.main_input_channel_pointers,
            self.main_output_channel_pointers,
        ) {
            self.main_buffer.set_slices(num_samples, |output_slices| {
                for (channel_idx, output_slice) in output_slices
                    .iter_mut()
                    .enumerate()
                    .take(input_channel_pointers.num_channels)
                {
                    debug_assert!(channel_idx < output_channel_pointers.num_channels);
                    match channel_pointer(&input_channel_pointers, channel_idx) {
                        // If the host processes the main IO out of place then the inputs need to
                        // be copied to the output buffers. Otherwise the input should already be
                        // there.
                        Some(input_channel_pointer) => {
                            let input = input_channel_pointer.as_ptr().add(sample_offset);
                            if input != output_slice.as_mut_ptr() {
                                output_slice
                                    .copy_from_slice(std::slice::from_raw_parts(input, num_samples))
                            }
                        }
                        // An input channel the host gave no memory for is silent
                        None => output_slice.fill(0.0),
                    }
                }
            });

            // Any excess channels will need to be filled with zeroes since they'd otherwise point
            // to whatever was left in the buffer
            if input_channel_pointers.num_channels < output_channel_pointers.num_channels {
                self.main_buffer.set_slices(num_samples, |output_slices| {
                    for slice in &mut output_slices[input_channel_pointers.num_channels..] {
                        slice.fill(0.0);
                    }
                });
            }
        }

        // Because NIH-plug's `Buffer` type is geared around in-place processing, auxiliary inputs
        // need to be copied to our own buffers first (backed by the 'storage' vectors on this
        // object). That way the plugin can modify those buffers like any other buffers.
        for (input_channel_pointers, (input_storage, input_buffer)) in
            self.aux_input_channel_pointers.iter().zip(
                self.aux_input_storage
                    .iter_mut()
                    .zip(self.aux_input_buffers.iter_mut()),
            )
        {
            // Since these buffers are backed by our own storage, we can fill them with zeroes if
            // the pointers are missing for whatever reason that might be
            nih_debug_assert!(input_channel_pointers.is_some());
            match input_channel_pointers {
                Some(input_channel_pointers) => {
                    nih_debug_assert_eq!(input_channel_pointers.num_channels, input_storage.len());
                    for (channel_idx, channel) in input_storage
                        .iter_mut()
                        .enumerate()
                        .take(input_channel_pointers.num_channels)
                    {
                        nih_debug_assert!(num_samples <= channel.capacity());
                        channel.resize(num_samples, 0.0);
                        match channel_pointer(input_channel_pointers, channel_idx) {
                            Some(input_channel_pointer) => {
                                channel.copy_from_slice(std::slice::from_raw_parts(
                                    input_channel_pointer.as_ptr().add(sample_offset),
                                    num_samples,
                                ))
                            }
                            // A channel the host gave no memory for (a bus it deactivated) is
                            // silent
                            None => channel.fill(0.0),
                        }
                    }

                    // In case we were provided too few channels we'll fill the rest with zeroes to
                    // avoid unexpected situations (as long as the block, like the others: a
                    // length left from an earlier block may be shorter)
                    for channel in input_storage
                        .iter_mut()
                        .skip(input_channel_pointers.num_channels)
                    {
                        channel.resize(num_samples, 0.0);
                        channel.fill(0.0);
                    }
                }
                None => {
                    for channel in input_storage.iter_mut() {
                        channel.resize(num_samples, 0.0);
                        channel.fill(0.0);
                    }
                }
            }

            input_buffer.set_slices(num_samples, |input_slices| {
                // Since we initialized both `input_buffer` and `input_storage` this invariant
                // should never fail unless we made an error ourselves
                debug_assert_eq!(input_slices.len(), input_storage.len());

                for (channel_slice, channel_storage) in
                    input_slices.iter_mut().zip(input_storage.iter_mut())
                {
                    // SAFETY: `channel_storage` is no longer used accessed directly after this
                    *channel_slice = &mut *(channel_storage.as_mut_slice() as *mut [f32]);
                }
            });
        }

        // The auxiliary output buffers can point directly to the host's buffers. This logic is the
        // same as the main outputs, minus the copying of input cdata
        for ((output_channel_pointers, output_buffer), output_scratch) in self
            .aux_output_channel_pointers
            .iter()
            .zip(self.aux_output_buffers.iter_mut())
            .zip(self.aux_output_scratch.iter_mut())
        {
            output_buffer.set_slices(num_samples, |output_slices| {
                match output_channel_pointers {
                    Some(output_channel_pointers) => {
                        nih_debug_assert_eq!(
                            output_slices.len(),
                            output_channel_pointers.num_channels
                        );
                        for (channel_idx, (output_slice, scratch)) in output_slices
                            .iter_mut()
                            .zip(output_scratch.iter_mut())
                            .enumerate()
                            .take(output_channel_pointers.num_channels)
                        {
                            *output_slice =
                                match channel_pointer(output_channel_pointers, channel_idx) {
                                    Some(output_channel_pointer) => std::slice::from_raw_parts_mut(
                                        output_channel_pointer.as_ptr().add(sample_offset),
                                        num_samples,
                                    ),
                                    None => scratch_slice(scratch, num_samples),
                                };

                            // The host may not zero out the buffers, and assume the plugin always
                            // write something there
                            output_slice.fill(0.0);
                        }

                        // If the caller/host should have provided buffer pointers but didn't then
                        // we must get rid of any dangling slices
                        output_slices[output_channel_pointers.num_channels..].fill_with(|| &mut [])
                    }
                    None => {
                        nih_debug_assert_eq!(output_slices.len(), 0);

                        // Same as above
                        output_slices.fill_with(|| &mut [])
                    }
                }
            });
        }

        // SAFETY: The 'static lifetimes on the objects are needed so we can store the buffers.
        //         Their actual lifetimes are `'a`, so we need to shrink them here. The contents are
        //         valid for as long as the returned object is borrowed.
        std::mem::transmute::<Buffers<'a, 'static>, Buffers<'a, 'buffer>>(Buffers {
            main_buffer: &mut self.main_buffer,
            aux_inputs: &mut self.aux_input_buffers,
            aux_outputs: &mut self.aux_output_buffers,
        })
    }
}

/// The host's pointer to one of a port's channels, or `None` where it gave a null pointer.
///
/// # Safety
///
/// `pointers.ptrs` must hold at least `channel_idx + 1` pointers.
unsafe fn channel_pointer(pointers: &ChannelPointers, channel_idx: usize) -> Option<NonNull<f32>> {
    NonNull::new(*pointers.ptrs.as_ptr().add(channel_idx))
}

/// A channel of zeroes as long as the block, backed by `scratch`, for an output channel the host
/// gave a null pointer for. The `'static` lifetime is shortened as the buffers' are.
///
/// # Safety
///
/// `scratch` must not be used otherwise while the returned slice is.
unsafe fn scratch_slice(scratch: &mut Vec<f32>, num_samples: usize) -> &'static mut [f32] {
    // It only allocates if the host's block is longer than the maximum it set up
    if scratch.len() < num_samples {
        scratch.resize(num_samples, 0.0);
    }
    let slice = &mut scratch[..num_samples];
    slice.fill(0.0);
    &mut *(slice as *mut [f32])
}

#[cfg(any(miri, test))]
mod miri {
    use super::*;
    use crate::prelude::{new_nonzero_u32, PortNames};

    const BUFFER_SIZE: usize = 512;
    const NUM_MAIN_INPUT_CHANNELS: usize = 1;
    const NUM_MAIN_OUTPUT_CHANNELS: usize = 2;

    const NUM_AUX_CHANNELS: usize = 2;
    const NUM_AUX_PORTS: usize = 2;

    const AUDIO_IO_LAYOUT: AudioIOLayout = AudioIOLayout {
        main_input_channels: Some(new_nonzero_u32(NUM_MAIN_INPUT_CHANNELS as u32)),
        main_output_channels: Some(new_nonzero_u32(NUM_MAIN_OUTPUT_CHANNELS as u32)),
        aux_input_ports: &[new_nonzero_u32(NUM_AUX_CHANNELS as u32); NUM_AUX_PORTS],
        aux_output_ports: &[new_nonzero_u32(NUM_AUX_CHANNELS as u32); NUM_AUX_PORTS],
        names: PortNames::const_default(),
    };

    #[test]
    fn buffer_io() {
        // This works very similarly to the standalone CPAL and dummy backends
        let mut main_io_storage = vec![vec![0.0f32; BUFFER_SIZE]; NUM_MAIN_OUTPUT_CHANNELS];
        let mut aux_input_storage =
            vec![vec![vec![0.0f32; BUFFER_SIZE]; NUM_AUX_CHANNELS]; NUM_AUX_PORTS];
        let mut aux_output_storage =
            vec![vec![vec![0.0f32; BUFFER_SIZE]; NUM_AUX_CHANNELS]; NUM_AUX_PORTS];

        let mut main_io_channel_pointers: Vec<*mut f32> = main_io_storage
            .iter_mut()
            .map(|channel_slice| channel_slice.as_mut_ptr())
            .collect();
        let mut aux_input_channel_pointers: Vec<Vec<*mut f32>> = aux_input_storage
            .iter_mut()
            .map(|aux_input_storage| {
                aux_input_storage
                    .iter_mut()
                    .map(|channel_slice| channel_slice.as_mut_ptr())
                    .collect()
            })
            .collect();
        let mut aux_output_channel_pointers: Vec<Vec<*mut f32>> = aux_output_storage
            .iter_mut()
            .map(|aux_output_storage| {
                aux_output_storage
                    .iter_mut()
                    .map(|channel_slice| channel_slice.as_mut_ptr())
                    .collect()
            })
            .collect();

        // The actual buffer management here works the same as in the JACK backend. See that
        // implementation for more information.
        let mut buffer_manager = BufferManager::for_audio_io_layout(BUFFER_SIZE, AUDIO_IO_LAYOUT);
        let buffers = unsafe {
            buffer_manager.create_buffers(0, BUFFER_SIZE, |buffer_sources| {
                *buffer_sources.main_output_channel_pointers = Some(ChannelPointers {
                    ptrs: NonNull::new(main_io_channel_pointers.as_mut_ptr()).unwrap(),
                    num_channels: main_io_channel_pointers.len(),
                });
                *buffer_sources.main_input_channel_pointers = Some(ChannelPointers {
                    ptrs: NonNull::new(main_io_channel_pointers.as_mut_ptr()).unwrap(),
                    num_channels: NUM_MAIN_INPUT_CHANNELS.min(main_io_channel_pointers.len()),
                });

                for (input_source_channel_pointers, input_channel_pointers) in buffer_sources
                    .aux_input_channel_pointers
                    .iter_mut()
                    .zip(aux_input_channel_pointers.iter_mut())
                {
                    *input_source_channel_pointers = Some(ChannelPointers {
                        ptrs: NonNull::new(input_channel_pointers.as_mut_ptr()).unwrap(),
                        num_channels: input_channel_pointers.len(),
                    });
                }

                for (output_source_channel_pointers, output_channel_pointers) in buffer_sources
                    .aux_output_channel_pointers
                    .iter_mut()
                    .zip(aux_output_channel_pointers.iter_mut())
                {
                    *output_source_channel_pointers = Some(ChannelPointers {
                        ptrs: NonNull::new(output_channel_pointers.as_mut_ptr()).unwrap(),
                        num_channels: output_channel_pointers.len(),
                    });
                }
            })
        };

        for channel_samples in buffers
            .main_buffer
            .iter_samples()
            .chain(
                buffers
                    .aux_inputs
                    .iter_mut()
                    .flat_map(|buffer| buffer.iter_samples()),
            )
            .chain(
                buffers
                    .aux_outputs
                    .iter_mut()
                    .flat_map(|buffer| buffer.iter_samples()),
            )
        {
            for sample in channel_samples {
                *sample += 1.0;
            }
        }

        // These checks are fine due to stacked borrows even without explicitly dropping `buffers`.
        // If we were to access `buffers` again after this miri would trigger an error.
        for channel in main_io_storage
            .iter()
            .chain(aux_output_storage.iter().flat_map(|storage| storage.iter()))
        {
            for sample in channel {
                assert!(*sample == 1.0);
            }
        }

        for channel in aux_input_storage.iter().flat_map(|storage| storage.iter()) {
            for sample in channel {
                assert!(*sample == 0.0);
            }
        }
    }

    /// A VST3 host gives a null pointer for every channel of a bus it deactivated (JUCE's hosts
    /// do), and may for single channels: inputs read as silence, outputs go to scratch storage,
    /// and every channel is as long as the block.
    #[test]
    fn null_channel_pointers() {
        const OFFSET: usize = 64;
        const LEN: usize = 128;

        // The host's own memory, where it gives any: the main output's first channel, and the
        // first auxiliary input and output ports
        let mut main_output = vec![7.0f32; BUFFER_SIZE];
        let mut aux_input = vec![vec![0.5f32; BUFFER_SIZE]; NUM_AUX_CHANNELS];
        let mut aux_output = vec![vec![7.0f32; BUFFER_SIZE]; NUM_AUX_CHANNELS];

        let mut main_output_pointers = [main_output.as_mut_ptr(), std::ptr::null_mut()];
        let mut main_input_pointers = [std::ptr::null_mut::<f32>(); NUM_MAIN_INPUT_CHANNELS];
        let mut aux_input_pointers = [
            aux_input
                .iter_mut()
                .map(|c| c.as_mut_ptr())
                .collect::<Vec<_>>(),
            vec![std::ptr::null_mut(); NUM_AUX_CHANNELS],
        ];
        let mut aux_output_pointers = [
            aux_output
                .iter_mut()
                .map(|c| c.as_mut_ptr())
                .collect::<Vec<_>>(),
            vec![std::ptr::null_mut(); NUM_AUX_CHANNELS],
        ];

        let mut buffer_manager = BufferManager::for_audio_io_layout(BUFFER_SIZE, AUDIO_IO_LAYOUT);
        let buffers = unsafe {
            buffer_manager.create_buffers(OFFSET, LEN, |buffer_sources| {
                *buffer_sources.main_output_channel_pointers = Some(ChannelPointers {
                    ptrs: NonNull::new(main_output_pointers.as_mut_ptr()).unwrap(),
                    num_channels: main_output_pointers.len(),
                });
                *buffer_sources.main_input_channel_pointers = Some(ChannelPointers {
                    ptrs: NonNull::new(main_input_pointers.as_mut_ptr()).unwrap(),
                    num_channels: main_input_pointers.len(),
                });
                for (source, pointers) in buffer_sources
                    .aux_input_channel_pointers
                    .iter_mut()
                    .zip(aux_input_pointers.iter_mut())
                {
                    *source = Some(ChannelPointers {
                        ptrs: NonNull::new(pointers.as_mut_ptr()).unwrap(),
                        num_channels: pointers.len(),
                    });
                }
                for (source, pointers) in buffer_sources
                    .aux_output_channel_pointers
                    .iter_mut()
                    .zip(aux_output_pointers.iter_mut())
                {
                    *source = Some(ChannelPointers {
                        ptrs: NonNull::new(pointers.as_mut_ptr()).unwrap(),
                        num_channels: pointers.len(),
                    });
                }
            })
        };

        // The main input the host gave no memory for is silence on every main output channel,
        // the one in the host's memory and the one in scratch storage
        assert_eq!(buffers.main_buffer.samples(), LEN);
        for channel in buffers.main_buffer.as_slice() {
            assert_eq!(&channel[..], &[0.0; LEN][..]);
        }
        assert_eq!(&buffers.aux_inputs[0].as_slice()[0][..], &[0.5; LEN][..]);
        for channel in buffers.aux_inputs[1].as_slice() {
            assert_eq!(&channel[..], &[0.0; LEN][..]);
        }
        for buffer in buffers.aux_outputs.iter_mut() {
            for channel in buffer.as_slice() {
                assert_eq!(&channel[..], &[0.0; LEN][..]);
            }
        }

        // The plugin writes every channel it has; the host sees its own, within the block
        for channel in buffers.main_buffer.as_slice() {
            channel.fill(1.0);
        }
        for buffer in buffers.aux_outputs.iter_mut() {
            for channel in buffer.as_slice() {
                channel.fill(1.0);
            }
        }
        for channel in std::iter::once(&main_output).chain(aux_output.iter()) {
            assert!(channel[..OFFSET].iter().all(|&x| x == 7.0));
            assert!(channel[OFFSET..OFFSET + LEN].iter().all(|&x| x == 1.0));
            assert!(channel[OFFSET + LEN..].iter().all(|&x| x == 7.0));
        }
    }
}
