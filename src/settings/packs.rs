//! 「宠物包」那一页:表格 + 搜索 + 导入 + 上桌 + 删除。
//!
//! 包是**本地生成物**(用导出器从自己的游戏安装里导,见 README),这一页只管
//! 「把生成好的包搬进包目录 / 从包目录里去掉」,不碰导出那一步 —— 空状态就是来说这件事的。
//!
//! **从网络下载还没做**。要加的话接在 [`SettingsApp::import`] 前面:下到临时文件,
//! 再交给同一条导入路径 —— 那条路径已经在管「校验能不能读」「重名怎么办」了。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use eframe::egui;

use super::{Page, SettingsApp, theme};
use crate::assets;

/// 表格四列的宽度:名称吃剩下的,后三列固定。
const COL_FORMS: f32 = 74.0;
const COL_SIZE: f32 = 92.0;
const COL_SOURCE: f32 = 78.0;
/// 搜索框右边那两个导入按钮占的宽度(含它们之间的间距)。
const IMPORT_BUTTONS_W: f32 = 190.0;

/// 统计行 + 那三个按钮要占的高度。
///
/// 「一行按钮」比看上去高:控件 28 + 上下内边距 + 表格与它之间那 10px 的空。
/// 给少了按钮会被窗口下沿切掉一半(实测 44 就不够)。
const FOOTER_H: f32 = 58.0;

/// 四列的 x 坐标。**表头与每一行必须用同一份** —— 分开算的话表头会飘到别处
/// (第一版就是:表头四个字挤在左边,数据在右边)。
struct Columns {
    /// 名称列的左边(也是整行的左边)。
    left: f32,
    /// 形态数那一列的左边。
    forms: f32,
    /// 体积那一列的**右**边(数值右对齐)。
    size: f32,
    /// 来源那一列的右边。
    right: f32,
}

impl Columns {
    fn new(rect: egui::Rect) -> Self {
        let right = rect.right();
        let size = right - COL_SOURCE;
        let forms = size - COL_SIZE;
        Self {
            left: rect.left(),
            forms: forms - COL_FORMS,
            size,
            right,
        }
    }

    /// 名称列能占多宽(超了要截断,链名可以很长)。
    fn name_width(&self) -> f32 {
        (self.forms - self.left - 8.0).max(80.0)
    }
}

