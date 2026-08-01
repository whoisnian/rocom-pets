//! Windows 的外部控制:通知区(托盘)图标 + 给已在跑的实例发窗口消息。
//!
//! 与 Linux 那边的差别:
//! - 托盘走 `Shell_NotifyIconW`,不是 StatusNotifier;菜单是**点的时候现建**的 `HMENU`,
//!   不像 ksni 那样声明一棵树。
//! - 不申请也不抢全局热键(两个平台都不做了):要快捷键就把系统的自定义快捷键
//!   绑到 `rocom-pets --toggle-passthrough`,或者在快捷方式上挂一个。
//! - 「通知已在跑的实例」不走 D-Bus:按窗口类名找到那个隐藏的消息窗口,`PostMessage` 过去。
//!
//! 托盘图标本身实机验过(能出现、菜单能点);**后来重排过两轮菜单、加了档位子菜单,
//! 那些改动只在 Linux 上验过**。见 docs/design.md §9 Phase 8。

use std::sync::mpsc::Sender;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowW, GetCursorPos, HMENU, IDI_APPLICATION,
    LoadIconW, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_LBUTTONUP, WM_RBUTTONUP,
};
use windows::core::{HSTRING, PCWSTR};

use super::{
    Common, Control, FPS_STEPS, PX_PER_CM_STANDARD, SIZE_STEPS, SettingsPage, VOLUME_STEPS,
    exact_step, nearest_step,
};

/// 隐藏的消息窗口的类名。`--toggle-passthrough` 这类命令靠它找到在跑的实例。
pub const CONTROL_CLASS: &str = "rocom-pets-control";

/// 托盘图标回调消息(鼠标点在图标上时发到我们的消息窗口)。
pub const WM_TRAY: u32 = WM_APP + 1;
/// 外部实例发来的命令,`wparam` 是 [`Control`] 的编号。
pub const WM_CONTROL: u32 = WM_APP + 2;

// 菜单项 id。0 是「没选」,所以从 100 起;两组档位各占一个号段。
// **`TrackPopupMenu` 只把这个数字还回来**,所以「点了哪一项」要能从数字反解出来。
const ID_PASSTHROUGH: usize = 100;
const ID_RECALL: usize = 101;
const ID_MUTE: usize = 102;
const ID_QUIT: usize = 103;
const ID_SETTINGS: usize = 105;
const ID_RELOAD: usize = 106;
const ID_CUSTOM_SIZE: usize = 107;
/// 大小倍率的第 n 档:`ID_SIZE + index`(档位表在 control/mod.rs)。
const ID_SIZE: usize = 200;
/// 音量的第 n 档。
const ID_VOLUME: usize = 300;
/// 帧率的第 n 档。
const ID_FPS: usize = 400;

/// 命令 ↔ 编号。跨进程只能传数字,所以要一张明确的表(别拿 `as` 硬转:
/// 带字段的变体转不了,而且加变体时静默改值)。
fn code_of(control: Control) -> Option<u32> {
    Some(match control {
        Control::TogglePassthrough => 1,
        Control::ToggleMute => 2,
        Control::Recall => 3,
        Control::Quit => 4,
        // 配置窗口存完盘就发这个,是**跨进程**的主用途
        Control::Reload => 5,
        Control::OpenSettings(_) => 6,
        // 配置窗口的动作表用,参数走 lparam(见 `play`)
        Control::Play(..) => 7,
        // 带参数的那几个只在本进程的托盘里发,不跨进程
        Control::SetFps(_) | Control::SetPxPerCm(_) | Control::SetVolume(_) => return None,
    })
}

pub fn control_of(code: u32) -> Option<Control> {
    Some(match code {
        1 => Control::TogglePassthrough,
        2 => Control::ToggleMute,
        3 => Control::Recall,
        4 => Control::Quit,
        5 => Control::Reload,
        6 => Control::OpenSettings(SettingsPage::Packs),
        // 7 带参数,由 `control_from_message` 拆 lparam,不走这儿
        _ => return None,
    })
}

