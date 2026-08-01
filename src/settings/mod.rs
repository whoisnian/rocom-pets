//! `rocom-pets --settings`:独立的配置窗口(宠物包 / 活跃宠物 / 常用配置)。
//!
//! ## 为什么是独立进程
//!
//! 桌宠那边两个后端各自跑着**手写的**事件循环(smithay 的 calloop / Win32 的消息循环),
//! 而 egui 要的是 winit 的事件循环。塞进同一个线程就是两套循环抢方向盘,
//! 分线程又要给整个 `App` 加锁。而这个窗口本来就是「偶尔打开、改完就关」的东西 ——
//! 单开一个进程最省事,也顺带保证配置窗口崩了不会带走桌宠。
//!
//! ## 两个进程怎么对上
//!
//! **只靠磁盘上那两份文件**:`config.toml`(手写的,写回时用 toml_edit 保注释)与
//! `roster.toml`(机器写的)。改完就存盘 + 发一条 `Reload`,在跑的实例重读这两份文件
//! 并把台上的一切对齐过去。没有第二套协议要维护;桌宠没在跑的时候,存下来的东西
//! 下次启动照样生效。
//!
//! ## 即时生效,而不是「保存」
//!
//! 桌宠是**看得见的**:把大小从 100% 拖到 124%,眼睛盯着屏幕就知道对不对。
//! 这种时候「先改再按保存」是多余的一步 —— 所以这里改什么都立刻落到桌面上,
//! 顶上那条只负责说「改了几项」并提供**撤销**。撤销的基线是打开窗口时那一份。
//!
//! 唯一的例外是滑杆:拖动过程里每帧都发 `Reload` 等于每帧重建一次宠物,
//! 所以拖的时候只改内存里的值(桌面上不动),**松手才落盘**。

mod common;
mod fonts;
mod packs;
mod pets;
mod theme;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use eframe::egui;

use crate::config::{Config, Setting};
use crate::control::SettingsPage;
use crate::pack::{Pack, PackEntry};
use crate::platform::PetOptions;
use crate::roster::{Roster, Slot};

/// 窗口现在停在哪一页。「活跃宠物」不是一页,而是**每只一页** ——
/// 侧栏把它们直接展开成子项,900px 宽里塞不下第三栏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Packs,
    Common,
    Pet(usize),
}

/// 底部状态栏那行字。做完一件事说一句,出错说得更显眼。
#[derive(Default)]
struct Status {
    text: String,
    error: bool,
}

impl Status {
    fn ok(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.error = false;
    }

    fn fail(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::warn!("{text}");
        self.text = text;
        self.error = true;
    }
}

/// 「桌宠退出了,你也关了吧」。
///
/// 托盘点「退出」时桌宠会喊一声(Linux 走 D-Bus、Windows 走具名事件,
/// 都是**单实例那套东西的另一半**:那边已经有一个「配置窗口在不在」的凭据了)。
/// 桌宠都没了,剩一个配置窗口对着不存在的宠物调大小,没有意义。
///
/// **不是直接关窗口**:喊话到的是别的线程,winit 的窗口只能在自己那条线程上关。
/// 所以这里只置个位再把界面叫醒,由下一帧 `ui()` 真正发关闭命令。
#[derive(Clone, Default)]
struct CloseRequest {
    asked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 界面起来之后才有。**没有它就只能等下一次鼠标动**,那可能是很久以后。
    ctx: std::sync::Arc<std::sync::Mutex<Option<egui::Context>>>,
}

impl CloseRequest {
    /// 别的线程调:请求关窗口。
    fn ask(&self) {
        self.asked.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Ok(ctx) = self.ctx.lock()
            && let Some(ctx) = ctx.as_ref()
        {
            ctx.request_repaint();
        }
    }

    fn attach(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx.lock() {
            *slot = Some(ctx.clone());
        }
    }

    /// 界面线程调:有人请求过吗(取走)。
    fn taken(&self) -> bool {
        self.asked.swap(false, std::sync::atomic::Ordering::Relaxed)
    }
}

/// 一条「改了什么」。顶上的条只显示条数,点「查看改动」才逐条列出来。
struct Change {
    what: String,
    from: String,
    to: String,
}

