//! 平台窗口层:每个显示器一个透明置顶表面。
//!
//! 只支持两个后端(见 docs/design.md §0):KDE Plasma Wayland 的 wlr-layer-shell,
//! 与 Windows 的 layered 窗口 + DirectComposition。所有状态逻辑在 `crate::stage`,
//! 后端只负责造表面、收事件、提交帧、设输入区。

use std::path::PathBuf;

mod shared;

pub use shared::{PetOptions, SCALE_RANGE};

#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "windows")]
mod windows;

/// 启动阵容里的一只:已经读好的包 + 要用哪个形态 + 这一只自己的选项。
pub struct StartupPet {
    pub pack: crate::pack::Pack,
    /// 形态资产名或中文名;None = 包里第一个(链首)。
    pub form: Option<String>,
    pub options: PetOptions,
}

/// 起 stage 时的配置。阵容为空时退回调试精灵(S1 的验收对象)。
pub struct Options {
    /// 启动阵容(命令行 / 阵容存档 / 配置,由 main 定优先级);空 = 调试精灵。
    pub pets: Vec<StartupPet>,
    /// 包目录:托盘的「加一只」从这里列包,存阵容时也按它决定存名字还是存路径。
    pub packs_dir: Option<PathBuf>,
    /// 阵容存档路径(托盘改动后写回);None = 定不出位置,只在内存里。
    pub roster_path: Option<PathBuf>,
    /// 配置文件路径。托盘改音量/整体大小时写回它,`Reload` 时重读它。
    pub config_path: Option<PathBuf>,
    /// 每厘米多少逻辑像素:宠物的屏幕高度 = height_cm × 这个值。
    pub px_per_cm: f32,
    /// 启动就开鼠标穿透。
    pub passthrough: bool,
    /// 起托盘图标(没有托盘宿主的桌面上失败也不致命)。
    pub tray: bool,
    /// 全局热键的建议按键;None = 不申请。
    pub hotkey: Option<String>,
    /// 叫声音量 0..1;0 = 干脆不开音频设备。
    pub volume: f32,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return wayland::run(options);
    #[cfg(target_os = "windows")]
    return windows::run(options);
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = options;
        anyhow::bail!("不支持的平台:只支持 KDE Plasma Wayland 与 Windows");
    }
}
