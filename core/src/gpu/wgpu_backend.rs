//! wgpu backend: one compute shader behind the `gpu` cargo feature.
//!
//! Teaching note: a GPU compute kernel is three steps — *upload* the
//! input into GPU buffers, *dispatch* a shader where each thread computes
//! one output sample, *read back* the result. The shader below is the
//! same double loop as [`crate::gpu::cpu::cpu_convolve`], except the
//! outer loop over `i` is spread across hundreds of GPU threads, each
//! running the inner loop over `k` privately. Same math, parallel outer
//! loop — that is the whole idea of GPU audio kernels.
//!
//! Without `--features gpu` this module is a stub that always reports
//! [`GpuError::Unavailable`], so the crate builds (and `bun run check`
//! stays green) on machines without GPU toolchains.

use super::runner::{GpuDeviceInfo, GpuError};

/// Power preference used when asking wgpu for an adapter.
/// Mirrors `wgpu::PowerPreference` without leaking the dependency
/// into the feature-off build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerPreference {
    /// Prefer a discrete/high-performance GPU.
    #[default]
    HighPerformance,
    /// Prefer an integrated/low-power GPU (laptops on battery).
    LowPower,
}

#[cfg(feature = "gpu")]
mod real {
    use super::*;

    const WORKGROUP_SIZE: u64 = 64;

