//! Test-only CLAP gain fixture bundle (`ccez-clap-gain`).
//!
//! A minimal gain effect built with `clack-plugin`, loaded by
//! `ccez_core::plugins::clap::ClapBackend` in tests. It is deliberately
//! boring: one `gain` param (id [`GAIN_ID`], 0.0..=4.0, default 1.0) and a
//! fixed declared latency of [`FIXTURE_LATENCY`] samples (the lookahead a
//! real limiter would report; the host compensates it with
//! [`LatencyMap`](ccez_core::plugins::latency::LatencyMap), the worker
//! never shifts audio itself).
//!
//! The bundle exposes three plugins through one custom entry, so the host
//! can prove multi-plugin selection instead of always instantiating index
//! 0:
//!
//! - [`PLUGIN_ID`] (`Ccez Gain`, mono) — index 0, the default every
//!   pre-existing test loads.
//! - [`PLUGIN_ID_TWO`] (`Ccez Gain Two`, mono) — index 1, proves
//!   select-by-id loads a non-first plugin.
//! - [`PLUGIN_ID_STEREO`] (`Ccez Stereo Gain`, stereo) — index 2, proves
//!   non-mono layouts refuse with `PortLayout` instead of misrouting.
//!
//! This crate is NOT a workspace member and is never shipped: the roundtrip
//! tests build it as a `cdylib`, copy the artifact to `ccez-gain.clap`, and
//! load that path through the real sandboxed worker.

use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_plugin::entry::prelude::*;
use clack_plugin::extensions::PluginExtensions;
use clack_plugin::host::{HostAudioProcessorHandle, HostMainThreadHandle, HostSharedHandle};
use clack_plugin::prelude::*;

use clack_plugin::events::event_types::ParamValueEvent;
use clack_plugin::events::io::{InputEvents, OutputEvents};
use clack_plugin::process::audio::ChannelPair;

/// Stable CLAP id of the one gain parameter (`ClapId` forbids 0).
pub const GAIN_ID: u32 = 1;
/// Param name the host matches worker-side (`set_param("gain", …)`).
pub const GAIN_NAME: &str = "gain";
/// Declared processing latency, in samples at any rate.
pub const FIXTURE_LATENCY: u32 = 64;
/// CLAP plugin id of the default mono gain effect (index 0).
pub const PLUGIN_ID: &CStr = c"com.ccez.gain";
/// CLAP plugin id of the second mono gain effect (index 1).
pub const PLUGIN_ID_TWO: &CStr = c"com.ccez.gain-two";
/// CLAP plugin id of the stereo gain effect (index 2, refused by the host).
pub const PLUGIN_ID_STEREO: &CStr = c"com.ccez.stereo-gain";

fn f64_bits(v: f64) -> u64 {
    v.to_bits()
}

/// Thread-safe gain cell: the main thread answers `get_value` from it and
/// the audio thread publishes event-driven updates into it.
pub struct GainShared {
    gain_bits: AtomicU64,
}

impl<'a> PluginShared<'a> for GainShared {}

/// Main-thread state: holds the shared gain plus the fixed latency.
/// `CHANNELS` is the port width this instance reports (1 = mono, the only
/// layout the host accepts; 2 = the stereo refusal probe).
pub struct GainMainThread<'a, const CHANNELS: u32> {
    shared: &'a GainShared,
}

impl<'a, const CHANNELS: u32> PluginMainThread<'a, GainShared> for GainMainThread<'a, CHANNELS> {}

impl<const CHANNELS: u32> PluginMainThreadParams for GainMainThread<'_, CHANNELS> {
    fn count(&self) -> u32 {
        1
    }

    fn get_info(&self, param_index: u32, info: &mut ParamInfoWriter) {
        if param_index != 0 {
            return;
        }
        info.set(&ParamInfo {
            id: ClapId::new(GAIN_ID),
            flags: ParamInfoFlags::IS_AUTOMATABLE,
            cookie: Default::default(),
            name: GAIN_NAME.as_bytes(),
            module: b"",
            min_value: 0.0,
            max_value: 4.0,
            default_value: 1.0,
        });
    }

    fn get_value(&self, param_id: ClapId) -> Option<f64> {
        if param_id.get() == GAIN_ID {
            Some(f64::from_bits(
                self.shared.gain_bits.load(Ordering::SeqCst),
            ))
        } else {
            None
        }
    }

    fn value_to_text(
        &self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> core::fmt::Result {
        if param_id.get() != GAIN_ID {
            return Err(core::fmt::Error);
        }
        use core::fmt::Write as _;
        write!(writer, "{value:.3}")?;
        Ok(())
    }

    fn text_to_value(&self, param_id: ClapId, text: &CStr) -> Option<f64> {
        if param_id.get() != GAIN_ID {
            return None;
        }
        text.to_str().ok()?.trim().parse().ok()
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        apply_param_events(&self.shared.gain_bits, input);
    }
}

impl<const CHANNELS: u32> PluginLatencyImpl for GainMainThread<'_, CHANNELS> {
    fn get(&self) -> u32 {
        FIXTURE_LATENCY
    }
}

impl<const CHANNELS: u32> PluginAudioPortsImpl for GainMainThread<'_, CHANNELS> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index != 0 {
            return;
        }
        let (name, port_type): (&[u8], _) = if CHANNELS == 2 {
            (b"Stereo", AudioPortType::STEREO)
        } else {
            (b"Mono", AudioPortType::MONO)
        };
        writer.set(&AudioPortInfo {
            id: ClapId::new(1),
            name,
            channel_count: CHANNELS,
            flags: AudioPortFlags::IS_MAIN,
            port_type: Some(port_type),
            in_place_pair: None,
        });
    }
}

