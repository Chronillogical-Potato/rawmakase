//! The stages before the per-pixel develop stage, on the device (`logs.wgsl`,
//! `local.wgsl`): the photo is uploaded once and kept, the local-tone blurs and gain
//! are computed and kept there, and each region is sampled through geometry, lens
//! correction and noise reduction straight into the develop stage's input. Clarity
//! edits on a large photo then no longer recompute full-resolution blurs on the CPU,
//! and panning at 100% no longer samples on the CPU.
use super::{
    Processor,
    develop::{Developer, DeviceSamples},
};
use crate::{
    develop::{
        pipeline::pixel_params::PixelParams,
        stage_cache::{BlurKey, LocalKey, SampleKey},
    },
    raw::CameraImage,
};
use anyhow::{Context, Result, ensure};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use wgpu::util::DeviceExt;

/// Device samples kept, as the stage cache keeps CPU samples.
const SAMPLES: usize = 4;
/// Parameters before the radial tables in the sampling pass (`S_TABLES`).
pub(crate) const SAMPLE_HEADER: usize = 64;

struct Blurs {
    logs: wgpu::Buffer,
    fine: wgpu::Buffer,
    broad: wgpu::Buffer,
    texture: Option<wgpu::Buffer>,
}
pub(super) struct Resident {
    logs_layout: wgpu::BindGroupLayout,
    logs: wgpu::ComputePipeline,
    /// running_sum, window, local_gain, sample_region, reduce_toned, with their layouts.
    pipelines: Vec<(wgpu::BindGroupLayout, wgpu::ComputePipeline)>,
    dummy: wgpu::Buffer,
    photo: Option<(Arc<CameraImage>, wgpu::Buffer)>,
    blurs: Option<(BlurKey, Blurs)>,
    gain: Option<(LocalKey, wgpu::Buffer)>,
    pub(super) samples: Vec<(SampleKey, Arc<DeviceSamples>)>,
}
const RUNNING_SUM: usize = 0;
const WINDOW: usize = 1;
const GAIN: usize = 2;
const SAMPLE: usize = 3;
const REDUCE: usize = 4;

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Resident {
    fn new(device: &wgpu::Device, developer: &Developer) -> Self {
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let pipeline = |module: &wgpu::ShaderModule, layouts: &[&wgpu::BindGroupLayout], entry| {
            let layouts: Vec<_> = layouts.iter().map(|l| Some(*l)).collect();
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let logs_source = crate::develop::pipeline::pixel_params::wgsl_prelude()
            + include_str!("develop.wgsl")
            + include_str!("logs.wgsl");
        let logs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Log luminance"),
            source: wgpu::ShaderSource::Wgsl(logs_source.into()),
        });
        let logs_layout = layout(
            "Log luminance",
            &[
                storage(0, true),
                storage(1, false),
                storage(2, true),
                uniform(3),
            ],
        );
        let logs = pipeline(
            &logs_module,
            &[&developer.layout, &logs_layout],
            "log_luminance",
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Local stages"),
            source: wgpu::ShaderSource::Wgsl(include_str!("local.wgsl").into()),
        });
        let entries: [(&str, Vec<wgpu::BindGroupLayoutEntry>); 5] = [
            (
                "running_sum",
                vec![storage(0, true), storage(1, false), uniform(3)],
            ),
            (
                "window",
                vec![storage(1, false), storage(2, false), uniform(3)],
            ),
            (
                "local_gain",
                vec![
                    storage(4, true),
                    storage(5, true),
                    storage(6, true),
                    storage(7, true),
                    storage(8, false),
                    uniform(9),
                ],
            ),
            (
                "sample_region",
                vec![
                    storage(10, true),
                    storage(11, true),
                    storage(12, true),
                    storage(14, false),
                    storage(15, false),
                ],
            ),
            (
                "reduce_toned",
                vec![
                    storage(10, true),
                    storage(11, true),
                    storage(12, true),
                    storage(13, false),
                ],
            ),
        ];
        let pipelines = entries
            .into_iter()
            .map(|(entry, entries)| {
                let layout = layout(entry, &entries);
                let pipeline = pipeline(&module, &[&layout], entry);
                (layout, pipeline)
            })
            .collect();
        Self {
            logs_layout,
            logs,
            pipelines,
            dummy: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Unused binding"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: false,
            }),
            photo: None,
            blurs: None,
            gain: None,
            samples: Vec::new(),
        }
    }
    fn release(&mut self) {
        self.photo = None;
        self.blurs = None;
        self.gain = None;
        self.samples.clear();
    }
}
/// Workgroups for `n` invocations of 256 as (x, y) within the device's limits.
fn groups(device: &wgpu::Device, n: u64) -> (u32, u32) {
    let groups = n.div_ceil(256) as u32;
    let max = device.limits().max_compute_workgroups_per_dimension;
    (groups.min(max), groups.div_ceil(groups.min(max).max(1)))
}
fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    entries: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    let entries: Vec<_> = entries
        .iter()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &entries,
    })
}
fn dispatch(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    groups: &[&wgpu::BindGroup],
    size: (u32, u32),
) {
    let mut pass = encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.dispatch_workgroups(size.0, size.1, 1);
}