    /// WGSL direct-form FIR convolution. One thread per output sample;
    /// the `k` loop order matches `cpu_convolve` so rounding agrees.
    pub const CONVOLVE_WGSL: &str = r#"
struct Params {
    n : u32,
    m : u32,
    _pad0 : u32,
    _pad1 : u32,
};

@group(0) @binding(0) var<storage, read> input : array<f32>;
@group(0) @binding(1) var<storage, read> ir : array<f32>;
@group(0) @binding(2) var<storage, read_write> output : array<f32>;
@group(0) @binding(3) var<uniform> params : Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let i = gid.x;
    if (i >= params.n) {
        return;
    }
    var acc : f32 = 0.0;
    for (var k : u32 = 0u; k < params.m; k++) {
        if (i >= k) {
            acc += input[i - k] * ir[k];
        }
    }
    output[i] = acc;
}
"#;

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct Params {
        n: u32,
        m: u32,
        _pad0: u32,
        _pad1: u32,
    }

    pub struct WgpuRunner {
        device: wgpu::Device,
        queue: wgpu::Queue,
        pipeline: wgpu::ComputePipeline,
        bind_layout: wgpu::BindGroupLayout,
        adapter_name: String,
        adapter_backend: String,
    }

    impl WgpuRunner {
        /// Blocking adapter+device acquisition. Fails gracefully
        /// (returns `Err`, never panics) when no GPU exists — headless
        /// CI, for example.
        pub fn new_blocking(pref: PowerPreference) -> Result<Self, GpuError> {
            let power = match pref {
                PowerPreference::HighPerformance => wgpu::PowerPreference::HighPerformance,
                PowerPreference::LowPower => wgpu::PowerPreference::LowPower,
            };
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
            let adapter = pollster::block_on(instance.request_adapter(
                &wgpu::RequestAdapterOptions {
                    power_preference: power,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                },
            ))
            .ok_or_else(|| GpuError::Unavailable("no wgpu adapter found".to_string()))?;
            let info = adapter.get_info();
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("ccez-gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            ))
            .map_err(|e| GpuError::Unavailable(format!("wgpu device request failed: {e}")))?;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ccez-convolve"),
                source: wgpu::ShaderSource::Wgsl(CONVOLVE_WGSL.into()),
            });
            let bind_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("ccez-convolve-layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ccez-convolve-pipeline-layout"),
                bind_group_layouts: &[&bind_layout],
                push_constant_ranges: &[],
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("ccez-convolve-pipeline"),
                layout: Some(&layout),
                module: &module,
                entry_point: "main",
            });
            Ok(Self {
                device,
                queue,
                pipeline,
                bind_layout,
                adapter_name: info.name,
                adapter_backend: format!("{:?}", info.backend),
            })
        }

        pub fn device_info(&self) -> GpuDeviceInfo {
            GpuDeviceInfo {
                name: self.adapter_name.clone(),
                backend: self.adapter_backend.clone(),
            }
        }

        fn storage_buffer(&self, label: &str, bytes: &[u8], output: bool) -> wgpu::Buffer {
            use wgpu::util::DeviceExt;
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: if output {
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC
                } else {
                    wgpu::BufferUsages::STORAGE
                },
            })
        }

        pub fn run_convolve(
            &self,
            input: &[f32],
            ir: &[f32],
            output: &mut [f32],
        ) -> Result<(), GpuError> {
            let n = input.len().min(output.len()) as u32;
            let m = ir.len() as u32;
            if n == 0 {
                return Ok(());
            }
            if m == 0 {
                output[..n as usize].fill(0.0);
                return Ok(());
            }
            let params = Params {
                n,
                m,
                _pad0: 0,
                _pad1: 0,
            };
            let in_bytes = bytemuck::cast_slice::<f32, u8>(input);
            let ir_bytes = bytemuck::cast_slice::<f32, u8>(ir);
            // Storage buffers of exactly n floats are fine (4-byte aligned).
            let in_buf = self.storage_buffer("ccez-input", in_bytes, false);
            let ir_buf = self.storage_buffer("ccez-ir", ir_bytes, false);
            let out_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ccez-output"),
                size: (n as u64) * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let param_buf = {
                use wgpu::util::DeviceExt;
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("ccez-params"),
                        contents: bytemuck::bytes_of(&params),
                        usage: wgpu::BufferUsages::UNIFORM,
                    })
            };
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ccez-convolve-bind"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: in_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: ir_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: out_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: param_buf.as_entire_binding(),
                    },
                ],
            });
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ccez-staging"),
                size: (n as u64) * 4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ccez-convolve-encoder"),
                });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("ccez-convolve-pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                let groups = n as u64 / WORKGROUP_SIZE + 1;
                pass.dispatch_workgroups(groups as u32, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, (n as u64) * 4);
            self.queue.submit(Some(encoder.finish()));
            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            // Drive the mapping callback to completion.
            self.device.poll(wgpu::Maintain::Wait);
            rx.recv()
                .map_err(|_| GpuError::Execution("staging map channel closed".to_string()))?
                .map_err(|e| GpuError::Execution(format!("staging map failed: {e:?}")))?;
            {
                let view = slice.get_mapped_range();
                let got: &[f32] = bytemuck::cast_slice(&view);
                output[..n as usize].copy_from_slice(&got[..n as usize]);
            }
            staging.unmap();
            Ok(())
        }

        /// Non-panicking adapter enumeration for device selection UI.
        pub fn probe_devices(pref: PowerPreference) -> Vec<GpuDeviceInfo> {
            let power = match pref {
                PowerPreference::HighPerformance => wgpu::PowerPreference::HighPerformance,
                PowerPreference::LowPower => wgpu::PowerPreference::LowPower,
            };
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                ..Default::default()
            });
            let adapters = instance.enumerate_adapters(wgpu::Backends::all());
            let mut out: Vec<GpuDeviceInfo> = adapters
                .iter()
                .map(|a| {
                    let info = a.get_info();
                    GpuDeviceInfo {
                        name: info.name.clone(),
                        backend: format!("{:?} ({:?})", info.backend, info.device_type),
                    }
                })
                .collect();
            // Preferred power class first is best-effort; keep it stable.
            let _ = power;
            out.sort_by(|a, b| a.name.cmp(&b.name));
            out.dedup_by(|a, b| a.name == b.name && a.backend == b.backend);
            out
        }
    }

}

#[cfg(feature = "gpu")]
pub use real::WgpuRunner;
#[cfg(feature = "gpu")]
pub use real::CONVOLVE_WGSL;

/// Feature-off stub: construction always fails with [`GpuError::Unavailable`].
#[cfg(not(feature = "gpu"))]
pub struct WgpuRunner {
    _never: std::convert::Infallible,
}

#[cfg(not(feature = "gpu"))]
impl WgpuRunner {
    pub fn new_blocking(_pref: PowerPreference) -> Result<Self, GpuError> {
        Err(GpuError::Unavailable(
            "crate built without the `gpu` cargo feature".to_string(),
        ))
    }

    pub fn device_info(&self) -> GpuDeviceInfo {
        unreachable!("stub WgpuRunner cannot exist")
    }

    pub fn run_convolve(
        &self,
        _input: &[f32],
        _ir: &[f32],
        _output: &mut [f32],
    ) -> Result<(), GpuError> {
        Err(GpuError::Unavailable(
            "crate built without the `gpu` cargo feature".to_string(),
        ))
    }

    pub fn probe_devices(_pref: PowerPreference) -> Vec<GpuDeviceInfo> {
        Vec::new()
    }
}
