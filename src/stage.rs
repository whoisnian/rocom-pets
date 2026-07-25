//! 与平台无关的 stage 逻辑:精灵位置、拖动、命中测试、输入区计算。
//!
//! 平台后端只负责「造表面 / 收事件 / 提交帧 / 设输入区」,所有状态都在这里,
//! 这样 Wayland 与 Windows 两边的行为天然一致,也能脱离窗口系统做单元测试。

use crate::sprite::{Rect, Sprite};

/// 后端喂进来的事件(坐标都是表面局部逻辑像素)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StageEvent {
    Resized { width: u32, height: u32 },
    PointerMoved { x: f64, y: f64 },
    PointerPressed { x: f64, y: f64 },
    PointerReleased,
    PointerLeft,
    TogglePassthrough,
}

/// 处理一个事件后,后端该做什么。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    /// 需要重画并提交一帧。
    pub redraw: bool,
    /// 输入区变了,后端要重新 `set_input_region` / 更新命中区域。
    pub regions_dirty: bool,
}

impl Reaction {
    const NONE: Self = Self {
        redraw: false,
        regions_dirty: false,
    };
    const REDRAW: Self = Self {
        redraw: true,
        regions_dirty: false,
    };
    const BOTH: Self = Self {
        redraw: true,
        regions_dirty: true,
    };
}

/// 输入区网格粒度与 alpha 阈值,见 `Sprite::coverage_rects`。
const REGION_CELL: u32 = 8;
const REGION_ALPHA_THRESHOLD: u8 = 8;

pub struct Stage {
    sprite: Sprite,
    /// 表面尺寸(逻辑像素)。
    size: (u32, u32),
    /// 精灵左上角在表面内的位置。
    pos: (f32, f32),
    /// 精灵局部坐标下的覆盖矩形,只在精灵换了才重算。
    coverage: Vec<Rect>,
    pointer: Option<(f64, f64)>,
    /// 按下时记下的「指针 - 精灵左上角」偏移。
    drag_offset: Option<(f64, f64)>,
    passthrough: bool,
}

impl Stage {
    pub fn new(sprite: Sprite, size: (u32, u32)) -> Self {
        let coverage = sprite.coverage_rects(REGION_CELL, REGION_ALPHA_THRESHOLD);
        let mut stage = Self {
            sprite,
            size,
            pos: (0.0, 0.0),
            coverage,
            pointer: None,
            drag_offset: None,
            passthrough: false,
        };
        stage.center();
        stage
    }

    pub fn sprite(&self) -> &Sprite {
        &self.sprite
    }

    /// 精灵左上角位置(表面局部逻辑像素)。
    pub fn sprite_pos(&self) -> (f32, f32) {
        self.pos
    }

    pub fn passthrough(&self) -> bool {
        self.passthrough
    }

    pub fn is_dragging(&self) -> bool {
        self.drag_offset.is_some()
    }

    /// 把精灵摆到表面中央。
    pub fn center(&mut self) {
        self.pos = (
            (self.size.0 as f32 - self.sprite.width as f32) * 0.5,
            (self.size.1 as f32 - self.sprite.height as f32) * 0.5,
        );
        self.clamp_to_surface();
    }

    fn clamp_to_surface(&mut self) {
        let max_x = (self.size.0 as f32 - self.sprite.width as f32).max(0.0);
        let max_y = (self.size.1 as f32 - self.sprite.height as f32).max(0.0);
        self.pos.0 = self.pos.0.clamp(0.0, max_x);
        self.pos.1 = self.pos.1.clamp(0.0, max_y);
    }

    /// 当前该交给合成器的输入区(表面局部坐标)。穿透时为空。
    pub fn input_regions(&self) -> Vec<Rect> {
        if self.passthrough {
            return Vec::new();
        }
        let (dx, dy) = (self.pos.0.round() as i32, self.pos.1.round() as i32);
        self.coverage.iter().map(|r| r.translated(dx, dy)).collect()
    }

    /// 表面坐标是否落在精灵的不透明像素上(比输入区更精确,用于自己内部的判定)。
    pub fn hit_test(&self, x: f64, y: f64) -> bool {
        let lx = (x - self.pos.0 as f64).floor() as i32;
        let ly = (y - self.pos.1 as f64).floor() as i32;
        self.sprite.alpha_at(lx, ly) >= REGION_ALPHA_THRESHOLD
    }

