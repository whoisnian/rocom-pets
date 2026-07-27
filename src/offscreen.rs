//! 离屏渲染:不开窗口,把宠物的若干动作/时刻渲成一张对比图。
//!
//! 这是 S2 的验收手段(docs/design.md §9):渲出来的形体要和
//! `tools/verify_glb.py`(纯 CPU、按 glTF 规范独立实现的一版)一致——两套实现互相校验,
//! 任何一边写错都会立刻看出来。同时它也是不需要桌面环境就能跑的回归入口。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::Vec3;

use crate::pet::{Model, PetGpu, Player, gpu::DEPTH_FORMAT, orthographic_view};

/// 输出纹理格式:和 stage 表面一致(非 sRGB,预乘 alpha)。
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// 取景余量。包围盒已经含了各动作的伸展(`Model::motion_bounds`),这里只需给描边与
/// 边缘光留一点点边;与平台层的 `CANVAS_PADDING` 保持一致。
const RENDER_PADDING: f32 = 1.15;

pub struct Request {
    pub pack: PathBuf,
    pub form: Option<String>,
    /// 要渲的动作名;每个动作出一格。
    pub clips: Vec<String>,
    /// 采样时刻占动作时长的比例。
    pub at: f32,
    /// 喂给 shader 的「秒」。默认 0(同一条命令两次跑结果一样);
    /// 要看随时间变的东西(火焰流动、球内星点的闪烁)就给个非零值。
    pub time: f32,
    pub size: u32,
    pub yaw_degrees: f32,
    pub out: PathBuf,
    /// 额外渲一格「淡化中」的画面,验证 clip 切换不跳变。
    pub fade_probe: bool,
    /// >0 时跑这么多帧测平均耗时(含 CPU 采样 + 上传 + 绘制)。
    pub bench: u32,
}

