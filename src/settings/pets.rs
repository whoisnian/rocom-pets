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
    /// 这一页装不下就滚(见 `theme::scroll_page`):表单本身十来行,窗口还能拉到 480 高。
    pub(super) fn pet_page(&mut self, ui: &mut egui::Ui, slot: usize) {
        theme::scroll_page(ui, |ui| self.pet_page_inner(ui, slot));
    }

    fn pet_page_inner(&mut self, ui: &mut egui::Ui, slot: usize) {
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

                appearance_rows(ui, slot, &mut options, form, self.glassy_ready);

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

/// 外观那几行:异色一个开关,炫彩一排单选;选了常规炫彩再多两行挑配色与粒子。
///
/// **异色与炫彩是两件独立的事** —— 游戏里是两个位标志(`MDT_SHINING` / `MDT_GLASS`),
/// 既有异色炫彩,也有原色炫彩。所以这里是「一个开关 + 一排单选」,不是三选一。
/// 底下两条路也是分开的:异色换整套材质(挑另一张材质表),炫彩往挑中的那套上刷一层。
fn appearance_rows(
    ui: &mut egui::Ui,
    slot: usize,
    options: &mut PetOptions,
    form: &crate::pack::Form,
    glassy_ready: bool,
) {
    use crate::pet::glassy;
    use crate::pet::Glassy;

    // 异色要包里真有那套材质,**多数宠物没有**(游戏里也是,得美术另做一套)。
    // 没有就整行不出 —— 摆一个永远点不动的开关只是占地方。
    //
    // 光一个方框,不写字:左边那格已经写着「异色」,开关再写一遍「换成异色」是同一个词
    // 在一行里出现两次;那句「美术另做的一套材质」讲的是它**为什么时有时无**,
    // 而这一行只在有的时候才出现 —— 看得见它的人不需要这句话。
    if form.has_shiny() {
        label(ui, "异色:");
        ui.checkbox(&mut options.mutation.shiny, "");
        ui.end_row();
    }

    label(ui, "炫彩:");
    ui.horizontal(|ui| {
        // 六档一排铺开,不做下拉:总共就这些,挑的时候要横着比,而下拉一滚就比不成了。
        // 顺序照游戏自己的分法(`HIDDEN_GLASS_CONF.type`):常驻隐藏款、常规、三个赛季款。
        if ui
            .selectable_label(options.mutation.glassy.is_none(), "无炫彩")
            .clicked()
        {
            options.mutation.glassy = None;
        }
        ui.add_enabled_ui(glassy_ready, |ui| {
            for h in glassy::hidden().iter().filter(|h| !h.season) {
                hidden_button(ui, &mut options.mutation.glassy, h);
            }
            let is_common = matches!(options.mutation.glassy, Some(Glassy::Common { .. }));
            if ui.selectable_label(is_common, "常规炫彩").clicked() && !is_common {
                // 头一次点进来给个确定的起点(1 号配色 · 1 号粒子),而不是随机 ——
                // 随机的话「我刚才选的是哪个」就说不清了。
                options.mutation.glassy = Some(Glassy::Common {
                    color: 1,
                    particle: 1,
                });
            }
            for h in glassy::hidden().iter().filter(|h| h.season) {
                hidden_button(ui, &mut options.mutation.glassy, h);
            }
        });
        // 窗口宽度固定 900,六个按钮排完右边只剩一小截 —— 只有这一句短的放得下,
        // 赛季款那句解释挂在按钮的 tooltip 上(见 `hidden_button`),不跟它抢位置。
        if !glassy_ready {
            theme::hint(ui, "这个二进制没烘炫彩素材");
        }
    });
    ui.end_row();

    // 常规炫彩才要自己挑配色与粒子;隐藏/赛季款是配好的一整套,没得挑。
    let Some(Glassy::Common { color, particle }) = options.mutation.glassy else {
        return;
    };
    let mut picked_color = color;
    let mut picked_particle = particle;

    label(ui, "配色:");
    ui.add_enabled_ui(glassy_ready, |ui| {
        ui.vertical(|ui| {
            // 39 组一次全铺开,不做下拉 —— 和上面那排炫彩档位同一个道理:一列 39 行
            // 装不下要滚,滚起来就横不成排,而挑配色恰恰要横着比。
            //
            // **格子里画色块、不写名字**:名字(「亮X暗 - 浅蓝红」)说的就是那两个色块,
            // 写出来等于把颜色翻译成字、再让人翻译回颜色。名字挂 tooltip,选中那组
            // 写在底下一行 —— 色块认得出颜色,认不出「这是第几组」。
            //
            // 间距比默认的窄:这 39 格是**一片**要横着比的东西,拉开了就变成一个一个
            // 孤立的按钮了。**最小格也必须显式给**:`Grid` 默认拿
            // `spacing.interact_size`(40 × 28)当下限,比色块格子(28 × 22)还宽,
            // 不给的话每列白撑到 40、15 列就是 642px,最右那列直接被窗口切掉。
            egui::Grid::new(("glass-color", slot))
                .num_columns(COLOR_COLUMNS)
                .spacing([3.0, 3.0])
                .min_col_width(CHIP.x)
                .min_row_height(CHIP.y)
                .show(ui, |ui| {
                    for (index, c) in glassy::colors().iter().enumerate() {
                        if swatch_chip(ui, c, picked_color == c.id).clicked() {
                            picked_color = c.id;
                        }
                        if index % COLOR_COLUMNS == COLOR_COLUMNS - 1 {
                            ui.end_row();
                        }
                    }
                });
            theme::hint(ui, glassy::color(picked_color).map_or("?", |c| c.name));
        });
    });
    ui.end_row();

    // 粒子四选一,同样铺开。**顺带给它一行标签**:先前两个下拉挤在「配色」一行里,
    // 右边那个连个名字都没有,得点开才知道它管的是什么。
    label(ui, "粒子:");
    ui.horizontal(|ui| {
        ui.add_enabled_ui(glassy_ready, |ui| {
            for p in glassy::particles() {
                if ui
                    .selectable_label(picked_particle == p.id, p.name)
                    .clicked()
                {
                    picked_particle = p.id;
                }
            }
        });
    });
    ui.end_row();

    if picked_color != color || picked_particle != particle {
        options.mutation.glassy = Some(Glassy::Common {
            color: picked_color,
            particle: picked_particle,
        });
    }
}

/// 配色一行铺几格。**15 不是凑的**:前 15 组是「亮X亮」(六个颜色两两组合,
/// C(6,2) = 15),正好占满第一行,余下 24 组「亮X暗」自己占两行 ——
/// 族的边界落在行的边界上,不用画分隔线也看得出是两拨。
const COLOR_COLUMNS: usize = 15;

/// 隐藏款那几个按钮。常驻那一款(黑白)后面缀个「隐藏」—— 光写「黑白」会被当成一组配色名
/// (常规炫彩那 39 组就叫「亮X暗 - 浅蓝蓝」这种),缀上才看得出它是另一档。
/// 赛季款自带专名(暗夜拾光/狂欢怪谈/铅字幻梦),不用缀。
fn hidden_button(
    ui: &mut egui::Ui,
    glassy: &mut Option<crate::pet::Glassy>,
    h: &crate::pet::glassy::HiddenGlass,
) {
    let want = crate::pet::Glassy::Hidden { id: h.id };
    let text = if h.season {
        h.name.to_string()
    } else {
        format!("{}隐藏", h.name)
    };
    let mut button = ui.selectable_label(*glassy == Some(want), text);
    if h.season {
        // 赛季款只有 `season_pet` 里那几只有专属贴图,别的宠物走与常驻款相同的通用覆盖。
        button = button.on_hover_text("赛季款:专属贴图只给游戏指定的那几只,别的宠物走通用外观");
    }
    if button.clicked() {
        *glassy = Some(want);
    }
}

/// 一组配色的格子:两个小方块就是这一组的两个颜色,选中/指着的那格垫个底。
///
/// **画 `ui_color_*` 而不是 `red_channel`/`green_channel`** —— 后者是线性空间里的
/// HDR 系数(取到 1.6),直接当颜色画会一片过曝;`ui_color_*` 是游戏图鉴上那两块。
///
/// 只在选中/指着的时候垫底,与 `selectable_label` 一致 —— 39 个格子要是各带一个底板,
/// 满屏都是框,反倒盖过了格子里那点颜色。
fn swatch_chip(
    ui: &mut egui::Ui,
    c: &crate::pet::glassy::GlassyColor,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(CHIP, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            ui.is_enabled(),
            selected,
            c.name,
        )
    });
    if selected || response.hovered() || response.has_focus() {
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter().rect(
            rect,
            4.0,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    let mut at = rect.center() - egui::vec2(SWATCH.x + 1.0, SWATCH.y * 0.5);
    for rgb in [c.ui_color_1, c.ui_color_2] {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(at, SWATCH),
            2.0,
            egui::Color32::from_rgb(
                (rgb >> 16) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            ),
        );
        at.x += SWATCH.x + 2.0;
    }
    response.on_hover_text(c.name)
}

