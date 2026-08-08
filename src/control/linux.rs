//! Linux(KDE Plasma)的外部控制:ksni 托盘、自己的 D-Bus 接口、信号。
//! 三者都把命令送进同一个 calloop 通道,事件循环那边只认 [`Control`],不关心命令是哪来的。

use super::{
    Common, Control, FPS_STEPS, PX_PER_CM_STANDARD, SIZE_STEPS, SettingsPage, VOLUME_STEPS,
    exact_step, nearest_step,
};
use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::{Connection, Proxy};

/// 托盘图标 + 菜单。菜单动作发命令,状态(穿透、静音、当前档位)回显在菜单上。
///
/// 结构:三个开关 → 三个档位子菜单 → 首选项/重新载入/退出(见 docs/design.md Phase 7)。
/// **顶层不再逐只展开宠物,也不报在场数量**。
pub struct Tray {
    sender: Sender<Control>,
    passthrough: bool,
    /// 静音了没有;None = 压根没有音频设备(那两项就不显示)。
    muted: Option<bool>,
    /// 在场宠物的名字,只用来写图标的悬停提示(菜单里不再有这一行)。
    pets: Vec<String>,
    /// 当前的帧率、每厘米像素数与音量,菜单里回显选中的那一档。
    common: Common,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "rocom-pets".into()
    }

    fn title(&self) -> String {
        self.headline()
    }

    /// 用主题里现成的图标名:自带 PNG 还要处理各种尺寸与主题深浅,收益不大。
    fn icon_name(&self) -> String {
        "face-smile".into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};

        // **顶上不放「几只在场」**:菜单是用来做事的,一条点不动的统计只是在占位置
        // (数量在图标的悬停提示里,和 Windows 那边一样)。
        //
        // 三个开关按「多久用一次」排:穿透与静音是随手切的,召回是出事了才找的。
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            CheckmarkItem {
                label: "点击穿透".into(),
                checked: self.passthrough,
                activate: Box::new(|tray: &mut Self| tray.send(Control::TogglePassthrough)),
                ..Default::default()
            }
            .into(),
        ];
        // **勾上 = 静音**,不是「叫声开着」。勾选项的语义是「这个状态生效中」,
        // 而用户要找的是「怎么让它别叫」——那就让被找的那个词出现在菜单上。
        if let Some(muted) = self.muted {
            items.push(
                CheckmarkItem {
                    label: "静音叫声".into(),
                    checked: muted,
                    activate: Box::new(|tray: &mut Self| tray.send(Control::ToggleMute)),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(
            StandardItem {
                label: "召回宠物".into(),
                icon_name: "go-home".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Control::Recall)),
                ..Default::default()
            }
            .into(),
        );

        items.push(ksni::MenuItem::Separator);
        // **三组档位直接摆在顶层**,不再套一层「常用配置」:套着的时候
        // 调个音量要点两次才看得见选项,而这三样正是最常调的
        items.push(
            SubMenu {
                label: "帧率设置".into(),
                submenu: vec![group(
                    FPS_STEPS,
                    exact_step(FPS_STEPS, self.common.fps),
                    Control::SetFps,
                )],
                ..Default::default()
            }
            .into(),
        );
        items.push(
            SubMenu {
                label: "大小倍率".into(),
                submenu: self.size_menu(),
                ..Default::default()
            }
            .into(),
        );
        // 没有音频设备时这一项点了也没用
        if self.muted.is_some() {
            items.push(
                SubMenu {
                    label: "叫声音量".into(),
                    submenu: vec![group(
                        VOLUME_STEPS,
                        nearest_step(VOLUME_STEPS, self.common.volume),
                        Control::SetVolume,
                    )],
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(ksni::MenuItem::Separator);
        // 档位放不下的全在窗口里 —— 一条入口,落在常用配置页:
        // 从上面那三项点进来的人本来就在找这几样,只是想要更精确的那一版
        items.push(
            StandardItem {
                label: "首选项".into(),
                icon_name: "configure".into(),
                activate: Box::new(|tray: &mut Self| {
                    tray.send(Control::OpenSettings(SettingsPage::Common))
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "重新载入".into(),
                icon_name: "view-refresh".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Control::ReloadPacks)),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "退出".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Control::Quit)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

impl Tray {
    /// 图标悬停时那一行字(菜单里已经不放它了)。
    fn headline(&self) -> String {
        match self.pets.len() {
            0 => "rocom-pets · 台上没有".into(),
            n => format!("rocom-pets · {n} 只在场"),
        }
    }

    /// 「大小倍率」:三档 + 一条「自定义…」通向窗口。
    /// 帧率与音量没有这一条 —— 那几档就是全部选择。
    fn size_menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            group(
                SIZE_STEPS,
                nearest_step(SIZE_STEPS, self.common.px_per_cm / PX_PER_CM_STANDARD),
                |factor| Control::SetPxPerCm(factor * PX_PER_CM_STANDARD),
            ),
            StandardItem {
                label: "自定义…".into(),
                activate: Box::new(|tray: &mut Self| {
                    tray.send(Control::OpenSettings(SettingsPage::Common))
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn send(&self, control: Control) {
        if self.sender.send(control).is_err() {
            log::warn!("主循环已退出,托盘命令没送出去");
        }
    }
}

/// 一组单选。`selected` 由调用方算(连续量用 `nearest_step`、整数用 `exact_step`),
/// **`None` = 一个都不勾**:`usize::MAX` 落在选项之外,DBusMenu 那边就是这个意思。
fn group<T: Copy + Send + Sync + 'static>(
    steps: &'static [(T, &'static str)],
    selected: Option<usize>,
    make: fn(T) -> Control,
) -> ksni::MenuItem<Tray> {
    use ksni::menu::{RadioGroup, RadioItem};
    RadioGroup {
        selected: selected.unwrap_or(usize::MAX),
        select: Box::new(move |tray: &mut Tray, index| {
            if let Some((value, _)) = steps.get(index) {
                tray.send(make(*value));
            }
        }),
        options: steps
            .iter()
            .map(|(_, label)| RadioItem {
                label: (*label).to_string(),
                ..Default::default()
            })
            .collect(),
    }
    .into()
}

/// 托盘句柄:拿着它才能更新勾选状态;drop 掉图标就消失。
pub struct TrayHandle(ksni::blocking::Handle<Tray>);

impl TrayHandle {
    /// 让菜单里的勾选跟上真实状态(穿透也可能是信号或 D-Bus 切的)。
    pub fn set_passthrough(&self, passthrough: bool) {
        self.0
            .update(move |tray: &mut Tray| tray.passthrough = passthrough);
    }

    /// 阵容变了:图标的悬停提示跟着变。
    pub fn set_roster(&self, pets: Vec<String>) {
        self.0.update(move |tray: &mut Tray| tray.pets = pets);
    }

    pub fn set_muted(&self, muted: bool) {
        self.0
            .update(move |tray: &mut Tray| tray.muted = Some(muted));
    }

    /// 那三组单选要回显真实值(可能是配置窗口改的,不是从这儿点的)。
    pub fn set_common(&self, common: Common) {
        self.0.update(move |tray: &mut Tray| tray.common = common);
    }
}

/// 起托盘。失败不致命(没有托盘宿主的桌面照样能跑),调用方只记个日志。
pub fn spawn_tray(
    sender: Sender<Control>,
    passthrough: bool,
    pets: Vec<String>,
    muted: Option<bool>,
    common: Common,
) -> Result<TrayHandle> {
    use ksni::blocking::TrayMethods;
    let handle = Tray {
        sender,
        passthrough,
        muted,
        pets,
        common,
    }
    .spawn()
    .context("注册托盘图标失败(桌面没有 StatusNotifier 宿主?)")?;
    Ok(TrayHandle(handle))
}

// ── 自己的 D-Bus 控制接口 ────────────────────────────────────────
// 全局热键那条路(XDG GlobalShortcuts portal)去掉了:它要桌面实现 portal、要用户
// 点一次授权弹窗,而这个接口把同一件事做完了 —— 在 KDE「自定义快捷键」里把任意键绑到
// `rocom-pets --toggle-passthrough`,键位归系统管,我们一个组合键都不用抢。
// 顺带让宠物可脚本化(比如录屏脚本里先把它藏起来)。

pub(super) const DBUS_NAME: &str = "org.rocom.Pets";
const DBUS_PATH: &str = "/org/rocom/Pets";

struct DbusControl {
    sender: std::sync::Mutex<Sender<Control>>,
}

#[zbus::interface(name = "org.rocom.Pets1")]
impl DbusControl {
    /// 切换鼠标穿透。
    fn toggle_passthrough(&self) {
        self.send(Control::TogglePassthrough);
    }

    /// 切换叫声静音。
    fn toggle_mute(&self) {
        self.send(Control::ToggleMute);
    }

    /// 把宠物召回屏幕中间。
    fn recall(&self) {
        self.send(Control::Recall);
    }

    /// 让第 `slot` 只播一段动作(配置窗口那张动作表点出来的)。
    fn play(&self, slot: u32, clip: u32) {
        self.send(Control::Play(slot, clip));
    }

    /// 重读配置与阵容存档。**配置窗口改完形态/大小/性格就调它**,于是改动立刻落到台上。
    ///
    /// **不重扫包目录**:那要把整个包目录的 manifest 读一遍(热缓存 40ms、冷缓存 400ms),
    /// 而这些改动跟包目录无关。要重扫的走 `ReloadPacks`。
    fn reload(&self) {
        self.send(Control::Reload);
    }

    /// 同上,外加重扫包目录。`--reload` 与「导入/删除了包」走这条。
    fn reload_packs(&self) {
        self.send(Control::ReloadPacks);
    }

    /// 打开配置窗口。
    fn open_settings(&self) {
        self.send(Control::OpenSettings(SettingsPage::Packs));
    }

    /// 退出。
    fn quit(&self) {
        self.send(Control::Quit);
    }
}

impl DbusControl {
    fn send(&self, control: Control) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(control);
        }
    }
}

/// 在会话总线上暴露控制接口。失败不致命(比如已经有一个实例占了名字)。
pub fn serve_dbus(sender: Sender<Control>) -> Result<()> {
    let control = DbusControl {
        sender: std::sync::Mutex::new(sender),
    };
    // 连接对象要一直活着,否则名字就丢了;这里故意 leak,进程退出时自然释放
    let connection = zbus::blocking::connection::Builder::session()
        .context("连不上会话总线")?
        .name(DBUS_NAME)
        .context("注册 D-Bus 名字失败(已经有一个实例在跑?)")?
        .serve_at(DBUS_PATH, control)
        .context("挂载 D-Bus 对象失败")?
        .build()
        .context("建立 D-Bus 连接失败")?;
    std::mem::forget(connection);
    log::info!("D-Bus 控制接口: {DBUS_NAME} {DBUS_PATH}(可用 --toggle-passthrough 等命令驱动)");
    Ok(())
}

/// 叫配置窗口关掉(托盘点「退出」时)。**它是另一个进程**,只能喊一声。
///
/// 名字与对象路径就是配置窗口用来占单实例的那一份(见 settings/mod.rs)——
/// 那边本来就有个「窗口在不在」的凭据,这里顺手用它当门铃,不再另起一套。
/// 没开着就是没人应答,静悄悄地算了。
pub fn close_settings() {
    let Ok(connection) = Connection::session() else {
        return;
    };
    let proxy = Proxy::new(
        &connection,
        "org.rocom.Pets.Settings",
        "/org/rocom/Pets/Settings",
        "org.rocom.Pets.Settings1",
    );
    let Ok(proxy) = proxy else { return };
    match proxy.call::<_, _, ()>("Quit", &()) {
        Ok(()) => log::info!("配置窗口跟着关"),
        Err(e) => log::debug!("配置窗口没在开着({e})"),
    }
}

/// 叫台上第 `slot` 只播一段动作。配置窗口那张动作表用。
///
/// **单独一条**:`send_command` 那条路只传方法名,而这个要带两个参数。
pub fn play(slot: u32, clip: u32) -> Result<()> {
    let connection = Connection::session().context("连不上会话总线")?;
    let proxy = Proxy::new(&connection, DBUS_NAME, DBUS_PATH, "org.rocom.Pets1")
        .context("拿不到控制接口")?;
    proxy
        .call::<_, _, ()>("Play", &(slot, clip))
        .context("调用 Play 失败(宠物没在跑?)")
}

/// 命令行子命令:通知已在运行的实例执行某个命令。
pub fn send_command(control: Control) -> Result<()> {
    let connection = Connection::session().context("连不上会话总线")?;
    let proxy = Proxy::new(&connection, DBUS_NAME, DBUS_PATH, "org.rocom.Pets1")
        .context("拿不到控制接口")?;
    let method = match control {
        Control::TogglePassthrough => "TogglePassthrough",
        Control::ToggleMute => "ToggleMute",
        Control::Recall => "Recall",
        Control::Reload => "Reload",
        Control::ReloadPacks => "ReloadPacks",
        Control::OpenSettings(_) => "OpenSettings",
        Control::Quit => "Quit",
        // 这几个要带参数,命令行没暴露(托盘菜单或配置窗口里调更直观)
        // Play 要带参数,走上面那个 `play`
        Control::Play(..) | Control::SetFps(_) | Control::SetPxPerCm(_) | Control::SetVolume(_) => {
            anyhow::bail!("这项请用托盘菜单或 `rocom-pets --settings`")
        }
    };
    proxy
        .call::<_, _, ()>(method, &())
        .with_context(|| format!("调用 {method} 失败(宠物没在跑?)"))?;
    Ok(())
}
