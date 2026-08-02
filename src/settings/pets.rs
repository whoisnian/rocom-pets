//! 「活跃宠物」逐只编辑。侧栏把每只展开成一项,这里画选中那一只的详情。
//!
//! 编辑的就是 `roster.toml` 里那一段 `[[pet]]`。**改完立刻生效**:形态、大小、性格、
//! 参与叫声、记住落脚点,每一项都会让桌宠那边重建这只角色。
//! 唯一的例外是大小那一行 —— 拖的时候只动数字,松手(或数值框提交)才落盘
//! (见 mod.rs 的说明)。

use eframe::egui;

use super::common::percent_slider;
use super::{Page, SettingsApp, theme};
use crate::persona;
use crate::platform::{PetOptions, SCALE_RANGE};

impl SettingsApp {
    pub(super) fn pet_page(&mut self, ui: &mut egui::Ui, slot: usize) {
        if slot >= self.roster.len() {
            self.page = Page::Packs;
            return;
        }
        let Some(pack) = self.pack_for_slot(slot) else {
            self.missing_pack(ui, slot);
            return;
        };
        let form_index = self.form_index(&pack, slot);
        let form = &pack.forms[form_index];

        // ── 标题 + 来源行 + 撤下 ───────────────────────────────
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(&form.name);
                ui.add_space(2.0);
                let archived = self
                    .path_for_slot(slot)
                    .is_some_and(|p| p.is_file());
                theme::hint(
                    ui,
                    format!(
                        "{} 进化链 · 第 {} 形态 · 包 {} {} · {}",
                        pack.species_name,
                        form_index + 1,
                        pack.species_id,
                        pack.species_name,
                        if archived { "rkpet" } else { "目录" }
                    ),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if ui.button("撤下").clicked() {
                    self.roster.remove(slot);
                    self.page = if self.roster.is_empty() {
                        Page::Packs
                    } else {
                        Page::Pet(slot.min(self.roster.len() - 1))
                    };
                    self.apply();
                    self.status.ok("已撤下");
                }
            });
        });
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(14.0);

        // ── 表单 ──────────────────────────────────────────────
        let mut options = PetOptions::from_slot(&self.roster[slot]);
        let before = options.clone();
        let mut form_changed = false;
        let mut commit = false;

        egui::Grid::new(("pet", slot))
            .num_columns(2)
            .min_col_width(theme::LABEL_W)
            .spacing([14.0, 16.0])
            .show(ui, |ui| {
                if pack.forms.len() > 1 {
                    label(ui, "形态:");
                    ui.horizontal(|ui| {
                        let mut picked = form_index;
                        egui::ComboBox::from_id_salt(("form", slot))
                            .width(220.0)
                            .selected_text(format!(
                                "{}({} / {})",
                                pack.forms[form_index].name,
                                form_index + 1,
                                pack.forms.len()
                            ))
                            .show_ui(ui, |ui| {
                                for (index, f) in pack.forms.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut picked,
                                        index,
                                        format!("{}({:.0}cm)", f.name, f.height_cm),
                                    );
                                }
                            });
                        if picked != form_index {
                            self.roster[slot].form = Some(pack.forms[picked].asset.clone());
                            form_changed = true;
                        }
                        theme::hint(ui, "切换后立即在桌面上变身");
                    });
                    ui.end_row();
                }

                label(ui, "大小:");
                ui.horizontal(|ui| {
                    if percent_slider(ui, &mut options.scale, SCALE_RANGE) {
                        commit = true;
                    }
                    // 屏幕像素比倍率直观:范围那句在旁边解释能调到哪儿
                    let px =
                        form.height_cm * form.scale * self.config.px_per_cm * options.scale;
                    theme::hint(ui, format!("屏幕上约 {px:.0}px 高 · 50% – 200%"));
                });
                ui.end_row();

                label(ui, "性格:");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("persona", slot))
                        .width(168.0)
                        // 七条一次全露出来。默认上限是 `Spacing::combo_height` = 200px,
                        // 而七行按这套间距要 232px —— 差这么一点点就得滚,滚起来正好
                        // 挡住最后一条(实测只露到「胆小」)
                        .height(f32::INFINITY)
                        .selected_text(persona_label(&options.persona))
                        .show_ui(ui, |ui| {
                            for candidate in persona::ALL {
                                if ui
                                    .selectable_label(
                                        options.persona.id == candidate.id,
                                        persona_label(candidate),
                                    )
                                    .clicked()
                                {
                                    options.persona = *candidate;
                                }
                            }
                        });
                    theme::hint(ui, options.persona.about);
                });
                ui.end_row();

                label(ui, "叫声:");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut options.voice, "参与叫声");
                    theme::hint(ui, "嗓音");
                    ui.label(theme::value(format!("{:+.0}", options.voice_value.unwrap_or(0.0))));
                    // 0 = 原调。**不再自动掷**:同一个包的两只听着一样是正常的,
                    // 想要不一样就自己按一下
                    theme::hint(ui, "0 = 原调");
                    if ui.add_enabled(options.voice, egui::Button::new("重掷")).clicked() {
                        options.voice_value = Some(reroll(&mut self.status));
                    }
                });
                ui.end_row();

                self.actions_row(ui, slot, form);

                label(ui, "位置:");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut options.remember, "记住上次落脚点");
                    match (options.remember, options.home_x) {
                        (true, Some(x)) => {
                            theme::hint(ui, "上次站在");
                            ui.label(theme::value(theme::percent(x)));
                            theme::hint(ui, "处");
                        }
                        (true, None) => theme::hint(ui, "还没记到位置"),
                        (false, _) => theme::hint(ui, "每次上台重新摆"),
                    }
                });
                ui.end_row();
            });


        if options != before {
            options.write_into(&mut self.roster[slot]);
            // 滑杆还在拖的时候不落盘;其余改动立刻生效
            if commit || options.scale == before.scale {
                self.apply();
            }
        } else if form_changed || commit {
            self.apply();
        }
    }

    /// 包不在了(被删了或改名了)。**不自动清掉**:也可能只是包目录暂时没挂上,
    /// 由用户决定要不要撤下。
    fn missing_pack(&mut self, ui: &mut egui::Ui, slot: usize) {
        let name = self.roster[slot].pack.clone();
        ui.heading(&name);
        ui.add_space(10.0);
        ui.colored_label(
            ui.visuals().error_fg_color,
            "这个包在包目录里找不到了,它上不了台。",
        );
        ui.add_space(12.0);
        if ui.button("撤下").clicked() {
            self.roster.remove(slot);
            self.page = Page::Packs;
            self.apply();
            self.status.ok("已撤下");
        }
    }

    /// 这只宠物的动作:**和上面那几行同一张表**里的一行,一格一个按钮。
    /// 这个形态没有的置灰,点一下当场在桌面上播一次。
    ///
    /// 有没有这段动作是**现算的**,不是读 manifest 的 `[report]`(全库没有一个包写了
    /// 那一节);降级也算有,见 stage.rs 的 `has_clip`。
    ///
    /// 点一下走 `Control::Play`:配置窗口是**另一个进程**,只能喊一声让桌宠去播。
    fn actions_row(&mut self, ui: &mut egui::Ui, slot: usize, form: &crate::pack::Form) {
        let clips = crate::stage::RUNTIME_CLIPS;
        let have = clips
            .iter()
            .filter(|(name, _)| crate::stage::has_clip(form, name))
            .count();
        let running = crate::control::is_running();
        let mut play: Option<(usize, &'static str)> = None;
        label(ui, "动作:");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(theme::value(format!("{have} / {}", clips.len())));
                theme::hint(ui, "点击预览");
            });
            ui.add_space(6.0);
            egui::Grid::new(("actions", slot))
                .num_columns(6)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (index, (name, label)) in clips.iter().enumerate() {
                        let ok = crate::stage::has_clip(form, name);
                        let button =
                            egui::Button::new(*label).min_size(egui::vec2(74.0, theme::CONTROL_H));
                        let response = ui.add_enabled(ok && running, button);
                        if !ok {
                            response.on_disabled_hover_text("这个形态没有这段动作");
                        } else if !running {
                            response.on_disabled_hover_text("桌宠没在跑");
                        } else if response.clicked() {
                            play = Some((index, label));
                        }
                        if index % 6 == 5 {
                            ui.end_row();
                        }
                    }
                });
        });
        ui.end_row();
        if let Some((index, label)) = play {
            match crate::control::play(slot as u32, index as u32) {
                Ok(()) => self.status.ok(format!("让它做了个「{label}」")),
                Err(e) => self.status.fail(format!("没送出去:{e:#}")),
            }
        }
    }
}