pub fn render(request: &Request) -> Result<()> {
    let glb = locate_glb(&request.pack, request.form.as_deref())?;
    // 材质表是必需的(贴图与 alpha 语义都由它定)。调试渲图必须走和运行时同一条路径,
    // 否则「渲出来对不对」验的不是运行时的行为。
    let spec = load_materials(&request.pack, &glb)
        .with_context(|| format!("{:?} 里找不到这个形态的材质表,重导一次包", request.pack))?;
    let model = Model::load(&glb, &spec)?;
    log::info!(
        "{}: {} 顶点 / {} 三角 / {} 关节 / {} 段动作",
        glb.display(),
        model.vertices.len(),
        model.indices.len() / 3,
        model.skeleton.joints.len(),
        model.clips.len()
    );
    log::info!(
        "动作: {}",
        model
            .clips
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(instance_desc);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .context("没有可用的 GPU 适配器")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rocom-pets-offscreen"),
        ..Default::default()
    }))
    .context("创建 GPU 设备失败")?;
    log::info!("适配器: {}", adapter.get_info().name);

    let pet = PetGpu::new(&device, &queue, &model, FORMAT)?;
    let size = request.size;
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen-color"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen-depth"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    // 回读缓冲:WebGPU 要求每行按 256 字节对齐
    let row_bytes = size * 4;
    let padded_row = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offscreen-readback"),
        size: (padded_row * size) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let view_proj = orthographic_view(
        model.motion_bounds,
        request.yaw_degrees.to_radians(),
        RENDER_PADDING,
    );
    // 光从左前上方来,和桌面场景里「屏幕外有光源」的直觉一致
    let light_dir = Vec3::new(-0.4, 0.8, 0.6);
    // 描边宽度按模型尺寸走,免得大小形态粗细不一
    let outline_width = (model.bounds.1 - model.bounds.0).length() * 0.004;

    let mut tiles: Vec<(String, Vec<u8>)> = Vec::new();
    // **零动画的形态渲绑定姿势。** 全库 819 个形态里有 64 个一段动作都没有 ——
    // 那不是导出漏了:解包里这些资产**根本没有 `Animation/` 目录**(实测
    // `Wat_ShuiLanLanBo_001` 只有 SKM + ABP + 材质,而同族的 `Wat_ShuiLanLan3_001` 有 62 段),
    // 游戏里它们多半是静态物件。
    //
    // 绑定姿势下 `世界变换 × 逆绑定矩阵 = I`,所以直接传单位阵就是绑定姿势,不必造一个
    // 空的 `Player`。这样至少能看见它们;运行时要不要当桌宠是另一回事(不能动)。
    if model.clips.is_empty() {
        let identity = vec![glam::Mat4::IDENTITY; model.skeleton.joints.len()];
        pet.update(&queue, view_proj, light_dir, outline_width, request.time, &identity);
        let pixels = draw_and_read(
            &device, &queue, &pet, &color, &color_view, &depth_view, &readback, size, padded_row,
        )?;
        tiles.push(("BindPose".into(), pixels));
    }
    for name in &request.clips {
        let Some(index) = model.clip(name) else {
            log::warn!("跳过 {name}:glb 里没有这段动作");
            continue;
        };
        let duration = model.clips[index].duration;
        let mut player = Player::new(&model, index);
        player.seek(duration * request.at);
        player.update(&model);
        pet.update(
            &queue,
            view_proj,
            light_dir,
            outline_width,
            request.time,
            &player.matrices,
        );
        let pixels = draw_and_read(
            &device,
            &queue,
            &pet,
            &color,
            &color_view,
            &depth_view,
            &readback,
            size,
            padded_row,
        )?;
        log::info!(
            "  {name}: {duration:.2}s,采样 {:.2}s",
            duration * request.at
        );
        tiles.push((name.clone(), pixels));
    }

    // 淡化探针:从第一段切到第二段,停在淡化中点,应当是两个姿势的平滑中间态
    if request.fade_probe && request.clips.len() >= 2 {
        if let (Some(a), Some(b)) = (model.clip(&request.clips[0]), model.clip(&request.clips[1])) {
            let mut player = Player::new(&model, a);
            player.seek(model.clips[a].duration * request.at);
            player.play(b);
            player.advance(&model, 0.09); // 淡化时长 0.18s,取中点
            player.update(&model);
            pet.update(
                &queue,
                view_proj,
                light_dir,
                outline_width,
                request.time,
                &player.matrices,
            );
            let pixels = draw_and_read(
                &device,
                &queue,
                &pet,
                &color,
                &color_view,
                &depth_view,
                &readback,
                size,
                padded_row,
            )?;
            log::info!("  淡化中点: {} → {}", request.clips[0], request.clips[1]);
            tiles.push((format!("{}→{}", request.clips[0], request.clips[1]), pixels));
        }
    }

    if request.bench > 0 {
        benchmark(
            &device,
            &queue,
            &pet,
            &color_view,
            &depth_view,
            &model,
            request.bench,
        );
    }

    if tiles.is_empty() {
        bail!("没有渲出任何一格");
    }
    write_sheet(&request.out, size, &tiles)?;
    log::info!(
        "写出 {} ({} 格): {}",
        request.out.display(),
        tiles.len(),
        tiles
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    Ok(())
}

