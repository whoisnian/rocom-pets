//! 外部控制入口:命令类型是跨平台共享的,发命令的那一头按平台各写各的。
//!
//! - Linux:ksni 托盘 + 自己的 D-Bus 接口 + 信号(见 `linux`);
//! - Windows:Shell_NotifyIcon 托盘 + 给已在跑的实例发窗口消息(见 `windows`)。
//!
//! 事件循环那边只认 [`Control`],不关心命令是哪来的。
//!
//! ## 托盘只放「菜单表达得了」的东西
//!
//! DBusMenu 与 Win32 菜单能表达的就那么几样:文字、勾选、单选、子菜单、分隔线、
//! 禁用项(拿来当分组标题)。**没有滑块**,所以整体大小与音量在这里降级成几个档位,
//! 连续值(124%、37%)只在配置窗口里存在。
//!
//! 加/撤宠物、切形态、改某一只的性格也不在托盘里了:那些都要先列出在场阵容再逐只展开,
//! 菜单一深就没法用。托盘上留一条「完整配置」直接开窗口。于是这里的命令只剩
//! **全局性的、一下就能点完的**那几条。

/// 能从外部发起的命令。
///
/// **只带数字,不带字符串**:这些值要么进跨进程的窗口消息,要么按值搬进托盘菜单的回调里。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// 切换点击穿透。
    TogglePassthrough,
    /// 把宠物召回到屏幕中间(它跑到边角或看不见时用)。
    Recall,
    /// 切换叫声静音。
    ToggleMute,
    /// 改目标帧率,并写回 config.toml。
    SetFps(u32),
    /// 改全局的每厘米像素数(所有宠物一起变大变小),并写回 config.toml。
    SetPxPerCm(f32),
    /// 改叫声音量 0..1,并写回 config.toml。
    SetVolume(f32),
    /// 让第 `slot` 只宠物播一段动作([`crate::stage::RUNTIME_CLIPS`] 的下标)。
    /// 配置窗口那张动作表点出来的。
    Play(u32, u32),
    /// 重新读配置与阵容存档,把台上的一切对齐过去。配置窗口存完盘就发这个。
    Reload,
    /// 打开配置窗口,并直接落在某一页。
    OpenSettings(SettingsPage),
    /// 退出。
    Quit,
}

/// 配置窗口的三页。托盘上那两个入口(「完整配置」与「大小倍率 ▸ 自定义…」)
/// **都落在常用配置页**:从托盘点进来的人本来就在找那几项,只是想要更精确的那一版。
/// `Pets`/`Packs` 留给命令行(`--settings --page pets`)与窗口里的导航。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Pets,
    Packs,
    Common,
}

impl SettingsPage {
    /// 命令行 `--settings --page <这个>`。跨进程只能传字符串。
    pub fn flag(self) -> &'static str {
        match self {
            Self::Pets => "pets",
            Self::Packs => "packs",
            Self::Common => "common",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pets" => Some(Self::Pets),
            "packs" => Some(Self::Packs),
            "common" => Some(Self::Common),
            _ => None,
        }
    }
}

/// 「常用配置」子菜单里那三组要回显的当前值。
///
/// **打包传**:它们总是一起变(托盘点一下、配置窗口存一次盘都是整份对齐),
/// 拆成三个参数的话两个后端的 `spawn_tray` 都要排到第八个形参。
#[derive(Debug, Clone, Copy)]
pub struct Common {
    /// 目标帧率。
    pub fps: u32,
    /// 每厘米多少逻辑像素(菜单里显示成百分比)。
    pub px_per_cm: f32,
    /// 叫声音量 0..1。
    pub volume: f32,
}

/// 100% 整体大小对应的 px_per_cm。菜单与配置窗口都把整体大小显示成**百分比**
/// (100% = 这个值),因为「每厘米几个像素」对着屏幕想象不出多大。
pub const PX_PER_CM_STANDARD: f32 = 2.0;

/// 托盘「大小倍率」能选的三档(后面还跟一条「自定义…」)。
///
/// 值是**倍率**,真正写进配置的是 `倍率 × PX_PER_CM_STANDARD`。
pub const SIZE_STEPS: &[(f32, &str)] = &[(0.5, "50%"), (1.0, "100%"), (1.5, "150%")];

/// 托盘「叫声音量」能选的档。百分比不带小数(见设计稿的数值规范)。
pub const VOLUME_STEPS: &[(f32, &str)] =
    &[(0.0, "静音"), (0.3, "30%"), (0.6, "60%"), (1.0, "100%")];

/// 托盘「帧率设置」能选的档。**目标帧率**:台上在干什么都按这个推进,
/// 不再有「没动就降频」那回事(见 `Stage::tick_interval`)。
///
/// 三档:20 给「就想让它少占点 CPU」的人,30 是默认,60 给「看着不够顺」的人。
/// 中间那些值肉眼分不出来,而每多一档就多一次「该选哪个」的犹豫。
pub const FPS_STEPS: &[(u32, &str)] = &[(20, "20 帧/秒"), (30, "30 帧/秒"), (60, "60 帧/秒")];

