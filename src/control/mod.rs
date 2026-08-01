//! 外部控制入口:命令类型是跨平台共享的,发命令的那一头按平台各写各的。
//!
//! - Linux:ksni 托盘 + XDG GlobalShortcuts 热键 + 自己的 D-Bus 接口 + 信号(见 `linux`);
//! - Windows:Shell_NotifyIcon 托盘 + 给已在跑的实例发窗口消息(见 `windows`)。
//!
//! 事件循环那边只认 [`Control`],不关心命令是哪来的。

/// 能从外部发起的命令。
///
/// Windows 的托盘菜单还没有「加一只/撤下/切形态」那几项(见 platform/windows.rs 的说明),
/// 那三个变体在那边构造不到 —— 不是死代码,是那个后端还没做到。
#[cfg_attr(target_os = "windows", allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// 切换鼠标穿透。
    TogglePassthrough,
    /// 切换叫声静音。
    ToggleMute,
    /// 把宠物召回到屏幕中间(它跑到边角或看不见时用)。
    Recall,
    /// 把阵容里第 `slot` 只切到进化链上的第 `form` 个形态。
    SwitchForm { slot: usize, form: usize },
    /// 从可选包列表里加一只(下标)。
    AddPet(usize),
    /// 撤下阵容里的第几只(下标)。
    RemovePet(usize),
    /// 退出。
    Quit,
}

/// 菜单里的一只宠物。
#[cfg_attr(target_os = "windows", allow(dead_code))]
#[derive(Debug, Clone)]
pub struct TrayPet {
    /// 当前形态名(菜单上显示的那一行)。
    pub name: String,
    /// 进化链上的形态名,与 `current_form` 的下标对应。
    pub forms: Vec<String>,
    pub current_form: usize,
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{TrayHandle, send_command, serve_dbus, spawn_hotkey, spawn_tray};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{
    CONTROL_CLASS, HANDLED, TrayHandle, WM_CONTROL, control_from_message, is_tray_message,
    send_command, spawn_tray,
};