/// 叫台上第 `slot` 只播一段动作。配置窗口那张动作表用。
///
/// 两个参数塞进 `lparam` 的高低 16 位 —— 窗口消息只有 wparam/lparam 两个格子,
/// wparam 已经装着命令编号了。阵容与动作表都远不到 65535 条。
pub fn play(slot: u32, clip: u32) -> Result<()> {
    let class = HSTRING::from(CONTROL_CLASS);
    // SAFETY: 只读地按类名查窗口;找不到返回错误。
    let hwnd = unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }
        .context("找不到在跑的 rocom-pets(它没起来?)")?;
    let packed = ((slot & 0xffff) << 16) | (clip & 0xffff);
    // SAFETY: hwnd 刚由系统给出;PostMessage 不等对方处理。
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_CONTROL,
            WPARAM(7),
            LPARAM(packed as isize),
        )
    }
    .context("发命令失败")
}

/// 叫配置窗口关掉(托盘点「退出」时)。**它是另一个进程**,只能喊一声。
///
/// 走具名事件而不是「按标题找窗口发 `WM_CLOSE`」:winit 的窗口类名是通用的,
/// 只能按标题匹配,而标题是会变的界面文案 —— 具名内核对象才是给进程间用的那种名字。
/// 打不开就是窗口没开着,静悄悄地算了。
pub fn close_settings() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};

    let name = HSTRING::from(super::SETTINGS_QUIT_EVENT);
    // SAFETY: 只按名字开一个已存在的事件,置位后立刻关掉自己这一份句柄。
    unsafe {
        let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())) else {
            return;
        };
        match SetEvent(event) {
            Ok(()) => log::info!("配置窗口跟着关"),
            Err(e) => log::debug!("通知配置窗口失败({e})"),
        }
        let _ = CloseHandle(event);
    }
}

/// 桌宠在不在跑:那个隐藏的消息窗口在不在。配置窗口的状态栏要说这件事。
///
/// **只查不发**:发一条命令过去也能试出来,但那会有副作用。
pub fn is_running() -> bool {
    let class = HSTRING::from(CONTROL_CLASS);
    // SAFETY: 只读地按类名查窗口。
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }.is_ok()
}

/// 通知已在跑的实例执行某个命令。
pub fn send_command(control: Control) -> Result<()> {
    let Some(code) = code_of(control) else {
        bail!("这项请用托盘菜单或 `rocom-pets --settings`");
    };
    let class = HSTRING::from(CONTROL_CLASS);
    // SAFETY: 只读地按类名查窗口;找不到返回错误。
    let hwnd = unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }
        .context("找不到在跑的 rocom-pets(它没起来?)")?;
    // SAFETY: hwnd 刚由系统给出;PostMessage 不等对方处理,失败会返回错误。
    unsafe { PostMessageW(Some(hwnd), WM_CONTROL, WPARAM(code as usize), LPARAM(0)) }
        .context("发命令失败")
}

/// 托盘图标。**持有它才有图标**;drop 时从通知区删掉。
pub struct TrayHandle {
    hwnd: HWND,
    sender: Sender<Control>,
    passthrough: bool,
    /// 静音了没有;None = 压根没有音频设备(那两项就不显示)。
    muted: Option<bool>,
    /// 在场宠物的名字,只用来写托盘提示。
    pets: Vec<String>,
    /// 当前的帧率、每厘米像素数与音量,菜单里回显选中的那一档。
    common: Common,
}

impl TrayHandle {
    pub fn set_passthrough(&mut self, passthrough: bool) {
        self.passthrough = passthrough;
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = Some(muted);
    }

    /// 「常用配置」那三组单选要回显真实值(可能是配置窗口改的,不是从这儿点的)。
    pub fn set_common(&mut self, common: Common) {
        self.common = common;
    }

    /// 阵容变了。Windows 这边菜单是点的时候现建的,存下来就行。
    pub fn set_roster(&mut self, pets: Vec<String>) {
        self.pets = pets;
        self.update_tip();
    }

    /// 托盘图标被点了:左右键都弹菜单(Windows 上左键通常是「主操作」,
    /// 但桌宠没有主窗口可显示,弹菜单最有用)。
    pub fn on_tray_message(&self, message: u32) {
        if message != WM_RBUTTONUP && message != WM_LBUTTONUP {
            return;
        }
        if let Err(e) = self.popup() {
            log::warn!("弹托盘菜单失败: {e}");
        }
    }

