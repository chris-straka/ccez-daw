//! Test-only VST3 gain fixture (`ccez-vst3-gain`).
//!
//! A minimal mono gain effect implementing the genuine Steinberg VST3 ABI
//! (via the `vst3` bindings crate: `GetPluginFactory` + `IComponent` +
//! `IAudioProcessor` + `IEditController`), loaded by
//! `ccez_core::plugins::vst3::Vst3Backend` (vst3-host) in tests. It is
//! deliberately boring: one `Gain` param (VST3 id `0`, normalized
//! `0.0..=1.0`, default `1.0`), mono in/out, and a fixed declared latency
//! of [`FIXTURE_LATENCY`] samples (the lookahead a real limiter would
//! report; the host compensates it with
//! [`LatencyMap`](ccez_core::plugins::latency::LatencyMap), the worker
//! never shifts audio itself).
//!
//! State persistence is real: `getState`/`setState` (on both the processor
//! and the controller) carry the gain as 8 little-endian bytes, so the
//! worker's opaque `blob` round-trips through kill + restore byte-exact.
//!
//! This crate is NOT a workspace member and is never shipped: the roundtrip
//! tests build it as a `cdylib`, stage it as a real `CcezGain.vst3` bundle
//! (`Contents/MacOS/<binary>` on macOS, `Contents/x86_64-linux/<binary>.so`
//! on Linux), and load that bundle path through the real sandboxed worker.
//!
//! Adapted from the `vst3` crate's `examples/gain.rs` (coupler-rs vst3-rs),
//! narrowed to mono with latency + state added.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::cell::Cell;
use std::ffi::{c_char, c_void, CString};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{ptr, slice};