impl SettingsApp {
    pub(super) fn packs_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("宠物包");
        let Some(packs_dir) = self.packs_dir.clone() else {
            ui.add_space(12.0);
            ui.colored_label(
                ui.visuals().error_fg_color,
                "定不出包目录,没法管理宠物包。用 --packs-dir 指定一个。",
            );
            return;
        };

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            // **要显式居中**:`add_sized` 把框撑到 28 高,而 TextEdit 默认把字贴着顶边放,
            // 于是光标与提示文字浮在框的上半截
            let width = ui.available_width() - IMPORT_BUTTONS_W;
            ui.add_sized(
                [width.max(120.0), theme::CONTROL_H],
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("搜索包名或形态…")
                    .vertical_align(egui::Align::Center),
            );
            // **两个按钮而不是一个**:原生文件对话框没有「文件和目录都行」这个模式,
            // 一个按钮就只能先弹一个、取消了再弹另一个 —— 取消了还弹是很怪的行为
            if ui.button("导入包…").clicked() {
                self.import_files();
            }
            if ui.button("导入目录…").clicked() {
                self.import_folder();
            }
        });

        if self.entries.is_empty() {
            self.empty_state(ui, &packs_dir);
            return;
        }

        ui.add_space(10.0);
        let matches = self.filtered();
        // **先给统计行留出高度**:不留的话表格会把剩余空间全吃掉,
        // 统计行与那三个按钮被挤到窗口外面去(第一版就是这样)
        let table_h = (ui.available_height() - FOOTER_H).max(120.0);
        self.table(ui, &matches, table_h);
        ui.add_space(10.0);
        self.footer(ui, &matches);
    }

    /// 一个包都没有:这一页要承担「为什么这里是空的」的解释责任。
    /// **不分发素材是这个项目的硬约束**,第一次打开就得说清。
    fn empty_state(&mut self, ui: &mut egui::Ui, packs_dir: &Path) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.label(egui::RichText::new("还没有宠物包").size(15.0).strong());
            ui.add_space(8.0);
            theme::hint(
                ui,
                "宠物包不随程序分发,需要用导出器从你自己的游戏安装里生成。",
            );
            theme::hint(ui, "已经有包的话,用下面的「导入包…」或「导入目录…」。");
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button("导入包…").clicked() {
                    self.import_files();
                }
                if ui.button("导入目录…").clicked() {
                    self.import_folder();
                }
            });
        });
        ui.add_space(24.0);
        // 命令抄下来就能用 —— 空状态最该给的就是「下一步敲什么」
        let visuals = ui.visuals().clone();
        egui::Frame::new()
            .fill(visuals.faint_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.label(theme::value(format!(
                    "dotnet run --project exporter -- --species 3001 --out {}",
                    packs_dir.display()
                )));
            });
    }

    /// 表格:名称(整条进化链)/ 形态数 / 体积 / 来源。
    fn table(&mut self, ui: &mut egui::Ui, matches: &[PathBuf], height: f32) {
        let visuals = ui.visuals().clone();
        egui::Frame::new()
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(14, 6))
            .show(ui, |ui| {
                // 滚动条:不浮在内容上(浮动条会盖住最右边那列「来源」),配色见 theme.rs。
                //
                // 连带**必须让它一直显示**:表头画在滚动区外面,而占位式滚动条只在
                // 需要滚的时候占那点宽度 —— 忽有忽无的话表头与数据行的四列就对不齐了,
                // 而「表头和行必须用同一份列坐标」正是 `Columns` 存在的理由。
                theme::scrollbar(ui);
                let bar = ui.spacing().scroll.allocated_width();

                // 表头:占一行的高度,四列按 `Columns` 摆
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width() - bar, 22.0),
                    egui::Sense::hover(),
                );
                let cols = Columns::new(rect);
                let head = egui::FontId::proportional(theme::GROUP);
                let weak = visuals.weak_text_color();
                let painter = ui.painter();
                let y = rect.center().y;
                painter.text(
                    egui::pos2(cols.left, y),
                    egui::Align2::LEFT_CENTER,
                    "名称",
                    head.clone(),
                    weak,
                );
                painter.text(
                    egui::pos2(cols.forms, y),
                    egui::Align2::LEFT_CENTER,
                    "形态",
                    head.clone(),
                    weak,
                );
                painter.text(
                    egui::pos2(cols.size, y),
                    egui::Align2::RIGHT_CENTER,
                    "体积",
                    head.clone(),
                    weak,
                );
                painter.text(
                    egui::pos2(cols.right, y),
                    egui::Align2::RIGHT_CENTER,
                    "来源",
                    head,
                    weak,
                );
                ui.separator();

                // **必须给滚动区一个上限**:`auto_shrink(false)` 会让它吃掉所有剩余高度,
                // 于是外面的统计行与那三个按钮被挤出窗口(实测过两次)。
                // 减掉的是表头 + 分隔线那一截。
                egui::ScrollArea::vertical()
                    .max_height((height - 34.0).max(60.0))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if matches.is_empty() {
                            ui.add_space(10.0);
                            ui.label("没有匹配的包。");
                            return;
                        }
                        for (index, path) in matches.iter().enumerate() {
                            self.row(ui, path, index);
                        }
                    });
            });
    }

    fn row(&mut self, ui: &mut egui::Ui, path: &Path, index: usize) {
        let Some(entry) = self.entries.iter().find(|e| e.path == path) else {
            return;
        };
        let (chain, forms, size, archived) = (
            entry.chain(),
            entry.forms.len().max(1),
            entry.size,
            entry.archived(),
        );
        let selected = self.selected_pack.as_deref() == Some(path);
        let response = ui.add(
            egui::Button::selectable(selected, "")
                .min_size(egui::vec2(ui.available_width(), theme::ROW_H)),
        );
        let rect = response.rect;
        let cols = Columns::new(rect);
        let visuals = ui.visuals().clone();
        let painter = ui.painter();
        // 斑马纹:一屏十几行的纯文字表格,没有它眼睛跟不住行
        if !selected && index % 2 == 1 {
            painter.rect_filled(rect, 0.0, visuals.faint_bg_color);
        }
        let text_color = visuals.text_color();
        let weak = visuals.weak_text_color();
        let body = egui::FontId::proportional(theme::BODY);
        let mono = egui::FontId::monospace(theme::GROUP);
        let y = rect.center().y;
        // 链名可以很长(「治愈兔 → 红丝绒 → 红绒十字」),超了要截断而不是压到下一列上
        let galley = painter.layout(
            chain,
            body.clone(),
            text_color,
            cols.name_width(),
        );
        painter.galley(
            egui::pos2(cols.left, y - galley.size().y * 0.5),
            galley,
            text_color,
        );
        painter.text(
            egui::pos2(cols.forms, y),
            egui::Align2::LEFT_CENTER,
            forms.to_string(),
            body,
            weak,
        );
        painter.text(
            egui::pos2(cols.size, y),
            egui::Align2::RIGHT_CENTER,
            theme::megabytes(size),
            mono.clone(),
            weak,
        );
        // `.rkpet` 与「目录」用文字标记区分,对应 `--list` 里的 [rkpet]
        painter.text(
            egui::pos2(cols.right, y),
            egui::Align2::RIGHT_CENTER,
            if archived { "rkpet" } else { "目录" },
            mono,
            weak,
        );
        if response.clicked() {
            self.selected_pack = Some(path.to_path_buf());
            let summary = self.pack_summary(path);
            self.status.ok(summary);
        }
        if response.double_clicked() {
            self.add_to_roster(path);
        }
    }

    /// 统计行 + 三个按钮。
    ///
    /// **整行都交给「从右往左」的布局**:按钮先从右边缘往里排,统计文字拿剩下的。
    /// 反过来(先放文字、再嵌一个右对齐的小块)时,最右边那个按钮会被窗口边缘切掉一条边。
    fn footer(&mut self, ui: &mut egui::Ui, matches: &[PathBuf]) {
        let total: u64 = self.entries.iter().map(|e| e.size).sum();
        let forms: usize = self.entries.iter().map(|e| e.forms.len().max(1)).sum();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 最右边那个按钮的描边正好压在裁剪边界上,不让开一点就只画得出三条边
            ui.add_space(4.0);
            let picked = self.selected_pack.clone();
            let enabled = picked.is_some();
            if ui.add_enabled(enabled, egui::Button::new("删除…")).clicked() {
                self.confirm_delete = picked.clone();
            }
            if ui
                .add_enabled(enabled, egui::Button::new("在文件管理器中显示"))
                .clicked()
                && let Some(path) = picked.as_deref()
            {
                show_in_file_manager(path);
            }
            if ui.add_enabled(enabled, egui::Button::new("上桌")).clicked()
                && let Some(path) = picked.as_deref()
            {
                self.add_to_roster(path);
            }
            let shown = if matches.len() == self.entries.len() {
                String::new()
            } else {
                format!("(筛出 {} 个)", matches.len())
            };
            // 统计拿剩下的宽度,从左边起写
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                theme::hint(
                    ui,
                    format!(
                        "{} 个包 · 共 {} 个形态 · {}{shown}",
                        self.entries.len(),
                        forms,
                        theme::megabytes(total)
                    ),
                );
            });
        });
    }

    /// 状态栏那句「已选…」。顺手报一下有几个形态带叫声 —— 那是导包时最常缺的一块。
    fn pack_summary(&mut self, path: &Path) -> String {
        let chain = self
            .entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.chain())
            .unwrap_or_default();
        match self.pack_at(path) {
            Some(pack) => {
                let voiced = pack.forms.iter().filter(|f| f.voice.is_some()).count();
                format!("已选「{chain}」· 其中 {voiced} 个形态有叫声")
            }
            None => format!("「{chain}」读不了(manifest 坏了或者不是宠物包)"),
        }
    }

    /// 按查找框过滤。**名称与形态名都能搜**:想找魔力猫的人不一定记得链首叫喵喵。
    fn filtered(&self) -> Vec<PathBuf> {
        let needle = self.filter.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                needle.is_empty()
                    || entry.name.to_lowercase().contains(&needle)
                    || entry
                        .forms
                        .iter()
                        .any(|f| f.to_lowercase().contains(&needle))
                    || entry
                        .path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle))
            })
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// 选 `.rkpet` 文件导入(可以多选)。
    fn import_files(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("宠物包", &[assets::PACK_EXT])
            .set_title("选要导入的宠物包(.rkpet)")
            .pick_files()
            && !files.is_empty()
        {
            self.import(&files);
        }
    }

    /// 选一个解开的包目录导入。
    fn import_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("选一个包目录(里面有 manifest.toml)")
            .pick_folder()
        {
            self.import(&[dir]);
        }
    }

    /// 把这些路径拷进包目录。文件对话框与拖放共用一条路。
    ///
    /// **拷贝而不是移动**:源多半在下载目录或导出器的输出目录里,把人家的东西挪走
    /// 是很讨嫌的行为;包也就几 MB 到十几 MB。
    pub(super) fn import(&mut self, paths: &[PathBuf]) {
        let Some(packs_dir) = self.packs_dir.clone() else {
            self.status.fail("定不出包目录,导入不了");
            return;
        };
        let mut ok = Vec::new();
        for source in paths {
            match import_one(source, &packs_dir) {
                Ok(name) => ok.push(name),
                // **一条一条报**:一次拖进来多个时,成功的继续,失败的单独说
                Err(e) => self
                    .status
                    .fail(format!("{} 导入失败:{e:#}", source.display())),
            }
        }
        if ok.is_empty() {
            return;
        }
        self.rescan_packs();
        self.page = Page::Packs;
        self.status.ok(match ok.len() {
            1 => format!("已导入 {}", ok[0]),
            n => format!("已导入 {n} 个包:{}", ok.join("、")),
        });
    }

    /// 删除确认。**删的是磁盘上的东西**,必须问一句,而且要说清连带后果。
    /// 按钮写动作本身(「移入回收站」),不写「确定」。
    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.confirm_delete.clone() else {
            return;
        };
        let chain = self
            .entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.chain())
            .unwrap_or_else(|| path.display().to_string());
        let size = self
            .entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.size)
            .unwrap_or(0);
        let doomed = self.slots_using(&path).len();

        let response = egui::Modal::new(egui::Id::new("confirm-delete"))
            .frame(super::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading("删除宠物包");
                ui.add_space(10.0);
                ui.label(format!("要删除「{chain}」吗?"));
                ui.add_space(6.0);
                theme::hint(
                    ui,
                    format!(
                        "包文件会从磁盘上删掉({})。删了就得重新导出或重新导入。",
                        theme::megabytes(size)
                    ),
                );
                if doomed > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("这条链上有 {doomed} 只正在桌面上,会被一并撤下。"),
                    );
                }
                ui.add_space(10.0);
                ui.label(theme::value(path.display().to_string()));
                ui.add_space(14.0);
                let mut done = false;
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("删除").clicked() {
                            self.delete_pack(&path);
                            done = true;
                        }
                        if ui.button("取消").clicked() {
                            done = true;
                        }
                    });
                });
                done
            });
        // 点背景或按 Esc 也算取消
        if response.inner || response.should_close() {
            self.confirm_delete = None;
        }
    }

    /// 阵容里哪几只在用这个包(删包之前要提示,删完要一并撤下)。
    fn slots_using(&self, path: &Path) -> Vec<usize> {
        let names = pack_aliases(path);
        self.roster
            .iter()
            .enumerate()
            .filter(|(_, slot)| names.iter().any(|n| n == &slot.pack))
            .map(|(index, _)| index)
            .collect()
    }

    fn delete_pack(&mut self, path: &Path) {
        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = result {
            self.status.fail(format!("{} 删不掉:{e}", path.display()));
            return;
        }
        // 阵容里在用它的那几只跟着撤下,否则下次启动只会看到一行「上不了台」的警告
        let doomed = self.slots_using(path);
        if !doomed.is_empty() {
            for slot in doomed.iter().rev() {
                self.roster.remove(*slot);
            }
            if let Page::Pet(slot) = self.page
                && slot >= self.roster.len()
            {
                self.page = Page::Packs;
            }
            self.apply();
        }
        if self.selected_pack.as_deref() == Some(path) {
            self.selected_pack = None;
        }
        self.rescan_packs();
        self.status.ok(format!(
            "已删除 {}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            if doomed.is_empty() {
                String::new()
            } else {
                format!(",并撤下 {} 只", doomed.len())
            }
        ));
    }
}