pub struct SettingsApp {
    config_path: Option<PathBuf>,
    roster_path: Option<PathBuf>,
    packs_dir: Option<PathBuf>,
    /// 编辑中的配置与阵容。**已经生效了**(除了正在拖的那根滑杆)。
    config: Config,
    roster: Vec<Slot>,
    /// 打开窗口时那一份。「撤销」回到这里,「已修改 N 项」也是和它比出来的。
    base_config: Config,
    base_roster: Vec<Slot>,
    /// 包目录扫描结果:名字、进化链、体积。**不含动作表与材质表**(见 `Pack::peek`)。
    entries: Vec<PackEntry>,
    /// 惰性读出来的包详情(要列形态或算动作覆盖率时才读)。
    /// `None` = 读过了但读不动,别每帧重试。
    loaded: HashMap<PathBuf, Option<Rc<Pack>>>,
    filter: String,
    selected_pack: Option<PathBuf>,
    /// 正等着确认删除的包。删包是不可逆的,必须问一句。
    confirm_delete: Option<PathBuf>,
    /// 「查看改动」那张弹窗开着没有。
    show_changes: bool,
    page: Page,
    status: Status,
    /// 桌宠退出时会通过它叫这个窗口一起关(见 [`CloseRequest`])。
    close: CloseRequest,
}

/// 起配置窗口。`config_path` / `packs_dir` 由 main 按与桌宠**完全一样**的规则定出来 ——
/// 两边看的必须是同一批文件。
pub fn run(
    config_path: Option<PathBuf>,
    packs_dir: Option<PathBuf>,
    page: SettingsPage,
) -> Result<()> {
    // 已经开着一个就别再开:两个窗口各改各的,后写的那个会把前一个的改动整份覆盖
    let close = CloseRequest::default();
    let _guard = match single_instance(close.clone()) {
        Some(guard) => guard,
        None => {
            log::info!("配置窗口已经开着一个了");
            return Ok(());
        }
    };

    let mut app = SettingsApp::new(config_path, packs_dir, page);
    app.close = close;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(theme::WINDOW)
            .with_min_inner_size([720.0, 480.0])
            .with_title("rocom-pets 配置"),
        ..Default::default()
    };
    eframe::run_native(
        "rocom-pets 配置",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            theme::install(&cc.egui_ctx);
            // 关窗口的请求是别的线程发来的,得有个 ctx 才能把界面叫醒
            app.close.attach(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("配置窗口起不来: {e}"))
}

impl SettingsApp {
    fn new(
        config_path: Option<PathBuf>,
        packs_dir: Option<PathBuf>,
        page: SettingsPage,
    ) -> Self {
        let mut app = Self {
            roster_path: config_path.as_deref().map(Roster::path_beside),
            config_path,
            packs_dir,
            config: Config::default(),
            roster: Vec::new(),
            base_config: Config::default(),
            base_roster: Vec::new(),
            entries: Vec::new(),
            loaded: HashMap::new(),
            filter: String::new(),
            selected_pack: None,
            confirm_delete: None,
            show_changes: false,
            page: Page::Packs,
            status: Status::default(),
            close: CloseRequest::default(),
        };
        app.reload_from_disk();
        app.status.ok(idle_status());
        app.page = match page {
            SettingsPage::Packs => Page::Packs,
            SettingsPage::Common => Page::Common,
            // 台上一只都没有时「宠物配置…」只能落到包页 —— 那儿才有东西可点
            SettingsPage::Pets if app.roster.is_empty() => Page::Packs,
            SettingsPage::Pets => Page::Pet(0),
        };
        app
    }

    /// 把两份文件重读一遍,并把它当成新的撤销基线。开窗口时走这里。
    fn reload_from_disk(&mut self) {
        if let Some(path) = self.config_path.as_deref() {
            match Config::load_or_create(path) {
                Ok(config) => self.config = config,
                Err(e) => self.status.fail(format!("配置读不了({e:#}),先用默认值")),
            }
        }
        self.roster = self
            .roster_path
            .as_deref()
            .and_then(Roster::load)
            .map(|saved| saved.pets)
            .unwrap_or_default();
        self.base_config = self.config.clone();
        self.base_roster = self.roster.clone();
        self.rescan_packs();
    }