    /// 弹菜单。结构与 KDE 那边逐条对齐;在场数量两边都只在图标的悬停提示里。
    fn popup(&self) -> Result<()> {
        let mut point = POINT::default();
        // SAFETY: 取当前光标位置,只写我们自己的栈变量。
        unsafe { GetCursorPos(&mut point) }.context("拿不到光标位置")?;
        // SAFETY: 下面这一串都是标准的托盘菜单流程,句柄都是刚建出来的;
        // `DestroyMenu` 会连子菜单一起销毁,所以子菜单挂上去之后就不用单独管了。
        unsafe {
            let menu = CreatePopupMenu().context("建菜单失败")?;
            let checked = |on: bool| if on { MF_CHECKED } else { MF_STRING };
            AppendMenuW(
                menu,
                checked(self.passthrough),
                ID_PASSTHROUGH,
                &HSTRING::from("点击穿透"),
            )?;
            // **勾上 = 静音**,与 KDE 那边同一套说法
            if let Some(muted) = self.muted {
                AppendMenuW(menu, checked(muted), ID_MUTE, &HSTRING::from("静音叫声"))?;
            }
            AppendMenuW(menu, MF_STRING, ID_RECALL, &HSTRING::from("召回宠物"))?;
            AppendMenuW(menu, MF_SEPARATOR, 0usize, PCWSTR::null())?;

            // **三组档位各自一个子菜单**,不再套一层「常用配置」:套着的时候
            // 调个音量要点两次才看得见选项,而这三样正是最常调的
            let fps = CreatePopupMenu()?;
            steps(
                fps,
                FPS_STEPS,
                exact_step(FPS_STEPS, self.common.fps),
                ID_FPS,
            )?;
            AppendMenuW(menu, MF_POPUP, fps.0 as usize, &HSTRING::from("帧率设置"))?;

            let size = self.size_menu()?;
            AppendMenuW(menu, MF_POPUP, size.0 as usize, &HSTRING::from("大小倍率"))?;

            // 没有音频设备时这一项点了也没用
            if self.muted.is_some() {
                let volume = CreatePopupMenu()?;
                steps(
                    volume,
                    VOLUME_STEPS,
                    nearest_step(VOLUME_STEPS, self.common.volume),
                    ID_VOLUME,
                )?;
                AppendMenuW(
                    menu,
                    MF_POPUP,
                    volume.0 as usize,
                    &HSTRING::from("叫声音量"),
                )?;
            }

            AppendMenuW(menu, MF_SEPARATOR, 0usize, PCWSTR::null())?;
            AppendMenuW(menu, MF_STRING, ID_SETTINGS, &HSTRING::from("首选项"))?;
            AppendMenuW(menu, MF_STRING, ID_RELOAD, &HSTRING::from("重新载入"))?;
            AppendMenuW(menu, MF_STRING, ID_QUIT, &HSTRING::from("退出"))?;

            // 不 SetForegroundWindow 的话菜单会「点别处不消失」——这是 Win32 的老毛病
            let _ = SetForegroundWindow(self.hwnd);
            let picked = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD,
                point.x,
                point.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            self.dispatch(picked.0 as usize);
        }
        Ok(())
    }

    /// 「大小倍率」:三档 + 一条「自定义…」通向窗口。
    /// 帧率与音量没有这一条 —— 那几档就是全部选择。
    ///
    /// # Safety
    /// 返回的 `HMENU` 必须挂到某个会被 `DestroyMenu` 的菜单上,否则泄漏。
    unsafe fn size_menu(&self) -> Result<HMENU> {
        unsafe {
            let root = CreatePopupMenu()?;
            let factor = self.common.px_per_cm / PX_PER_CM_STANDARD;
            steps(root, SIZE_STEPS, nearest_step(SIZE_STEPS, factor), ID_SIZE)?;
            AppendMenuW(root, MF_STRING, ID_CUSTOM_SIZE, &HSTRING::from("自定义…"))?;
            Ok(root)
        }
    }

    /// 菜单 id → 命令。**从大到小按号段门槛反解**,所以号段只能往后加,不能插在中间。
    fn dispatch(&self, id: usize) {
        let control = match id {
            ID_PASSTHROUGH => Control::TogglePassthrough,
            ID_RECALL => Control::Recall,
            ID_MUTE => Control::ToggleMute,
            ID_QUIT => Control::Quit,
            // 「完整配置」落在常用配置页:从子菜单点进来的人本来就在找这几项
            ID_SETTINGS => Control::OpenSettings(SettingsPage::Common),
            ID_RELOAD => Control::Reload,
            ID_CUSTOM_SIZE => Control::OpenSettings(SettingsPage::Common),
            id if id >= ID_FPS => match FPS_STEPS.get(id - ID_FPS) {
                Some((value, _)) => Control::SetFps(*value),
                None => return,
            },
            id if id >= ID_VOLUME => match VOLUME_STEPS.get(id - ID_VOLUME) {
                Some((value, _)) => Control::SetVolume(*value),
                None => return,
            },
            id if id >= ID_SIZE => match SIZE_STEPS.get(id - ID_SIZE) {
                Some((factor, _)) => Control::SetPxPerCm(factor * PX_PER_CM_STANDARD),
                None => return,
            },
            _ => return, // 0 = 什么都没选,或者点在禁用的分组标题上
        };
        if self.sender.send(control).is_err() {
            log::warn!("主循环已退出,托盘命令没送出去");
        }
    }

