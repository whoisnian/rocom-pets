//! 宠物:从包里的 glb 加载模型/骨架/动作,采样动画,用 toon 管线渲染。

pub mod anim;
pub mod glassy;
mod glassy_table;
pub mod gpu;
pub mod mask;
pub mod model;
pub mod target;

pub use anim::Player;
pub use glassy::{Glassy, Mutation};
pub use gpu::{
    FrameParams, PetGpu, desktop_outline_scale, framing_radius, orbit_rotation, orbit_view,
    orthographic_view,
};
pub use model::Model;
