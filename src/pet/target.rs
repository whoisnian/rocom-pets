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
    depth_bind: wgpu::BindGroup,
    format: wgpu::TextureFormat,
}

impl PetTarget {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        pet: &PetGpu,
    ) -> Self {
        let (color, color_view, depth_view) = create(device, format, size);
        let depth_bind = pet.bind_scene_depth(device, &depth_view);
        Self {
            size,
            color,
            color_view,
            depth_view,
            depth_bind,
            format,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.color_view
    }

    /// 供掩码回读拷贝用(纹理建的时候带了 COPY_SRC)。
    pub fn texture(&self) -> &wgpu::Texture {
        &self.color
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 宠物在屏幕上的显示尺寸变了(缩放/换形态)就重建。
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32), pet: &PetGpu) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return false;
        }
        let (color, color_view, depth_view) = create(device, self.format, size);
        let depth_bind = pet.bind_scene_depth(device, &depth_view);
        self.size = size;
        self.color = color;
        self.color_view = color_view;
        self.depth_view = depth_view;
        self.depth_bind = depth_bind;
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
            pet.draw_opaque(&mut pass, true);
        }
        {
            // 深度不再写，只作为 attachment 做遮挡测试并由 shader 采样做 depth-fade。
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pet-translucent"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pet.draw_translucent(&mut pass, &self.depth_bind);
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, depth_view)
}

/// 宠物在离屏画布里的取景:正交、按包围盒最长边取框。
///
/// `yaw` 用 [`camera_yaw`] 算,别自己按直觉填角度(符号是反的,理由见那里)。
/// `padding` 要留余量:跳跃/伸展类动作会超出绑定姿势的包围盒。
pub fn view_proj(bounds: (Vec3, Vec3), yaw: f32, padding: f32) -> Mat4 {
    super::gpu::orthographic_view(bounds, yaw, padding)
}

/// 「宠物朝屏幕哪边」→ 相机 yaw。
///
/// **符号是反直觉的**:yaw 转的是**相机**而不是模型。相机绕到 -X(yaw = -90°)时,
/// 屏幕右方向对应世界 +Z,而宠物的前方正是 +Z(见 docs/spike-s3.md:root motion 恒沿 +Z),
/// 于是这时看到的是「朝右站」。所以**朝右取负角**。
/// 写成 +90° 的话宠物会背朝行进方向倒着走——Phase 1 实测踩过这个坑。
pub fn camera_yaw(facing_right: bool) -> f32 {
    if facing_right {
        -std::f32::consts::FRAC_PI_2
    } else {
        std::f32::consts::FRAC_PI_2
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec4;

    use super::*;

    /// 把「朝向」这件事钉死:宠物前方是世界 +Z(见 docs/spike-s3.md),
    /// 朝右时它必须落在屏幕右半边。符号写反就是倒着走,这个测试专门防那次回归。
    fn forward_screen_x(facing_right: bool) -> f32 {
        let bounds = (Vec3::splat(-1.0), Vec3::splat(1.0));
        let vp = view_proj(bounds, camera_yaw(facing_right), 1.0);
        let forward = vp * Vec4::new(0.0, 0.0, 1.0, 1.0);
        let center = vp * Vec4::new(0.0, 0.0, 0.0, 1.0);
        forward.x / forward.w - center.x / center.w
    }

    #[test]
    fn facing_right_puts_forward_on_screen_right() {
        assert!(forward_screen_x(true) > 0.1, "朝右时前方该在屏幕右侧");
    }

    #[test]
    fn facing_left_puts_forward_on_screen_left() {
        assert!(forward_screen_x(false) < -0.1, "朝左时前方该在屏幕左侧");
    }
}
