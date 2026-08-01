//! 外部控制入口:命令类型是跨平台共享的,发命令的那一头按平台各写各的。
//!
//! - Linux:ksni 托盘 + XDG GlobalShortcuts 热键 + 自己的 D-Bus 接口 + 信号(见 `linux`);
//! - Windows:Shell_NotifyIcon 托盘 + 给已在跑的实例发窗口消息(见 `windows`)。
//!
//! 事件循环那边只认 [`Control`],不关心命令是哪来的。

/// 能从外部发起的命令。
///
/// **只带数字,不带字符串**:这些值要么进跨进程的窗口消息,要么按值搬进托盘菜单的
/// 回调里。「哪个性格」之类一律用下标,表在 persona.rs。
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// 改全局的每厘米像素数(所有宠物一起变大变小),并写回 config.toml。
    SetPxPerCm(f32),
    /// 改叫声音量 0..1,并写回 config.toml。
    SetVolume(f32),
    /// 改第 `slot` 只的相对大小。
    SetPetScale { slot: usize, scale: f32 },
    /// 改第 `slot` 只的性格(persona::ALL 的下标)。
    SetPetPersona { slot: usize, persona: usize },
    /// 重新读配置与阵容存档,把台上的一切对齐过去。配置窗口存完盘就发这个。
    Reload,
    /// 打开配置窗口。
    OpenSettings,
    /// 退出。
    Quit,
}

/// 菜单里的一只宠物。
#[derive(Debug, Clone)]
pub struct TrayPet {
    /// 当前形态名(菜单上显示的那一行)。
    pub name: String,
    /// 进化链上的形态名,与 `current_form` 的下标对应。
    pub forms: Vec<String>,
    pub current_form: usize,
    /// 相对大小,菜单里回显选中的那一档。
    pub scale: f32,
    /// 性格在 persona::ALL 里的下标。
    pub persona: usize,
}

/// 托盘里单只宠物的「大小」能选的档。**两个平台共用**:Windows 的菜单 id 是按下标编的,
/// 两边档位数量不一样号段就对不上了。
pub const SCALE_STEPS: &[(f32, &str)] = &[
    (0.5, "很小"),
    (0.75, "小"),
    (1.0, "标准"),
    (1.5, "大"),
    (2.0, "很大"),
];

/// 托盘「音量」能选的档。
pub const VOLUME_STEPS: &[(f32, &str)] = &[
    (0.0, "静音"),
    (0.15, "轻"),
    (0.35, "标准"),
    (0.6, "响"),
    (1.0, "最响"),
];

/// 托盘「整体大小」能选的 px_per_cm。80cm 的喵喵在这几档下是 120/160/240/320px 高。
pub const PX_PER_CM_STEPS: &[(f32, &str)] =
    &[(1.5, "小"), (2.0, "标准"), (3.0, "大"), (4.0, "特大")];

/// 这一串档位里离 `value` 最近的下标(菜单回显用)。
/// 不能要求精确相等:值可能是手写进配置的 2.4,或者配置窗口的滑杆拖出来的 1.37。
pub fn nearest_step(steps: &[(f32, &str)], value: f32) -> usize {
    steps
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (a.0 - value).abs().total_cmp(&(b.0 - value).abs()))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// 把自己的 exe 再拉起来一个 `--settings`。托盘点「宠物配置…」走这里。
///
/// **不在本进程里开窗口**:Wayland 后端整个事件循环是 smithay 手写的、Windows 那边是
/// 一个 Win32 消息循环,再塞一个 winit 事件循环进去等于两套循环抢同一个线程。
/// 配置窗口与运行时之间只靠**磁盘上那两份文件** + 一条 `Reload` 通信(见 settings/)。
pub fn open_settings() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log::warn!("找不到自己的可执行文件({e}),开不了配置窗口");
            return;
        }
    };
    match std::process::Command::new(&exe).arg("--settings").spawn() {
        Ok(child) => log::info!("配置窗口已启动(pid {})", child.id()),
        Err(e) => log::warn!(
            "配置窗口起不来({e});也可以直接跑 `{} --settings`",
            exe.display()
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_snap_to_the_nearest() {
        // 配置里手写的 2.4 该落在离它更近的「标准」(2.0)而不是「大」(3.0)
        assert_eq!(nearest_step(PX_PER_CM_STEPS, 2.4), 1);
        assert_eq!(nearest_step(PX_PER_CM_STEPS, 2.0), 1);
        assert_eq!(
            nearest_step(PX_PER_CM_STEPS, 100.0),
            PX_PER_CM_STEPS.len() - 1
        );
        assert_eq!(nearest_step(VOLUME_STEPS, 0.0), 0);
    }

    #[test]
    fn the_standard_step_is_the_default() {
        // 单只的「标准」必须正好是 1.0,否则默认阵容一进菜单就显示成被调过
        assert_eq!(SCALE_STEPS[nearest_step(SCALE_STEPS, 1.0)].0, 1.0);
        // 音量与整体大小的「标准」也要与 config.rs 的默认值对上
        assert_eq!(
            VOLUME_STEPS[nearest_step(VOLUME_STEPS, crate::audio::DEFAULT_VOLUME)].0,
            crate::audio::DEFAULT_VOLUME
        );
    }
}
