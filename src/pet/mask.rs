//! 宠物轮廓的 alpha 掩码:从离屏画布回读,供命中测试与输入区使用。
//!
//! 为什么要回读:宠物的轮廓每帧都在变(动画 + 转身),CPU 侧没法便宜地算出来。
//! 而画布本来就渲好了,把它的 alpha 拿回来降采样成格子,就是现成的轮廓。
//!
//! 回读是**异步且滞后**的:提交拷贝 → 后续帧里 poll 到完成 → 换上新掩码。
//! 绝不能同步等 GPU(那会把出帧卡住)。滞后一两帧对「点宠物」这件事无所谓——
//! 格子粒度本来就有 8 物理像素,宠物一帧也走不了这么远。

use crate::sprite::Rect;

/// 掩码格子边长(物理像素)。8 与精灵那套输入区粒度一致:
/// 太小则矩形数量爆炸(wl_region 每个矩形都是一次调用),太大则腿/尾之间的空隙点不穿。
const CELL: u32 = 8;

/// 判定「这一格算宠物」的 alpha 阈值。
const ALPHA_THRESHOLD: u8 = 24;

/// 归约时的像素步长:格子有 8 像素,隔 2 个取一次(每格 16 个采样点)足够判覆盖,
/// CPU 侧的扫描量直接降到 1/4。再稀就可能漏掉胡须这类细结构。
const SAMPLE_STEP: u32 = 2;

/// 两次回读至少隔这么久。轮廓变化远慢于出帧,7Hz 足够跟上,还省掉每帧一次全画布扫描。
const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(140);

pub struct Mask {
    cols: u32,
    rows: u32,
    covered: Vec<bool>,
}

impl Mask {
    /// 用画布内的相对坐标(0..1)判命中。用比例而不是像素,省掉调用方换算缩放。
    pub fn hit(&self, u: f32, v: f32) -> bool {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return false;
        }
        let col = ((u * self.cols as f32) as u32).min(self.cols.saturating_sub(1));
        let row = ((v * self.rows as f32) as u32).min(self.rows.saturating_sub(1));
        self.covered[(row * self.cols + col) as usize]
    }

    /// 输出覆盖区域的矩形并集,坐标按 `logical_size` 缩放到角色局部逻辑像素。
    /// 只做行内合并:cell=8 时矩形是几十个量级,够用。
    pub fn rects(&self, logical_size: (u32, u32)) -> Vec<Rect> {
        let sx = logical_size.0 as f32 / (self.cols * CELL) as f32;
        let sy = logical_size.1 as f32 / (self.rows * CELL) as f32;
        let mut out = Vec::new();
        for row in 0..self.rows {
            let mut run: Option<u32> = None;
            for col in 0..=self.cols {
                let covered = col < self.cols && self.covered[(row * self.cols + col) as usize];
                match (covered, run) {
                    (true, None) => run = Some(col),
                    (false, Some(start)) => {
                        let x0 = (start * CELL) as f32 * sx;
                        let x1 = (col * CELL) as f32 * sx;
                        let y0 = (row * CELL) as f32 * sy;
                        let y1 = ((row + 1) * CELL) as f32 * sy;
                        out.push(Rect {
                            x: x0.floor() as i32,
                            y: y0.floor() as i32,
                            w: (x1 - x0).ceil().max(1.0) as u32,
                            h: (y1 - y0).ceil().max(1.0) as u32,
                        });
                        run = None;
                    }
                    _ => {}
                }
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        !self.covered.iter().any(|c| *c)
    }
}

/// 画布 → 掩码的异步回读。一次只在飞行中放一个请求。
pub struct MaskReadback {
    buffer: wgpu::Buffer,
    /// 缓冲对应的画布尺寸(物理像素)与行距。
    canvas: (u32, u32),
    padded_row: u32,
    pending: bool,
    receiver: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    last_request: Option<std::time::Instant>,
}

impl MaskReadback {
    pub fn new(device: &wgpu::Device, canvas: (u32, u32)) -> Self {
        let padded_row = (canvas.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pet-mask-readback"),
            size: (padded_row * canvas.1.max(1)) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            canvas,
            padded_row,
            pending: false,
            receiver: None,
            last_request: None,
        }
    }

    /// 画布尺寸变了要重建(缓冲大小与行距都变了)。
    pub fn resize(&mut self, device: &wgpu::Device, canvas: (u32, u32)) {
        if canvas == self.canvas {
            return;
        }
        *self = Self::new(device, canvas);
    }

    /// 提交一次「画布 → 缓冲」的拷贝并开始映射。
    /// 已有请求在飞、或距上次请求还不到 `MIN_INTERVAL` 就什么都不做。
    /// 这一份现在该不该再要一次:没有在途的请求,且离上次够久了。
    ///
    /// 多实体时**一帧只回读一只**(见 wayland.rs 的轮转),轮到谁要先问这一句 ——
    /// 否则轮到一个还在节流里的,这一帧的名额就白白浪费了。
    pub fn is_due(&self) -> bool {
        !self.pending
            && self
                .last_request
                .is_none_or(|t| std::time::Instant::now() - t >= MIN_INTERVAL)
    }

    pub fn request(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, canvas: &wgpu::Texture) {
        let now = std::time::Instant::now();
        if self.pending || self.last_request.is_some_and(|t| now - t < MIN_INTERVAL) {
            return;
        }
        self.last_request = Some(now);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mask-copy"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: canvas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.canvas.1),
                },
            },
            wgpu::Extent3d {
                width: self.canvas.0,
                height: self.canvas.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.receiver = Some(rx);
        self.pending = true;
    }

    /// 非阻塞地看一眼有没有回读完成;完成了就返回新掩码。
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Mask> {
        if !self.pending {
            return None;
        }
        // Poll 而不是 Wait:等 GPU 会把出帧卡住,宁可下一帧再来看
        let _ = device.poll(wgpu::PollType::Poll);
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                log::warn!("掩码回读失败: {e}");
                self.pending = false;
                self.receiver = None;
                return None;
            }
            Err(_) => return None, // 还没好
        }

        let mask = {
            let view = match self.buffer.slice(..).get_mapped_range() {
                Ok(view) => view,
                Err(e) => {
                    log::warn!("掩码映射失败: {e}");
                    self.pending = false;
                    self.receiver = None;
                    return None;
                }
            };
            Some(build(&view, self.canvas, self.padded_row))
        };
        self.buffer.unmap();
        self.pending = false;
        self.receiver = None;
        mask
    }
}