/// 阵容存档里可能用来指这个包的几种写法(文件名 / 去后缀 / 绝对路径)。
fn pack_aliases(path: &Path) -> Vec<String> {
    let mut names = vec![path.to_string_lossy().into_owned()];
    if let Some(name) = path.file_name() {
        names.push(name.to_string_lossy().into_owned());
    }
    if let Some(stem) = path.file_stem() {
        names.push(stem.to_string_lossy().into_owned());
    }
    names
}

/// 导入一个包,返回它的名字。
fn import_one(source: &Path, packs_dir: &Path) -> Result<String> {
    // **先验能不能读**:拷完再发现不是宠物包的话,包目录里就多了一堆垃圾
    anyhow::ensure!(
        assets::is_pack(source),
        "不是宠物包({}里没有 {})",
        if source.is_dir() { "目录" } else { "文件" },
        assets::MANIFEST
    );
    let name = crate::pack::Pack::peek_name(source);
    let file_name = source.file_name().context("路径没有文件名")?.to_os_string();
    let target = packs_dir.join(&file_name);
    if target == source {
        anyhow::bail!("它已经在包目录里了");
    }
    anyhow::ensure!(
        !target.exists(),
        "包目录里已经有 {} 了,先删掉再导入",
        file_name.to_string_lossy()
    );
    std::fs::create_dir_all(packs_dir).with_context(|| format!("建不了 {packs_dir:?}"))?;
    if source.is_dir() {
        copy_dir(source, &target)?;
    } else {
        std::fs::copy(source, &target).with_context(|| format!("拷不过去 {target:?}"))?;
    }
    Ok(name)
}