    /// 重扫包目录。导入/删除之后要调,否则列表还是旧的。
    fn rescan_packs(&mut self) {
        self.entries = match self.packs_dir.as_deref() {
            Some(dir) => Pack::list_entries(dir),
            None => Vec::new(),
        };
        // 详情缓存按路径存,包被删掉之后那条就该作废
        let alive: Vec<PathBuf> = self.entries.iter().map(|e| e.path.clone()).collect();
        self.loaded.retain(|path, _| alive.contains(path));
    }

    /// 取包的详情(形态表、动作表)。第一次问才读盘,读不动就记下来别再试。
    fn pack_at(&mut self, path: &Path) -> Option<Rc<Pack>> {
        if let Some(cached) = self.loaded.get(path) {
            return cached.clone();
        }
        let loaded = match Pack::load(path) {
            Ok(pack) => Some(Rc::new(pack)),
            Err(e) => {
                log::warn!("{path:?} 读不了: {e:#}");
                None
            }
        };
        self.loaded.insert(path.to_path_buf(), loaded.clone());
        loaded
    }

    /// 阵容里某一只对应的包。存档里写的是名字,得先解析成路径。
    fn pack_for_slot(&mut self, slot: usize) -> Option<Rc<Pack>> {
        let path = self.path_for_slot(slot)?;
        self.pack_at(&path)
    }

    fn path_for_slot(&self, slot: usize) -> Option<PathBuf> {
        let name = &self.roster.get(slot)?.pack;
        let hit = self.entries.iter().find(|e| {
            &e.name == name
                || e.path.file_name().is_some_and(|n| n == name.as_str())
                || e.path.file_stem().is_some_and(|n| n == name.as_str())
        });
        // 阵容里也可能是个绝对路径(`--pack /some/where` 存下来的)
        Some(match hit {
            Some(entry) => entry.path.clone(),
            None => Config::expand_path(name),
        })
    }

    // ── 改动、落盘、撤销 ──────────────────────────────────────────

