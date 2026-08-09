//! rocom-pets 的库部分。
//!
//! **为什么是 lib 而不是只有一个 bin**:同一份渲染与行为代码要出两个东西 ——
//! 桌面上的 `rocom-pets`(bin),和下载站上的预览(`cdylib`,见 `web`)。
//! 拆之前那 11000 行核心藏在 bin 里,只能靠 `#[path]` 从别处挂进来。
//!
//! 模块分两类:
//!
//! - **跟平台无关的**(动画、渲染、包格式、行为、性格)—— 两个目标都编,
//!   `cargo check --target wasm32-unknown-unknown` 一行不改就过;
//! - **平台外壳**(窗口、托盘、配置窗口、离屏渲染)—— 只在原生目标下编。
//!   它们依赖 wayland/win32/eframe/ksni,那些在 wasm 上根本不存在。
//!
//! 分界线就是下面那两组 `cfg`。往「无关」那一组里加依赖之前先想一下:
//! 加进去的东西得在浏览器里也说得通。

pub mod act;
pub mod assets;
pub mod audio;
pub mod config;
pub mod pack;
pub mod persona;
pub mod pet;
pub mod sprite;
pub mod stage;

#[cfg(not(target_arch = "wasm32"))]
pub mod control;
#[cfg(not(target_arch = "wasm32"))]
pub mod fatal;
#[cfg(not(target_arch = "wasm32"))]
pub mod offscreen;
#[cfg(not(target_arch = "wasm32"))]
pub mod pack_list;
#[cfg(not(target_arch = "wasm32"))]
pub mod platform;
#[cfg(not(target_arch = "wasm32"))]
pub mod render;
#[cfg(not(target_arch = "wasm32"))]
pub mod roster;
#[cfg(not(target_arch = "wasm32"))]
pub mod settings;

#[cfg(target_arch = "wasm32")]
pub mod web;