use vst3::{uid, Class, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

/// VST3 param id of the one gain parameter.
pub const GAIN_PARAM_ID: u32 = 0;
/// Param name the host matches worker-side (`set_param("gain", …)`).
pub const GAIN_NAME: &str = "Gain";
/// Declared processing latency, in samples at any rate.
pub const FIXTURE_LATENCY: u32 = 32;
/// Magic heading the 12-byte state chunk (`b"CGN1"` + LE f64 gain).
const STATE_MAGIC: [u8; 4] = *b"CGN1";

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let c_string = CString::new(src).unwrap_or_else(|_| CString::default());
    let bytes = c_string.as_bytes_with_nul();

    for (src, dst) in bytes.iter().zip(dst.iter_mut()) {
        *dst = *src as c_char;
    }

    if bytes.len() > dst.len() {
        if let Some(last) = dst.last_mut() {
            *last = 0;
        }
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (src, dst) in src.encode_utf16().zip(dst.iter_mut()) {
        *dst = src as TChar;
        len += 1;
    }

    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

unsafe fn len_wstring(string: *const TChar) -> usize {
    let mut len = 0;

    while *string.offset(len) != 0 {
        len += 1;
    }

    len as usize
}

/// Write the 12-byte state chunk (`MAGIC` + LE gain) into a host stream.
unsafe fn write_gain_state(stream: *mut IBStream, gain: f64) -> tresult {
    if stream.is_null() {
        return kInvalidArgument;
    }
    let mut chunk = [0u8; 12];
    chunk[..4].copy_from_slice(&STATE_MAGIC);
    chunk[4..].copy_from_slice(&gain.to_le_bytes());
    let mut written = 0;
    let result = ((*(*stream).vtbl).write)(
        stream,
        chunk.as_ptr() as *mut c_void,
        chunk.len() as int32,
        &mut written,
    );
    if result == kResultOk && written == chunk.len() as int32 {
        kResultOk
    } else {
        kResultFalse
    }
}

/// Read the 12-byte state chunk back; `Ok(gain)` on a well-formed chunk.
unsafe fn read_gain_state(stream: *mut IBStream) -> Result<f64, tresult> {
    if stream.is_null() {
        return Err(kInvalidArgument);
    }
    let mut chunk = [0u8; 12];
    let mut read = 0;
    let result = ((*(*stream).vtbl).read)(
        stream,
        chunk.as_mut_ptr() as *mut c_void,
        chunk.len() as int32,
        &mut read,
    );
    if result != kResultOk || read != chunk.len() as int32 {
        return Err(kResultFalse);
    }
    if chunk[..4] != STATE_MAGIC {
        return Err(kResultFalse);
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&chunk[4..]);
    Ok(f64::from_le_bytes(bytes))
}

const PLUGIN_NAME: &str = "Ccez Gain";

struct GainProcessor {
    gain: AtomicU64,
}

impl Class for GainProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl GainProcessor {
    const CID: TUID = uid(0xC7ECDAE1, 0x07A14C05, 0xB3E20117, 0x9A64A70D);

    fn new() -> GainProcessor {
        GainProcessor {
            gain: AtomicU64::new(1.0f64.to_bits()),
        }
    }
}

impl IPluginBaseTrait for GainProcessor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for GainProcessor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        *class_id = GainController::CID;
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, mediaType: MediaType, dir: BusDirection) -> i32 {
        match mediaType as BusDirections {
            MediaTypes_::kAudio => match dir as BusDirections {
                BusDirections_::kInput => 1,
                BusDirections_::kOutput => 1,
                _ => 0,
            },
            MediaTypes_::kEvent => 0,
            _ => 0,
        }
    }

    unsafe fn getBusInfo(
        &self,
        mediaType: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        match mediaType as MediaTypes {
            MediaTypes_::kAudio => match dir as BusDirections {
                BusDirections_::kInput | BusDirections_::kOutput => {
                    let bus = &mut *bus;

                    bus.mediaType = MediaTypes_::kAudio as MediaType;
                    bus.direction = dir;
                    bus.channelCount = 1;
                    copy_wstring(
                        if dir as BusDirections == BusDirections_::kInput {
                            "Input"
                        } else {
                            "Output"
                        },
                        &mut bus.name,
                    );
                    bus.busType = BusTypes_::kMain as BusType;
                    bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;

                    kResultOk
                }
                _ => kInvalidArgument,
            },
            MediaTypes_::kEvent => kInvalidArgument,
            _ => kInvalidArgument,
        }
    }

    unsafe fn getRoutingInfo(
        &self,
        _in_info: *mut RoutingInfo,
        _out_info: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        _media_type: MediaType,
        _dir: BusDirection,
        _index: i32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        match read_gain_state(state) {
            Ok(gain) => {
                self.gain.store(gain.to_bits(), Ordering::SeqCst);
                kResultOk
            }
            Err(code) => code,
        }
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        write_gain_state(state, f64::from_bits(self.gain.load(Ordering::SeqCst)))
    }
}