/// 把 RGBA 字节按格子 OR 归约成掩码(只看 alpha)。
fn build(bytes: &[u8], canvas: (u32, u32), padded_row: u32) -> Mask {
    let cols = canvas.0.div_ceil(CELL).max(1);
    let rows = canvas.1.div_ceil(CELL).max(1);
    let mut covered = vec![false; (cols * rows) as usize];
    for y in (0..canvas.1).step_by(SAMPLE_STEP as usize) {
        let row_start = (y * padded_row) as usize;
        let cell_row = y / CELL;
        for x in (0..canvas.0).step_by(SAMPLE_STEP as usize) {
            let alpha = bytes[row_start + (x * 4) as usize + 3];
            if alpha >= ALPHA_THRESHOLD {
                covered[(cell_row * cols + x / CELL) as usize] = true;
            }
        }
    }
    Mask {
        cols,
        rows,
        covered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张「中间一块不透明」的假画布字节。
    fn canvas(width: u32, height: u32, opaque: (u32, u32, u32, u32)) -> (Vec<u8>, u32) {
        let padded_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let mut bytes = vec![0u8; (padded_row * height) as usize];
        let (x0, y0, x1, y1) = opaque;
        for y in y0..y1 {
            for x in x0..x1 {
                bytes[(y * padded_row + x * 4) as usize + 3] = 255;
            }
        }
        (bytes, padded_row)
    }

    #[test]
    fn hit_only_inside_opaque_area() {
        let (bytes, padded) = canvas(64, 64, (16, 16, 48, 48));
        let mask = build(&bytes, (64, 64), padded);
        assert!(mask.hit(0.5, 0.5), "正中应命中");
        assert!(!mask.hit(0.05, 0.05), "左上角透明处不该命中");
        assert!(!mask.hit(1.5, 0.5), "越界不该命中");
        assert!(!mask.is_empty());
    }

    #[test]
    fn rects_cover_opaque_and_scale_to_logical() {
        let (bytes, padded) = canvas(64, 64, (16, 16, 48, 48));
        let mask = build(&bytes, (64, 64), padded);
        // 画布 64 物理像素 → 32 逻辑像素:矩形坐标应当整体减半
        let rects = mask.rects((32, 32));
        assert!(!rects.is_empty());
        assert!(
            rects.iter().any(|r| r.contains(16.0, 16.0)),
            "逻辑坐标 (16,16) 该被覆盖"
        );
        assert!(
            !rects.iter().any(|r| r.contains(2.0, 2.0)),
            "逻辑坐标 (2,2) 在轮廓外"
        );
        for r in &rects {
            assert!(r.x >= 0 && r.y >= 0 && r.x as u32 + r.w <= 32 && r.y as u32 + r.h <= 32);
        }
    }

    #[test]
    fn empty_canvas_yields_empty_mask() {
        let (bytes, padded) = canvas(32, 32, (0, 0, 0, 0));
        let mask = build(&bytes, (32, 32), padded);
        assert!(mask.is_empty());
        assert!(mask.rects((32, 32)).is_empty());
    }
}
