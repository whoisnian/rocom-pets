//! `rocom-pets --settings`:独立的配置窗口(宠物包管理 + 活跃宠物管理 + 常用配置)。
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
//! `roster.toml`(机器写的)。配置窗口存完盘发一条 `Reload`,在跑的实例重读这两份文件
//! 并把台上的一切对齐过去。没有第二套协议要维护,也就不存在「命令加了字段但对面是旧版本」
//! 这类问题;桌宠没在跑的时候,存下来的东西下次启动照样生效。

mod fonts;
mod packs;
mod pets;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use eframe::egui;

use crate::config::{Config, Setting};
use crate::pack::{Pack, PackEntry};
use crate::roster::{Roster, Slot};

/// 窗口的三页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Pets,
    Packs,
    Common,
}

/// 底部那行提示:做完一件事说一句,出错说得更显眼。
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

pub struct SettingsApp {
    config_path: Option<PathBuf>,
    roster_path: Option<PathBuf>,
    packs_dir: Option<PathBuf>,
    /// 编辑中的配置与阵容;存盘之前只在内存里。
    config: Config,
    roster: Vec<Slot>,
    /// 包目录扫描结果:**只有名字与位置**。全库五百多个包,进来就全解析要一秒多。
    entries: Vec<PackEntry>,
    /// 惰性读出来的包详情(选中某一行、或某只宠物要列形态时才读)。
    /// `None` = 读过了但读不动,别每帧重试。
    loaded: HashMap<PathBuf, Option<Rc<Pack>>>,
    /// 「查找」框(两页共用一个:找的是同一批包)。
    filter: String,
    selected_pack: Option<PathBuf>,
    /// 正等着确认删除的包。删包是不可逆的,必须问一句。
    confirm_delete: Option<PathBuf>,
    tab: Tab,
    dirty: bool,
    status: Status,
}

/// 起配置窗口。`config_path` / `packs_dir` 由 main 按与桌宠**完全一样**的规则定出来 ——
/// 两边看的必须是同一批文件。
pub fn run(config_path: Option<PathBuf>, packs_dir: Option<PathBuf>) -> Result<()> {
    // 已经开着一个就别再开:两个窗口各改各的,后存的那个会把前一个的改动整份覆盖
    let _guard = match single_instance() {
        Some(guard) => guard,
        None => {
            log::info!("配置窗口已经开着一个了");
            return Ok(());
        }
    };

    let app = SettingsApp::new(config_path, packs_dir);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([560.0, 400.0])
            .with_title("rocom-pets 配置"),
        ..Default::default()
    };
    eframe::run_native(
        "rocom-pets 配置",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("配置窗口起不来: {e}"))
}

impl SettingsApp {
    fn new(config_path: Option<PathBuf>, packs_dir: Option<PathBuf>) -> Self {
        let mut app = Self {
            roster_path: config_path.as_deref().map(Roster::path_beside),
            config_path,
            packs_dir,
            config: Config::default(),
            roster: Vec::new(),
            entries: Vec::new(),
            loaded: HashMap::new(),
            filter: String::new(),
            selected_pack: None,
            confirm_delete: None,
            tab: Tab::Pets,
            dirty: false,
            status: Status::default(),
        };
        app.reload_from_disk();
        app
    }

    /// 把两份文件重读一遍,丢掉未保存的改动。开窗口时与「放弃改动」都走这里。
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
        self.rescan_packs();
        self.dirty = false;
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

