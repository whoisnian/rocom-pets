//! 演出脚本:跨宠物互动的编排(design.md §6 与 §9 Phase 5 第 6 步)。
//!
//! **走时间轴,不让两个状态机自发协商**。协商听着聪明,实际是「谁先动、等多久、
//! 一方被打断了另一方怎么办」全都要现推,调不出稳定观感;时间轴是「第几秒谁做什么」,
//! 可靠也可调。
//!
//! 这一版**硬编码在 Rust 里**:先把编排与打断语义跑通,确认这套结构够用,再决定要不要
//! 抬到 Lua / 互动包。先引脚本 VM 会把「编排怎么写才好用」和「VM 怎么接」两个问题搅在一起。
//!
//! **诚实的限制**:游戏里由行为树驱动、没有独立 clip 的行为(比如「清扫」)只能用现成动作
//! 拼近似 —— 这里是 `Run` 过去 + 两次 `Show` + 退半步,不是复刻。

/// 一个角色在某一拍要做的事。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Beat {
    /// 转向对方。
    Face,
    /// 播一段逻辑动作(manifest 里的动作名)。**这只没有这段就跳过**——
    /// 全库动作覆盖不齐,缺一段不该让整场演出卡住。
    Play(&'static str),
    /// 走到对方旁边,与对方留 `gap` 个身位。停在自己**当前那一侧**,不穿过对方。
    Approach { gap: f32, running: bool },
    /// 回到开演前站的地方。
    GoHome { running: bool },
}

/// 时间轴上的一拍。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    /// 从开演算起第几秒。
    pub at: f32,
    /// 演员号(`Script::cast` 的下标)。
    pub role: usize,
    pub beat: Beat,
}

/// 一场演出。
pub struct Script {
    pub id: &'static str,
    pub name: &'static str,
    /// 两个角色的**形态 id**。注意不是物种 id:珀尔鼬 3758 是「点点」这条链(物种 3757)
    /// 的二阶形态,捕尘长绒 3604 同理属于「毛头小蛛」3603。
    pub cast: [i64; 2],
    /// 触发距离(身位)。太远就先各走各的 —— 演出里第一拍是喊话,隔半个屏幕喊不合理;
    /// 而且 `Approach` 是按时间给的档期,起点太远会走不到。
    pub max_distance: f32,
    /// 演完之后隔这么久才可能再演。
    pub cooldown: f32,
    /// 总时长,到点收场(最后一拍的动作还要播完,所以比末拍时刻长一些)。
    pub length: f32,
    pub steps: &'static [Step],
}

/// 「珀尔鼬指挥捕尘长绒清扫」——design.md §6 点名的第一个样例。
///
/// 拍子的依据:`CallOut`(呼叫)1.5s、`Alert`(警觉)4.2s、`Show`(展示)1.5s、
/// `Happy` 1.5s。`Alert` 太长,第 2.0 秒就被下一拍接管 —— 时间轴**可以打断自己**,
/// 一段动作没播完就换下一件事是允许的。
// 时间轴排成一行一拍的表:rustfmt 会把每个字段拆成一行,那样就读不出「第几秒谁做什么」了
#[rustfmt::skip]
const PEEL_COMMANDS_CLEANER: Script = Script {
    id: "peel_commands_cleaner",
    name: "珀尔鼬指挥捕尘长绒清扫",
    cast: [3758, 3604],
    max_distance: 2.5,
    cooldown: 90.0,
    length: 11.5,
    steps: &[
        // 珀尔鼬转过去喊它
        Step { at: 0.0, role: 0, beat: Beat::Face },
        Step { at: 0.2, role: 0, beat: Beat::Play("CallOut") },
        // 捕尘长绒听见了
        Step { at: 0.9, role: 1, beat: Beat::Face },
        Step { at: 1.1, role: 1, beat: Beat::Play("Alert") },
        // 跑过去,「扫」两下(没有清扫动作,用 Show + 退半步凑往返)
        Step { at: 2.0, role: 1, beat: Beat::Approach { gap: 1.3, running: true } },
        Step { at: 3.6, role: 1, beat: Beat::Play("Show") },
        Step { at: 5.2, role: 1, beat: Beat::Approach { gap: 2.1, running: false } },
        Step { at: 6.2, role: 1, beat: Beat::Play("Show") },
        // 回原位;珀尔鼬表示满意
        Step { at: 7.8, role: 1, beat: Beat::GoHome { running: false } },
        Step { at: 9.4, role: 0, beat: Beat::Face },
        Step { at: 9.6, role: 0, beat: Beat::Play("Happy") },
    ],
};

pub const SCRIPTS: &[Script] = &[PEEL_COMMANDS_CLEANER];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_in_time_order_and_fit_the_length() {
        // 执行时是「按时间顺序往下走」,乱序的脚本会被静默跳过几拍
        for script in SCRIPTS {
            let mut last = f32::MIN;
            for step in script.steps {
                assert!(
                    step.at >= last,
                    "{}: 第 {} 秒这拍排在 {} 秒之后了",
                    script.id,
                    step.at,
                    last
                );
                assert!(step.role < script.cast.len(), "{}: 演员号越界", script.id);
                last = step.at;
            }
            assert!(
                last < script.length,
                "{}: 末拍 {last}s 落在总时长 {}s 之外,永远轮不到",
                script.id,
                script.length
            );
            // 两个角色不能是同一个形态:选角时要在台上找出**两只不同的**
            assert_ne!(
                script.cast[0], script.cast[1],
                "{}: 两个角色重了",
                script.id
            );
        }
    }
}
