//! Windows 的外部控制:通知区(托盘)图标 + 给已在跑的实例发窗口消息。
//!
//! 与 Linux 那边的差别:
//! - 托盘走 `Shell_NotifyIconW`,不是 StatusNotifier;菜单是**点的时候现建**的 `HMENU`,
//!   不像 ksni 那样声明一棵树。
//! - 没有 GlobalShortcuts portal 那种「向桌面申请热键」的机制。要全局热键得
//!   `RegisterHotKey` 自己占一个组合键 —— 那是**抢**而不是申请,冲突了别人就用不了,
//!   所以这里不做;`rocom-pets --toggle-passthrough` 仍然可用(见下),
//!   要热键就在快捷方式上挂「快捷键」或用任何一个第三方热键工具调它。
//! - 「通知已在跑的实例」不走 D-Bus:按窗口类名找到那个隐藏的消息窗口,`PostMessage` 过去。
//!
//! 托盘图标本身实机验过(能出现、四项菜单能点);**加一只/撤下/切形态那几项是后补的,
//! 还没验**。见 docs/design.md §9 Phase 8。

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

use super::{Control, TrayPet};

/// 隐藏的消息窗口的类名。`--toggle-passthrough` 这类命令靠它找到在跑的实例。
pub const CONTROL_CLASS: &str = "rocom-pets-control";

/// 托盘图标回调消息(鼠标点在图标上时发到我们的消息窗口)。
pub const WM_TRAY: u32 = WM_APP + 1;
/// 外部实例发来的命令,`wparam` 是 [`Control`] 的编号。
pub const WM_CONTROL: u32 = WM_APP + 2;

// 菜单项 id。0 是「没选」,所以从 100 起;后面几段各占一个号段。
// **`TrackPopupMenu` 只把这个数字还回来**,所以「点了哪一项」要能从数字反解出来。
const ID_PASSTHROUGH: usize = 100;
const ID_RECALL: usize = 101;
const ID_MUTE: usize = 102;
const ID_QUIT: usize = 103;
/// 撤下第 n 只:`ID_REMOVE + slot`。
const ID_REMOVE: usize = 2000;
/// 把第 n 只切到第 f 个形态:`ID_FORM + slot * FORMS_PER_PET + f`。
/// 进化链最长也就几阶,留 64 个号绰绰有余。
const ID_FORM: usize = 3000;
const FORMS_PER_PET: usize = 64;
/// 加第 n 个包:`ID_ADD + index`。包目录里有五百多个,号段要够宽
/// (菜单 id 实际只有 16 位可用,10000 + 600 还远没到 65535)。
const ID_ADD: usize = 10000;
/// 「加一只」菜单一屏最多列这么多个包;超了就切成一段一段的子菜单。
/// 全库 539 个包平铺出来根本没法用,而按首字分组的话中文名会分出上百个组。
const ADD_CHUNK: usize = 24;

/// 命令 ↔ 编号。跨进程只能传数字,所以要一张明确的表(别拿 `as` 硬转:
/// 带字段的变体转不了,而且加变体时静默改值)。
fn code_of(control: Control) -> Option<u32> {
    Some(match control {
        Control::TogglePassthrough => 1,
        Control::ToggleMute => 2,
        Control::Recall => 3,
        Control::Quit => 4,
        // 带下标的那几个只在本进程的托盘里发,不跨进程
        Control::SwitchForm { .. } | Control::AddPet(_) | Control::RemovePet(_) => return None,
    })
}

pub fn control_of(code: u32) -> Option<Control> {
    Some(match code {
        1 => Control::TogglePassthrough,
        2 => Control::ToggleMute,
        3 => Control::Recall,
        4 => Control::Quit,
        _ => return None,
    })
}