    /// 取包的详情(形态表等)。第一次问才读盘,读不动就记下来别再试。
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
        let name = self.roster.get(slot)?.pack.clone();
        let path = self
            .entries
            .iter()
            .find(|e| {
                e.name == name
                    || e.path.file_name().is_some_and(|n| n == name.as_str())
                    || e.path.file_stem().is_some_and(|n| n == name.as_str())
            })
            .map(|e| e.path.clone())
            // 阵容里也可能是个绝对路径(`--pack /some/where` 存下来的)
            .unwrap_or_else(|| Config::expand_path(&name));
        self.pack_at(&path)
    }

    /// 存两份文件,然后通知在跑的实例重读。
    fn save(&mut self) {
        if let Err(e) = self.save_inner() {
            self.status.fail(format!("保存失败:{e:#}"));
            return;
        }
        self.dirty = false;
        // 桌宠没在跑是**正常情况**(先配置好再启动),所以这里失败只是少一句「已应用」
        match crate::control::send_command(crate::control::Control::Reload) {
            Ok(()) => self.status.ok("已保存,桌宠已经跟着变了"),
            Err(_) => self.status.ok("已保存(桌宠没在跑,下次启动生效)"),
        }
    }

    fn save_inner(&self) -> Result<()> {
        if let Some(path) = self.roster_path.as_deref() {
            Roster {
                pets: self.roster.clone(),
            }
            .save(path)?;
        }
        let Some(path) = self.config_path.as_deref() else {
            return Ok(());
        };
        // **只写这几项**:`pack`/`form` 是「还没碰过阵容」时的老路,阵容存档一存在
        // 就轮不到它们了,这儿动它反而会让两份文件打架
        // 热键清空要写**空串**而不是删掉那一行:删掉等于「用内置默认」,
        // 于是下次读回来又是 CTRL+ALT+p(见 config.rs 的 `load_or_create`)
        let hotkey = self.config.hotkey.clone().unwrap_or_default();
        Config::write_back(
            path,
            &[
                ("px_per_cm", Setting::Num(self.config.px_per_cm)),
                ("volume", Setting::Num(self.config.volume)),
                ("passthrough", Setting::Flag(self.config.passthrough)),
                ("hotkey", Setting::Text(&hotkey)),
            ],
        )
        .context("写配置文件失败")
    }

    /// 把一只加进阵容(包页与宠物页都用)。
    fn add_to_roster(&mut self, path: &Path) {
        let name = pack_key(path, self.packs_dir.as_deref());
        let display = Pack::peek_name(path);
        self.roster.push(Slot::new(name, None));
        self.dirty = true;
        self.tab = Tab::Pets;
        self.status.ok(format!("{display} 已加进阵容,记得保存"));
    }

    /// 常用配置那一页。
    fn common_tab(&mut self, ui: &mut egui::Ui) {
        let config = &mut self.config;
        ui.heading("常用配置");
        ui.add_space(4.0);

        egui::Grid::new("common")
            .num_columns(2)
            .spacing([12.0, 10.0])
            .show(ui, |ui| {
                ui.label("整体大小");
                ui.vertical(|ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut config.px_per_cm, 1.0..=6.0)
                                .suffix(" px/cm")
                                .fixed_decimals(1),
                        )
                        .changed()
                    {
                        self.dirty = true;
                    }
                    // 换算成看得见的数字:光看 px/cm 想象不出多大
                    ui.small(format!(
                        "80cm 的喵喵 ≈ {:.0}px 高,204cm 的魔力猫 ≈ {:.0}px",
                        80.0 * config.px_per_cm,
                        204.0 * config.px_per_cm
                    ));
                });
                ui.end_row();

                ui.label("叫声音量");
                ui.vertical(|ui| {
                    if ui
                        .add(egui::Slider::new(&mut config.volume, 0.0..=1.0).fixed_decimals(2))
                        .changed()
                    {
                        self.dirty = true;
                    }
                    ui.small("0 = 完全不开音频设备");
                });
                ui.end_row();

                ui.label("鼠标穿透");
                if ui
                    .checkbox(&mut config.passthrough, "启动就开(宠物只显示,不接鼠标)")
                    .changed()
                {
                    self.dirty = true;
                }
                ui.end_row();

                ui.label("全局热键");
                ui.vertical(|ui| {
                    let mut hotkey = config.hotkey.clone().unwrap_or_default();
                    if ui.text_edit_singleline(&mut hotkey).changed() {
                        config.hotkey = (!hotkey.trim().is_empty()).then_some(hotkey);
                        self.dirty = true;
                    }
                    ui.small(
                        "切换鼠标穿透,留空 = 不用热键。走 XDG GlobalShortcuts,\
                         只在 KDE 这类实现了它的桌面上有效;Windows 上不申请热键(见 README)",
                    );
                });
                ui.end_row();
            });

        ui.add_space(12.0);
        ui.separator();
        ui.label("文件位置");
        let row = |ui: &mut egui::Ui, label: &str, path: Option<&Path>| {
            ui.horizontal(|ui| {
                ui.label(label);
                match path {
                    Some(path) => {
                        let text = path.display().to_string();
                        // **必须截断**:路径可能比窗口还长,不截的话它会把这一行整个撑出
                        // 窗口右边(实机截图里就是这样);悬停能看全文
                        ui.add(egui::Label::new(egui::RichText::new(&text).monospace()).truncate())
                            .on_hover_text(&text);
                    }
                    None => {
                        ui.colored_label(ui.visuals().warn_fg_color, "定不出位置");
                    }
                }
            });
        };
        row(ui, "配置文件", self.config_path.as_deref());
        row(ui, "阵容存档", self.roster_path.as_deref());
        row(ui, "宠物包目录", self.packs_dir.as_deref());
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
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

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Pets, "活跃宠物");
                ui.selectable_value(&mut self.tab, Tab::Packs, "宠物包");
                ui.selectable_value(&mut self.tab, Tab::Common, "常用配置");
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.dirty, egui::Button::new("保存并应用"))
                    .clicked()
                {
                    self.save();
                }
                if ui
                    .add_enabled(self.dirty, egui::Button::new("放弃改动"))
                    .clicked()
                {
                    self.reload_from_disk();
                    self.status.ok("已回到存盘时的样子");
                }
                if self.dirty {
                    ui.colored_label(ui.visuals().warn_fg_color, "有未保存的改动");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.status.error {
                        ui.colored_label(ui.visuals().error_fg_color, &self.status.text);
                    } else {
                        ui.label(&self.status.text);
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default_margins().show(ui, |ui| match self.tab {
            Tab::Pets => self.pets_tab(ui),
            Tab::Packs => self.packs_tab(ui),
            Tab::Common => self.common_tab(ui),
        });

        self.delete_dialog(&ctx);
    }
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

// ── 单实例 ────────────────────────────────────────────────────────
// 两个配置窗口同时开着的话,后按保存的那个会把前一个的改动整份覆盖
// (两边都是「把内存里那份写下去」)。挡在开窗口之前最省事。

/// 拿到就说明「这个进程是唯一的配置窗口」;drop 掉就放开。
struct InstanceGuard {
    #[cfg(target_os = "linux")]
    _connection: zbus::blocking::Connection,
    #[cfg(target_os = "windows")]
    _mutex: windows::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "linux")]
