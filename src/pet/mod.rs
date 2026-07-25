//! 宠物:从包里的 glb 加载模型/骨架/动作,采样动画,用 toon 管线渲染。

pub mod anim;
pub mod gpu;
pub mod model;

pub use anim::Player;
pub use gpu::{PetGpu, orthographic_view};
pub use model::Model;