impl Processor {
    fn resident(&mut self) -> &mut Resident {
        let device = &self.device;
        let developer = self.developer.get_or_insert_with(|| Developer::new(device));
        self.resident
            .get_or_insert_with(|| Resident::new(device, developer))
    }
    /// Whether `image` and its per-pixel buffers fit the device.
    pub(crate) fn fits_resident(&self, image: &CameraImage) -> bool {
        let limits = self.device.limits();
        let bytes = image.pixels.len() as u64 * 12;
        bytes <= limits.max_storage_buffer_binding_size && bytes <= limits.max_buffer_size
    }
    /// The photo's buffer, uploaded once while it stays the current photo.
    fn photo(&mut self, image: &Arc<CameraImage>) -> wgpu::Buffer {
        let device = self.device.clone();
        let resident = self.resident();
        if let Some((kept, buffer)) = &resident.photo
            && Arc::ptr_eq(kept, image)
        {
            return buffer.clone();
        }
        // Another photo (or pyramid level): everything derived from the old one goes.
        resident.release();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Photo"),
            contents: bytemuck::cast_slice(&image.pixels),
            usage: wgpu::BufferUsages::STORAGE,
        });
        resident.photo = Some((image.clone(), buffer.clone()));
        buffer
    }
    fn buffer(&self, label: &str, bytes: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.max(16),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    /// The local-tone gain of `image` on the device, as `quality::local_blurs` and
    /// `apply_local` compute it on the CPU. `camera` holds the camera stage of the
    /// blurs' recipe; `vignetting` the built-in vignetting and its radial table;
    /// `radii` the fine, broad and (for Texture) texture box radii; `sliders` exposure,
    /// Shadows, Highlights, Clarity and Texture.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn local_gain(
        &mut self,
        image: &Arc<CameraImage>,
        camera: &PixelParams,
        vignetting: Option<&([f32; 4], Vec<f32>)>,
        radii: [Option<u32>; 3],
        sliders: [f32; 5],
        (blur_key, key): (BlurKey, LocalKey),
        cancel: &AtomicBool,
    ) -> Result<wgpu::Buffer> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let photo = self.photo(image);
        if let Some((kept, gain)) = &self.resident().gain
            && *kept == key
        {
            return Ok(gain.clone());
        }
        let device = self.device.clone();
        let (w, h) = (image.width, image.height);
        let n = w as u64 * h as u64;
        let mut encoder = device.create_command_encoder(&Default::default());
        if !self
            .resident()
            .blurs
            .as_ref()
            .is_some_and(|(k, _)| *k == blur_key)
        {
            self.resident().blurs = None;
            let logs = self.buffer("Log luminance", n * 4);
            // The develop layout's other bindings are unused by the log pass.
            let resident = self.resident.as_ref().unwrap();
            let dummy = &resident.dummy;
            let init = |label, contents: &[u8], usage| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                })
            };
            let params = init(
                "Camera parameters",
                bytemuck::cast_slice(&camera.params),
                wgpu::BufferUsages::STORAGE,
            );
            let tables = init(
                "Camera tables",
                bytemuck::cast_slice(&camera.tables),
                wgpu::BufferUsages::STORAGE,
            );
            let developer = self.developer.as_ref().unwrap();
            let camera_group = bind(
                &device,
                &developer.layout,
                &[
                    (0, dummy),
                    (1, dummy),
                    (2, &self.buffer("Unused", 16)),
                    (3, &params),
                    (4, &tables),
                    (5, dummy),
                ],
            );
            let (gx, gy) = groups(&device, n);
            let (header, table) = match vignetting {
                Some((v, table)) => (
                    [
                        0f32.to_bits(),
                        table.len() as u32 / 2,
                        v[0].to_bits(),
                        v[1].to_bits(),
                        v[2].to_bits(),
                        v[3].to_bits(),
                    ],
                    table.clone(),
                ),
                None => ([(-1i32) as u32, 0, 0, 0, 0, 0], vec![0.; 4]),
            };
            let mut values = [0u32; 12];
            values[..3].copy_from_slice(&[w, h, gx]);
            values[3] = header[0];
            values[4] = header[1];
            values[5..9].copy_from_slice(&header[2..6]);
            let log_uniform = init(
                "Log parameters",
                bytemuck::cast_slice(&values),
                wgpu::BufferUsages::UNIFORM,
            );
            let radial = init(
                "Vignetting",
                bytemuck::cast_slice(&table),
                wgpu::BufferUsages::STORAGE,
            );
            let group = bind(
                &device,
                &resident.logs_layout,
                &[(0, &photo), (1, &logs), (2, &radial), (3, &log_uniform)],
            );
            dispatch(
                &mut encoder,
                &resident.logs,
                &[&camera_group, &group],
                (gx, gy),
            );
            let prefix = self.buffer("Running sums", n * 4);
            let rows = self.buffer("Row blur", n * 4);
            let mut blur = |radius: u32, label: &str| {
                let out = self.buffer(label, n * 4);
                let resident = self.resident.as_ref().unwrap();
                for (axis, src, dst) in [(0u32, &logs, &rows), (1, &rows, &out)] {
                    let uniform = init(
                        "Blur parameters",
                        bytemuck::cast_slice(&[w, h, radius, axis]),
                        wgpu::BufferUsages::UNIFORM,
                    );
                    let (layout, pipeline) = &resident.pipelines[RUNNING_SUM];
                    let group = bind(&device, layout, &[(0, src), (1, &prefix), (3, &uniform)]);
                    let lines = if axis == 0 { h } else { w };
                    dispatch(&mut encoder, pipeline, &[&group], (lines.div_ceil(64), 1));
                    let (layout, pipeline) = &resident.pipelines[WINDOW];
                    let group = bind(&device, layout, &[(1, &prefix), (2, dst), (3, &uniform)]);
                    dispatch(
                        &mut encoder,
                        pipeline,
                        &[&group],
                        (w.div_ceil(16), h.div_ceil(16)),
                    );
                }
                out
            };
            let fine = blur(radii[0].context("No fine radius")?, "Fine blur");
            let broad = blur(radii[1].context("No broad radius")?, "Broad blur");
            let texture = radii[2].map(|r| blur(r, "Texture blur"));
            self.resident().blurs = Some((
                blur_key,
                Blurs {
                    logs,
                    fine,
                    broad,
                    texture,
                },
            ));
        }
        let gain = self.buffer("Local-tone gain", n * 4);
        let resident = self.resident.as_ref().unwrap();
        let blurs = &resident.blurs.as_ref().unwrap().1;
        let (gx, gy) = groups(&device, n);
        let f = f32::to_bits;
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sliders"),
            contents: bytemuck::cast_slice(&[
                n as u32,
                gx,
                blurs.texture.is_some() as u32,
                0,
                f(sliders[0]),
                f(sliders[1]),
                f(sliders[2]),
                f(sliders[3]),
                f(sliders[4]),
                0,
                0,
                0,
            ]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (layout, pipeline) = &resident.pipelines[GAIN];
        let group = bind(
            &device,
            layout,
            &[
                (4, &blurs.logs),
                (5, &blurs.fine),
                (6, &blurs.broad),
                (7, blurs.texture.as_ref().unwrap_or(&blurs.logs)),
                (8, &gain),
                (9, &uniform),
            ],
        );
        dispatch(&mut encoder, pipeline, &[&group], (gx, gy));
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        self.queue.submit([encoder.finish()]);
        self.resident().gain = Some((key, gain.clone()));
        Ok(gain)
    }
    /// The toned photo reduced to `size` (`pipeline::preview_source`), read back for
    /// the Shadows/Highlights map.
    pub(crate) fn reduce_toned(
        &mut self,
        image: &Arc<CameraImage>,
        gain: Option<&wgpu::Buffer>,
        (w, h): (u32, u32),
        cancel: &AtomicBool,
    ) -> Result<CameraImage> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let photo = self.photo(image);
        let device = self.device.clone();
        let mut header = vec![0f32; SAMPLE_HEADER];
        header[0] = image.width as f32;
        header[1] = image.height as f32;
        header[2] = gain.is_some() as u8 as f32;
        header[57] = w as f32;
        header[58] = h as f32;
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Reduce parameters"),
            contents: bytemuck::cast_slice(&header),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bytes = w as u64 * h as u64 * 12;
        let out = self.buffer("Reduced photo", bytes);
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Reduced readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let resident = self.resident();
        let (layout, pipeline) = &resident.pipelines[REDUCE];
        let group = bind(
            &device,
            layout,
            &[
                (10, &photo),
                (11, gain.unwrap_or(&resident.dummy)),
                (12, &params),
                (13, &out),
            ],
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        dispatch(
            &mut encoder,
            pipeline,
            &[&group],
            (w.div_ceil(16), h.div_ceil(16)),
        );
        encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, bytes);
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(10)),
        })?;
        rx.recv_timeout(Duration::from_secs(1))
            .context("GPU readback timed out")??;
        let pixels =
            bytemuck::cast_slice::<u8, [f32; 3]>(&staging.slice(..).get_mapped_range()?).to_vec();
        staging.unmap();
        Ok(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: image.metadata.clone(),
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
        })
    }
    /// Samples a region on the device (`pipeline::sample_region`) with the sampling
    /// parameters `params` (see `local.wgsl`), reusing the samples for an equal `key`.
    pub(crate) fn sample(
        &mut self,
        image: &Arc<CameraImage>,
        gain: Option<&wgpu::Buffer>,
        mut params: Vec<f32>,
        (width, height): (u32, u32),
        key: SampleKey,
        cancel: &AtomicBool,
    ) -> Result<Arc<DeviceSamples>> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let n = width as u64 * height as u64;
        ensure!(
            n > 0 && n * 12 <= self.device.limits().max_storage_buffer_binding_size,
            "Region exceeds GPU buffer limits"
        );
        let photo = self.photo(image);
        let resident = self.resident();
        if let Some(i) = resident.samples.iter().position(|(k, _)| *k == key) {
            let entry = resident.samples.remove(i);
            let samples = entry.1.clone();
            resident.samples.insert(0, entry);
            return Ok(samples);
        }
        let device = self.device.clone();
        let (gx, gy) = groups(&device, n);
        params[59] = gx as f32;
        params[2] = gain.is_some() as u8 as f32;
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sampling parameters"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let samples = DeviceSamples {
            width,
            height,
            pixels: self.buffer("Device samples", n * 12),
            positions: self.buffer("Device positions", n * 8),
            output: self.buffer("Developed pixels", n * 12),
        };
        let resident = self.resident();
        let (layout, pipeline) = &resident.pipelines[SAMPLE];
        let group = bind(
            &device,
            layout,
            &[
                (10, &photo),
                (11, gain.unwrap_or(&resident.dummy)),
                (12, &params),
                (14, &samples.pixels),
                (15, &samples.positions),
            ],
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        dispatch(&mut encoder, pipeline, &[&group], (gx, gy));
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        self.queue.submit([encoder.finish()]);
        let samples = Arc::new(samples);
        let resident = self.resident();
        resident.samples.insert(0, (key, samples.clone()));
        resident.samples.truncate(SAMPLES);
        Ok(samples)
    }
    /// Drops everything kept on the device for the resident path.
    pub(super) fn release_resident(&mut self) {
        if let Some(r) = &mut self.resident {
            r.release();
        }
    }
}