fn single_instance() -> Option<InstanceGuard> {
    use zbus::fdo::RequestNameFlags;

    // 抢一个会话总线上的名字:已经有人占着就说明窗口开着了。名字本身就是全部目的,
    // 不挂任何对象(zbus 为此会警告一句,`--settings` 那条路把它压到 error 了,见 main.rs)。
    //
    // **`DoNotQueue` 是关键**:不加的话抢不到会排队等着,于是第二个窗口既不出现、
    // 也不退出,静静地卡在那儿。
    let connection = zbus::blocking::Connection::session().ok()?;
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
fn single_instance() -> Option<InstanceGuard> {
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
        Some(InstanceGuard { _mutex: mutex })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn single_instance() -> Option<InstanceGuard> {
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

    /// 「保存并应用」那条路:两份文件都写对,而且**读回来等于刚才编辑的那份**。
    ///
    /// 这条路平时只有点按钮才走得到,不测的话回归了也没人知道。
    #[test]
    fn saving_writes_both_files_and_round_trips() {
        let dir = scratch("save");
        let config_path = dir.join("config.toml");
        let mut app = SettingsApp::new(Some(config_path.clone()), Some(dir.join("packs")));
        // 开窗口时文件还不在,应该已经写出一份模板了
        assert!(config_path.is_file(), "首次打开该生成配置模板");

        app.config.px_per_cm = 3.5;
        app.config.volume = 0.8;
        app.config.passthrough = true;
        app.config.hotkey = None;
        app.roster = vec![Slot {
            pack: "喵喵.rkpet".into(),
            form: Some("Gra_MiaoMiao2_001".into()),
            scale: Some(1.25),
            persona: Some("lazy".into()),
            emotes: Some(vec!["Happy".into()]),
        }];
        app.save_inner().expect("该能存");

        // 换一个 app 从盘上读回来 —— 等价于「关掉窗口再打开」
        let reopened = SettingsApp::new(Some(config_path.clone()), Some(dir.join("packs")));
        assert_eq!(reopened.config.px_per_cm, 3.5);
        assert_eq!(reopened.config.volume, 0.8);
        assert!(reopened.config.passthrough);
        assert_eq!(reopened.config.hotkey, None, "清空热键要真的写没了");
        assert_eq!(reopened.roster, app.roster);
        // 模板的注释必须还在(这就是 config.toml 走 toml_edit 的理由)
        let text = std::fs::read_to_string(&config_path).expect("该能读");
        assert!(text.contains("# rocom-pets 配置"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_does_not_touch_the_legacy_single_pet_keys() {
        // config.toml 的 pack/form 是「还没碰过阵容」时的老路。阵容存档一存在就轮不到
        // 它们了,这时候再去写只会让两份文件打架
        let dir = scratch("legacy");
        let config_path = dir.join("config.toml");
        std::fs::write(
            &config_path,
            "pack = \"幽星光\"\nform = \"Ill_XingGuang1_001\"\npx_per_cm = 2.0\n",
        )
        .expect("该能写");
        let mut app = SettingsApp::new(Some(config_path.clone()), None);
        app.roster = vec![Slot::new("喵喵".into(), None)];
        app.save_inner().expect("该能存");

        let text = std::fs::read_to_string(&config_path).expect("该能读");
        assert!(text.contains("pack = \"幽星光\""), "不该动 pack:{text}");
        assert!(text.contains("form = \"Ill_XingGuang1_001\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
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
}
