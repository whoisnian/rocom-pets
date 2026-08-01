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
use crate::persona::Persona;
use crate::pet::{Model, PetGpu};
use crate::render::Gpu;
use crate::roster::{Roster, Slot};
use crate::stage::{Actor, PetActor, PetBuild, VoiceBank};

/// 离屏画布的取景余量。伸展类动作已经算进 `Model::motion_bounds`,这里只给描边外扩与
/// 边缘光留一点边。**两个后端必须用同一个值**:它同时是 `view_proj` 的入参。
pub const CANVAS_PADDING: f32 = 1.15;

/// 每只宠物自己的选项。存在 roster.toml 里,托盘与配置窗口都能改。
///
/// 和 [`crate::roster::Slot`] 的关系:那边是**存档形状**(全是 `Option`,默认值不落盘),
/// 这边是**运行时形状**(默认值已经填好)。转换只此一处,别在别处再解释一遍默认值。
#[derive(Debug, Clone, PartialEq)]
pub struct PetOptions {
    /// 相对大小倍数。
    pub scale: f32,
    pub persona: Persona,
    /// 允许的表情;None = 全部。
    pub emotes: Option<Vec<String>>,
}

/// 大小倍数的上下限。太小看不清,太大挡住半个屏幕 —— 两头都不是「桌宠」了。
pub const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.3..=3.0;

impl Default for PetOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            persona: Persona::default(),
            emotes: None,
        }
    }
}

impl PetOptions {
    /// 存档形状 → 运行时形状。手改坏了的值在这里兜住。
    pub fn from_slot(slot: &Slot) -> Self {
        Self {
            // 存档可能被手改成 0 或负数,那会算出 0 像素的画布
            scale: slot
                .scale
                .filter(|s| s.is_finite())
                .map(|s| s.clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end()))
                .unwrap_or(1.0),
            persona: slot
                .persona
                .as_deref()
                .map(Persona::by_id)
                .unwrap_or_default(),
            emotes: slot.emotes.clone(),
        }
    }

    /// 写回存档形状。默认值一律留空,见 [`Slot`] 的说明。
    fn write_into(&self, slot: &mut Slot) {
        slot.scale = (self.scale != 1.0).then_some(self.scale);
        slot.persona = self.persona.saved_id();
        slot.emotes = self.emotes.clone();
    }
}

/// 阵容里的一只:包(整条进化链都在,供切形态)+ 当前形态下标 + 这一只的选项。
pub struct Member {
    pub pack: Pack,
    pub form: usize,
    pub options: PetOptions,
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
        options: &PetOptions,
        with_audio: bool,
        salt: u64,
    ) -> Result<Actor> {
        // 每只自己的大小倍数就叠在 px_per_cm 上:走速/跑速是按 px_per_cm 换算的,
        // 一起放大才不会出现「个头大了却还是原来的步幅」那种滑步
        let px_per_cm = px_per_cm * options.scale;
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
            "  {} 屏幕高 {:.0}px(画布 {}px,脚底 {:.0}px),走速 {:.0}cm/s,跑速 {:.0}cm/s,\
             性格 {}{}",
            form.name,
            height_px,
            side as u32,
            foot_offset,
            walk_speed_cm,
            run_speed_cm,
            options.persona.name,
            if options.scale == 1.0 {
                String::new()
            } else {
                format!(",大小 ×{:.2}", options.scale)
            }
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
            persona: options.persona,
            emotes: options.emotes.clone(),
            seed,
        })))
    }
}

/// 把 main 读好的启动阵容变成 [`Member`]:只剩「挑哪个形态」这一步。
pub fn start_roster(pets: Vec<crate::platform::StartupPet>) -> Vec<Member> {
    let mut roster = Vec::with_capacity(pets.len());
    for pet in pets {
        let form = match pet.pack.form_index(pet.form.as_deref()) {
            Ok(index) => index,
            Err(e) => {
                log::warn!("{} 的形态选不出来({e:#}),退用链首", pet.pack.species_name);
                0
            }
        };
        let f = &pet.pack.forms[form];
        log::info!(
            "宠物包 {}({}):{} 个形态,当前 {}({}),高 {:.0}cm,{} 个动作",
            pet.pack.species_name,
            pet.pack.species_id,
            pet.pack.forms.len(),
            f.name,
            f.asset,
            f.height_cm,
            f.clips.len()
        );
        roster.push(Member {
            pack: pet.pack,
            form,
            options: pet.options,
        });
    }
    roster
}

/// 按存档里的名单读出阵容。给 `Reload` 用:配置窗口改完存盘,在跑的实例照这份重来。
///
/// 读不动的**只警告**(和启动时对存档的处理一致):某个包被删了不该让整次重载失败,
/// 那会让「在配置窗口里删掉一个包」变成「宠物全没了」。
pub fn load_roster(slots: &[Slot], packs_dir: Option<&Path>) -> Vec<Member> {
    let mut roster = Vec::with_capacity(slots.len());
    for slot in slots {
        let pack = match Pack::resolve(&slot.pack, packs_dir) {
            Ok(pack) => pack,
            Err(e) => {
                log::warn!("阵容里的 {} 上不了台({e:#}),跳过", slot.pack);
                continue;
            }
        };
        let form = pack.form_index(slot.form.as_deref()).unwrap_or_else(|e| {
            log::warn!("{} 的形态选不出来({e:#}),退用链首", pack.species_name);
            0
        });
        roster.push(Member {
            pack,
            form,
            options: PetOptions::from_slot(slot),
        });
    }
    roster
}

/// 托盘菜单要的那份阵容快照。
pub fn tray_pets(roster: &[Member]) -> Vec<TrayPet> {
    roster
        .iter()
        .map(|m| TrayPet {
            name: m.form().name.clone(),
            forms: m.pack.forms.iter().map(|f| f.name.clone()).collect(),
            current_form: m.form,
            scale: m.options.scale,
            persona: m.options.persona.index(),
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
                let in_packs_dir = packs_dir.is_some_and(|dir| m.pack.path.parent() == Some(dir));
                let pack = match (in_packs_dir, m.pack.path.file_name()) {
                    (true, Some(name)) => name.to_string_lossy().into_owned(),
                    _ => m.pack.path.to_string_lossy().into_owned(),
                };
                let mut slot = Slot::new(pack, Some(m.form().asset.clone()));
                m.options.write_into(&mut slot);
                slot
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
