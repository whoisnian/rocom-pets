//! 宠物的离屏画布:一张彩色纹理 + 深度缓冲,宠物先画在这里,再由 stage 合成上去。
//!
//! 为什么不直接画在 stage 表面上:那需要与屏幕同尺寸的深度缓冲(4K 下 Depth32Float 要
//! 几十 MB,而且分数缩放下还要跟着重建);离屏画布只要宠物在屏幕上的实际大小,
//! 宠物的渲染分辨率与屏幕分辨率也就解耦了。副产品是这张纹理可以直接回读当 alpha mask,
//! 正好是 Phase 2 轮廓命中测试要的东西。

use glam::{Mat4, Vec3};

use super::gpu::{DEPTH_FORMAT, PetGpu};

pub struct PetTarget {
    size: (u32, u32),
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    format: wgpu::TextureFormat,
}

impl PetTarget {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let (color, color_view, depth_view) = create(device, format, size);
        Self {
            size,
            color,
            color_view,
            depth_view,
            format,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.color_view
    }

    /// 宠物在屏幕上的显示尺寸变了(缩放/换形态)就重建。
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return false;
        }
        let (color, color_view, depth_view) = create(device, self.format, size);
        self.size = size;
        self.color = color;
        self.color_view = color_view;
        self.depth_view = depth_view;
        true
    }

    /// 把宠物画进这张画布(清成全透明)。`pet` 的 uniform 须已 update 过。
    pub fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, pet: &PetGpu) {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pet") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pet"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pet.draw(&mut pass, true);
        }
        queue.submit(Some(encoder.finish()));
    }
}

fn create(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    size: (u32, u32),
) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView) {
    let extent = wgpu::Extent3d {
        width: size.0.max(1),
        height: size.1.max(1),
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pet-canvas"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // COPY_SRC 是给 Phase 2 回读 alpha mask 留的
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pet-depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, depth_view)
}

/// 宠物在离屏画布里的取景:正交、按包围盒最长边取框,`yaw` 决定朝向(+90° 朝屏幕右)。
///
/// `padding` 要留余量:跳跃/伸展类动作会超出绑定姿势的包围盒。
pub fn view_proj(bounds: (Vec3, Vec3), yaw: f32, padding: f32) -> Mat4 {
    super::gpu::orthographic_view(bounds, yaw, padding)
}
