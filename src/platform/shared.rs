//! 两个后端共用的那一半:资产缓存、把 manifest 换算成角色、阵容与托盘状态。
//!
//! 划界的依据是**「碰不碰窗口系统」**:这里一句 Wayland/Win32 都没有,后端那边只剩
//! 造窗口、收事件、提交帧、设输入区。
//!
//! 抽出来的时机是故意压后的:Windows 后端实机验通(2026-08-01)之前,先抽公共层是
//! 本末倒置 —— 那时连它能不能跑都不知道。验通之后再抽,才知道哪些是真共用、
//! 哪些是某个平台的特例。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::control::TrayPet;
use crate::pack::{Form, Pack};
use crate::pet::{Model, PetGpu};
use crate::render::Gpu;
use crate::roster::{Roster, Slot};
use crate::stage::{Actor, PetActor, PetBuild, VoiceBank};

/// 离屏画布的取景余量。伸展类动作已经算进 `Model::motion_bounds`,这里只给描边外扩与
/// 边缘光留一点边。**两个后端必须用同一个值**:它同时是 `view_proj` 的入参。
pub const CANVAS_PADDING: f32 = 1.15;

/// 阵容里的一只:包(整条进化链都在,供切形态)+ 当前形态下标。
pub struct Member {
    pub pack: Pack,
    pub form: usize,
}

impl Member {
    pub fn form(&self) -> &Form {
        &self.pack.forms[self.form]
    }
}

/// 按形态共享的资产。
///
/// 三张表同一把键(glb 路径 = 包 + 形态),同一套「没人用就清掉」:
/// `Arc::strong_count == 1` 说明只剩缓存自己持有。不清的话每访问一个形态就永久多占几 MB
/// (模型一两百 MB、叫声几 MB)。
#[derive(Default)]
pub struct Assets {
    models: HashMap<PathBuf, Arc<Model>>,
    pet_gpus: HashMap<PathBuf, Arc<PetGpu>>,
    voices: HashMap<PathBuf, Arc<VoiceBank>>,
}

impl Assets {
    /// 取这个形态的模型:缓存里有就直接共享,没有才读盘。
    pub fn model(&mut self, form: &Form) -> Result<Arc<Model>> {
        if let Some(model) = self.models.get(&form.model) {
            return Ok(Arc::clone(model));
        }
        let model = Arc::new(Model::load(&form.model, &form.materials)?);
        self.models
            .retain(|_, cached| Arc::strong_count(cached) > 1);
        self.models.insert(form.model.clone(), Arc::clone(&model));
        Ok(model)
    }

    /// 取这个形态的 GPU 资源(管线/顶点缓冲/贴图)。每实体独立的只有画布。
    pub fn pet_gpu(&mut self, gpu: &Gpu, model: &Arc<Model>) -> Result<Arc<PetGpu>> {
        if let Some(cached) = self.pet_gpus.get(&model.source) {
            return Ok(Arc::clone(cached));
        }
        let built = Arc::new(PetGpu::new(&gpu.device, &gpu.queue, model, gpu.format())?);
        self.pet_gpus
            .retain(|_, cached| Arc::strong_count(cached) > 1);
        self.pet_gpus
            .insert(model.source.clone(), Arc::clone(&built));
        Ok(built)
    }

    /// 取这个形态的叫声库。`with_audio = false`(没声卡或音量为 0)时直接不读。
    /// 读不到文件只警告 —— 少一段叫声不该让宠物上不了台。
    pub fn voice(&mut self, form: &Form, with_audio: bool) -> Option<Arc<VoiceBank>> {
        if !with_audio {
            return None;
        }
        if let Some(bank) = self.voices.get(&form.model) {
            return Some(Arc::clone(bank));
        }
        let Some(voice) = form.voice.as_ref() else {
            log::debug!("{} 没有叫声", form.name);
            return None;
        };
        // **加载时就解码**:每次叫都重解是白费,而且直接把解码器丢进 mixer 出不了声
        // (见 audio.rs 的 `Pcm`)
        let mut clips = HashMap::new();
        for (key, clip) in &voice.clips {
            match crate::audio::decode(&clip.path) {
                Ok(pcm) => {
                    if pcm.peak() <= 0.0 {
                        log::warn!("叫声 {:?} 解出来是静音的", clip.path);
                    }
                    clips.insert(key.clone(), Arc::new(pcm));
                }
                Err(e) => log::warn!("叫声读不了({e:#})"),
            }
        }
        if clips.is_empty() {
            return None;
        }
        let bank = Arc::new(VoiceBank {
            clips,
            cents_low: voice.cents_low,
            cents_high: voice.cents_high,
        });
        log::debug!("{} 的叫声 {} 段", form.name, bank.clips.len());
        self.voices
            .retain(|_, cached| Arc::strong_count(cached) > 1);
        self.voices.insert(form.model.clone(), Arc::clone(&bank));
        Some(bank)
    }

    /// 撤一只/切形态之后清掉没人用的。不清的话它的网格与贴图会一直占着。
    pub fn prune(&mut self) {
        self.models
            .retain(|_, cached| Arc::strong_count(cached) > 1);
        self.pet_gpus
            .retain(|_, cached| Arc::strong_count(cached) > 1);
        self.voices
            .retain(|_, cached| Arc::strong_count(cached) > 1);
    }