fn label(ui: &mut egui::Ui, text: &str) {
    // 表单标签右对齐 —— 设计稿 KDE 栏的规格
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

/// 下拉里那一行:性格 + 它带来的那双眼睛。
///
/// 表情跟着性格走、单独改不了,所以**不给它一行**(只读的一行反而像是能点),
/// 把结果直接写进选项里:挑性格的时候多半正是冲着那张脸去的,不该先选一个
/// 再回头看提示才知道选中了什么。
///
/// 「默认眼」不写:那是**没有变化**的那一档,写出来等于给三条各挂一个不说明
/// 任何事情的后缀;带后缀的四条也就此从「七条里挑」变成「一眼看见的四条」。
fn persona_label(persona: &persona::Persona) -> String {
    match persona.face.name == persona::DEFAULT_FACE.name {
        true => persona.name.to_owned(),
        false => format!("{}「{}眼」", persona.name, persona.face.name),
    }
}

/// 掷一个新嗓音 −100~100。
///
/// 用时间当种子:这里只掷一个数,犯不着为它把 stage 里那个 xorshift 搬过来。
fn reroll(status: &mut super::Status) -> f32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let value = (nanos % 2001) as f32 / 10.0 - 100.0;
    status.ok(format!("嗓音重掷成 {value:+.0}"));
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::Slot;

    #[test]
    fn defaults_do_not_land_in_the_saved_slot() {
        // 默认那一只在存档里只该有 pack/form,不然「谁被调过」就看不出来了
        let mut slot = Slot::new("喵喵".into(), None);
        PetOptions::default().write_into(&mut slot);
        assert_eq!(slot.scale, None);
        assert_eq!(slot.persona, None);
        assert_eq!(slot.voice, None);
        assert_eq!(slot.remember, None);
    }

    #[test]
    fn edited_options_round_trip() {
        let mut slot = Slot::new("喵喵".into(), None);
        let options = PetOptions {
            scale: 1.5,
            persona: persona::Persona::by_id("lazy"),
            voice: false,
            voice_value: Some(-37.0),
            remember: true,
            home_x: Some(0.62),
        };
        options.write_into(&mut slot);
        assert_eq!(PetOptions::from_slot(&slot), options);
    }

    #[test]
    fn a_hand_edited_scale_is_clamped() {
        // 存档是文本文件:有人写个 99 进去,画布就会大到显存装不下
        let slot = Slot {
            scale: Some(99.0),
            ..Slot::new("喵喵".into(), None)
        };
        assert_eq!(PetOptions::from_slot(&slot).scale, *SCALE_RANGE.end());
    }

    #[test]
    fn a_reroll_lands_in_range() {
        let mut status = super::super::Status::default();
        for _ in 0..64 {
            let v = reroll(&mut status);
            assert!((-100.0..=100.0).contains(&v), "{v}");
        }
    }
}