/// 递归拷贝目录。std 没有现成的,而包目录就是一棵普通的文件树
/// (没有符号链接、没有特殊文件),照着走一遍即可。
fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target).with_context(|| format!("建不了 {target:?}"))?;
    for entry in std::fs::read_dir(source).with_context(|| format!("读不了 {source:?}"))? {
        let entry = entry?;
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).with_context(|| format!("拷不过去 {to:?}"))?;
        }
    }
    Ok(())
}

/// 在文件管理器里定位这个包。找不到就算了(只是个方便按钮)。
fn show_in_file_manager(path: &Path) {
    // 定位到**包所在的目录**:各家文件管理器「选中某一项」的参数五花八门,
    // 打开父目录是唯一到处都work 的做法
    let dir = path.parent().unwrap_or(path);
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("explorer", vec![dir.as_os_str()]);
    #[cfg(not(target_os = "windows"))]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("xdg-open", vec![dir.as_os_str()]);
    if let Err(e) = std::process::Command::new(program).args(args).spawn() {
        log::warn!("打不开文件管理器({e});包在 {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rocom-import-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("该能建目录");
        dir
    }

    fn make_pack_dir(at: &Path) {
        std::fs::create_dir_all(at.join("forms/a")).expect("该能建");
        std::fs::write(at.join(assets::MANIFEST), "schema = 1").expect("该能写");
        std::fs::write(at.join("forms/a/model.glb"), b"glb").expect("该能写");
    }

    #[test]
    fn importing_copies_the_whole_tree_and_leaves_the_source_alone() {
        let root = scratch("copy");
        let source = root.join("src/喵喵");
        make_pack_dir(&source);
        let packs = root.join("packs");

        import_one(&source, &packs).expect("该能导入");
        assert!(
            packs.join("喵喵/forms/a/model.glb").is_file(),
            "子目录没拷过去"
        );
        assert!(source.is_dir(), "**拷贝**而不是移动:源必须还在");

        // 重名要拦住,而不是悄悄覆盖别人的包
        let again = import_one(&source, &packs);
        assert!(again.is_err(), "重名该报错");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn importing_something_that_is_not_a_pack_is_refused_before_copying() {
        // 先验后拷:否则包目录里会多出一堆没法读的东西
        let root = scratch("bad");
        let junk = root.join("随便一个目录");
        std::fs::create_dir_all(&junk).expect("该能建");
        let packs = root.join("packs");
        assert!(import_one(&junk, &packs).is_err());
        assert!(!packs.join("随便一个目录").exists(), "验不过就不该拷");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn aliases_cover_how_a_roster_might_name_a_pack() {
        let names = pack_aliases(Path::new("/data/packs/喵喵.rkpet"));
        assert!(names.contains(&"喵喵.rkpet".to_string()));
        assert!(names.contains(&"喵喵".to_string()));
        assert!(names.contains(&"/data/packs/喵喵.rkpet".to_string()));
    }
}
