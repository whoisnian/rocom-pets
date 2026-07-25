//! 平台窗口层:每个显示器一个透明置顶表面。
//!
//! 只支持两个后端(见 docs/design.md §0):KDE Plasma Wayland 的 wlr-layer-shell,
//! 与 Windows 的 layered 窗口 + DirectComposition。所有状态逻辑在 `crate::stage`,
//! 后端只负责造表面、收事件、提交帧、设输入区。

#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "windows")]
mod windows;

pub fn run() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return wayland::run();
    #[cfg(target_os = "windows")]
    return windows::run();
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    anyhow::bail!("不支持的平台:只支持 KDE Plasma Wayland 与 Windows");
}