    /// 把 manifest 里的厘米单位换成屏幕像素,算出画布尺寸与脚底位置。
    ///
    /// `salt` 只是随机种子的调味料(平台层拿 stage 下标传进来),让同物种多实体不同步。
    pub fn build_actor(
        &mut self,
        form: &Form,
        px_per_cm: f32,
        with_audio: bool,
        salt: u64,
    ) -> Result<Actor> {
        let model = self.model(form)?;
        // 两个包围盒各管一件事:**尺寸**按绑定姿势(站姿高度不能随动作变),
        // **取景**按动作包围盒(否则伸手/张翅/跳跃会被画布裁掉,见 model.rs 的 motion_bounds)
        let stand = model.bounds.1 - model.bounds.0;
        let (frame_min, frame_max) = model.motion_bounds;
        let frame_extent = frame_max - frame_min;
        let frame_center = (frame_min + frame_max) * 0.5;
        let height_px = form.height_cm * form.scale * px_per_cm;
        // 画布是方的,取景按动作包围盒最长边;正交框半径 = 最长边/2 × 余量
        let longest = frame_extent
            .x
            .max(frame_extent.y)
            .max(frame_extent.z)
            .max(1e-4);
        let radius = longest * 0.5 * CANVAS_PADDING;
        // 画布边长 = 正交框的 2×半径(米),按「站姿高 ↔ height_px」的比例换成像素
        let side = (height_px * 2.0 * radius / stand.y.max(1e-4))
            .round()
            .max(16.0);
        // 脚底 = 绑定姿势下沿在正交框里的 NDC 位置(框心是动作包围盒中心,不一定等于站姿中心)
        let ndc_bottom = (model.bounds.0.y - frame_center.y) / radius;
        let foot_offset = (1.0 - ndc_bottom) * 0.5 * side;

        // 走路速度优先用动画自带位移反推的值(见 spike-s3.md),没有就给个常速
        let walk_speed_cm = form
            .clip("Walk")
            .map(|c| c.speed_cm_s)
            .filter(|v| *v > 1.0)
            .unwrap_or(40.0);
        // 跑速同理,但**必须钳制**:全库反推值中位 417cm/s、p90 563、最高 1125
        // (魔力猫那只 7.5m/s),照搬会让宠物一瞬间横穿屏幕。按走速的倍数夹 ——
        // 保留「这只跑起来相对更快」的个性,又不至于离谱。
        let run_speed_cm = form
            .clip("Run")
            .map(|c| c.speed_cm_s)
            .filter(|v| *v > 1.0)
            .unwrap_or(walk_speed_cm * 2.0)
            .clamp(walk_speed_cm * 1.2, walk_speed_cm * 3.0);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5eed)
            ^ (salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));

        log::info!(
            "  {} 屏幕高 {:.0}px(画布 {}px,脚底 {:.0}px),走速 {:.0}cm/s,跑速 {:.0}cm/s",
            form.name,
            height_px,
            side as u32,
            foot_offset,
            walk_speed_cm,
            run_speed_cm
        );
        let voice = self.voice(form, with_audio);
        Ok(Actor::Pet(PetActor::new(PetBuild {
            model,
            size: (side as u32, side as u32),
            foot_offset,
            // 本体高度(≠ 画布边长:画布带取景余量)。距离阈值按它换算成「身位」
            body_px: height_px,
            walk_speed: walk_speed_cm * px_per_cm,
            run_speed: run_speed_cm * px_per_cm,
            form_id: form.id,
            voice,
            seed,
        })))
    }
}

/// 托盘菜单要的那份阵容快照。
pub fn tray_pets(roster: &[Member]) -> Vec<TrayPet> {
    roster
        .iter()
        .map(|m| TrayPet {
            name: m.form().name.clone(),
            forms: m.pack.forms.iter().map(|f| f.name.clone()).collect(),
            current_form: m.form,
        })
        .collect()
}

/// 把阵容写回存档。存**包名**而不是路径 —— 包目录整个搬走时阵容还认得出来;
/// 只有包不在包目录里(`--pack /some/where`)才存绝对路径。
pub fn save_roster(roster: &[Member], packs_dir: Option<&Path>, path: Option<&Path>) {
    let Some(path) = path else {
        return;
    };
    let saved = Roster {
        pets: roster
            .iter()
            .map(|m| {
                let in_packs_dir = packs_dir.is_some_and(|dir| m.pack.dir.parent() == Some(dir));
                let pack = match (in_packs_dir, m.pack.dir.file_name()) {
                    (true, Some(name)) => name.to_string_lossy().into_owned(),
                    _ => m.pack.dir.to_string_lossy().into_owned(),
                };
                Slot {
                    pack,
                    form: Some(m.form().asset.clone()),
                }
            })
            .collect(),
    };
    if let Err(e) = saved.save(path) {
        log::warn!("阵容没存上({e:#});下次启动会回到上一次的阵容");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_start_empty_and_prune_is_safe_on_empty() {
        // `prune` 会在「撤掉最后一只」之后被调到,那时三张表可能都是空的
        let mut assets = Assets::default();
        assets.prune();
        assert!(assets.models.is_empty() && assets.pet_gpus.is_empty() && assets.voices.is_empty());
    }
}
