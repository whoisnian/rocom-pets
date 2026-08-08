//! 起不来时的兜底报错窗口。
//!
//! 桌宠没有主窗口,release 版在 Windows 上还按 GUI 子系统链接 —— 出错时**用户看不到任何
//! 东西**,进程一闪就没了。更糟的是坏状态往往存在盘上(阵容存档里写着一只会让 GPU 崩的
//! 宠物),于是**重启还是同一条崩**,人就此进不去了。这个窗口就是给那种时候留的出口:
//! 把错误原文摆出来、能一键复制,再给一个「重置配置」把两份存档挪走。
//!
//! 只在**本来就要开窗口**的两条路上弹(桌宠 / 配置窗口)。`--list`、`--reload` 那些
//! 命令行子命令照旧只往 stderr 写一行 —— 脚本里跑的东西不该弹窗。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::settings::{fonts, theme};

/// 备份目录名(建在配置文件旁边)。
const BACKUP_DIR: &str = "backup";

/// 弹一个报错窗口。**这是最后一道**:它自己再失败就只能往 stderr 写了。
pub fn report(title: &str, detail: &str, config_path: Option<&Path>) {
    let app = Fatal {
        title: title.to_string(),
        detail: detail.to_string(),
        config_path: config_path.map(Path::to_path_buf),
        status: None,
        reset_done: false,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([620.0, 420.0])
            .with_min_inner_size([420.0, 280.0])
            .with_title("rocom-pets 出错了"),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "rocom-pets 出错了",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            theme::install(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    ) {
        // 连报错窗口都起不来(没有显示服务器?):至少别把原始错误吞掉
        eprintln!("报错窗口也起不来({e});原始错误:\n{title}\n{detail}");
    }
}

struct Fatal {
    title: String,
    detail: String,
    config_path: Option<PathBuf>,
    status: Option<String>,
    /// 重置过一次就把按钮禁掉:第二次点只会把刚建的空配置也挪走,徒增困惑。
    reset_done: bool,
}

impl eframe::App for Fatal {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 自己留一圈内边距:eframe 交进来的 `Ui` 是贴着窗口边的,不留的话标题与按钮
        // 几乎顶在边框上
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| self.body(&ctx, ui));
    }
}

impl Fatal {
    fn body(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.heading("rocom-pets 起不来");
        ui.add_space(4.0);
        theme::hint(ui, &self.title);
        ui.add_space(10.0);

        // 错误原文:可选中、可滚动。**等宽字体** —— 里面多半是路径与 wgpu 的报错。
        // 描个边:不然一大片同色区域看不出「这块是报错内容」,和窗口背景糊成一片
        let visuals = ui.visuals().clone();
        let height = (ui.available_height() - 78.0).max(80.0);
        egui::Frame::new()
            .fill(visuals.extreme_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.detail.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    });
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("复制报错信息").clicked() {
                ctx.copy_text(format!("{}\n\n{}", self.title, self.detail));
                self.status = Some("已复制到剪贴板".to_string());
            }
            let can_reset = self.config_path.is_some() && !self.reset_done;
            if ui
                .add_enabled(can_reset, egui::Button::new("重置配置…"))
                .on_hover_text(
                    "把配置与阵容存档挪进旁边的 backup 目录(不删),下次启动重新生成默认配置",
                )
                .clicked()
                && let Some(path) = self.config_path.clone()
            {
                match reset_config(&path) {
                    Ok(moved) if moved.is_empty() => {
                        self.status = Some("没有可重置的配置文件".to_string());
                        self.reset_done = true;
                    }
                    Ok(moved) => {
                        self.status =
                            Some(format!("已挪走 {},重新启动试试", moved.join("、")));
                        self.reset_done = true;
                    }
                    Err(e) => self.status = Some(format!("重置失败:{e:#}")),
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("关闭").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        if let Some(status) = &self.status {
            ui.add_space(4.0);
            theme::hint(ui, status);
        }
    }
}

/// 把配置与阵容存档挪进 `<配置目录>/backup/<时间戳>/`,返回挪走了哪几个文件名。
///
/// **挪而不是删**:里面可能有手写的注释与调过很久的参数,删了就找不回来了。
/// 时间戳分子目录,连着重置几次也不会互相覆盖。
fn reset_config(config_path: &Path) -> anyhow::Result<Vec<String>> {
    use anyhow::Context;

    let dir = config_path
        .parent()
        .context("配置文件没有所在目录,重置不了")?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = dir.join(BACKUP_DIR).join(stamp.to_string());
    std::fs::create_dir_all(&backup).with_context(|| format!("建不了 {backup:?}"))?;

    // 阵容存档是**这次崩溃最常见的元凶**(里面记着一只会让 GPU 崩的宠物),
    // 所以它和 config.toml 一起挪 —— 只挪一个的话重启照崩
    let roster = crate::roster::Roster::path_beside(config_path);
    let mut moved = Vec::new();
    for path in [config_path.to_path_buf(), roster] {
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name() {
            Some(name) => name.to_owned(),
            None => continue,
        };
        std::fs::rename(&path, backup.join(&name))
            .with_context(|| format!("挪不动 {path:?}"))?;
        moved.push(name.to_string_lossy().into_owned());
    }
    if moved.is_empty() {
        // 空目录留着只会让 backup/ 越来越乱
        let _ = std::fs::remove_dir(&backup);
    } else {
        log::info!("配置已重置,原文件在 {backup:?}");
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rocom-fatal-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("该能建目录");
        dir
    }

    #[test]
    fn reset_moves_both_files_and_keeps_them() {
        let dir = scratch("both");
        let config = dir.join("config.toml");
        std::fs::write(&config, "px_per_cm = 8.0").expect("该能写");
        let roster = crate::roster::Roster::path_beside(&config);
        std::fs::write(&roster, "[[pet]]\npack = \"坏包\"").expect("该能写");

        let moved = reset_config(&config).expect("该能重置");
        assert_eq!(moved.len(), 2, "配置与阵容存档都要挪走 —— 只挪一个的话重启照崩");
        assert!(!config.exists() && !roster.exists(), "原位置该空了");
        // **挪而不是删**:备份里两份都还在
        let backups: Vec<_> = walk(&dir.join(BACKUP_DIR));
        assert_eq!(backups.len(), 2, "备份里该有两份,实得 {backups:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resetting_with_nothing_to_move_leaves_no_empty_backup() {
        let dir = scratch("empty");
        let config = dir.join("config.toml");
        let moved = reset_config(&config).expect("没有文件也不该报错");
        assert!(moved.is_empty());
        assert!(
            !dir.join(BACKUP_DIR).join("").exists() || walk(&dir.join(BACKUP_DIR)).is_empty(),
            "不该留下空的备份目录"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                out.extend(walk(&entry.path()));
            } else {
                out.push(entry.path());
            }
        }
        out
    }
}