    fn update_tip(&self) {
        let mut data = self.icon_data();
        data.uFlags = NIF_TIP;
        let tip = match self.pets.len() {
            0 => "rocom-pets · 台上没有".to_string(),
            n => format!("rocom-pets · {n} 只在场"),
        };
        write_tip(&mut data, &tip);
        // SAFETY: data 的 hWnd/uID 与注册时一致;失败只是提示文字没更新。
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    fn icon_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        let data = self.icon_data();
        // SAFETY: 同上;进程退出时把图标从通知区摘掉,否则会留个死图标。
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }
}

/// 一组档位直接追加到 `menu` 上(选中的打勾)。`base + 下标` 就是菜单 id。
///
/// **不套一层子菜单**:这几组就是平铺在「常用配置」下面的,靠禁用项标题分段。
/// `selected` 由调用方算(连续量用 `nearest_step`、整数用 `exact_step`),
/// `None` = 一个都不勾(窗口里调出来的 124% 就是这种)。
///
/// # Safety
/// `menu` 必须是个有效的、之后会被销毁的菜单。
unsafe fn steps<T: Copy>(
    menu: HMENU,
    steps: &[(T, &str)],
    selected: Option<usize>,
    base: usize,
) -> Result<()> {
    unsafe {
        for (index, (_, label)) in steps.iter().enumerate() {
            let flag = if Some(index) == selected {
                MF_CHECKED
            } else {
                MF_STRING
            };
            AppendMenuW(menu, flag, base + index, &HSTRING::from(*label))?;
        }
        Ok(())
    }
}

/// 提示文字要写进定长数组(128 个 UTF-16 单元,含结尾 0)。
fn write_tip(data: &mut NOTIFYICONDATAW, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let n = wide.len().min(data.szTip.len() - 1);
    data.szTip[..n].copy_from_slice(&wide[..n]);
    data.szTip[n] = 0;
}

/// 挂上托盘图标。`hwnd` 是那个隐藏的消息窗口(托盘回调发到它上面)。
pub fn spawn_tray(
    hwnd: HWND,
    sender: Sender<Control>,
    passthrough: bool,
    pets: Vec<String>,
    muted: Option<bool>,
    common: Common,
) -> Result<TrayHandle> {
    let handle = TrayHandle {
        hwnd,
        sender,
        passthrough,
        muted,
        pets,
        common,
    };
    let mut data = handle.icon_data();
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    // 用系统自带图标:自带 .ico 还要处理各种尺寸与浅深主题,收益不大(和 Linux 那边同理)
    // SAFETY: IDI_APPLICATION 是系统内置资源,hInstance 传 None 即可。
    data.hIcon = unsafe { LoadIconW(None, IDI_APPLICATION) }.context("加载图标失败")?;
    write_tip(&mut data, "rocom-pets");
    // SAFETY: data 是刚填好的完整结构;失败返回 false(比如通知区还没起来)。
    unsafe { Shell_NotifyIconW(NIM_ADD, &data) }
        .ok()
        .context("加不上托盘图标(通知区没就绪?)")?;
    Ok(handle)
}

/// 托盘消息要不要交给 [`TrayHandle::on_tray_message`]。
pub fn is_tray_message(message: u32) -> bool {
    message == WM_TRAY
}

/// 从窗口消息里取出命令(`WM_CONTROL` 的 `wparam`;`Play` 的两个参数在 `lparam`)。
pub fn control_from_message(wparam: WPARAM, lparam: LPARAM) -> Option<Control> {
    if wparam.0 as u32 == 7 {
        let packed = lparam.0 as u32;
        return Some(Control::Play(packed >> 16, packed & 0xffff));
    }
    control_of(wparam.0 as u32)
}

/// 给窗口过程用的「已处理」返回值。
pub const HANDLED: LRESULT = LRESULT(0);
