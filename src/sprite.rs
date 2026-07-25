//! S1 用的测试精灵:软边圆 + 内部半透棋盘格。
//!
//! 刻意选这个图形:软边能暴露预乘 alpha 错误(暗边/亮边),半透格能验证合成器
//! 真的在做 per-pixel alpha 混合(格子里应当能看见底下窗口的内容)。

/// 预乘 alpha 的 RGBA8 位图。
#[derive(Clone)]
pub struct Sprite {
    pub width: u32,
    pub height: u32,
    /// 长度 = width * height * 4,RGBA 顺序,**已预乘**。
    pub rgba: Vec<u8>,
}

impl Sprite {
    /// 生成测试精灵。`size` 是边长(像素)。
    pub fn test_pattern(size: u32) -> Self {
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        let r = size as f32 * 0.5;
        let cell = (size / 8).max(1);
        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 + 0.5 - r;
                let dy = y as f32 + 0.5 - r;
                let d = (dx * dx + dy * dy).sqrt();

                // 边缘 1.5px 内做线性过渡,得到抗锯齿软边
                let edge = ((r - d) / 1.5).clamp(0.0, 1.0);
                // 棋盘格:亮格不透明,暗格半透(用来看穿到底下窗口)
                let checker = ((x / cell) + (y / cell)) % 2 == 0;
                let alpha = edge * if checker { 1.0 } else { 0.45 };

                // 颜色:径向渐变的绿(取自宠物主色调),外圈略深便于看边界
                let t = (d / r).clamp(0.0, 1.0);
                let (cr, cg, cb) = (0.45 - 0.25 * t, 0.80 - 0.30 * t, 0.20 - 0.10 * t);

                let i = ((y * size + x) * 4) as usize;
                // 预乘:颜色先乘 alpha,合成器/wgpu 都按 PreMultiplied 处理
                rgba[i] = (cr * alpha * 255.0).round() as u8;
                rgba[i + 1] = (cg * alpha * 255.0).round() as u8;
                rgba[i + 2] = (cb * alpha * 255.0).round() as u8;
                rgba[i + 3] = (alpha * 255.0).round() as u8;
            }
        }
        Self {
            width: size,
            height: size,
            rgba,
        }
    }

    /// 精灵坐标 (x, y) 处的 alpha,用于命中测试。越界返回 0。
    pub fn alpha_at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return 0;
        }
        let i = ((y as u32 * self.width + x as u32) * 4 + 3) as usize;
        self.rgba[i]
    }

    /// 把不透明区域近似成矩形并集(精灵局部坐标)。
    ///
    /// Wayland 的 `wl_surface.set_input_region` 只吃矩形,所以「按轮廓穿透」实际是
    /// 「按矩形并集近似轮廓」。`cell` 是网格粒度(越小越贴合、矩形越多),
    /// `threshold` 是判定覆盖的 alpha 阈值。这里只做行内合并,不做跨行合并
    /// (矩形数量在 cell=8 时是几十个量级,够用;真需要再上 RLE 合并)。
    pub fn coverage_rects(&self, cell: u32, threshold: u8) -> Vec<Rect> {
        let cell = cell.max(1);
        let cols = self.width.div_ceil(cell);
        let rows = self.height.div_ceil(cell);
        let mut out = Vec::new();
        for row in 0..rows {
            let mut run_start: Option<u32> = None;
            for col in 0..=cols {
                let covered = col < cols && self.cell_covered(col, row, cell, threshold);
                match (covered, run_start) {
                    (true, None) => run_start = Some(col),
                    (false, Some(start)) => {
                        out.push(Rect {
                            x: (start * cell) as i32,
                            y: (row * cell) as i32,
                            w: ((col - start) * cell).min(self.width - start * cell),
                            h: cell.min(self.height - row * cell),
                        });
                        run_start = None;
                    }
                    _ => {}
                }
            }
        }
        out
    }

    fn cell_covered(&self, col: u32, row: u32, cell: u32, threshold: u8) -> bool {
        let x1 = ((col + 1) * cell).min(self.width);
        let y1 = ((row + 1) * cell).min(self.height);
        for y in row * cell..y1 {
            for x in col * cell..x1 {
                if self.alpha_at(x as i32, y as i32) >= threshold {
                    return true;
                }
            }
        }
        false
    }
}

/// 表面局部坐标系里的矩形(像素)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// 命中测试用;Wayland 侧走 alpha 判定,这里主要服务测试与将来的 Windows 后端。
    #[allow(dead_code)]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64
            && y >= self.y as f64
            && x < self.x as f64 + self.w as f64
            && y < self.y as f64 + self.h as f64
    }

    pub fn translated(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_edge_and_transparent_corners() {
        let s = Sprite::test_pattern(64);
        // 四角在圆外,必须全透明
        assert_eq!(s.alpha_at(0, 0), 0);
        assert_eq!(s.alpha_at(63, 63), 0);
        // 圆心不透明
        assert!(s.alpha_at(32, 32) > 100);
        // 预乘不变式:任何像素的颜色分量不得超过其 alpha
        for px in s.rgba.chunks_exact(4) {
            let a = px[3];
            assert!(px[0] <= a && px[1] <= a && px[2] <= a, "非预乘像素: {px:?}");
        }
    }

    #[test]
    fn coverage_rects_cover_opaque_pixels_only() {
        let s = Sprite::test_pattern(64);
        let rects = s.coverage_rects(8, 8);
        assert!(!rects.is_empty());
        // 圆心必被覆盖
        assert!(rects.iter().any(|r| r.contains(32.0, 32.0)));
        // 左上角(圆外)不该被任何矩形覆盖
        assert!(!rects.iter().any(|r| r.contains(0.5, 0.5)));
        // 矩形不越界
        for r in &rects {
            assert!(r.x >= 0 && r.y >= 0);
            assert!(r.x as u32 + r.w <= s.width && r.y as u32 + r.h <= s.height);
        }
    }
}