/// 一格配色的大小,以及格子里那两个色块的大小。
/// 15 格一行:`15 × 28 + 14 × 3 = 462`,窗口按 900 宽算,标签列之后放得下。
const CHIP: egui::Vec2 = egui::vec2(28.0, 22.0);
const SWATCH: egui::Vec2 = egui::vec2(10.0, 12.0);

fn label(ui: &mut egui::Ui, text: &str) {
    // 表单标签右对齐 —— 设计稿 KDE 栏的规格
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

/// 下拉里那一行:性格 + 它带来的那双眼睛。
///
/// 眼神跟着性格走、单独改不了,所以**不给它一行**(只读的一行反而像是能点),
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
            // 两个轴一起带:异色炫彩要能原样存回来,不能在存档里退成其中一个
            mutation: crate::pet::Mutation {
                shiny: true,
                glassy: Some(crate::pet::Glassy::Common {
                    color: 33,
                    particle: 3,
                }),
            },
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

    /// 这一页装不下就得**能滚**,而不是把底下几行裁掉。
    ///
    /// 曾经就是裁掉的:`CentralPanel` 里一个滚动区都没有(宠物包那页自带一个,
    /// 另外两页没有),窗口按默认的 620 高开着、这一只又带炫彩配色,
    /// 底下的「动作 / 位置」两行就够不着了 —— **看不见也点不着**,还没有滚动条提示。
    #[test]
    fn the_pet_page_scrolls_when_it_does_not_fit() {
        use super::super::SettingsApp;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let dir = std::env::temp_dir().join(format!("rocom-scroll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let packs = dir.join("packs");
        write_pack(&packs, "喵喵", 3001, 2);
        std::fs::write(
            dir.join("roster.toml"),
            "[[pet]]\npack = \"喵喵\"\nmutation = \"炫彩:1/1\"\n",
        )
        .expect("该能写阵容");

        let app = std::rc::Rc::new(std::cell::RefCell::new(SettingsApp::new(
            Some(dir.join("config.toml")),
            Some(packs),
            crate::control::SettingsPage::Pets,
        )));
        let driven = app.clone();
        let page = egui::vec2(theme::WINDOW[0] - theme::SIDEBAR_W, theme::WINDOW[1]);
        let mut harness = Harness::builder().with_size(page).build_ui(move |ui| {
            theme::install(ui.ctx());
            driven.borrow_mut().pet_page(ui, 0);
        });
        harness.run();

        // 「位置」是表单最后一行。前置条件:它本来就在页面下边界之外
        let before = harness.get_by_label("记住上次落脚点").rect();
        assert!(
            before.max.y > page.y,
            "前置条件:这一页按 {} 高本来就装不下(最后一行在 {})",
            page.y,
            before.max.y
        );

        // 把指针放进页面里再滚 —— 滚轮只作用在指着的那个滚动区上
        harness.event(egui::Event::PointerMoved(egui::pos2(
            page.x * 0.5,
            page.y * 0.5,
        )));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -600.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();

        let after = harness.get_by_label("记住上次落脚点").rect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            after.max.y <= page.y,
            "滚到底之后最后一行要在页面里:它在 {},页面下边界在 {}",
            after.max.y,
            page.y
        );
    }

    /// 39 组配色要**一行 15 格、三行铺完,且不越过这一页的右边**。
    ///
    /// 曾经越界:`egui::Grid` 的最小格子是 `spacing.interact_size`(40 × 28),比色块
    /// 格子(28 × 22)还宽 —— 不显式给 `min_col_width`,每列白撑到 40,15 列就要 642px,
    /// 最右那列直接被窗口切掉,而**被切掉的格子照样有 rect**(裁的是绘制不是布局),
    /// 所以得拿它和页面右边比,不能光看它自己。
    #[test]
    fn the_colour_swatches_fit_one_page_wide() {
        use super::super::SettingsApp;
        use crate::pet::glassy;
        use egui_kittest::Harness;
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;

        let dir = std::env::temp_dir().join(format!("rocom-swatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let packs = dir.join("packs");
        write_pack(&packs, "喵喵", 3001, 1);
        // 常规炫彩才有「配色」这一行;隐藏/赛季款是配好的一整套,没得挑
        std::fs::write(
            dir.join("roster.toml"),
            "[[pet]]\npack = \"喵喵\"\nmutation = \"炫彩:1/1\"\n",
        )
        .expect("该能写阵容");

        let app = std::rc::Rc::new(std::cell::RefCell::new(SettingsApp::new(
            Some(dir.join("config.toml")),
            Some(packs),
            crate::control::SettingsPage::Pets,
        )));
        let driven = app.clone();
        // 详情页占的是**窗口减去侧栏**那一块,量宽度就得按这个来;
        // 字号与间距也要真主题,默认样式量出来的格子小一圈,这个测试就白做了
        let page = egui::vec2(theme::WINDOW[0] - theme::SIDEBAR_W, theme::WINDOW[1]);
        let mut harness = Harness::builder().with_size(page).build_ui(move |ui| {
            theme::install(ui.ctx());
            driven.borrow_mut().pet_page(ui, 0);
        });
        harness.run();

        // **要按 role 找**:选中那组的名字还写在格子底下一行,光按名字会撞上那句
        let rects: Vec<egui::Rect> = glassy::colors()
            .iter()
            .map(|c| {
                harness
                    .get_by_role_and_label(Role::RadioButton, c.name)
                    .rect()
            })
            .collect();
        // 详情页就画在这块屏幕上,左边从 0 起,所以页面右边就是它的宽
        let right = page.x;
        let over = rects.iter().map(|r| r.max.x).fold(f32::MIN, f32::max);
        let rows = rects.iter().filter(|r| r.min.y == rects[0].min.y).count();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            over <= right,
            "39 组配色要在这一页里排得下:最右一格到 {over},页面右边在 {right},超了 {:.1}px",
            over - right
        );
        assert_eq!(
            rows, COLOR_COLUMNS,
            "第一行要正好铺满 15 格 ——「亮X亮」那一族(六色两两组合)占满它"
        );
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