impl IAudioProcessorTrait for GainProcessor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        if num_ins != 1 || num_outs != 1 {
            return kResultFalse;
        }

        if *inputs != SpeakerArr::kMono || *outputs != SpeakerArr::kMono {
            return kResultFalse;
        }

        kResultTrue
    }

    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        match dir as BusDirections {
            BusDirections_::kInput | BusDirections_::kOutput => {
                if index == 0 {
                    *arr = SpeakerArr::kMono;
                    kResultOk
                } else {
                    kInvalidArgument
                }
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn canProcessSampleSize(&self, symbolic_sample_size: i32) -> tresult {
        match symbolic_sample_size as SymbolicSampleSizes {
            SymbolicSampleSizes_::kSample32 => kResultOk as i32,
            SymbolicSampleSizes_::kSample64 => kNotImplemented as i32,
            _ => kInvalidArgument,
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        FIXTURE_LATENCY
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let process_data = &*data;

        if let Some(param_changes) = ComRef::from_raw(process_data.inputParameterChanges) {
            let param_count = param_changes.getParameterCount();
            for param_index in 0..param_count {
                if let Some(param_queue) =
                    ComRef::from_raw(param_changes.getParameterData(param_index))
                {
                    let param_id = param_queue.getParameterId();
                    let point_count = param_queue.getPointCount();

                    if param_id == GAIN_PARAM_ID && point_count > 0 {
                        let mut sample_offset = 0;
                        let mut value = 0.0;
                        let result = param_queue.getPoint(
                            point_count - 1,
                            &mut sample_offset,
                            &mut value,
                        );

                        if result == kResultTrue {
                            self.gain.store(value.to_bits(), Ordering::Relaxed);
                        }
                    }
                }
            }
        }

        let gain = f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32;

        let num_samples = process_data.numSamples as usize;

        if process_data.numInputs != 1 || process_data.numOutputs != 1 {
            return kResultOk;
        }

        let input_buses =
            slice::from_raw_parts(process_data.inputs, process_data.numInputs as usize);
        let output_buses =
            slice::from_raw_parts(process_data.outputs, process_data.numOutputs as usize);

        if input_buses[0].numChannels != 1 || output_buses[0].numChannels != 1 {
            return kResultOk;
        }

        let input_channels = slice::from_raw_parts(
            input_buses[0].__field0.channelBuffers32,
            input_buses[0].numChannels as usize,
        );
        let output_channels = slice::from_raw_parts(
            output_buses[0].__field0.channelBuffers32,
            output_buses[0].numChannels as usize,
        );

        let input = slice::from_raw_parts(input_channels[0], num_samples);
        let output = slice::from_raw_parts_mut(output_channels[0], num_samples);

        for (i, o) in input.iter().zip(output.iter_mut()) {
            *o = gain * i;
        }

        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IProcessContextRequirementsTrait for GainProcessor {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        0
    }
}

/// Extra fixture effect classes: the same gain DSP as [`GainProcessor`]
/// with their own class ids, generated by macro so 200 lines of COM
/// boilerplate are not hand-copied per class.
///
/// - `GainTwoProcessor`: a second mono effect — proves select-by-class-id
///   loads a non-first entry.
/// - `StereoGainProcessor` (`$stereo = true`): declares stereo buses while
///   still accepting a mono arrangement, so the host loads it far enough
///   to refuse with `PortLayout` instead of misrouting.
///
/// Both share [`GainController`] (one controller class, like many real
/// bundles that pair several processors with a single controller).
macro_rules! define_fixture_processor {
    ($proc:ident, $cid:expr, $channels:expr, $stereo:expr) => {
        struct $proc {
            gain: AtomicU64,
        }

        impl Class for $proc {
            type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
        }

        impl $proc {
            const CID: TUID = $cid;

            fn new() -> $proc {
                $proc {
                    gain: AtomicU64::new(1.0f64.to_bits()),
                }
            }
        }

        impl IPluginBaseTrait for $proc {
            unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
                kResultOk
            }

            unsafe fn terminate(&self) -> tresult {
                kResultOk
            }
        }

        impl IComponentTrait for $proc {
            unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
                *class_id = GainController::CID;
                kResultOk
            }

            unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
                kResultOk
            }

            unsafe fn getBusCount(&self, mediaType: MediaType, dir: BusDirection) -> i32 {
                match mediaType as BusDirections {
                    MediaTypes_::kAudio => match dir as BusDirections {
                        BusDirections_::kInput => 1,
                        BusDirections_::kOutput => 1,
                        _ => 0,
                    },
                    MediaTypes_::kEvent => 0,
                    _ => 0,
                }
            }

            unsafe fn getBusInfo(
                &self,
                mediaType: MediaType,
                dir: BusDirection,
                index: i32,
                bus: *mut BusInfo,
            ) -> tresult {
                if index != 0 {
                    return kInvalidArgument;
                }
                match mediaType as MediaTypes {
                    MediaTypes_::kAudio => match dir as BusDirections {
                        BusDirections_::kInput | BusDirections_::kOutput => {
                            let bus = &mut *bus;

                            bus.mediaType = MediaTypes_::kAudio as MediaType;
                            bus.direction = dir;
                            bus.channelCount = $channels;
                            copy_wstring(
                                if dir as BusDirections == BusDirections_::kInput {
                                    "Input"
                                } else {
                                    "Output"
                                },
                                &mut bus.name,
                            );
                            bus.busType = BusTypes_::kMain as BusType;
                            bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;

                            kResultOk
                        }
                        _ => kInvalidArgument,
                    },
                    MediaTypes_::kEvent => kInvalidArgument,
                    _ => kInvalidArgument,
                }
            }

            unsafe fn getRoutingInfo(
                &self,
                _in_info: *mut RoutingInfo,
                _out_info: *mut RoutingInfo,
            ) -> tresult {
                kNotImplemented
            }

            unsafe fn activateBus(
                &self,
                _media_type: MediaType,
                _dir: BusDirection,
                _index: i32,
                _state: TBool,
            ) -> tresult {
                kResultOk
            }

            unsafe fn setActive(&self, _state: TBool) -> tresult {
                kResultOk
            }

            unsafe fn setState(&self, state: *mut IBStream) -> tresult {
                match read_gain_state(state) {
                    Ok(gain) => {
                        self.gain.store(gain.to_bits(), Ordering::SeqCst);
                        kResultOk
                    }
                    Err(code) => code,
                }
            }

            unsafe fn getState(&self, state: *mut IBStream) -> tresult {
                write_gain_state(state, f64::from_bits(self.gain.load(Ordering::SeqCst)))
            }
        }

        impl IAudioProcessorTrait for $proc {
            unsafe fn setBusArrangements(
                &self,
                inputs: *mut SpeakerArrangement,
                num_ins: i32,
                outputs: *mut SpeakerArrangement,
                num_outs: i32,
            ) -> tresult {
                if num_ins != 1 || num_outs != 1 {
                    return kResultFalse;
                }

                let mono = *inputs == SpeakerArr::kMono && *outputs == SpeakerArr::kMono;
                let stereo = *inputs == SpeakerArr::kStereo && *outputs == SpeakerArr::kStereo;
                if mono || ($stereo && stereo) {
                    kResultTrue
                } else {
                    kResultFalse
                }
            }

            unsafe fn getBusArrangement(
                &self,
                dir: BusDirection,
                index: i32,
                arr: *mut SpeakerArrangement,
            ) -> tresult {
                match dir as BusDirections {
                    BusDirections_::kInput | BusDirections_::kOutput => {
                        if index == 0 {
                            *arr = if $stereo {
                                SpeakerArr::kStereo
                            } else {
                                SpeakerArr::kMono
                            };
                            kResultOk
                        } else {
                            kInvalidArgument
                        }
                    }
                    _ => kInvalidArgument,
                }
            }

            unsafe fn canProcessSampleSize(&self, symbolic_sample_size: i32) -> tresult {
                match symbolic_sample_size as SymbolicSampleSizes {
                    SymbolicSampleSizes_::kSample32 => kResultOk as i32,
                    SymbolicSampleSizes_::kSample64 => kNotImplemented as i32,
                    _ => kInvalidArgument,
                }
            }

            unsafe fn getLatencySamples(&self) -> u32 {
                FIXTURE_LATENCY
            }

            unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
                kResultOk
            }

            unsafe fn setProcessing(&self, _state: TBool) -> tresult {
                kResultOk
            }

            unsafe fn process(&self, data: *mut ProcessData) -> tresult {
                let process_data = &*data;

                if let Some(param_changes) = ComRef::from_raw(process_data.inputParameterChanges) {
                    let param_count = param_changes.getParameterCount();
                    for param_index in 0..param_count {
                        if let Some(param_queue) =
                            ComRef::from_raw(param_changes.getParameterData(param_index))
                        {
                            let param_id = param_queue.getParameterId();
                            let point_count = param_queue.getPointCount();

                            if param_id == GAIN_PARAM_ID && point_count > 0 {
                                let mut sample_offset = 0;
                                let mut value = 0.0;
                                let result = param_queue.getPoint(
                                    point_count - 1,
                                    &mut sample_offset,
                                    &mut value,
                                );

                                if result == kResultTrue {
                                    self.gain.store(value.to_bits(), Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }

                let gain = f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32;

                let num_samples = process_data.numSamples as usize;

                if process_data.numInputs != 1 || process_data.numOutputs != 1 {
                    return kResultOk;
                }

                let input_buses =
                    slice::from_raw_parts(process_data.inputs, process_data.numInputs as usize);
                let output_buses =
                    slice::from_raw_parts(process_data.outputs, process_data.numOutputs as usize);

                if input_buses[0].numChannels != 1 || output_buses[0].numChannels != 1 {
                    return kResultOk;
                }

                let input_channels = slice::from_raw_parts(
                    input_buses[0].__field0.channelBuffers32,
                    input_buses[0].numChannels as usize,
                );
                let output_channels = slice::from_raw_parts(
                    output_buses[0].__field0.channelBuffers32,
                    output_buses[0].numChannels as usize,
                );

                let input = slice::from_raw_parts(input_channels[0], num_samples);
                let output = slice::from_raw_parts_mut(output_channels[0], num_samples);

                for (i, o) in input.iter().zip(output.iter_mut()) {
                    *o = gain * i;
                }

                kResultOk
            }

            unsafe fn getTailSamples(&self) -> u32 {
                0
            }
        }

        impl IProcessContextRequirementsTrait for $proc {
            unsafe fn getProcessContextRequirements(&self) -> u32 {
                0
            }
        }
    };
}

define_fixture_processor!(
    GainTwoProcessor,
    uid(0xC7ECDAE2, 0x07A14C05, 0xB3E20117, 0x9A64A70D),
    1,
    false
);
define_fixture_processor!(
    StereoGainProcessor,
    uid(0xC7ECDAE3, 0x07A14C05, 0xB3E20117, 0x9A64A70D),
    2,
    true
);

const PLUGIN_NAME_TWO: &str = "Ccez Gain Two";
const PLUGIN_NAME_STEREO: &str = "Ccez Stereo Gain";

struct GainController {
    gain: Cell<f64>,
}

impl Class for GainController {
    type Interfaces = (IEditController,);
}

impl GainController {
    const CID: TUID = uid(0x3C1ED90B, 0x44A0B6E2, 0x71C0FF5A, 0x2D88C41F);

    fn new() -> GainController {
        GainController {
            gain: Cell::new(1.0),
        }
    }
}

impl IPluginBaseTrait for GainController {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for GainController {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        match read_gain_state(state) {
            Ok(gain) => {
                self.gain.set(gain);
                kResultOk
            }
            Err(code) => code,
        }
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        write_gain_state(state, self.gain.get())
    }

    unsafe fn getParameterCount(&self) -> i32 {
        1
    }

    unsafe fn getParameterInfo(&self, param_index: i32, info: *mut ParameterInfo) -> tresult {
        match param_index {
            0 => {
                let info = &mut *info;

                info.id = GAIN_PARAM_ID;
                copy_wstring(GAIN_NAME, &mut info.title);
                copy_wstring(GAIN_NAME, &mut info.shortTitle);
                copy_wstring("", &mut info.units);
                info.stepCount = 0;
                info.defaultNormalizedValue = 1.0;
                info.unitId = 0;
                info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32;

                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getParamStringByValue(
        &self,
        id: u32,
        value_normalized: f64,
        string: *mut String128,
    ) -> tresult {
        let slice = unsafe { &mut *string };

        match id {
            GAIN_PARAM_ID => {
                let display = value_normalized.to_string();
                copy_wstring(&display, slice);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getParamValueByString(
        &self,
        id: u32,
        string: *mut TChar,
        value_normalized: *mut f64,
    ) -> tresult {
        match id {
            GAIN_PARAM_ID => {
                let len = len_wstring(string as *const TChar);
                if let Ok(string) =
                    String::from_utf16(slice::from_raw_parts(string as *const u16, len))
                {
                    if let Ok(value) = f64::from_str(&string) {
                        *value_normalized = value;
                        return kResultOk;
                    }
                }
                kInvalidArgument
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn normalizedParamToPlain(&self, id: u32, value_normalized: f64) -> f64 {
        match id {
            GAIN_PARAM_ID => value_normalized,
            _ => 0.0,
        }
    }

    unsafe fn plainParamToNormalized(&self, id: u32, plain_value: f64) -> f64 {
        match id {
            GAIN_PARAM_ID => plain_value,
            _ => 0.0,
        }
    }

    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        match id {
            GAIN_PARAM_ID => self.gain.get(),
            _ => 0.0,
        }
    }

    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> tresult {
        match id {
            GAIN_PARAM_ID => {
                self.gain.set(value);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }

    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

struct Factory {}

impl Class for Factory {
    type Interfaces = (IPluginFactory,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;

        copy_cstring("ccez", &mut info.vendor);
        copy_cstring("https://example.com", &mut info.url);
        copy_cstring("test@example.com", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;

        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        4
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        match index {
            0 => {
                let info = &mut *info;
                info.cid = GainProcessor::CID;
                info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
                copy_cstring("Audio Module Class", &mut info.category);
                copy_cstring(PLUGIN_NAME, &mut info.name);

                kResultOk
            }
            1 => {
                let info = &mut *info;
                info.cid = GainTwoProcessor::CID;
                info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
                copy_cstring("Audio Module Class", &mut info.category);
                copy_cstring(PLUGIN_NAME_TWO, &mut info.name);

                kResultOk
            }
            2 => {
                let info = &mut *info;
                info.cid = StereoGainProcessor::CID;
                info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
                copy_cstring("Audio Module Class", &mut info.category);
                copy_cstring(PLUGIN_NAME_STEREO, &mut info.name);

                kResultOk
            }
            3 => {
                let info = &mut *info;
                info.cid = GainController::CID;
                info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
                copy_cstring("Component Controller Class", &mut info.category);
                copy_cstring(PLUGIN_NAME, &mut info.name);

                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let instance = match *(cid as *const TUID) {
            GainProcessor::CID => Some(
                ComWrapper::new(GainProcessor::new())
                    .to_com_ptr::<FUnknown>()
                    .unwrap(),
            ),
            GainTwoProcessor::CID => Some(
                ComWrapper::new(GainTwoProcessor::new())
                    .to_com_ptr::<FUnknown>()
                    .unwrap(),
            ),
            StereoGainProcessor::CID => Some(
                ComWrapper::new(StereoGainProcessor::new())
                    .to_com_ptr::<FUnknown>()
                    .unwrap(),
            ),
            GainController::CID => Some(
                ComWrapper::new(GainController::new())
                    .to_com_ptr::<FUnknown>()
                    .unwrap(),
            ),
            _ => None,
        };

        if let Some(instance) = instance {
            let ptr = instance.as_ptr();
            ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj)
        } else {
            kInvalidArgument
        }
    }
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleExit() -> bool {
    true
}

// vst3-host resolves the entry points through CoreFoundation
// (`CFBundleGetFunctionPointerForName(bundle, "bundleEntry")`), which is
// case-sensitive: the lowercase spells are REQUIRED on macOS, alongside
// the canonical `BundleEntry` above.
#[cfg(target_os = "macos")]
#[no_mangle]
extern "C" fn bundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "C" fn bundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_library_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory {})
        .to_com_ptr::<IPluginFactory>()
        .unwrap()
        .into_raw()
}
