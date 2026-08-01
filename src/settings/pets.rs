//! 「活跃宠物」那一页:加/撤宠物,改每一只的形态、大小、性格、表情。
//!
//! 这一页编辑的就是 `roster.toml` 里那一串 `[[pet]]`。托盘菜单里也能改其中几项
//! (形态/大小/性格),但表情池是多选、大小是连续值 —— 菜单表达不了,只有这儿有。

use eframe::egui;

use super::SettingsApp;
use crate::persona;
use crate::platform::PetOptions;
use crate::roster::Slot;
use crate::stage::EMOTES;

impl SettingsApp {
    pub(super) fn pets_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("活跃宠物");
            ui.label(format!("({} 只)", self.roster.len()));
        });
        ui.small("这里改的是「桌面上现在有哪几只、各是什么脾气」。改完记得保存。");
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.add_section(ui);
                ui.add_space(8.0);
                if self.roster.is_empty() {
                    ui.label("台上一只都没有。从上面挑一个包加进来。");
                    return;
                }
                // 撤下要等这一轮画完再做,否则下标会在遍历中途错位
                let mut remove: Option<usize> = None;
                for slot in 0..self.roster.len() {
                    if self.pet_card(ui, slot) {
                        remove = Some(slot);
                    }
                    ui.add_space(6.0);
                }
                if let Some(slot) = remove {
                    self.roster.remove(slot);
                    self.dirty = true;
                    self.status.ok("已撤下,记得保存");
                }
            });
    }

    /// 「添加宠物」:一个查找框 + 匹配到的包。
    ///
    /// **不做成下拉菜单**:包目录里有五百多个包,拉一个五百项的列表没法用;
    /// 打两个字筛出来才是实际的用法(与托盘「加一只」按名字切段是同一个问题的两种解法)。
    fn add_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("添加宠物")
            .default_open(self.roster.is_empty())
            .show(ui, |ui| {
                if self.entries.is_empty() {
                    ui.label("包目录里没有包。去「宠物包」那页导入。");
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label("查找");
                    ui.text_edit_singleline(&mut self.filter);
                });
                let needle = self.filter.trim().to_lowercase();
                let hits: Vec<(String, std::path::PathBuf)> = self
                    .entries
                    .iter()
                    .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
                    .map(|e| (e.name.clone(), e.path.clone()))
                    .collect();
                // 一屏放不下就先给个提示,别把五百个按钮全画出来
                const MAX_SHOWN: usize = 40;
                ui.small(format!(
                    "{} 个匹配{}",
                    hits.len(),
                    if hits.len() > MAX_SHOWN {
                        format!(",只列前 {MAX_SHOWN} 个,再打几个字缩小范围")
                    } else {
                        String::new()
                    }
                ));
                egui::ScrollArea::vertical()
                    .id_salt("add-pet")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (name, path) in hits.iter().take(MAX_SHOWN) {
                            if ui.button(format!("+ {name}")).clicked() {
                                self.add_to_roster(path);
                            }
                        }
                    });
            });
    }

    /// 一只宠物的那一块。返回 true 表示「撤下我」。
    fn pet_card(&mut self, ui: &mut egui::Ui, slot: usize) -> bool {
        let pack = self.pack_for_slot(slot);
        let title = match &pack {
            Some(pack) => pack.species_name.clone(),
            None => self.roster[slot].pack.clone(),
        };
        let mut remove = false;

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(&title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("撤下").clicked() {
                        remove = true;
                    }
                });
            });

            let Some(pack) = pack else {
                // 包被删了或者改名了。**不自动清掉**:也可能只是包目录暂时没挂上,
                // 由用户决定要不要撤下
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "这个包在包目录里找不到了,它上不了台。",
                );
                return;
            };

            // 编辑的是运行时形状,改完再写回存档形状(默认值不落盘)
            let mut options = PetOptions::from_slot(&self.roster[slot]);
            let mut changed = false;

            egui::Grid::new(("pet", slot))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    // ── 形态 ─────────────────────────────────
                    if pack.forms.len() > 1 {
                        ui.label("形态");
                        let current = self.roster[slot]
                            .form
                            .as_deref()
                            .and_then(|want| {
                                pack.forms.iter().position(|f| f.asset == want || f.name == want)
                            })
                            .unwrap_or(0);
                        let mut picked = current;
                        egui::ComboBox::from_id_salt(("form", slot))
                            .selected_text(&pack.forms[current].name)
                            .show_ui(ui, |ui| {
                                for (index, form) in pack.forms.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut picked,
                                        index,
                                        format!("{}({:.0}cm)", form.name, form.height_cm),
                                    );
                                }
                            });
                        if picked != current {
                            self.roster[slot].form = Some(pack.forms[picked].asset.clone());
                            changed = true;
                        }
                        ui.end_row();
                    }

                    // ── 大小 ─────────────────────────────────
                    ui.label("大小");
                    ui.vertical(|ui| {
                        if ui
                            .add(
                                egui::Slider::new(
                                    &mut options.scale,
                                    crate::platform::SCALE_RANGE,
                                )
                                .suffix("×")
                                .fixed_decimals(2),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        // 换算成像素:倍率本身没有直觉,像素高度有
                        let form = &pack.forms[self.roster[slot]
                            .form
                            .as_deref()
                            .and_then(|want| {
                                pack.forms.iter().position(|f| f.asset == want || f.name == want)
                            })
                            .unwrap_or(0)];
                        ui.small(format!(
                            "屏幕上约 {:.0}px 高",
                            form.height_cm * form.scale * self.config.px_per_cm * options.scale
                        ));
                    });
                    ui.end_row();

                    // ── 性格 ─────────────────────────────────
                    ui.label("性格");
                    ui.vertical(|ui| {
                        egui::ComboBox::from_id_salt(("persona", slot))
                            .selected_text(options.persona.name)
                            .show_ui(ui, |ui| {
                                for candidate in persona::ALL {
                                    if ui
                                        .selectable_label(
                                            options.persona.id == candidate.id,
                                            candidate.name,
                                        )
                                        .clicked()
                                    {
                                        options.persona = *candidate;
                                        changed = true;
                                    }
                                }
                            });
                        ui.small(options.persona.about);
                    });
                    ui.end_row();

                    // ── 表情 ─────────────────────────────────
                    ui.label("表情");
                    ui.vertical(|ui| {
                        // 存档里 None = 全都要;这里摊成一组勾选,勾满了再存回 None。
                        // **`allowed` 拿的是 String 而不是 `&str`**:借着 `options.emotes`
                        // 的话,下面那一行赋值就借不到了
                        let mut allowed: Vec<String> = match &options.emotes {
                            Some(list) => list.clone(),
                            None => EMOTES.iter().map(|(name, _)| name.to_string()).collect(),
                        };
                        let mut touched = false;
                        ui.horizontal_wrapped(|ui| {
                            for (name, label) in EMOTES {
                                let mut on = allowed.iter().any(|n| n == name);
                                // 包里没有这段动作的话勾了也没用,直接标出来
                                let missing =
                                    !pack.forms.iter().any(|f| f.clips.contains_key(*name));
                                let response =
                                    ui.add_enabled(!missing, egui::Checkbox::new(&mut on, *label));
                                if missing {
                                    response.on_disabled_hover_text("这个包里没有这段动作");
                                } else if response.changed() {
                                    if on {
                                        allowed.push(name.to_string());
                                    } else {
                                        allowed.retain(|n| n != name);
                                    }
                                    touched = true;
                                }
                            }
                        });
                        if touched {
                            options.emotes =
                                (allowed.len() != EMOTES.len()).then_some(allowed);
                            changed = true;
                        }
                        ui.small("待机时随手做的表情。一个不留的话它就只会站着。");
                    });
                    ui.end_row();
                });

            if changed {
                write_options(&mut self.roster[slot], &options);
                self.dirty = true;
            }
        });
        remove
    }
}

