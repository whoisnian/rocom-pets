//! Windows 后端:每个显示器一个 layered 置顶窗口 + DirectComposition。
//!
//! 尚未实现(S1 的 Windows 半边)。既定路线:
//! - `CreateWindowExW` 带 `WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW |
//!   WS_EX_NOACTIVATE`,不用 `UpdateLayeredWindow`(那条 GDI 路线不适合 GPU 渲染);
//! - 自己建 `IDCompositionDevice` → target → visual,把 visual 指针交给
//!   `wgpu::SurfaceTargetUnsafe::CompositionVisual`(wgpu 30 原生支持,这是 N1 的答案);
//! - 命中穿透:`WM_NCHITTEST` 返回 `HTTRANSPARENT`;全局穿透 = 加 `WS_EX_TRANSPARENT`;
//! - 输入区没有 Wayland 那种 region 概念,用 `crate::stage::Stage::hit_test` 逐点判定即可,
//!   `Sprite::coverage_rects` 那套矩形近似在这边用不上。

pub fn run(_options: &super::Options) -> anyhow::Result<()> {
    anyhow::bail!("Windows 后端还没实现,见 docs/spike-s1.md")
}
