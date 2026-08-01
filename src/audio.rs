//! 叫声播放。
//!
//! 设计上**只有这一个文件碰音频设备**:`stage` 那边只产出「放哪段、什么速率」的音效请求
//! (`SoundCue`),平台层拿过来交给这里。这样行为逻辑照旧可以脱离窗口系统与声卡做单元测试。
//!
//! 变调就是**变速**:游戏里 −100~100 的 `voice` 属性喂给 Wwise 的 `Pet_Vo_Pitch`,
//! 由 RTPC 曲线换成音分,而 Wwise 的 pitch 本身是重采样(变调同时变速)——
//! 所以按 `2^(音分/1200)` 调播放速率就是等价实现,不需要为每个音调预生成音频。
//! (rocom-petvo 的网页版用 `playbackRate` 做的是同一件事。)

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rodio::{ChannelCount, SampleRate, Source};

use crate::stage::SoundCue;

/// 默认音量。桌宠是常驻程序,**默认必须小声**。
pub const DEFAULT_VOLUME: f32 = 0.35;

/// 解好的一段 PCM。
///
/// **加载时就解码,不在播放时解**。两个理由,后一个是被坑出来的:
/// ① 每次叫都重解一遍 ogg 是白费;
/// ② 把 `rodio::Decoder` 直接丢进 mixer **一声不响** —— 同一台机器、同一个 mixer,
///    换成自带样本的源就正常(rodio 0.22.2 实测,见 design.md §9 Phase 6)。
pub struct Pcm {
    samples: Arc<[f32]>,
    channels: ChannelCount,
    rate: SampleRate,
}

impl Pcm {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.rate.get() as f32 / self.channels.get() as f32
    }

    /// 峰值。全是静音的话多半是解码出了岔子,加载时会警告。
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// 给测试用的一小段(内容无所谓,不会真的送进声卡)。
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            samples: vec![0.0, 0.5, -0.5, 1.0].into(),
            channels: ChannelCount::new(1).expect("1 声道"),
            rate: SampleRate::new(44_100).expect("44.1k"),
        }
    }
}

/// 解一个音频文件(包里是 ogg vorbis)。
pub fn decode(path: &Path) -> Result<Pcm> {
    // 走 assets:叫声可能在一个 .rkpet 里(见 assets.rs 的「虚拟路径」)
    let bytes = crate::assets::read(path)?;
    let decoder = rodio::Decoder::new(std::io::Cursor::new(bytes))
        .with_context(|| format!("{path:?} 解不开"))?;
    let channels = decoder.channels();
    let rate = decoder.sample_rate();
    let samples: Vec<f32> = decoder.collect();
    anyhow::ensure!(!samples.is_empty(), "{path:?} 解出来是空的");
    Ok(Pcm {
        samples: samples.into(),
        channels,
        rate,
    })
}

/// 播放游标。自己实现 `Source` 是为了**共享同一份样本** ——
/// `SamplesBuffer::new` 要吃一个 `Vec`,每放一次就复制一遍(一段叫声 400KB)。
struct PcmSource {
    pcm: Arc<Pcm>,
    pos: usize,
}

impl Iterator for PcmSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        let s = self.pcm.samples.get(self.pos).copied();
        self.pos += 1;
        s
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.pcm.samples.len().saturating_sub(self.pos);
        (left, Some(left))
    }
}

impl Source for PcmSource {
    /// 整段自始至终一个格式,没有分段 —— 返回 `None`(「一直到放完」)。
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.pcm.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.pcm.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f32(self.pcm.seconds()))
    }
}

/// 音频输出。拿不到声卡就是 `None` —— 没有声音不该拦住桌宠跑起来。
pub struct Audio {
    sink: rodio::MixerDeviceSink,
    volume: f32,
    muted: bool,
}

impl Audio {
    /// 打开默认输出设备。失败只记日志(远程会话、没有 PipeWire/PulseAudio 都可能)。
    pub fn open(volume: f32) -> Option<Self> {
        match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(mut sink) => {
                // 退出时 rodio 默认往 stderr 打一行「设备被 drop 了」,对用户没意义
                sink.log_on_drop(false);
                log::info!(
                    "音频输出就绪(音量 {:.0}%,{:?})",
                    volume * 100.0,
                    sink.config()
                );
                Some(Self {
                    sink,
                    volume: volume.clamp(0.0, 1.0),
                    muted: false,
                })
            }
            Err(e) => {
                log::warn!("打不开音频设备({e});叫声关掉,其余照常");
                None
            }
        }
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// 改音量。**下一声才生效**:已经在混音器里的那段是按当时的音量放大过的,
    /// 追不回来 —— 叫声都是一两秒的短音,不值得为此维护一条可调的增益链。
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// 放一段。**不排队**:同一只连点几下就该盖过去 —— 混音器会把它们叠在一起,
    /// 短促的叫声这样比排队等前一段放完自然。
    pub fn play(&self, cue: &SoundCue) {
        if self.muted || self.volume <= 0.0 {
            return;
        }
        let source = PcmSource {
            pcm: Arc::clone(&cue.pcm),
            pos: 0,
        };
        log::debug!("放叫声(速率 {:.3},音量 {:.2})", cue.speed, self.volume);
        self.sink
            .mixer()
            .add(source.speed(cue.speed).amplify(self.volume));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::speed_for_cents;

    #[test]
    fn cents_map_to_playback_rate() {
        // 一个八度 = 1200 音分 = 两倍速;0 音分 = 原速。这条换算是「粗嗓门/婉转声」的全部
        assert!((speed_for_cents(0.0) - 1.0).abs() < 1e-6);
        assert!((speed_for_cents(1200.0) - 2.0).abs() < 1e-6);
        assert!((speed_for_cents(-1200.0) - 0.5).abs() < 1e-6);
        // 实测最常见的曲线是 ±300 音分,听感上是明显但不失真的高低
        assert!((speed_for_cents(300.0) - 1.189_207).abs() < 1e-5);
    }

    #[test]
    fn default_volume_is_quiet() {
        // 桌宠常驻,默认音量必须小 —— 这条是产品约束,写成测试免得日后被人调大
        const { assert!(DEFAULT_VOLUME <= 0.5, "默认音量该小声") };
    }

    #[test]
    fn pcm_source_yields_every_sample_once() {
        let pcm = Arc::new(Pcm {
            samples: vec![0.0, 0.5, -0.5, 1.0].into(),
            channels: ChannelCount::new(2).expect("2 声道"),
            rate: SampleRate::new(48_000).expect("48k"),
        });
        assert!((pcm.peak() - 1.0).abs() < 1e-6);
        assert!((pcm.seconds() - 4.0 / 48_000.0 / 2.0).abs() < 1e-9);
        let source = PcmSource {
            pcm: Arc::clone(&pcm),
            pos: 0,
        };
        // **`current_span_len` 必须是 None**:把「有限 span」的源丢进 mixer 会一声不响,
        // 这是 Phase 6 排了半天的那个坑
        assert_eq!(source.current_span_len(), None);
        assert_eq!(source.collect::<Vec<_>>(), vec![0.0, 0.5, -0.5, 1.0]);
    }
}