    /// 和打开窗口时那一份比,改了哪几项。顶上的条与「查看改动」共用。
    fn changes(&self) -> Vec<Change> {
        let mut out = Vec::new();
        let mut note = |what: &str, from: String, to: String| {
            if from != to {
                out.push(Change {
                    what: what.to_string(),
                    from,
                    to,
                });
            }
        };
        let scale = |c: &Config| theme::percent(c.px_per_cm / crate::control::PX_PER_CM_STANDARD);
        note(
            "目标帧率",
            format!("{} 帧/秒", self.base_config.fps),
            format!("{} 帧/秒", self.config.fps),
        );
        note("整体大小", scale(&self.base_config), scale(&self.config));
        note(
            "叫声音量",
            theme::percent(self.base_config.volume),
            theme::percent(self.config.volume),
        );
        note(
            "启动就穿透",
            yes_no(self.base_config.passthrough),
            yes_no(self.config.passthrough),
        );

        // 阵容按位置比。加/撤会让后面全体错位,那就干脆整体报一句 ——
        // 逐只对齐(最长公共子序列那一套)对一份几只的名单是杀鸡用牛刀
        if self.base_roster.len() != self.roster.len() {
            note(
                "在场宠物",
                format!("{} 只", self.base_roster.len()),
                format!("{} 只", self.roster.len()),
            );
            return out;
        }
        for (index, (before, after)) in self.base_roster.iter().zip(&self.roster).enumerate() {
            if before == after {
                continue;
            }
            let who = self
                .entries
                .iter()
                .find(|e| e.name == after.pack)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| after.pack.clone());
            let (b, a) = (PetOptions::from_slot(before), PetOptions::from_slot(after));
            note(
                &format!("{who} · 形态"),
                before.form.clone().unwrap_or_else(|| "链首".into()),
                after.form.clone().unwrap_or_else(|| "链首".into()),
            );
            note(
                &format!("{who} · 大小"),
                theme::percent(b.scale),
                theme::percent(a.scale),
            );
            note(
                &format!("{who} · 性格"),
                b.persona.name.to_string(),
                a.persona.name.to_string(),
            );
            note(
                &format!("{who} · 参与叫声"),
                yes_no(b.voice),
                yes_no(a.voice),
            );
            note(
                &format!("{who} · 记住落脚点"),
                yes_no(b.remember),
                yes_no(a.remember),
            );
            let _ = index;
        }
        out
    }

    /// 改了东西:存盘并让在跑的桌宠跟上。**每一次改动都走这里**。
    fn apply(&mut self) {
        if let Err(e) = self.write_files() {
            self.status.fail(format!("存不进去:{e:#}"));
            return;
        }
        match crate::control::send_command(crate::control::Control::Reload) {
            Ok(()) => self.status.ok("改动已通过 Reload 送达桌宠"),
            // 桌宠没在跑是**正常情况**(先配置好再启动)
            Err(_) => self.status.ok("已存盘;桌宠没在跑,下次启动生效"),
        }
    }

    fn write_files(&self) -> Result<()> {
        if let Some(path) = self.roster_path.as_deref() {
            let mut pets = self.roster.clone();
            carry_over_runtime_fields(&mut pets, path);
            Roster { pets }.save(path)?;
        }
        let Some(path) = self.config_path.as_deref() else {
            return Ok(());
        };
        // **只写这几项**:`pack`/`form` 是「还没碰过阵容」时的老路,阵容存档一存在
        // 就轮不到它们了,这儿动它反而会让两份文件打架。老配置里那两行 hotkey
        // 也不碰 —— 它已经不起作用了,但那是用户手写的东西,删不删由他自己定。
        Config::write_back(
            path,
            &[
                ("px_per_cm", Setting::Num(self.config.px_per_cm)),
                ("volume", Setting::Num(self.config.volume)),
                ("fps", Setting::Int(self.config.fps)),
                ("passthrough", Setting::Flag(self.config.passthrough)),
            ],
        )
        .context("写配置文件失败")
    }

    /// 撤销:回到打开窗口时那一份,并立刻生效。
    fn undo(&mut self) {
        self.config = self.base_config.clone();
        self.roster = self.base_roster.clone();
        if let Page::Pet(slot) = self.page
            && slot >= self.roster.len()
        {
            self.page = Page::Packs;
        }
        self.apply();
        self.status.ok("已撤销,回到打开窗口时的样子");
    }

    /// 把一只加进阵容(包页的「上桌」与侧栏的「添加宠物…」都用)。
    fn add_to_roster(&mut self, path: &Path) {
        let name = pack_key(path, self.packs_dir.as_deref());
        let display = Pack::peek_name(path);
        self.roster.push(Slot::new(name, None));
        self.page = Page::Pet(self.roster.len() - 1);
        self.apply();
        self.status.ok(format!("{display} 已上桌"));
    }

    // ── 界面 ────────────────────────────────────────────────────

    /// 侧栏:两个固定页 + 活跃宠物逐只展开 + 底部路径。
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let count = self.entries.len();
        ui.add_space(4.0);
        // 常用配置在最上面:托盘那三项与「首选项」都落在这一页,是进来得最多的一页;
        // 宠物包是「偶尔导一次」的事
        self.nav_row(ui, Page::Common, "常用配置", None);
        self.nav_row(ui, Page::Packs, "宠物包", Some(count.to_string()));

        ui.add_space(8.0);
        ui.separator();
        theme::group_label(ui, &format!("活跃宠物 · {}", self.roster.len()));

        for slot in 0..self.roster.len() {
            let name = self
                .pack_for_slot(slot)
                .map(|pack| {
                    let index = self.form_index(&pack, slot);
                    pack.forms[index].name.clone()
                })
                .unwrap_or_else(|| self.roster[slot].pack.clone());
            let scale = PetOptions::from_slot(&self.roster[slot]).scale;
            self.nav_row(ui, Page::Pet(slot), &name, Some(theme::percent(scale)));
        }

        ui.add_space(2.0);
        let accent = ui.visuals().hyperlink_color;
        let add = ui.add(
            egui::Button::new(egui::RichText::new("＋ 添加宠物…").color(accent))
                .fill(egui::Color32::TRANSPARENT)
                .min_size(egui::vec2(ui.available_width(), theme::ROW_H)),
        );
        if add.clicked() {
            self.page = Page::Packs;
            self.status.ok("挑一个包,点「上桌」");
        }
    }

    /// 侧栏的一行。选中的整行高亮,右边可以带一个次要数值。
    fn nav_row(&mut self, ui: &mut egui::Ui, page: Page, label: &str, badge: Option<String>) {
        let selected = self.page == page;
        let width = ui.available_width();
        let response = ui.add(
            egui::Button::selectable(selected, "").min_size(egui::vec2(width, theme::ROW_H)),
        );
        // SelectableLabel 自己不排「左文字 + 右数值」,所以底色由它画、内容我们画
        let rect = response.rect.shrink2(egui::vec2(10.0, 0.0));
        let color = if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        };
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(theme::BODY),
            color,
        );
        if let Some(badge) = badge {
            ui.painter().text(
                rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                badge,
                egui::FontId::monospace(theme::GROUP),
                ui.visuals().weak_text_color(),
            );
        }
        if response.clicked() {
            self.page = page;
        }
    }

    /// 顶上那条:没改动时说「改动即时生效,不需要手动保存」,
    /// 改过之后说改了几项并给「查看改动 / 撤销」。
    ///
    /// **一直在那儿,宽高都不变**。原来是没改动就整条不画,于是拖滑杆拖出第一处改动的
    /// 那一瞬间,底下整页会往下跳一截 —— 而正在拖的那根滑杆就在这页上,手还按着,
    /// 它自己从指针底下溜走了。行高按 `CONTROL_H` 兜住:两种状态里只有一种有按钮,
    /// 而按钮比文字高。
    fn modified_bar(&mut self, ui: &mut egui::Ui) {
        let changes = self.changes();
        let visuals = ui.visuals().clone();
        egui::Frame::new()
            .fill(visuals.faint_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(theme::CONTROL_H);
                    // **两种状态一样宽**:有改动那版靠右对齐的按钮把整条撑满,
                    // 没改动那版只有一句话 —— 不撑一下的话这条会缩成一小块,
                    // 看着像是「冒出来又缩回去」的另一个东西
                    ui.set_min_width(ui.available_width());
                    if changes.is_empty() {
                        theme::hint(ui, "改动即时生效,不需要手动保存");
                        return;
                    }
                    // 文案是「已生效」而不是「待保存」—— 即时生效模型下,
                    // 用户要知道的不是「还没存」,而是「改了这些,可以撤」
                    ui.label(format!("已修改 {} 项,均已即时生效", changes.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("撤销").clicked() {
                            self.undo();
                        }
                        if ui.button("查看改动").clicked() {
                            self.show_changes = true;
                        }
                    });
                });
            });
        ui.add_space(8.0);
    }

    /// 「查看改动」那张弹窗:逐条列出「什么 · 从 → 到」。
    fn changes_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_changes {
            return;
        }
        let changes = self.changes();
        let response = egui::Modal::new(egui::Id::new("changes"))
            .frame(modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                ui.heading("这次改了什么");
                ui.add_space(8.0);
                if changes.is_empty() {
                    ui.label("没有改动。");
                }
                theme::scrollbar(ui);
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for change in &changes {
                            ui.horizontal(|ui| {
                                ui.label(&change.what);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(theme::value(&change.to));
                                        theme::hint(ui, "→");
                                        theme::hint(ui, &change.from);
                                    },
                                );
                            });
                        }
                    });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.button("关闭").clicked()
                    })
                    .inner
                })
                .inner
            });
        if response.inner || response.should_close() {
            self.show_changes = false;
        }
    }

    /// 底部状态栏。设计稿里 KDE 有、Windows 没有;这里统一保留 ——
    /// 它是「桌宠在不在跑」的唯一去处,而那件事任何时候都值得知道。
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.status.error {
                ui.colored_label(ui.visuals().error_fg_color, &self.status.text);
            } else {
                theme::hint(ui, &self.status.text);
            }
        });
    }

    /// 这一只当前选中的形态在包里的下标。
    fn form_index(&self, pack: &Pack, slot: usize) -> usize {
        self.roster
            .get(slot)
            .and_then(|s| s.form.as_deref())
            .and_then(|want| {
                pack.forms
                    .iter()
                    .position(|f| f.asset == want || f.name == want)
            })
            .unwrap_or(0)
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 桌宠退出了 —— 跟着关。改动都是即时生效的,这里没有「还没保存」的东西
        if self.close.taken() {
            log::info!("桌宠退出了,配置窗口跟着关");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // 拖进来的文件当「导入」处理 —— 比翻文件对话框快
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.import(&dropped);
        }

        egui::Panel::left("sidebar")
            .exact_size(theme::SIDEBAR_W)
            .resizable(false)
            .show(ui, |ui| self.sidebar(ui));

        egui::Panel::bottom("status")
            .exact_size(26.0)
            .resizable(false)
            .show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.add_space(4.0);
            self.modified_bar(ui);
            match self.page {
                Page::Packs => self.packs_page(ui),
                Page::Common => self.common_page(ui),
                Page::Pet(slot) => self.pet_page(ui, slot),
            }
        });

        self.changes_dialog(&ctx);
        self.delete_dialog(&ctx);
        self.drop_overlay(&ctx);
    }
}