    pub fn handle(&mut self, event: StageEvent) -> Reaction {
        match event {
            StageEvent::Resized { width, height } => {
                if (width, height) == self.size {
                    return Reaction::NONE;
                }
                self.size = (width, height);
                self.clamp_to_surface();
                Reaction::BOTH
            }
            StageEvent::PointerMoved { x, y } => {
                self.pointer = Some((x, y));
                match self.drag_offset {
                    Some((ox, oy)) => {
                        self.pos = ((x - ox) as f32, (y - oy) as f32);
                        self.clamp_to_surface();
                        Reaction::BOTH
                    }
                    None => Reaction::NONE,
                }
            }
            StageEvent::PointerPressed { x, y } => {
                self.pointer = Some((x, y));
                if self.passthrough || !self.hit_test(x, y) {
                    return Reaction::NONE;
                }
                self.drag_offset = Some((x - self.pos.0 as f64, y - self.pos.1 as f64));
                Reaction::REDRAW
            }
            StageEvent::PointerReleased => {
                if self.drag_offset.take().is_some() {
                    Reaction::REDRAW
                } else {
                    Reaction::NONE
                }
            }
            StageEvent::PointerLeft => {
                self.pointer = None;
                if self.drag_offset.take().is_some() {
                    Reaction::REDRAW
                } else {
                    Reaction::NONE
                }
            }
            StageEvent::TogglePassthrough => {
                self.passthrough = !self.passthrough;
                self.drag_offset = None;
                Reaction::BOTH
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> Stage {
        Stage::new(Sprite::test_pattern(64), (800, 600))
    }

    #[test]
    fn starts_centered_and_regions_follow_sprite() {
        let s = stage();
        assert_eq!(s.sprite_pos(), (368.0, 268.0));
        let regions = s.input_regions();
        assert!(!regions.is_empty());
        // 输入区跟着精灵平移:圆心必被覆盖,精灵外必不被覆盖
        assert!(regions.iter().any(|r| r.contains(400.0, 300.0)));
        assert!(!regions.iter().any(|r| r.contains(10.0, 10.0)));
    }

    #[test]
    fn drag_moves_sprite_and_dirties_regions() {
        let mut s = stage();
        let start = s.sprite_pos();
        // 按在圆心上
        let hit = s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        assert!(hit.redraw && s.is_dragging());
        let moved = s.handle(StageEvent::PointerMoved { x: 450.0, y: 320.0 });
        assert_eq!(
            moved,
            Reaction {
                redraw: true,
                regions_dirty: true
            }
        );
        assert_eq!(s.sprite_pos(), (start.0 + 50.0, start.1 + 20.0));
        s.handle(StageEvent::PointerReleased);
        assert!(!s.is_dragging());
    }

    #[test]
    fn press_on_transparent_pixel_is_ignored() {
        let mut s = stage();
        // 精灵包围盒左上角是圆外的透明像素
        let (px, py) = s.sprite_pos();
        assert_eq!(
            s.handle(StageEvent::PointerPressed {
                x: px as f64,
                y: py as f64
            }),
            Reaction::NONE
        );
        assert!(!s.is_dragging());
    }

    #[test]
    fn passthrough_clears_regions_and_blocks_drag() {
        let mut s = stage();
        assert_eq!(s.handle(StageEvent::TogglePassthrough), Reaction::BOTH);
        assert!(s.passthrough());
        assert!(s.input_regions().is_empty());
        s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        assert!(!s.is_dragging());
    }

    #[test]
    fn sprite_stays_inside_after_shrink() {
        let mut s = stage();
        s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        s.handle(StageEvent::PointerMoved { x: 790.0, y: 590.0 });
        s.handle(StageEvent::PointerReleased);
        s.handle(StageEvent::Resized {
            width: 300,
            height: 200,
        });
        let (x, y) = s.sprite_pos();
        assert!(x + 64.0 <= 300.0 && y + 64.0 <= 200.0, "精灵越界: {x},{y}");
    }
}
