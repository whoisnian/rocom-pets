//! 「活跃宠物」逐只编辑。侧栏把每只展开成一项,这里画选中那一只的详情。
//!
//! 编辑的就是 `roster.toml` 里那一段 `[[pet]]`。**改完立刻生效**:形态、大小、性格、
//! 参与叫声、记住落脚点,每一项都会让桌宠那边重建这只角色。
//! 例外是大小与嗓音那两个数 —— 拖的时候只动数字,松手(或输入框提交)才落盘
//! (见 mod.rs 的说明)。

use eframe::egui;

use super::common::percent_slider;
use super::{Page, SettingsApp, theme};
use crate::persona;
use crate::platform::{PetOptions, SCALE_RANGE, VOICE_RANGE};

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
        // 「撤下」会把这一只从 roster 里删掉,而底下整张表还按 `slot` 索引它 ——
        // 所以点了之后**这一帧就到此为止**,剩下的下一帧按新的 `self.page` 画。
        let mut removed = false;
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
                    removed = true;
                }
            });
        });
        if removed {
            return;
        }
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
                        let items: Vec<String> = pack
                            .forms
                            .iter()
                            .map(|f| format!("{}({:.0}cm)", f.name, f.height_cm))
                            .collect();
                        // popup 要多宽:量最长那条,别让它换行或被裁。
                        // **归并成一个包之后形态名可以很长**(`晶石蜗(西瓜碧玺的样子)`),
                        // 按固定宽度给的话十有八九不够
                        let font = egui::TextStyle::Button.resolve(ui.style());
                        let widest = items
                            .iter()
                            .map(|text| {
                                ui.painter()
                                    .layout_no_wrap(
                                        text.clone(),
                                        font.clone(),
                                        egui::Color32::PLACEHOLDER,
                                    )
                                    .size()
                                    .x
                            })
                            .fold(0.0_f32, f32::max);
                        // **id 里带上形态个数**,否则同一个槽位换成形态更多的包会卡住:
                        // popup 那个 `Area` 把上一帧量到的尺寸记在 `ctx.memory().areas()` 里
                        // (egui 0.35 area.rs:466/666),而 `height(f32::INFINITY)` 让里面那个
                        // `ScrollArea` 退回 `available_rect_before_wrap()`(scroll_area.rs:763)
                        // —— 于是**可用高度 = 上一帧的自己**,只会缩不会涨,滚动条一出就下不去。
                        // 尺寸由几行决定,那就按几行分开记。
                        egui::ComboBox::from_id_salt(("form", slot, pack.forms.len()))
                            .width(220.0)
                            // 形态最多的那几条链有十三个(雪绒鸟、蹦蹦种子、脆筒甜甜),
                            // 默认上限 `Spacing::combo_height` = 200px 只够七八条 ——
                            // 和性格那个下拉一样,一次全露出来,别让人滚
                            .height(f32::INFINITY)
                            .selected_text(format!(
                                "{}({} / {})",
                                pack.forms[form_index].name,
                                form_index + 1,
                                pack.forms.len()
                            ))
                            .show_ui(ui, |ui| {
                                // 选项比框宽是正常的(框里那行还带「几 / 几」),
                                // 撑开 popup 就不会横向滚
                                ui.set_min_width(widest + theme::COMBO_ITEM_PAD);
                                for (index, text) in items.into_iter().enumerate() {
                                    ui.selectable_value(&mut picked, index, text);
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
                    let mut voice_value = options.voice_value.unwrap_or(0.0);
                    // 定宽:数字从 +0 变成 −100 时框子不该跟着长,
                    // 否则右边那句提示和「重掷」会横着跳
                    let edit = ui
                        .add_enabled_ui(options.voice, |ui| {
                            ui.add_sized(
                                [58.0, theme::CONTROL_H],
                                egui::DragValue::new(&mut voice_value)
                                    .range(VOICE_RANGE)
                                    .custom_formatter(|v, _| format!("{v:+.0}"))
                                    .fixed_decimals(0)
                                    .speed(1.0),
                            )
                        })
                        .inner;
                    // 0 = 原调,**不落盘**(默认值一律不写进 roster.toml)。上下限交给
                    // 输入框自己夹,不另写一句提示 —— 打个超范围的数进去立刻就看见了。
                    // **不再自动掷**:同一个包的两只听着一样是正常的,想要不一样就自己按一下
                    if ui.add_enabled(options.voice, egui::Button::new("重掷")).clicked() {
                        voice_value = reroll(&mut self.status);
                        commit = true;
                    }
                    options.voice_value = (voice_value != 0.0).then_some(voice_value);
                    // 跟大小那根滑杆同一个道理:数值框自己也能拖,拖的过程中别落盘
                    if edit.drag_stopped() || (edit.changed() && !edit.dragged()) {
                        commit = true;
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
            // 滑杆/数值框还在拖的时候不落盘;其余改动立刻生效。
            // 「变了但没提交」= 正拖着 —— 松手那一帧值已经不再变,走下面 commit 那条
            let dragging =
                options.scale != before.scale || options.voice_value != before.voice_value;
            if commit || !dragging {
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
    fn a_hand_edited_voice_is_clamped() {
        // 同上:存档是文本文件,写个 999 进去调出来的速率听不出是叫声
        let slot = Slot {
            voice_value: Some(999.0),
            ..Slot::new("喵喵".into(), None)
        };
        assert_eq!(
            PetOptions::from_slot(&slot).voice_value,
            Some(*VOICE_RANGE.end())
        );
    }

    #[test]
    fn a_reroll_lands_in_range() {
        let mut status = super::super::Status::default();
        for _ in 0..64 {
            let v = reroll(&mut status);
            assert!((-100.0..=100.0).contains(&v), "{v}");
        }
    }

    /// 撤下**唯一一只**宠物之后,这一帧不能再往下画。
    ///
    /// 曾经会 panic:`撤下` 把这一只从 roster 里删掉,而底下那张表接着
    /// `PetOptions::from_slot(&self.roster[slot])` —— 空 Vec 上取 [0]。
    /// (报告里的现场:`index out of bounds: the len is 0 but the index is 0`。)
    #[test]
    fn dismissing_the_last_pet_does_not_index_an_empty_roster() {
        use super::super::{Page, SettingsApp};
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let dir = std::env::temp_dir().join(format!("rocom-dismiss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("packs/喵喵")).expect("该能建目录");
        // 只要 manifest:pet_page 走的是 Pack::load,它不碰 glb
        std::fs::write(
            dir.join("packs/喵喵/manifest.toml"),
            "schema = 1\n\
             [species]\n\
             id = 3001\n\
             name = \"喵喵\"\n\
             chain = [3001]\n\
             [[forms]]\n\
             id = 3001\n\
             name = \"喵喵\"\n\
             stage = 1\n\
             asset = \"Gra_MiaoMiao1_001\"\n\
             model = \"forms/Gra_MiaoMiao1_001/model.glb\"\n\
             scale = 1\n\
             height_cm = 80.0\n\
             locomotion = \"ground\"\n\
             [forms.clips]\n\
             Idle = { clip = \"Idle\", ms = 1000, frames = 30 }\n",
        )
        .expect("该能写 manifest");
        std::fs::write(dir.join("roster.toml"), "[[pet]]\npack = \"喵喵\"\n")
            .expect("该能写阵容");

        let app = std::rc::Rc::new(std::cell::RefCell::new(SettingsApp::new(
            Some(dir.join("config.toml")),
            Some(dir.join("packs")),
            crate::control::SettingsPage::Pets,
        )));
        assert_eq!(app.borrow().roster.len(), 1, "前置条件:台上正好一只");
        assert!(
            matches!(app.borrow().page, Page::Pet(0)),
            "前置条件:停在这一只的页上"
        );

        let driven = app.clone();
        let mut harness = Harness::new_ui(move |ui| driven.borrow_mut().pet_page(ui, 0));
        harness.run();
        // 点「撤下」—— 修好之前,这一下就是那句 index out of bounds
        harness.get_by_label("撤下").click();
        harness.run();

        assert!(app.borrow().roster.is_empty(), "撤下之后台上不该还有宠物");
        assert!(
            matches!(app.borrow().page, Page::Packs),
            "台上空了就该回到宠物包那一页"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 当前那个下拉 popup 的高度:前景层里最高的那个 `Area`。
    fn popup_height(ctx: &egui::Context) -> Option<f32> {
        ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer| layer.order == egui::Order::Foreground)
                .filter_map(|layer| m.area_rect(layer.id))
                .map(|rect| rect.height())
                .max_by(f32::total_cmp)
        })
    }

    /// 写一个有 `forms` 个形态的包,返回包名。
    fn write_pack(root: &std::path::Path, name: &str, id: u32, forms: usize) -> String {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("该能建目录");
        let mut toml =
            format!("schema = 1\n[species]\nid = {id}\nname = \"{name}\"\nchain = [{id}]\n");
        for i in 0..forms {
            toml.push_str(&format!(
                "[[forms]]\n\
                 id = {}\n\
                 name = \"{name}{}\"\n\
                 stage = {}\n\
                 asset = \"Asset_{name}_{i}\"\n\
                 model = \"forms/Asset_{name}_{i}/model.glb\"\n\
                 scale = 1\n\
                 height_cm = 80.0\n\
                 locomotion = \"ground\"\n\
                 [forms.clips]\n\
                 Idle = {{ clip = \"Idle\", ms = 1000, frames = 30 }}\n",
                id + i as u32,
                i + 1,
                i + 1,
            ));
        }
        std::fs::write(dir.join("manifest.toml"), toml).expect("该能写 manifest");
        name.to_string()
    }

    /// 形态下拉必须**把每个形态都露出来**,哪怕这个槽位上一次开的是个形态更少的包。
    ///
    /// 曾经会偶现滚动条、还把最后一个形态挡在外面:popup 的 `Area` 把上一帧量到的尺寸
    /// 记在 `ctx.memory().areas()` 里(egui 0.35 `area.rs:466/666`),而
    /// `ComboBox::height(f32::INFINITY)` 让里面那个 `ScrollArea` 退回
    /// `available_rect_before_wrap()`(`scroll_area.rs:763`)—— 于是**可用高度 = 上一帧的
    /// 自己**,只会缩不会涨。同一个 `id_salt` 换成形态更多的包,就永远卡在旧高度上。
    #[test]
    fn the_form_dropdown_never_hides_a_form() {
        use super::super::{Page, SettingsApp};
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let dir = std::env::temp_dir().join(format!("rocom-combo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let packs = dir.join("packs");
        write_pack(&packs, "短链", 3001, 2);
        write_pack(&packs, "长链", 4001, 13);
        std::fs::write(dir.join("roster.toml"), "[[pet]]\npack = \"短链\"\n").expect("该能写阵容");

        let app = std::rc::Rc::new(std::cell::RefCell::new(SettingsApp::new(
            Some(dir.join("config.toml")),
            Some(packs.clone()),
            crate::control::SettingsPage::Pets,
        )));
        assert!(
            matches!(app.borrow().page, Page::Pet(0)),
            "前置条件:停在这一只的页上"
        );

        let driven = app.clone();
        // **要装真主题**:行高由 `theme::install` 的字号与 `button_padding` 决定,
        // 用 egui 默认样式量出来的行只有一半高,十三行轻松塞进去,这个测试就白做了。
        let mut harness = Harness::new_ui(move |ui| {
            theme::install(ui.ctx());
            driven.borrow_mut().pet_page(ui, 0);
        });
        harness.run();

        // ① 先开短链那个包的下拉,把这个 slot 的 popup 尺寸喂成「两行」
        harness.get_by_value("短链1(1 / 2)").click();
        harness.run();
        harness.run();
        harness.get_by_label("短链2(80cm)").click();
        harness.run();

        // ② 同一个 slot 换成九个形态的包,再开一次
        app.borrow_mut().roster[0].pack = "长链".into();
        app.borrow_mut().roster[0].form = None;
        harness.run();
        harness.get_by_value("长链1(1 / 13)").click();
        harness.run();
        harness.run();

        // popup 的高度要装得下九行。**不能拿选项自己的 rect 判** —— 被滚动区裁掉的那几行
        // 照样按真实位置分配矩形,裁的是绘制不是布局。看 popup 那个 `Area` 自己的高度。
        // 行距从相邻两条自己量,别把间隔写死
        let first = harness.get_by_label("长链1(80cm)").rect();
        let second = harness.get_by_label("长链2(80cm)").rect();
        let pitch = second.min.y - first.min.y;
        let need = pitch * 12.0 + first.height();
        let popup = popup_height(&harness.ctx).expect("下拉该是开着的");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            popup >= need,
            "十三个形态要一次全露出来:popup 高 {popup},装下要 {need}(行距 {pitch}),\
             差 {:.1} 行",
            (need - popup) / pitch
        );
    }
}