/// Apply incoming param-value events to the shared gain cell.
fn apply_param_events(cell: &AtomicU64, input: &InputEvents) {
    for event in input.iter() {
        if let Some(param) = event.as_event::<ParamValueEvent>() {
            if param.param_id().is_some_and(|id| id.get() == GAIN_ID) {
                cell.store(f64_bits(param.value()), Ordering::SeqCst);
            }
        }
    }
}

/// The audio processor: stateless gain, everything flows through events.
/// Channel-generic, so the stereo probe renders the same math the host
/// refuses to route.
pub struct GainAudio<'a, const CHANNELS: u32> {
    shared: &'a GainShared,
}

impl<'a, const CHANNELS: u32> PluginAudioProcessor<'a, GainShared, GainMainThread<'a, CHANNELS>>
    for GainAudio<'a, CHANNELS>
{
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &GainMainThread<'a, CHANNELS>,
        shared: &'a GainShared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self { shared })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        apply_param_events(&self.shared.gain_bits, events.input);
        let gain = f64::from_bits(self.shared.gain_bits.load(Ordering::SeqCst)) as f32;
        for mut pair in &mut audio {
            let Some(channels) = pair.channels()?.into_f32() else {
                continue;
            };
            for channel in channels {
                match channel {
                    ChannelPair::InputOutput(input, output) => {
                        for (i, o) in input.iter().zip(output.iter_mut()) {
                            *o = *i * gain;
                        }
                    }
                    ChannelPair::InPlace(buf) => {
                        for s in buf.iter_mut() {
                            *s *= gain;
                        }
                    }
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(buf) => buf.fill(0.0),
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

impl<const CHANNELS: u32> PluginAudioProcessorParams for GainAudio<'_, CHANNELS> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        apply_param_events(&self.shared.gain_bits, input);
    }
}

/// The plugin type tying the three thread roles together.
/// `CHANNELS` selects the reported port width (see [`GainMainThread`]).
pub struct GainPlugin<const CHANNELS: u32>;

impl<const CHANNELS: u32> Plugin for GainPlugin<CHANNELS> {
    type AudioProcessor<'a> = GainAudio<'a, CHANNELS>;
    type Shared<'a> = GainShared;
    type MainThread<'a> = GainMainThread<'a, CHANNELS>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Self::Shared<'_>>) {
        builder.register::<PluginParams>();
        builder.register::<PluginLatency>();
        builder.register::<PluginAudioPorts>();
    }
}

fn new_shared(_host: HostSharedHandle<'_>) -> Result<GainShared, PluginError> {
    Ok(GainShared {
        gain_bits: AtomicU64::new(f64_bits(1.0)),
    })
}

fn new_main_thread<'a, const CHANNELS: u32>(
    _host: HostMainThreadHandle<'a>,
    shared: &'a GainShared,
) -> Result<GainMainThread<'a, CHANNELS>, PluginError> {
    Ok(GainMainThread { shared })
}

/// The bundle entry: one factory exposing all three fixture plugins, so
/// index 0 stays the default mono gain while index 1+ prove selection and
/// layout refusal.
pub struct FixtureEntry {
    factory: PluginFactoryWrapper<FixtureFactory>,
}

impl Entry for FixtureEntry {
    fn new(_bundle_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        Ok(Self {
            factory: PluginFactoryWrapper::new(FixtureFactory::new()),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.factory);
    }
}

struct FixtureFactory {
    mono: PluginDescriptor,
    mono_two: PluginDescriptor,
    stereo: PluginDescriptor,
}

impl FixtureFactory {
    fn new() -> Self {
        Self {
            mono: PluginDescriptor::new(PLUGIN_ID.to_str().expect("id"), "Ccez Gain"),
            mono_two: PluginDescriptor::new(PLUGIN_ID_TWO.to_str().expect("id"), "Ccez Gain Two"),
            stereo: PluginDescriptor::new(PLUGIN_ID_STEREO.to_str().expect("id"), "Ccez Stereo Gain"),
        }
    }
}

impl PluginFactoryImpl for FixtureFactory {
    fn plugin_count(&self) -> u32 {
        3
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        match index {
            0 => Some(&self.mono),
            1 => Some(&self.mono_two),
            2 => Some(&self.stereo),
            _ => None,
        }
    }

    fn create_plugin<'a>(
        &'a self,
        host_info: HostInfo<'a>,
        plugin_id: &CStr,
    ) -> Option<PluginInstance<'a>> {
        if plugin_id == self.mono.id().unwrap_or_default() {
            Some(PluginInstance::new::<GainPlugin<1>>(
                host_info,
                &self.mono,
                new_shared,
                new_main_thread::<1>,
            ))
        } else if plugin_id == self.mono_two.id().unwrap_or_default() {
            Some(PluginInstance::new::<GainPlugin<1>>(
                host_info,
                &self.mono_two,
                new_shared,
                new_main_thread::<1>,
            ))
        } else if plugin_id == self.stereo.id().unwrap_or_default() {
            Some(PluginInstance::new::<GainPlugin<2>>(
                host_info,
                &self.stereo,
                new_shared,
                new_main_thread::<2>,
            ))
        } else {
            None
        }
    }
}

clack_export_entry!(FixtureEntry);