/// 这一串档位里离 `value` 最近的下标(菜单回显用)。
///
/// 不能要求精确相等:值可能是配置窗口里调出来的 124%。**离得太远就谁都不选中** ——
/// 124% 落在 100% 与 150% 之间,硬勾一个会让人以为菜单里就是那个值。
pub fn nearest_step(steps: &[(f32, &str)], value: f32) -> Option<usize> {
    let (index, (step, _)) = steps
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (a.0 - value).abs().total_cmp(&(b.0 - value).abs()))?;
    ((step - value).abs() < 0.02).then_some(index)
}

/// 整数档位(帧率)的回显:**要求正好相等**。这类值没有滑杆,
/// 只可能是从这几档里点出来的,或者用户手写进 config.toml —— 手写了 45 就该一个都不勾。
pub fn exact_step(steps: &[(u32, &str)], value: u32) -> Option<usize> {
    steps.iter().position(|(step, _)| *step == value)
}

/// 桌宠在不在跑。配置窗口的状态栏要说这件事 —— 「改了没生效」和
/// 「改了但桌宠根本没开」是两回事,不说清楚会让人以为是坏了。
///
/// **只查不发**:发一条命令过去也能试出来,但那会有副作用。
#[cfg(target_os = "linux")]
pub fn is_running() -> bool {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&connection) else {
        return false;
    };
    let Ok(name) = linux::DBUS_NAME.try_into() else {
        return false;
    };
    dbus.name_has_owner(name).unwrap_or(false)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn is_running() -> bool {
    false
}

/// 配置窗口用来「我在」的具名事件(Windows)。桌宠退出时 `SetEvent`,那边等到就关窗口。
/// **两边必须是同一个字符串**,所以放在这儿共享。
#[cfg(target_os = "windows")]
pub const SETTINGS_QUIT_EVENT: &str = "Local\\rocom-pets-settings-quit";

/// 把自己的 exe 再拉起来一个 `--settings`。托盘点「完整配置」走这里。
///
/// **不在本进程里开窗口**:Wayland 后端整个事件循环是 smithay 手写的、Windows 那边是
/// 一个 Win32 消息循环,再塞一个 winit 事件循环进去等于两套循环抢同一个线程。
/// 配置窗口与运行时之间只靠**磁盘上那两份文件** + 一条 `Reload` 通信(见 settings/)。
pub fn open_settings(page: SettingsPage) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log::warn!("找不到自己的可执行文件({e}),开不了配置窗口");
            return;
        }
    };
    let spawned = std::process::Command::new(&exe)
        .arg("--settings")
        .arg("--page")
        .arg(page.flag())
        .spawn();
    match spawned {
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
pub use linux::{TrayHandle, close_settings, play, send_command, serve_dbus, spawn_tray};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{
    CONTROL_CLASS, HANDLED, TrayHandle, WM_CONTROL, close_settings, control_from_message,
    is_running, is_tray_message, play, send_command, spawn_tray,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_snap_only_when_close() {
        assert_eq!(nearest_step(SIZE_STEPS, 1.0), Some(1));
        assert_eq!(nearest_step(SIZE_STEPS, 0.5), Some(0));
        // 配置窗口里调出来的 124% 不属于任何一档 —— 菜单里就该一个都不勾,
        // 否则会显示成 100%,而桌面上明明不是原大
        assert_eq!(nearest_step(SIZE_STEPS, 1.24), None);
        assert_eq!(nearest_step(VOLUME_STEPS, 0.37), None);
        assert_eq!(nearest_step(VOLUME_STEPS, 0.3), Some(1));
    }

    #[test]
    fn the_hundred_percent_size_matches_the_config_default() {
        // 100% 必须正好等于配置里的默认 px_per_cm,
        // 否则默认配置一进菜单就显示成被调过
        assert_eq!(SIZE_STEPS[1].0, 1.0);
        assert_eq!(PX_PER_CM_STANDARD, 2.0);
    }

    #[test]
    fn the_fps_steps_want_an_exact_match() {
        assert_eq!(exact_step(FPS_STEPS, 20), Some(0));
        assert_eq!(exact_step(FPS_STEPS, 60), Some(2));
        // 手写进配置的 45 不属于任何一档
        assert_eq!(exact_step(FPS_STEPS, 45), None);
        // 默认值必须正好是某一档,否则默认配置一进菜单就一个都不勾
        assert_eq!(
            exact_step(FPS_STEPS, crate::config::DEFAULT_FPS),
            Some(1),
            "默认帧率该落在中间那一档"
        );
        // 每一档都得在允许区间里,否则点了会被 clamp 成别的值
        for (fps, _) in FPS_STEPS {
            assert!(crate::config::FPS_RANGE.contains(fps), "{fps} 超出允许区间");
        }
    }

    #[test]
    fn settings_pages_round_trip_through_the_command_line() {
        for page in [
            SettingsPage::Pets,
            SettingsPage::Packs,
            SettingsPage::Common,
        ] {
            assert_eq!(SettingsPage::parse(page.flag()), Some(page));
        }
        assert_eq!(SettingsPage::parse("nope"), None);
    }
}
