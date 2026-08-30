//! GPU 侧的几个独立小工具:本帧物体盒、表情卡回退、贴图上传。
//!
//! 从 `gpu.rs` 里拆出来的 —— 它们与管线/uniform 无关,放在一起只是历史。

use super::*;

/// 用与顶点着色器完全相同的线性混合蒙皮计算本帧物体盒。FakeFulid 的 cooked PS
/// 通过 PrimitiveSceneData 读取当前 `ObjectWorldPositionAndRadius/ObjectBounds`，液面
/// 平面以那个中心为原点；这是材质输入，不是为某个模型拟合液位。
pub(super) fn posed_object_bounds(vertices: &[Vertex], matrices: &[Mat4]) -> Option<[f32; 4]> {
    if vertices.is_empty() || matrices.is_empty() {
        return None;
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in vertices {
        let total: f32 = vertex.weights.iter().sum();
        let weights = if total > 0.0001 {
            vertex.weights.map(|weight| weight / total)
        } else {
            vertex.weights
        };
        let mut skin = Mat4::ZERO;
        for (slot, weight) in weights.into_iter().enumerate() {
            if weight > 0.0 {
                let joint = vertex.joints[slot] as usize;
                if joint < matrices.len() {
                    skin += matrices[joint] * weight;
                }
            }
        }
        let position = skin.transform_point3(Vec3::from_array(vertex.pos));
        min = min.min(position);
        max = max.max(position);
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let center = (min + max) * 0.5;
    Some([center.x, center.y, center.z, (max - min).max_element()])
}

/// 想画的那张表情卡这只有没有;没有就退档(`cards` 是这只真有的卡号,升序)。
///
/// 退档顺序:**先退回 2 号**(网格脸的默认脸,见 `Expression::card`),再退到最小的那张。
/// 卡是按需做的,缺号不少见 —— 觅觅蝠一/三阶没有 1 号、蝴蝶陶陶三阶没有 5 号(困倦),
/// 它睡着时若照着 5 号剔就整张脸都不画了。
/// `cards` 为空(不是网格脸)时原样返回:着色器那条判据本来就不生效。
pub(super) fn resolve_face_card(cards: &[u32], want: u32) -> u32 {
    if cards.is_empty() || cards.contains(&want) {
        return want;
    }
    if cards.contains(&2) {
        return 2;
    }
    cards[0]
}

pub(super) fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    image: &crate::pet::model::Image,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
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
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: Some(image.height),
        },
        wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
