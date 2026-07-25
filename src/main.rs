//! Phase 0 spike S1:平台层验证。见 docs/spike-s1.md。

mod platform;
mod render;
mod sprite;
mod stage;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    platform::run()
}
