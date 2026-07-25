//! wgpu 渲染:把预乘 alpha 的精灵贴图画到透明表面上。
//!
//! 关键点全在 alpha 上:表面按 `PreMultiplied` 配置(拿不到就退 `Auto` 并告警)、
//! 清屏用全透明、混合用 `PREMULTIPLIED_ALPHA_BLENDING`、贴图数据本身也是预乘的。
//! 任一环节用了非预乘约定,软边就会出现暗边或亮边——这正是 S1 要肉眼验的东西。

use anyhow::{Context, Result};

use crate::sprite::Sprite;

/// 与 WGSL 里的 `U` 对应。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    surface: [f32; 2],
    pos: [f32; 2],
    size: [f32; 2],
    /// >0.5 时给精灵加一点提亮,用来肉眼确认「拖动中」状态。
    highlight: f32,
    _pad: f32,
}

/// 所有 stage 共享的 GPU 资源。
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sprite_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
}

impl Gpu {
    /// `compatible` 只用于挑适配器与探测表面能力,之后每个 stage 各自建表面。
    pub fn new(
        instance: &wgpu::Instance,
        compatible: &wgpu::Surface<'static>,
        sprite: &Sprite,
    ) -> Result<Self> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(compatible),
            ..Default::default()
        }))
        .context("没有可用的 GPU 适配器")?;
        let info = adapter.get_info();
        log::info!(
            "适配器: {} ({:?}, {:?})",
            info.name,
            info.backend,
            info.device_type
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rocom-pets"),
            ..Default::default()
        }))
        .context("创建 GPU 设备失败")?;

        let caps = compatible.get_capabilities(&adapter);
        log::info!("表面能力: formats={:?}", caps.formats);
        log::info!("表面能力: alpha_modes={:?}", caps.alpha_modes);
        log::info!("表面能力: present_modes={:?}", caps.present_modes);

        // 优先非 sRGB 的 8 位格式:精灵字节已是最终颜色,过一道 sRGB 编码只会偏色
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb() && f.target_component_alignment() == Some(1))
            .or_else(|| caps.formats.first().copied())
            .context("表面没有可用格式")?;
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            log::warn!("表面不支持 PreMultiplied alpha,退回 Auto(软边可能有暗边)");
            wgpu::CompositeAlphaMode::Auto
        };
        log::info!("选定 format={format:?} alpha_mode={alpha_mode:?}");

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite"),
            size: wgpu::Extent3d {
                width: sprite.width,
                height: sprite.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &sprite.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(sprite.width * 4),
                rows_per_image: Some(sprite.height),
            },
            wgpu::Extent3d {
                width: sprite.width,
                height: sprite.height,
                depth_or_array_layers: 1,
            },
        );
        let sprite_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stage"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_layout,
            sprite_view,
            sampler,
            format,
            alpha_mode,
        })
    }

    /// 为一个已建好的表面创建渲染目标。
    pub fn create_target(&self, surface: wgpu::Surface<'static>, size: (u32, u32)) -> Target {
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage-uniform"),
            size: size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stage"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.sprite_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut target = Target {
            surface,
            size,
            uniform,
            bind_group,
        };
        target.configure(self);
        target
    }
}

/// 一个 stage 表面对应的渲染目标。
pub struct Target {
    surface: wgpu::Surface<'static>,
    size: (u32, u32),
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Target {
    fn configure(&mut self, gpu: &Gpu) {
        self.surface.configure(
            &gpu.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                view_formats: vec![],
                alpha_mode: gpu.alpha_mode,
                // 精灵字节就是最终颜色,交给后端按格式默认处理即可
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: self.size.0.max(1),
                height: self.size.1.max(1),
                desired_maximum_frame_latency: 2,
                present_mode: wgpu::PresentMode::Mailbox,
            },
        );
    }

    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) {
        if size == self.size || size.0 == 0 || size.1 == 0 {
            return;
        }
        self.size = size;
        self.configure(gpu);
    }

    /// 画一帧:清成全透明,再画精灵。
    pub fn render(
        &mut self,
        gpu: &Gpu,
        sprite_pos: (f32, f32),
        sprite_size: (u32, u32),
        highlight: bool,
    ) -> Result<()> {
        gpu.queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Uniform {
                surface: [self.size.0 as f32, self.size.1 as f32],
                pos: [sprite_pos.0, sprite_pos.1],
                size: [sprite_size.0 as f32, sprite_size.1 as f32],
                highlight: if highlight { 1.0 } else { 0.0 },
                _pad: 0.0,
            }),
        );

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(frame) | Cst::Suboptimal(frame) => frame,
            // 尺寸变更、合成器重启会让表面过期:重配后再取一次
            Cst::Outdated | Cst::Lost => {
                self.configure(gpu);
                match self.surface.get_current_texture() {
                    Cst::Success(frame) | Cst::Suboptimal(frame) => frame,
                    other => anyhow::bail!("重配表面后仍取不到帧: {other:?}"),
                }
            }
            // 超时/被完全遮挡:这一帧不画,下次事件会再来
            Cst::Timeout | Cst::Occluded => return Ok(()),
            Cst::Validation => anyhow::bail!("表面配置非法"),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        gpu.queue.submit(Some(encoder.finish()));
        // wgpu 30 起 present 挂在 queue 上
        gpu.queue.present(frame);
        Ok(())
    }
}