/// 走完整的「采样动画 → 上传 → 绘制」循环,量单只宠物一帧要多少时间。
/// 不做回读:回读会等 GPU 排空,量出来的是同步开销而不是出帧开销。
fn benchmark(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pet: &PetGpu,
    color_view: &wgpu::TextureView,
    depth_view: &wgpu::TextureView,
    model: &Model,
    frames: u32,
) {
    let clip = 0;
    let mut player = Player::new(model, clip);
    let view_proj = orthographic_view(model.motion_bounds, 0.0, RENDER_PADDING);
    let light = Vec3::new(-0.4, 0.8, 0.6);
    let start = std::time::Instant::now();
    // bench 是逐帧推进的,时间也跟着走 —— 正好顺带压到「随时间变的那几层」的开销
    let mut frame_time = 0.0f32;
    for _ in 0..frames {
        frame_time += 1.0 / 60.0;
        player.advance(model, 1.0 / 60.0);
        player.update(model);
        pet.update(
            queue,
            view_proj,
            light,
            0.004,
            frame_time,
            &player.matrices,
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
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
    // 最后等一次,确保所有帧真的做完了才计时
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let elapsed = start.elapsed();
    log::info!(
        "基准: {frames} 帧 {:.0}ms,平均 {:.3}ms/帧(≈{:.0} fps 上限)",
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / frames as f64,
        frames as f64 / elapsed.as_secs_f64()
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_and_read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pet: &PetGpu,
    color: &wgpu::Texture,
    color_view: &wgpu::TextureView,
    depth_view: &wgpu::TextureView,
    readback: &wgpu::Buffer,
    size: u32,
    padded_row: u32,
) -> Result<Vec<u8>> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offscreen"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pet"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
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
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("等 GPU 回读失败")?;
    rx.recv().context("回读回调没来")?.context("回读映射失败")?;

    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    {
        let view = readback
            .slice(..)
            .get_mapped_range()
            .context("取回读映射失败")?;
        for row in 0..size {
            let start = (row * padded_row) as usize;
            pixels.extend_from_slice(&view[start..start + (size * 4) as usize]);
        }
    }
    readback.unmap();
    Ok(pixels)
}

/// 把各格横向拼成一张 PNG。渲出的是预乘 alpha,写文件前反预乘回普通 RGBA 才不会发暗。
fn write_sheet(out: &Path, size: u32, tiles: &[(String, Vec<u8>)]) -> Result<()> {
    let width = size * tiles.len() as u32;
    let mut sheet = image::RgbaImage::new(width, size);
    for (index, (_, pixels)) in tiles.iter().enumerate() {
        let offset = index as u32 * size;
        for y in 0..size {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                let a = pixels[i + 3];
                let unpremultiply = |c: u8| {
                    if a == 0 {
                        0
                    } else {
                        ((c as u32 * 255) / a as u32).min(255) as u8
                    }
                };
                sheet.put_pixel(
                    offset + x,
                    y,
                    image::Rgba([
                        unpremultiply(pixels[i]),
                        unpremultiply(pixels[i + 1]),
                        unpremultiply(pixels[i + 2]),
                        a,
                    ]),
                );
            }
        }
    }
    sheet
        .save(out)
        .with_context(|| format!("写 {out:?} 失败"))?;
    Ok(())
}

/// 从包目录里定位形态的 glb(也接受直接给 .glb)。
/// 从包的 manifest 里取这个形态的材质表。给的是裸 glb、或包没有材质表时返回 None。
fn load_materials(
    pack: &Path,
    glb: &Path,
) -> Option<std::collections::HashMap<String, crate::pack::Material>> {
    let dir = if pack.extension().is_some_and(|e| e == "glb") {
        // 裸 glb:往上两级找包目录(forms/<资产>/model.glb)
        pack.parent()?.parent()?.parent()?
    } else {
        pack
    };
    let loaded = crate::pack::Pack::load(dir).ok()?;
    let asset = glb.parent()?.file_name()?.to_str()?;
    let form = loaded.forms.iter().find(|f| f.asset == asset)?;
    (!form.materials.is_empty()).then(|| form.materials.clone())
}

fn locate_glb(pack: &Path, form: Option<&str>) -> Result<PathBuf> {
    if pack.extension().is_some_and(|e| e == "glb") {
        return Ok(pack.to_path_buf());
    }
    let forms_dir = pack.join("forms");
    let mut forms: Vec<PathBuf> = std::fs::read_dir(&forms_dir)
        .with_context(|| format!("{forms_dir:?} 读不到(不是宠物包目录?)"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    forms.sort();
    if let Some(want) = form {
        forms.retain(|p| p.file_name().is_some_and(|n| n == want));
    }
    let form_dir = forms
        .first()
        .with_context(|| format!("{forms_dir:?} 里没有匹配的形态"))?;
    Ok(form_dir.join("model.glb"))
}
