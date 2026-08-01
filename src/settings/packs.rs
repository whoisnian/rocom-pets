//! 「宠物包」那一页:列表 + 查找 + 导入 + 删除。
//!
//! 包是**本地生成物**(用导出器从自己的游戏安装里导,见 README),这一页只管
//! 「把生成好的包搬进包目录 / 从包目录里去掉」,不碰导出那一步。
//!
//! **从网络下载还没做**。要加的话接在 [`SettingsApp::import`] 前面:下到临时文件,
//! 再交给同一条导入路径 —— 那条路径已经在管「校验能不能读」「重名怎么办」了。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use eframe::egui;

use super::{SettingsApp, Tab};
use crate::assets;

impl SettingsApp {
    pub(super) fn packs_tab(&mut self, ui: &mut egui::Ui) {
        let Some(packs_dir) = self.packs_dir.clone() else {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "定不出包目录,没法管理宠物包。用 --packs-dir 指定一个。",
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label("包目录");
            // **按钮排在路径前面**:路径可能比窗口还长,排在后面的东西会被挤出窗口
            // (第一版就是这样,那个按钮在实机上根本看不见)
            if ui.button("打开目录").clicked() {
                open_in_file_manager(&packs_dir);
            }
            let text = packs_dir.display().to_string();
            ui.add(egui::Label::new(egui::RichText::new(&text).monospace()).truncate())
                .on_hover_text(&text);
        });
        ui.horizontal(|ui| {
            if ui.button("导入 .rkpet…").clicked() {
                let picked = rfd::FileDialog::new()
                    .add_filter("宠物包", &[assets::PACK_EXT])
                    .set_title("选要导入的宠物包")
                    .pick_files();
                if let Some(files) = picked {
                    self.import(&files);
                }
            }
            if ui.button("导入包目录…").clicked()
                && let Some(dir) = rfd::FileDialog::new()
                    .set_title("选要导入的宠物包目录(里面有 manifest.toml)")
                    .pick_folder()
            {
                self.import(&[dir]);
            }
            if ui.button("重新扫描").clicked() {
                self.rescan_packs();
                self.status.ok(format!("包目录里有 {} 个包", self.entries.len()));
            }
            ui.label("· 也可以把包直接拖进这个窗口");
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("查找");
            ui.text_edit_singleline(&mut self.filter);
            if !self.filter.is_empty() && ui.button("清空").clicked() {
                self.filter.clear();
            }
        });

        let matches = self.filtered();
        ui.small(if self.filter.is_empty() {
            format!("共 {} 个包", self.entries.len())
        } else {
            format!("{} / {} 个包", matches.len(), self.entries.len())
        });
        ui.separator();

        // 上面是列表,下面是选中那个的详情。详情要占固定高度,否则选中/取消时
        // 列表会跳一下
        let detail_height = 132.0;
        let list_height = (ui.available_height() - detail_height).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if matches.is_empty() {
                    ui.add_space(8.0);
                    ui.label(if self.entries.is_empty() {
                        "包目录是空的。用导出器生成宠物包,或把 .rkpet 拖进来。"
                    } else {
                        "没有匹配的包。"
                    });
                    return;
                }
                for path in matches {
                    let name = self
                        .entries
                        .iter()
                        .find(|e| e.path == path)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    let archived = path.is_file();
                    let selected = self.selected_pack.as_deref() == Some(path.as_path());
                    let label = if archived {
                        format!("{name}    [rkpet]")
                    } else {
                        name.clone()
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        self.selected_pack = Some(path.clone());
                    }
                }
            });

        ui.separator();
        self.pack_detail(ui);
    }

    /// 选中那个包的详情:形态表、体积,以及「加到阵容 / 删除」。
    fn pack_detail(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.selected_pack.clone() else {
            ui.add_space(8.0);
            ui.label("选一个包看详情。");
            return;
        };
        let Some(pack) = self.pack_at(&path) else {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("{} 读不了(manifest 坏了或者不是宠物包)", path.display()),
            );
            if ui.button("删除…").clicked() {
                self.confirm_delete = Some(path);
            }
            return;
        };

        ui.horizontal(|ui| {
            ui.heading(&pack.species_name);
            ui.label(format!("id {}", pack.species_id));
            ui.label(format!("{} 个形态", pack.forms.len()));
            ui.label(format!(
                "{:.1}MB",
                assets::size(&path) as f64 / 1024.0 / 1024.0
            ));
        });
        ui.small(
            pack.forms
                .iter()
                .map(|f| format!("{}({:.0}cm)", f.name, f.height_cm))
                .collect::<Vec<_>>()
                .join(" → "),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("加到阵容").clicked() {
                self.add_to_roster(&path);
            }
            if ui.button("删除…").clicked() {
                self.confirm_delete = Some(path.clone());
            }
        });
    }

    /// 按查找框过滤出的包路径。**大小写与全半角不管**,中文名本来也没有大小写;
    /// 拿 `contains` 就够 —— 包名都很短。
    fn filtered(&self) -> Vec<PathBuf> {
        let needle = self.filter.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                needle.is_empty()
                    || entry.name.to_lowercase().contains(&needle)
                    || entry
                        .path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle))
            })
            .map(|entry| entry.path.clone())
            .collect()
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
        let mut ok = 0usize;
        for source in paths {
            match import_one(source, &packs_dir) {
                Ok(name) => {
                    ok += 1;
                    self.status.ok(format!("已导入 {name}"));
                }
                Err(e) => self.status.fail(format!("{} 导入失败:{e:#}", source.display())),
            }
        }
        if ok > 0 {
            self.rescan_packs();
            self.tab = Tab::Packs;
            if ok > 1 {
                self.status.ok(format!("导入了 {ok} 个包"));
            }
        }
    }

    /// 删除确认框。**删的是磁盘上的东西**,必须问一句,而且要把路径显示出来。
    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.confirm_delete.clone() else {
            return;
        };
        // 用 Modal 而不是普通窗口:删除是不可逆的,这时候不该还能去点别处
        let response = egui::Modal::new(egui::Id::new("confirm-delete")).show(ctx, |ui| {
            ui.heading("删除宠物包");
            ui.label("要从磁盘上删掉这个包吗?删了就得重新导出或重新导入。");
            ui.monospace(path.display().to_string());
            let in_roster = self.slots_using(&path);
            if !in_roster.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("阵容里有 {} 只在用它,会一并撤下。", in_roster.len()),
                );
            }
            ui.add_space(8.0);
            let mut done = false;
            ui.horizontal(|ui| {
                if ui.button("删除").clicked() {
                    self.delete_pack(&path);
                    done = true;
                }
                if ui.button("取消").clicked() {
                    done = true;
                }
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
            self.dirty = true;
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
                format!(",并从阵容里撤下 {} 只", doomed.len())
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
    let file_name = source
        .file_name()
        .context("路径没有文件名")?
        .to_os_string();
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

/// 用系统的文件管理器打开包目录。找不到就算了(只是个方便按钮)。
fn open_in_file_manager(dir: &Path) {
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("explorer", vec![dir.as_os_str()]);
    #[cfg(not(target_os = "windows"))]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("xdg-open", vec![dir.as_os_str()]);
    if let Err(e) = std::process::Command::new(program).args(args).spawn() {
        log::warn!("打不开文件管理器({e});包目录是 {}", dir.display());
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
        assert!(packs.join("喵喵/forms/a/model.glb").is_file(), "子目录没拷过去");
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