/// 弹窗的外框。**比 egui 默认的宽松**:默认那圈 6px 的内边距下,标题与按钮几乎贴着
/// 边框,一眼看过去像是没画完。这里给到 20/18,与页面本身的留白量级对上。
fn modal_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.style_of(ctx.theme())).inner_margin(egui::Margin::symmetric(20, 18))
}

/// 什么都没发生时状态栏说什么。**桌宠在不在跑**是这里唯一值得常驻的一句:
/// 「改了没反应」在两种情况下都会发生,不说清楚会让人以为程序坏了。
fn idle_status() -> String {
    if crate::control::is_running() {
        "桌宠正在运行 · 改动即时生效".into()
    } else {
        "桌宠没在运行 · 改动存下来,下次启动生效".into()
    }
}

fn yes_no(on: bool) -> String {
    if on { "开" } else { "关" }.to_string()
}

/// 阵容存档里该怎么写这个包:在包目录里就写文件名,否则写绝对路径
/// (与 platform/shared.rs 的 `save_roster` 同一条规矩)。
fn pack_key(path: &Path, packs_dir: Option<&Path>) -> String {
    let in_packs_dir = packs_dir.is_some_and(|dir| path.parent() == Some(dir));
    match (in_packs_dir, path.file_name()) {
        (true, Some(name)) => name.to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// 把**运行时写的**那两项(掷出来的嗓音、站过的位置)从盘上原样带过去。
///
/// 这个窗口是整份重写 roster.toml 的,而桌宠随时可能刚往里写了个新位置 ——
/// 不带过去就等于把它刚记下的东西抹掉。按**位置**对齐:这两项都跟着插槽走,
/// 而插槽顺序在这两次读写之间只有本窗口能改。
fn carry_over_runtime_fields(pets: &mut [Slot], path: &Path) {
    let Some(on_disk) = Roster::load(path) else {
        return;
    };
    for (slot, saved) in pets.iter_mut().zip(&on_disk.pets) {
        // 换了包就不是同一只了,那两项跟着作废
        if slot.pack != saved.pack {
            continue;
        }
        if slot.voice_value.is_none() {
            slot.voice_value = saved.voice_value;
        }
        if slot.remember.unwrap_or(false) && slot.home_x.is_none() {
            slot.home_x = saved.home_x;
        }
    }
}

// ── 单实例 ────────────────────────────────────────────────────────
// 两个配置窗口同时开着的话,后写的那个会把前一个的改动整份覆盖
// (两边都是「把内存里那份写下去」)。挡在开窗口之前最省事。

/// 拿到就说明「这个进程是唯一的配置窗口」;drop 掉就放开。
struct InstanceGuard {
    #[cfg(target_os = "linux")]
    _connection: zbus::blocking::Connection,
    #[cfg(target_os = "windows")]
    _mutex: windows::Win32::Foundation::HANDLE,
}

/// 桌宠喊「退出」时调到的那个方法。
#[cfg(target_os = "linux")]
struct QuitService {
    close: CloseRequest,
}

#[cfg(target_os = "linux")]
#[zbus::interface(name = "org.rocom.Pets.Settings1")]
impl QuitService {
    /// 桌宠退出了,这个窗口也关掉。
    fn quit(&self) {
        self.close.ask();
    }
}

#[cfg(target_os = "linux")]
fn single_instance(close: CloseRequest) -> Option<InstanceGuard> {
    use zbus::fdo::RequestNameFlags;

    // 抢一个会话总线上的名字:已经有人占着就说明窗口开着了。
    //
    // **`DoNotQueue` 是关键**:不加的话抢不到会排队等着,于是第二个窗口既不出现、
    // 也不退出,静静地卡在那儿。
    let connection = zbus::blocking::Connection::session().ok()?;
    // **先挂对象再要名字**:反过来 zbus 会警告「Requesting name before setting up
    // the object server」—— 那句话是对的,名字一旦到手就可能立刻有人调进来。
    let served = connection
        .object_server()
        .at("/org/rocom/Pets/Settings", QuitService { close });
    if let Err(e) = served {
        // 挂不上只是「桌宠退出时关不掉这个窗口」,窗口本身照开
        log::warn!("配置窗口的 D-Bus 对象挂不上({e});桌宠退出时不会带上它");
    }
    let reply = connection
        .request_name_with_flags(
            "org.rocom.Pets.Settings",
            RequestNameFlags::DoNotQueue.into(),
        )
        .ok()?;
    if reply != zbus::fdo::RequestNameReply::PrimaryOwner {
        return None;
    }
    Some(InstanceGuard {
        _connection: connection,
    })
}

#[cfg(target_os = "windows")]
fn single_instance(close: CloseRequest) -> Option<InstanceGuard> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    // SAFETY: 造一个具名互斥量;已经存在时 GetLastError 会是 ERROR_ALREADY_EXISTS。
    // 句柄留在 guard 里,进程退出时系统回收。
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\rocom-pets-settings")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return None;
        }
        watch_for_quit(close);
        Some(InstanceGuard { _mutex: mutex })
    }
}

