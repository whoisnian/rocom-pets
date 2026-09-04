//! 宠物的离屏画布:一张彩色纹理 + 深度缓冲,宠物先画在这里,再由 stage 合成上去。
//!
//! 为什么不直接画在 stage 表面上:那需要与屏幕同尺寸的深度缓冲(4K 下 Depth32Float 要
//! 几十 MB,而且分数缩放下还要跟着重建);离屏画布只要宠物在屏幕上的实际大小,
//! 宠物的渲染分辨率与屏幕分辨率也就解耦了。副产品是这张纹理可以直接回读当 alpha mask,
//! 正好是 Phase 2 轮廓命中测试要的东西。
//!
//! **画布按 [`SUPERSAMPLE`] 倍开、合成时缩回去(SSAA)**,理由见那个常量。

use glam::{Mat4, Vec3};

use super::gpu::{DEPTH_FORMAT, PetGpu};

/// 离屏画布的超采样倍率:纹理按屏幕尺寸的这么多倍开,合成那一遍再线性缩回去。
///
/// ## 为什么需要
///
/// 描边宽度是**屏幕空间常数**(见 `pet::gpu::desktop_outline_scale`):落到桌宠这一侧是
/// `0.255% × 参考身高 117.3cm × px_per_cm × 显示缩放` ≈ **0.30 × px_per_cm × 缩放** 像素。
/// 默认 `px_per_cm = 2`、1 倍屏 ⇒ **0.6 像素**,比一个像素还窄。
///
/// 而整条管线一个采样点都没有(到处 `sample_count: 1`,实测渲出来的 alpha
/// **一个半透像素都没有**,全是 0 或 1)。一条 0.6 像素宽的黑线只能靠「哪些像素中心
/// 恰好被盖住」来表达 ⇒ 画出来是一圈**虚线**;而姿势每帧动零点几个像素,那串点就重新
/// 掷一次骰子 —— 用户看到的「描边在动作中跟着动、发糊」就是这个,不是描边宽度在变
/// (实测暗环积分逐帧只差 4%,是稳的)。
///
/// ## 为什么是 SSAA 而不是 MSAA
///
/// MSAA 只抗几何边,而且这张深度缓冲后面还要被半透那一遍**采样**(见 `50-encode.wgsl`
/// 的 `textureLoad(scene_depth, …)`)—— 换成多重采样纹理要一路改 WGSL 与 web 那侧。
/// 超采样只动纹理尺寸:合成那块四边形本来就用线性采样器
/// (`render.rs` 的 `quad` sampler),目标像素中心正好落在 2×2 个源纹素的正中,
/// 双线性 = 精确的盒式降采样。顺带把色带、贴脸图集的硬边也一起抗了。
///
/// 取 2 而不是 4:2 倍就足以把虚线连成一条(4 倍只再平滑一点点),而显存/填充按平方涨。
pub const SUPERSAMPLE: u32 = 2;

/// 超采样后允许的最大边长(像素)。超过就退回 1 倍。
///
/// 到这个尺寸时描边本身已有 3 像素多、锯齿不再是问题,而 4096² 的
/// `Depth32Float` 要 64MB —— 正是本文件开头「不用全屏深度缓冲」那条理由。
const SUPERSAMPLE_MAX_SIDE: u32 = 2048;

/// 这块画布该按几倍开。`limit` 是允许的最大边长。
///
/// **只放整数倍**:合成那一遍靠「目标像素中心正好落在 n×n 个源纹素的正中」把双线性
/// 采样变成精确的盒式降采样,非整数倍就不成立了(会重新引入采样噪声)。
fn factor(size: (u32, u32), limit: u32) -> u32 {
    let side = size.0.max(size.1).max(1);
    if side.saturating_mul(SUPERSAMPLE) <= limit {
        SUPERSAMPLE
    } else {
        1
    }
}

/// 逻辑尺寸 → 实际开的纹理尺寸。设备的纹理上限也算进去(小机器上
/// `max_texture_dimension_2d` 可能只有 2048)。
fn render_size(device: &wgpu::Device, size: (u32, u32)) -> (u32, u32) {
    let limit = SUPERSAMPLE_MAX_SIDE.min(device.limits().max_texture_dimension_2d);
    let n = factor(size, limit);
    (size.0.max(1) * n, size.1.max(1) * n)
}

pub struct PetTarget {
    size: (u32, u32),
    /// 纹理的真实尺寸 = `size × supersample`。回读掩码要按它来。
    render: (u32, u32),
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
        let render = render_size(device, size);
        let (color, color_view, depth_view) = create(device, format, render);
        let depth_bind = pet.bind_scene_depth(device, &depth_view);
        Self {
            size,
            render,
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

    /// 宠物在屏幕上的显示尺寸(物理像素)。合成那块四边形按它画。
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 纹理的真实尺寸(= `size × supersample`)。回读掩码按它来。
    pub fn render_size(&self) -> (u32, u32) {
        self.render
    }

    /// 这块画布实际用了几倍超采样(见 [`SUPERSAMPLE`];大画布上会退回 1)。
    pub fn supersample(&self) -> u32 {
        (self.render.0 / self.size.0.max(1)).max(1)
    }

    /// 宠物在屏幕上的显示尺寸变了(缩放/换形态)就重建。
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32), pet: &PetGpu) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return false;
        }
        let render = render_size(device, size);
        let (color, color_view, depth_view) = create(device, self.format, render);
        let depth_bind = pet.bind_scene_depth(device, &depth_view);
        self.size = size;
        self.render = render;
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

    /// 默认那档(`px_per_cm = 2`、1 倍屏)的画布是几百像素,该吃到超采样 ——
    /// 描边在那一档只有 0.6 像素宽,不超采样就是一圈虚线(见 [`SUPERSAMPLE`])。
    #[test]
    fn a_desktop_sized_canvas_gets_supersampled() {
        assert_eq!(factor((384, 384), 2048), SUPERSAMPLE);
        assert_eq!(factor((1024, 1024), 2048), SUPERSAMPLE);
    }

    /// 超过上限就退回 1 倍,而不是开一张超过设备上限、或几十 MB 深度缓冲的纹理。
    #[test]
    fn an_oversized_canvas_falls_back_to_one() {
        assert_eq!(factor((1025, 800), 2048), 1);
        // 小机器:设备上限本身就只有 2048
        assert_eq!(factor((1500, 1500), 2048), 1);
    }

    /// 长边说了算:一张瘦高的画布不能因为窄边小就整体放大过头。
    #[test]
    fn the_long_side_decides() {
        assert_eq!(factor((100, 1600), 2048), 1);
    }

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
