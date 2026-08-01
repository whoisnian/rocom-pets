//! 「常用配置」那一页:目标帧率、整体大小、叫声音量、启动就穿透。
//!
//! 整体大小在这里是**百分比**(100% = 配置里的 `px_per_cm = 2.0`),不是「每厘米几像素」——
//! 后者对着屏幕想象不出多大。托盘里那三档也是同一套说法,两边对得上。

use eframe::egui;

use super::{SettingsApp, theme};
use crate::control::PX_PER_CM_STANDARD;

/// 整体大小能调的范围(倍率)。托盘那三档 50% / 100% / 150% 都落在里面。
const SIZE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=3.0;

impl SettingsApp {
    pub(super) fn common_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("常用配置");
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(10.0);

        egui::Grid::new("common")
            .num_columns(2)
            .min_col_width(theme::LABEL_W)
            .spacing([14.0, 16.0])
            .show(ui, |ui| {
                self.fps_row(ui);
                self.size_row(ui);
                self.volume_row(ui);
                self.passthrough_row(ui);
            });
    }

    fn label(ui: &mut egui::Ui, text: &str) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(text);
        });
    }

    /// 帧率是**档位不是滑杆**:托盘里就那三档,这里也只给这三个,
    /// 否则两处对同一件事给出的选择不一样。手写进配置的怪值(45)照样显示得出来。
    fn fps_row(&mut self, ui: &mut egui::Ui) {
        Self::label(ui, "目标帧率:");
        ui.horizontal(|ui| {
            for (value, label) in crate::control::FPS_STEPS {
                let picked = self.config.fps == *value;
                if ui.add(egui::Button::selectable(picked, *label)).clicked() && !picked {
                    self.config.fps = *value;
                    self.apply();
                }
            }
            if crate::control::exact_step(crate::control::FPS_STEPS, self.config.fps).is_none() {
                ui.label(theme::value(format!("{} 帧/秒", self.config.fps)));
            }
            theme::hint(ui, "越高越顺,也越费 CPU");
        });
        ui.end_row();
    }

    fn size_row(&mut self, ui: &mut egui::Ui) {
        Self::label(ui, "整体大小:");
        let commit = ui
            .horizontal(|ui| {
                let mut factor = self.config.px_per_cm / PX_PER_CM_STANDARD;
                let commit = percent_slider(ui, &mut factor, SIZE_RANGE);
                self.config.px_per_cm = factor * PX_PER_CM_STANDARD;
                theme::hint(ui, "乘在每只宠物自己的倍率上");
                commit
            })
            .inner;
        if commit {
            self.apply();
        }
        ui.end_row();
    }

    fn volume_row(&mut self, ui: &mut egui::Ui) {
        Self::label(ui, "叫声音量:");
        let commit = ui
            .horizontal(|ui| {
                let commit = percent_slider(ui, &mut self.config.volume, 0.0..=1.0);
                theme::hint(ui, "0 = 完全不开音频设备");
                commit
            })
            .inner;
        if commit {
            self.apply();
        }
        ui.end_row();
    }

    fn passthrough_row(&mut self, ui: &mut egui::Ui) {
        Self::label(ui, "启动穿透:");
        if ui
            .checkbox(&mut self.config.passthrough, "启动时就开启点击穿透")
            .changed()
        {
            self.apply();
        }
        ui.end_row();
    }
}

/// 一根滑杆 + 右边**可编辑**的百分比框,两边盯着同一个值。
///
/// 输入框用 `DragValue`:点一下就能打字、回车或点开就提交、**超出上下限自动夹回来** ——
/// 这正好是这里要的全部行为。自己拿 `TextEdit` 拼一遍等于把解析、夹取、
/// 「什么时候算提交」再写一遍。
///
/// 界面上一律以「百分之几」为准,滑杆拖出来的也取整 —— 于是**显示的数字就是存下的数字**,
/// 不会出现「框里写 124%、其实是 123.7%」这种对不上的情况。
///
/// 手写进配置的越界值(`px_per_cm = 8.0`)只是**显示**成上限,不动盘上那份 ——
/// 打开个窗口就把人手写的东西改了不合适;碰一下滑杆自然就落回范围内。
///
/// 返回 true = 该落盘了。**拖动过程中不落盘**:每帧发一次 `Reload` 等于每帧重建一次宠物;
/// 松手、或者输入框提交时才算数。
pub(super) fn percent_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let (lo, hi) = (
        (range.start() * 100.0).round(),
        (range.end() * 100.0).round(),
    );
    let mut percent = (*value * 100.0).round().clamp(lo, hi);
    let slider = ui.add(
        egui::Slider::new(&mut percent, lo..=hi)
            .show_value(false)
            .trailing_fill(true),
    );
    let edit = ui.add_sized(
        [58.0, theme::CONTROL_H],
        egui::DragValue::new(&mut percent)
            .range(lo..=hi)
            .suffix("%")
            .fixed_decimals(0)
            .speed(1.0),
    );
    if slider.changed() || edit.changed() {
        *value = percent / 100.0;
    }
    // 输入框自己也能拖(它是个 DragValue),所以那条路和滑杆一样要等松手
    slider.drag_stopped() || edit.drag_stopped() || (edit.changed() && !edit.dragged())
}