/// 通知已在跑的实例执行某个命令。
pub fn send_command(control: Control) -> Result<()> {
    let Some(code) = code_of(control) else {
        bail!("加/撤宠物与换形态请用托盘菜单");
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
    voice: Option<bool>,
    pets: Vec<TrayPet>,
    /// 包目录里能加的包名,下标即 [`Control::AddPet`] 的参数。
    available: Vec<String>,
}

impl TrayHandle {
    pub fn set_passthrough(&mut self, passthrough: bool) {
        self.passthrough = passthrough;
    }

    pub fn set_voice(&mut self, on: bool) {
        self.voice = Some(on);
    }

    /// 阵容变了。Windows 这边菜单是点的时候现建的,存下来就行。
    pub fn set_roster(&mut self, pets: Vec<TrayPet>) {
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
                &HSTRING::from("鼠标穿透"),
            )?;
            AppendMenuW(menu, MF_STRING, ID_RECALL, &HSTRING::from("召回宠物"))?;
            if let Some(on) = self.voice {
                AppendMenuW(menu, checked(on), ID_MUTE, &HSTRING::from("叫声"))?;
            }
            AppendMenuW(menu, MF_SEPARATOR, 0usize, PCWSTR::null())?;

            // 在场的每一只一个子菜单:换形态 + 撤下(与 Linux 托盘同构)
            for (slot, pet) in self.pets.iter().enumerate() {
                let sub = CreatePopupMenu()?;
                if pet.forms.len() > 1 {
                    for (index, name) in pet.forms.iter().enumerate().take(FORMS_PER_PET) {
                        AppendMenuW(
                            sub,
                            checked(index == pet.current_form),
                            ID_FORM + slot * FORMS_PER_PET + index,
                            &HSTRING::from(name.as_str()),
                        )?;
                    }
                    AppendMenuW(sub, MF_SEPARATOR, 0usize, PCWSTR::null())?;
                }
                AppendMenuW(sub, MF_STRING, ID_REMOVE + slot, &HSTRING::from("撤下"))?;
                AppendMenuW(
                    menu,
                    MF_POPUP,
                    sub.0 as usize,
                    &HSTRING::from(pet.name.as_str()),
                )?;
            }

            if !self.available.is_empty() {
                let add = self.add_menu()?;
                AppendMenuW(menu, MF_POPUP, add.0 as usize, &HSTRING::from("加一只"))?;
            }

            AppendMenuW(menu, MF_SEPARATOR, 0usize, PCWSTR::null())?;
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

    /// 「加一只」的内容:包少就平铺,包多就按名字切段(与 Linux 托盘同一套做法)。
    /// 段的标签取首尾两个名字,于是能像翻通讯录一样找。
    ///
    /// # Safety
    /// 返回的 `HMENU` 必须挂到某个会被 `DestroyMenu` 的菜单上,否则泄漏。
    unsafe fn add_menu(&self) -> Result<HMENU> {
        unsafe {
            let root = CreatePopupMenu()?;
            if self.available.len() <= ADD_CHUNK {
                for (index, name) in self.available.iter().enumerate() {
                    AppendMenuW(
                        root,
                        MF_STRING,
                        ID_ADD + index,
                        &HSTRING::from(name.as_str()),
                    )?;
                }
                return Ok(root);
            }
            for (chunk, names) in self.available.chunks(ADD_CHUNK).enumerate() {
                let sub = CreatePopupMenu()?;
                let base = chunk * ADD_CHUNK;
                for (i, name) in names.iter().enumerate() {
                    AppendMenuW(
                        sub,
                        MF_STRING,
                        ID_ADD + base + i,
                        &HSTRING::from(name.as_str()),
                    )?;
                }
                let label = format!("{} … {}", names[0], names[names.len() - 1]);
                AppendMenuW(root, MF_POPUP, sub.0 as usize, &HSTRING::from(label))?;
            }
            Ok(root)
        }
    }

    fn dispatch(&self, id: usize) {
        let control = match id {
            ID_PASSTHROUGH => Control::TogglePassthrough,
            ID_RECALL => Control::Recall,
            ID_MUTE => Control::ToggleMute,
            ID_QUIT => Control::Quit,
            id if id >= ID_ADD => Control::AddPet(id - ID_ADD),
            id if id >= ID_FORM => Control::SwitchForm {
                slot: (id - ID_FORM) / FORMS_PER_PET,
                form: (id - ID_FORM) % FORMS_PER_PET,
            },
            id if id >= ID_REMOVE => Control::RemovePet(id - ID_REMOVE),
            _ => return, // 0 = 什么都没选
        };
        if self.sender.send(control).is_err() {
            log::warn!("主循环已退出,托盘命令没送出去");
        }
    }

    fn update_tip(&self) {
        let mut data = self.icon_data();
        data.uFlags = NIF_TIP;
        let tip = match self.pets.len() {
            0 => "rocom-pets".to_string(),
            1 => format!("rocom-pets — {}", self.pets[0].name),
            n => format!("rocom-pets — {} 等 {n} 只", self.pets[0].name),
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
    pets: Vec<TrayPet>,
    available: Vec<String>,
    voice: Option<bool>,
) -> Result<TrayHandle> {
    let handle = TrayHandle {
        hwnd,
        sender,
        passthrough,
        voice,
        pets,
        available,
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

/// 从窗口消息里取出命令(`WM_CONTROL` 的 `wparam`)。
pub fn control_from_message(wparam: WPARAM) -> Option<Control> {
    control_of(wparam.0 as u32)
}

/// 给窗口过程用的「已处理」返回值。
pub const HANDLED: LRESULT = LRESULT(0);