/// 等桌宠喊退出:一个具名事件 + 一条专门等着它的线程。
///
/// 为什么不是「按标题找窗口再发 `WM_CLOSE`」:winit 的窗口类名是通用的,
/// 只能按标题匹配,而标题是会变的界面文案 —— 具名内核对象才是**给进程间用的**那种名字。
#[cfg(target_os = "windows")]
fn watch_for_quit(close: CloseRequest) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
    use windows::core::{HSTRING, PCWSTR};

    let name = HSTRING::from(crate::control::SETTINGS_QUIT_EVENT);
    // SAFETY: 手动重置的具名事件;桌宠那边 `OpenEventW` + `SetEvent`(见 control/windows.rs)。
    let event = match unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) } {
        Ok(event) => event,
        Err(e) => {
            log::warn!("等不了桌宠的退出通知({e});桌宠退出时不会带上这个窗口");
            return;
        }
    };
    // HANDLE 是裸指针包出来的,过不了线程边界;按整数搬过去再拼回来
    let raw = event.0 as usize;
    std::thread::spawn(move || {
        let event = HANDLE(raw as *mut std::ffi::c_void);
        // SAFETY: 句柄就是上面那个,一直活到进程结束。
        unsafe { WaitForSingleObject(event, INFINITE) };
        close.ask();
    });
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn single_instance(_close: CloseRequest) -> Option<InstanceGuard> {
    Some(InstanceGuard {})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rocom-settings-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("该能建目录");
        dir
    }

    fn app(dir: &Path) -> SettingsApp {
        SettingsApp::new(
            Some(dir.join("config.toml")),
            Some(dir.join("packs")),
            SettingsPage::Packs,
        )
    }

    #[test]
    fn roster_key_prefers_the_file_name_inside_the_packs_dir() {
        let dir = PathBuf::from("/data/packs");
        // 包目录里的:存文件名,于是整个包目录搬走时阵容还认得出来
        assert_eq!(pack_key(&dir.join("喵喵.rkpet"), Some(&dir)), "喵喵.rkpet");
        assert_eq!(pack_key(&dir.join("幽星光"), Some(&dir)), "幽星光");
        // 不在包目录里的:只能存绝对路径
        assert_eq!(
            pack_key(Path::new("/elsewhere/波波拉"), Some(&dir)),
            "/elsewhere/波波拉"
        );
        assert_eq!(pack_key(&dir.join("喵喵"), None), "/data/packs/喵喵");
    }

    /// 改一项 → 落盘 → 重开窗口读回来,而且注释还在。
    #[test]
    fn a_change_is_written_and_reads_back() {
        let dir = scratch("apply");
        let mut app = app(&dir);
        assert!(dir.join("config.toml").is_file(), "首次打开该生成配置模板");

        app.config.px_per_cm = 3.5;
        app.config.volume = 0.8;
        app.config.passthrough = true;
        app.config.fps = 60;
        app.roster = vec![Slot {
            scale: Some(1.25),
            persona: Some("lazy".into()),
            ..Slot::new("喵喵.rkpet".into(), Some("Gra_MiaoMiao2_001".into()))
        }];
        app.write_files().expect("该能写");

        let reopened = app_at(&dir);
        assert_eq!(reopened.config.px_per_cm, 3.5);
        assert_eq!(reopened.config.volume, 0.8);
        assert!(reopened.config.passthrough);
        // 帧率走的是整数那条写回路,写成 60.0 的话这里根本读不回来
        assert_eq!(reopened.config.fps, 60);
        assert_eq!(reopened.roster, app.roster);
        let text = std::fs::read_to_string(dir.join("config.toml")).expect("该能读");
        assert!(text.contains("# rocom-pets 配置"), "注释没了:{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn app_at(dir: &Path) -> SettingsApp {
        app(dir)
    }

    /// **桌宠刚记下的东西不能被这个窗口抹掉**:嗓音与落脚点是运行时写的。
    #[test]
    fn runtime_written_fields_are_carried_over() {
        let dir = scratch("carry");
        let path = dir.join("roster.toml");
        // 盘上那份带着桌宠掷出来的嗓音与站过的位置
        Roster {
            pets: vec![Slot {
                voice_value: Some(-37.0),
                remember: Some(true),
                home_x: Some(0.62),
                ..Slot::new("喵喵".into(), None)
            }],
        }
        .save(&path)
        .expect("该能写");

        // 窗口里那份是「改性格」之前读进来的,不知道这两项
        let mut pets = vec![Slot {
            persona: Some("lively".into()),
            remember: Some(true),
            ..Slot::new("喵喵".into(), None)
        }];
        carry_over_runtime_fields(&mut pets, &path);
        assert_eq!(pets[0].voice_value, Some(-37.0), "嗓音被抹掉了");
        assert_eq!(pets[0].home_x, Some(0.62), "落脚点被抹掉了");
        assert_eq!(pets[0].persona.as_deref(), Some("lively"), "改动该保住");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 换了包就是另一只,旧的嗓音/位置不能跟过去。
    #[test]
    fn carrying_over_stops_at_a_different_pack() {
        let dir = scratch("carry2");
        let path = dir.join("roster.toml");
        Roster {
            pets: vec![Slot {
                voice_value: Some(-37.0),
                ..Slot::new("喵喵".into(), None)
            }],
        }
        .save(&path)
        .expect("该能写");
        let mut pets = vec![Slot::new("幽星光".into(), None)];
        carry_over_runtime_fields(&mut pets, &path);
        assert_eq!(pets[0].voice_value, None, "换了包还带着旧嗓音");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「已修改 N 项」要数得准,而且撤销之后回到 0。
    #[test]
    fn changes_are_counted_and_undone() {
        let dir = scratch("changes");
        let mut app = app(&dir);
        assert!(app.changes().is_empty(), "刚打开不该有改动");

        app.config.px_per_cm = 3.0;
        app.config.volume = 0.6;
        assert_eq!(app.changes().len(), 2);
        // 条目里要写清楚从什么变成什么
        let sizes = &app.changes()[0];
        assert_eq!(sizes.what, "整体大小");
        assert_eq!(sizes.from, "100%");
        assert_eq!(sizes.to, "150%");

        app.undo();
        assert!(app.changes().is_empty(), "撤销之后该回到 0 项");
        assert_eq!(app.config.px_per_cm, 2.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