/// 把编辑中的选项写回存档形状。**默认值不落盘**,理由见 roster.rs 的 `Slot`。
///
/// 不复用 `PetOptions::write_into`:那个是 platform 内部用的(私有),
/// 而这里还要把 `scale` 按合法区间夹一次 —— 滑杆给的值总在区间里,但存档是文本文件,
/// 谁都可能手改。
fn write_options(slot: &mut Slot, options: &PetOptions) {
    let scale = options.scale.clamp(
        *crate::platform::SCALE_RANGE.start(),
        *crate::platform::SCALE_RANGE.end(),
    );
    slot.scale = (scale != 1.0).then_some(scale);
    slot.persona = options.persona.saved_id();
    slot.emotes = options.emotes.clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_do_not_land_in_the_saved_slot() {
        // 默认那一只在存档里只该有 pack/form,不然「谁被调过」就看不出来了
        let mut slot = Slot::new("喵喵".into(), None);
        write_options(&mut slot, &PetOptions::default());
        assert_eq!(slot.scale, None);
        assert_eq!(slot.persona, None);
        assert_eq!(slot.emotes, None);
    }

    #[test]
    fn edited_options_round_trip() {
        let mut slot = Slot::new("喵喵".into(), None);
        let options = PetOptions {
            scale: 1.5,
            persona: persona::Persona::by_id("lazy"),
            emotes: Some(vec!["Happy".into()]),
        };
        write_options(&mut slot, &options);
        assert_eq!(PetOptions::from_slot(&slot), options);
    }

    #[test]
    fn a_hand_edited_scale_is_clamped() {
        // 存档是文本文件:有人写个 99 进去,画布就会大到显存装不下
        let mut slot = Slot::new("喵喵".into(), None);
        write_options(
            &mut slot,
            &PetOptions {
                scale: 99.0,
                ..PetOptions::default()
            },
        );
        assert_eq!(slot.scale, Some(*crate::platform::SCALE_RANGE.end()));
    }
}
