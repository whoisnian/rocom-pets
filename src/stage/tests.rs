//! `stage` 的单元测试:落脚点 / 实体 / 宠物状态机 / 感知 / 行动 / 叫声 / 频率 / 行为。
//!
//! **从 stage.rs 里拆出来的** —— 那份文件曾经是 3593 行,测试占一多半,
//! 找一条生产代码要翻很久。测试全部沿用 `use super::super::*`,行为一字未改。
//! 共用的两个夹具(`TEST_BODY_PX` / `test_build`)也在这儿。

use super::*;

/// 脸槽的编号(见 `pack::face_slot`)。测里直接写数字看不出是哪个槽。
const EYE: usize = 0;
const MOUTH: usize = 2;
const DYNAMIC1: usize = 4;

/// 测试宠物的本体高度。画布是 200×200,本体比画布小(取景余量),取 120 与真实比例相当;
/// 于是一个身位 = 120px,`NOTICE_DISTANCE` 折合 240px。
const TEST_BODY_PX: f32 = 120.0;

/// 一份测试宠物的参数。要改哪项就 `PetBuild { form_id: 3758, ..test_build(m, 1) }`。
fn test_build(model: Arc<Model>, seed: u64) -> PetBuild {
    PetBuild {
        model,
        size: (200, 200),
        foot_offset: 180.0,
        body_px: TEST_BODY_PX,
        walk_speed: 100.0,
        run_speed: 250.0,
        form_id: 0,
        voice: None,
        persona: Persona::default(),
        voice_value: None,
        seed,
    }
}

mod home_tests {
    use super::*;

    /// 落脚点存的是**比例**,所以换了屏幕宽度还能回到「同样靠左三成」的地方。
    #[test]
    fn a_remembered_spot_survives_a_resize() {
        let mut stage = Stage::new((1000, 600));
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let id = stage.spawn_at(Actor::Pet(PetActor::new(test_build(model, 1))), Some(0.25));
        // 可走范围 = 1000 − 画布 200 = 800,四分之一处是 200
        assert!((stage.entity(id).expect("在台上").pos().0 - 200.0).abs() < 1.0);
        assert!((stage.home_fraction(id).expect("该读得到") - 0.25).abs() < 0.01);

        // 换到更宽的屏:比例不变,像素跟着变
        stage.handle(StageEvent::Resized {
            width: 2000,
            height: 600,
        });
        assert!((stage.home_fraction(id).expect("该读得到") - 0.25).abs() < 0.01);
        assert!((stage.entity(id).expect("在台上").pos().0 - 450.0).abs() < 1.0);
    }

    /// 召回**不**尊重落脚点:它的用途就是「跑没影了,拉回来」。
    #[test]
    fn recall_ignores_the_remembered_spot() {
        let mut stage = Stage::new((1000, 600));
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let id = stage.spawn_at(
            Actor::Pet(PetActor::new(test_build(model.clone(), 1))),
            Some(0.9),
        );
        stage.reset_position();
        let centred = stage.entity(id).expect("在台上").pos().0;
        assert!((centred - 400.0).abs() < 1.0, "该回到正中,实际 {centred}");

        // **记录也要作废**:留着的话下一次重建角色又会把它拽回 90% 去
        stage.replace_actor(id, Actor::Pet(PetActor::new(test_build(model, 2))));
        let after = stage.entity(id).expect("在台上").pos().0;
        assert!((after - 400.0).abs() < 1.0, "重建之后又跑回去了:{after}");
    }

    /// 重建角色(改整体大小、切形态)要回到记下的落脚点,而不是一律居中。
    ///
    /// 这条是实机逮到的:改一次整体大小,每只记下的位置就被抹成正中间,
    /// 而那个「正中间」还会被当成新的落脚点存回阵容 —— 记了等于没记。
    #[test]
    fn rebuilding_an_actor_returns_to_the_remembered_spot() {
        let mut stage = Stage::new((1000, 600));
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let id = stage.spawn_at(
            Actor::Pet(PetActor::new(test_build(model.clone(), 1))),
            Some(0.9),
        );
        assert!((stage.home_fraction(id).expect("该读得到") - 0.9).abs() < 0.01);

        stage.replace_actor(id, Actor::Pet(PetActor::new(test_build(model, 2))));
        assert!(
            (stage.home_fraction(id).expect("该读得到") - 0.9).abs() < 0.01,
            "重建之后落脚点被抹掉了"
        );
    }

    /// 嗓音给了就用给的,没给才随机 —— 存档里存的就是掷出来那一次。
    #[test]
    fn a_saved_voice_value_is_used_verbatim() {
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        let pet = PetActor::new(PetBuild {
            voice_value: Some(-0.37),
            ..test_build(model.clone(), 1)
        });
        assert!((pet.voice_value + 0.37).abs() < 1e-6);
        // 不给就现掷一个,落在 −1..1
        let rolled = PetActor::new(test_build(model, 7));
        assert!((-1.0..=1.0).contains(&rolled.voice_value));
    }

    /// 覆盖率把「降级也算有」算进去 —— 否则只有 SleepStand 的那批会被误报成不会睡。
    #[test]
    fn coverage_counts_the_documented_fallbacks() {
        let mut clips = std::collections::HashMap::new();
        for name in ["Idle", "Walk", "SleepStand", "Alert", "Show"] {
            clips.insert(
                name.to_string(),
                crate::pack::Clip {
                    seconds: 1.0,
                    ..Default::default()
                },
            );
        }
        let form = crate::pack::Form::for_test(clips);
        // SleepLoop→SleepStand、Shock→Alert、Happy→Show、Fear→Alert 四个降级都该算有
        for name in ["SleepLoop", "Shock", "Happy", "Fear"] {
            assert!(has_clip(&form, name), "{name} 该按降级算有");
        }
        // 真没有的照样要报出来
        assert!(!has_clip(&form, "Run"));
        assert!(!has_clip(&form, "CallOut"));
    }

    /// 动作表说能点的,点下去就得真播出来。
    ///
    /// 幽星光只有 `SleepStand`:表格按 `has_clip` 算「睡着」能点,而 `play_clip`
    /// 以前拿 `model.clip` 直接找,于是点了只在日志里留一句「这只没有这段」。
    #[test]
    fn every_clip_the_table_offers_can_actually_be_played() {
        // 与上面那个形态同一套动作,只是换成运行时的模型
        let clips = ["Idle", "Walk", "SleepStand", "Alert", "Show"];
        let model = Arc::new(Model::for_test(&clips));
        let mut stage = Stage::new((1000, 600));
        let id = stage.spawn(Actor::Pet(PetActor::new(test_build(model, 3))));
        for name in ["SleepLoop", "Shock", "Happy", "Fear"] {
            assert!(stage.play_clip(id, name), "{name} 该按降级播得出来");
        }
        assert!(!stage.play_clip(id, "Run"), "真没有的还是得返回 false");
    }

    /// 眼睛跟着动作走:生气时是生气眼,睡着时是困倦眼,平时才是性格那张脸。
    #[test]
    fn the_face_follows_the_clip_being_played() {
        let model = Arc::new(Model::for_test(&["Idle", "Anger", "SleepStand"]));
        let mut stage = Stage::new((1000, 600));
        // 胆小 = 哭哭眼,拿它当「平时那张脸」才看得出被盖掉
        let timid = crate::persona::Persona::by_id("timid");
        let id = stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            persona: timid,
            ..test_build(model, 5)
        })));
        // `Model::for_test` 不带眼神曲线 ⇒ 走 `face_for_clip` 那条兜底路
        let face = |stage: &Stage| match stage.entity(id).map(|e| e.actor()) {
            Some(Actor::Pet(pet)) => pet.faces()[EYE].uv_offset(),
            _ => panic!("不是宠物"),
        };
        assert_eq!(face(&stage), timid.face.uv_offset(), "待机时是性格那张脸");
        assert!(stage.play_clip(id, "Anger"));
        assert_eq!(face(&stage), crate::persona::ANGRY.uv_offset());
        // 睡的那段只有 SleepStand,降级过去之后眼睛也得跟着变困
        assert!(stage.play_clip(id, "SleepLoop"));
        assert_eq!(face(&stage), crate::persona::SLEEPY.uv_offset());
    }

    /// 有曲线时**逐帧**查表,而且各个脸槽各查各的。
    ///
    /// 钉住四件事:① 曲线第 1 格 =「默认」= 性格那张脸(待机眨眼就靠这条);
    /// ② 眼和嘴可以同时是两格(游戏里就是两条独立曲线);
    /// ③ 有曲线的段里,嘴那条空着时嘴停在性格那张脸上,**不跟着眼睛跑**;
    /// ④ Dynamic 槽同理各走各的(幽影树的两条藤就是这样)。
    #[test]
    fn the_face_curve_is_sampled_per_frame_for_each_slot_separately() {
        let mut model = Model::for_test(&["Idle", "Shock"]);
        // 待机:0.3 秒起闭眼(第 5 格),0.4 秒回默认 —— 这就是一次眨眼
        model.clips[0].faces[EYE] = vec![(0.0, 1), (0.3, 5), (0.4, 1)];
        // 受惊:眼第 3 格、嘴第 7 格(幽星光的 Shock 就是这样),藤第 4 格
        model.clips[1].faces[EYE] = vec![(0.0, 3)];
        model.clips[1].faces[MOUTH] = vec![(0.0, 7)];
        model.clips[1].faces[DYNAMIC1] = vec![(0.0, 4)];
        let mut stage = Stage::new((1000, 600));
        let timid = crate::persona::Persona::by_id("timid");
        let id = stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            persona: timid,
            ..test_build(Arc::new(model), 5)
        })));
        let face = |stage: &Stage| match stage.entity(id).map(|e| e.actor()) {
            Some(Actor::Pet(pet)) => pet.faces(),
            _ => panic!("不是宠物"),
        };
        // 只推进播放头,不跑行为逻辑 —— 这条测的是「按时刻查曲线」,不是状态机
        let advance = |stage: &mut Stage, dt: f32| {
            let Some(Actor::Pet(pet)) = stage.entity_mut(id).map(|e| &mut e.actor) else {
                panic!("不是宠物");
            };
            pet.player.advance(&pet.model, dt);
        };
        assert_eq!(face(&stage)[EYE], timid.face, "第 1 格 = 性格那张脸");
        advance(&mut stage, 0.35);
        assert_eq!(face(&stage)[EYE], crate::persona::SLEEPY, "0.35 秒该闭着眼");
        advance(&mut stage, 0.1);
        assert_eq!(face(&stage)[EYE], timid.face, "0.45 秒睁回来");

        assert!(stage.play_clip(id, "Shock"));
        let faces = face(&stage);
        assert_eq!(faces[EYE], crate::persona::SURPRISED);
        assert_eq!(faces[MOUTH], crate::persona::CLENCHED, "嘴不跟着眼睛走");
        assert_eq!(
            faces[DYNAMIC1],
            crate::persona::ANGRY,
            "Dynamic 槽也是自己一条"
        );

        // 待机段有眼曲线、没有别的槽 ⇒ 那几个槽停在性格那张脸上
        assert!(stage.play_clip(id, "Idle"));
        assert_eq!(face(&stage)[MOUTH], timid.face);
        assert_eq!(face(&stage)[DYNAMIC1], timid.face);
    }

    /// 槽号是**定死的**:manifest 里写的是名字,而材质那份 uniform 里存的是编号,
    /// 两边靠这张表对上。改一个编号 = 所有已导出的包整体串位。
    #[test]
    fn face_slot_numbers_are_frozen() {
        use crate::pack::face_slot;
        assert_eq!(face_slot("eye"), Some(EYE));
        assert_eq!(face_slot("eye_1"), Some(1));
        assert_eq!(face_slot("mouth"), Some(MOUTH));
        assert_eq!(face_slot("mouth_1"), Some(3));
        assert_eq!(face_slot("dynamic1"), Some(DYNAMIC1));
        assert_eq!(face_slot("dynamic2"), Some(5));
        assert_eq!(face_slot("dynamic3"), Some(6));
        // 不认得的名字按「不是脸」处理,别让新包把旧运行时带崩
        assert_eq!(face_slot("dynamic9"), None);
        assert_eq!(face_slot("eye_9"), None);
    }
}

mod tests {
    use super::*;

    fn stage() -> Stage {
        let mut stage = Stage::new((800, 600));
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage
    }

    #[test]
    fn loop_clips_are_held_for_whole_cycles() {
        // 走/跑与技能循环段:凑够 3 秒,而且必须是整周期(收尾时姿势回到起点)
        for (name, duration) in [("Walk", 1.033), ("Run", 0.533), ("Skill2Loop1", 1.6)] {
            let held = play_hold(name, duration);
            assert!(held >= LOOP_HOLD, "{name} 只按住了 {held}s");
            let cycles = held / duration;
            assert!(
                (cycles - cycles.round()).abs() < 1e-3,
                "{name} 按住 {held}s 不是整周期"
            );
            assert!(held < LOOP_HOLD + duration, "{name} 按过头了:{held}s");
        }
        // 一次性动作照旧播一遍;短得离谱的给下限
        assert_eq!(play_hold("Happy", 1.5), 1.5);
        assert_eq!(play_hold("CallOut", 0.1), 0.3);
    }

    #[test]
    fn starts_centered_and_regions_follow_actor() {
        let s = stage();
        assert_eq!(s.actor_pos(), (368.0, 268.0));
        let regions = s.input_regions();
        assert!(!regions.is_empty());
        // 输入区跟着角色平移:圆心必被覆盖,角色外必不被覆盖
        assert!(regions.iter().any(|r| r.contains(400.0, 300.0)));
        assert!(!regions.iter().any(|r| r.contains(10.0, 10.0)));
    }

    #[test]
    fn drag_moves_actor_and_dirties_regions() {
        let mut s = stage();
        let start = s.actor_pos();
        // 按在圆心上
        let hit = s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        assert!(hit.redraw && s.is_dragging());
        let moved = s.handle(StageEvent::PointerMoved { x: 450.0, y: 320.0 });
        assert_eq!(
            moved,
            Reaction {
                redraw: true,
                regions_dirty: true
            }
        );
        assert_eq!(s.actor_pos(), (start.0 + 50.0, start.1 + 20.0));
        s.handle(StageEvent::PointerReleased);
        assert!(!s.is_dragging());
    }

    #[test]
    fn press_on_transparent_pixel_is_ignored() {
        let mut s = stage();
        // 精灵包围盒左上角是圆外的透明像素
        let (px, py) = s.actor_pos();
        assert_eq!(
            s.handle(StageEvent::PointerPressed {
                x: px as f64,
                y: py as f64
            }),
            Reaction::NONE
        );
        assert!(!s.is_dragging());
    }

    #[test]
    fn passthrough_clears_regions_and_blocks_drag() {
        let mut s = stage();
        assert_eq!(s.handle(StageEvent::TogglePassthrough), Reaction::BOTH);
        assert!(s.passthrough());
        assert!(s.input_regions().is_empty());
        // 形状不跟着空:Win32 的窗口区域要拿它当形状用(见 platform/windows.rs)
        assert!(!s.shape_regions().is_empty());
        s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        assert!(!s.is_dragging());
    }

    #[test]
    fn actor_stays_inside_after_shrink() {
        let mut s = stage();
        s.handle(StageEvent::PointerPressed { x: 400.0, y: 300.0 });
        s.handle(StageEvent::PointerMoved { x: 790.0, y: 590.0 });
        s.handle(StageEvent::PointerReleased);
        s.handle(StageEvent::Resized {
            width: 300,
            height: 200,
        });
        let (x, y) = s.actor_pos();
        assert!(x + 64.0 <= 300.0 && y + 64.0 <= 200.0, "角色越界: {x},{y}");
    }

    #[test]
    fn sprite_actor_does_not_tick() {
        let mut s = stage();
        assert_eq!(s.tick(0.1), Reaction::NONE);
    }

    #[test]
    fn rng_stays_in_unit_range() {
        let mut rng = Rng::new(12345);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "越界: {v}");
        }
    }
}

mod entity_tests {
    use super::*;

    /// 两只精灵:第二只放在第一只右下方一点,重叠一块。
    fn two_sprites() -> Stage {
        let mut stage = Stage::new((800, 600));
        for _ in 0..2 {
            stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        }
        stage
    }

    #[test]
    fn spawn_and_despawn_track_by_id() {
        let mut stage = two_sprites();
        assert_eq!(stage.entities().len(), 2);
        let second = stage.entities()[1].id();
        assert!(stage.despawn(second));
        assert_eq!(stage.entities().len(), 1);
        // 同一个标识不会再命中(下标滑动了也不会误伤别人)
        assert!(!stage.despawn(second));
    }

    #[test]
    fn placement_waits_for_the_real_surface_size() {
        // stage 是先建再等 configure 的:那之前尺寸是 (1, 1),错开量会被整个夹掉。
        // 实测两只 315px 的宠物双双落在 x = 0,演出开场「相隔 0.0 身位」
        let mut stage = Stage::new((1, 1));
        for _ in 0..2 {
            stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        }
        assert_eq!(
            stage.entities[0].pos.0, stage.entities[1].pos.0,
            "这时确实重叠"
        );
        stage.handle(StageEvent::Resized {
            width: 800,
            height: 600,
        });
        assert_ne!(
            stage.entities[0].pos.0, stage.entities[1].pos.0,
            "拿到真实尺寸就该重摆开"
        );
        // 之后的尺寸变化不再重摆(用户自己拖过的位置要留着)
        stage.entities[1].pos.0 = 700.0;
        stage.handle(StageEvent::Resized {
            width: 900,
            height: 600,
        });
        assert_eq!(stage.entities[1].pos.0, 700.0, "后续 resize 不该把它挪回去");
        // 召回也要错开:三只叠在一起的「召回」等于把它们藏成一只
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage.reset_position();
        let xs: Vec<f32> = stage.entities.iter().map(|e| e.pos.0).collect();
        assert!(
            xs[0] != xs[1] && xs[1] != xs[2] && xs[0] != xs[2],
            "召回之后不该叠在一起: {xs:?}"
        );
    }

    #[test]
    fn empty_stage_is_a_valid_state() {
        // 托盘可以把最后一只也撤掉。那之后 stage 必须还能正常挨帧推进:
        // 输入区空 = 全穿透,点哪儿都不在,tick 也不该恐慌
        let mut stage = Stage::new((800, 600));
        assert!(stage.entities().is_empty());
        assert!(stage.input_regions().is_empty());
        assert_eq!(stage.pick(400.0, 300.0), None);
        assert_eq!(stage.tick(0.1), Reaction::NONE);
        assert!(stage.tick_interval() > Duration::ZERO, "空台的间隔不能是 0");
    }

    #[test]
    fn replace_actor_keeps_the_id_and_spares_the_others() {
        // 切形态换的是**那一只**:标识必须留着(托盘插槽与掩码回读都还认着它),
        // 同台其余的不能被动到 —— 早先那版 `replace_actor` 只认第一只,
        // 于是第二只永远换不了形态,而第一只会被别人的操作换掉
        let mut stage = two_sprites();
        stage.entities[1].pos = (100.0, 100.0);
        let target = stage.entities()[1].id();
        let untouched = stage.entities()[0].pos();

        assert!(stage.replace_actor(target, Actor::Sprite(Sprite::test_pattern(128))));
        assert_eq!(stage.entities().len(), 2);
        assert_eq!(stage.entities()[1].id(), target, "标识不该变");
        assert_eq!(
            stage.entity(target).expect("还在台上").actor().size(),
            (128, 128)
        );
        assert_eq!(stage.entities()[0].pos(), untouched, "不该动到别人");
        assert_eq!(stage.entities()[0].actor().size(), (64, 64));

        // 已经撤掉的标识:换不上,也不能误伤还在台上的
        assert!(stage.despawn(target));
        assert!(!stage.replace_actor(target, Actor::Sprite(Sprite::test_pattern(32))));
        assert_eq!(stage.entities()[0].actor().size(), (64, 64));
    }

    #[test]
    fn input_regions_are_the_union() {
        let mut stage = two_sprites();
        // 两只错开:各自的圆心都必须在输入区里
        stage.entities[1].pos = (100.0, 100.0);
        let regions = stage.input_regions();
        let first = stage.entities[0].pos;
        let (fx, fy) = (first.0 as f64 + 32.0, first.1 as f64 + 32.0);
        assert!(regions.iter().any(|r| r.contains(fx, fy)));
        assert!(regions.iter().any(|r| r.contains(132.0, 132.0)));
    }

    #[test]
    fn pick_takes_the_topmost() {
        let mut stage = two_sprites();
        // 完全重叠;第二只脚底更靠下 ⇒ 它在上面
        stage.entities[0].pos = (100.0, 100.0);
        stage.entities[1].pos = (100.0, 110.0);
        let top = stage.entities[1].id();
        assert_eq!(stage.pick(132.0, 142.0), Some(top));
        // 把第一只挪到更下面,z 序随之翻转
        stage.entities[0].pos = (100.0, 120.0);
        let other = stage.entities[0].id();
        assert_eq!(stage.pick(132.0, 152.0), Some(other));
    }

    #[test]
    fn same_form_entities_share_one_model() {
        let model = Arc::new(Model::for_test(&["Idle", "Walk"]));
        assert_eq!(Arc::strong_count(&model), 1);
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(test_build(Arc::clone(&model), 1))));
        stage.spawn(Actor::Pet(PetActor::new(test_build(Arc::clone(&model), 2))));
        // 两只在场,加上这里持有的那份 = 3;网格/动画/贴图只有一份
        assert_eq!(Arc::strong_count(&model), 3);
        let (Actor::Pet(a), Actor::Pet(b)) = (&stage.entities[0].actor, &stage.entities[1].actor)
        else {
            panic!("两只都该是宠物");
        };
        assert!(Arc::ptr_eq(&a.model, &b.model));
        // 撤掉一只,引用计数跟着降(缓存那边靠它判断能不能清)
        let second = stage.entities[1].id();
        assert!(stage.despawn(second));
        assert_eq!(Arc::strong_count(&model), 2);
    }

    #[test]
    fn dragging_moves_only_the_picked_one() {
        let mut stage = two_sprites();
        stage.entities[0].pos = (100.0, 100.0);
        stage.entities[1].pos = (400.0, 100.0);
        let still = stage.entities[0].pos;
        stage.handle(StageEvent::PointerPressed { x: 432.0, y: 132.0 });
        stage.handle(StageEvent::PointerMoved { x: 482.0, y: 152.0 });
        assert_eq!(stage.entities[1].pos, (450.0, 120.0));
        assert_eq!(stage.entities[0].pos, still, "没被点中的那只不该动");
        assert!(stage.is_dragging());
        stage.handle(StageEvent::PointerReleased);
        assert!(!stage.is_dragging());
    }
}

mod pet_tests {
    use super::*;

    /// 一只测试宠物:200×200 的画布,脚底在 180,走速 100px/s。
    fn pet_stage() -> Stage {
        let model = Model::for_test(&[
            "Idle",
            "Walk",
            "Run",
            "Shock",
            "Happy",
            "Fear",
            "SleepStart",
            "SleepLoop",
            "SleepEnd",
        ]);
        let actor = Actor::Pet(PetActor::new(test_build(Arc::new(model), 7)));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(actor);
        stage
    }

    fn activity(stage: &Stage) -> Activity {
        match stage.actor() {
            Actor::Pet(pet) => pet.activity,
            _ => panic!("不是宠物"),
        }
    }

    /// 宠物中心附近的表面坐标(必落在包围盒内)。
    fn center(stage: &Stage) -> (f64, f64) {
        let (x, y) = stage.actor_pos();
        (x as f64 + 100.0, y as f64 + 100.0)
    }

    #[test]
    fn stands_on_the_ground_line() {
        let s = pet_stage();
        // 脚底(180)应落在屏幕底边上方 GROUND_MARGIN 处
        assert_eq!(s.actor_pos().1 + 180.0, 600.0 - GROUND_MARGIN);
    }

    #[test]
    fn click_without_moving_startles() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        s.handle(StageEvent::PointerPressed { x, y });
        // 没移动就松手 = 点了一下
        s.handle(StageEvent::PointerReleased);
        assert!(
            matches!(activity(&s), Activity::React { .. }),
            "点击应触发反应(受惊)"
        );
        // 受惊动作播完 → 逃开(**用跑**),而不是原地回待机
        let start_x = s.actor_pos().0;
        let mut fled_to = None;
        for _ in 0..40 {
            s.tick(0.05);
            if let Activity::Walk { running, target_x } = activity(&s) {
                assert!(running, "受惊逃跑该用跑");
                fled_to = Some(target_x);
                break;
            }
        }
        let target_x = fled_to.expect("受惊动作播完该起跑逃开");
        // 点的是画布正中,往哪边逃都行,但得逃出至少一个身位
        assert!(
            (target_x - start_x).abs() > TEST_BODY_PX,
            "逃跑目标该在一个身位之外,实际从 {start_x} 逃到 {target_x}"
        );
        // 逃到了就回常规状态(之后爱去哪去哪,不再断言位置)
        let mut arrived = false;
        for _ in 0..80 {
            s.tick(0.05);
            if (s.actor_pos().0 - target_x).abs() < 1.0 {
                arrived = true;
                break;
            }
        }
        assert!(arrived, "该跑到逃跑目标点");
    }

    #[test]
    fn dragging_picks_up_then_lands() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        s.handle(StageEvent::PointerPressed { x, y });
        s.handle(StageEvent::PointerMoved {
            x: x + 60.0,
            y: y - 120.0,
        });
        assert_eq!(activity(&s), Activity::Dragged, "移动超过阈值算拎起来");
        assert!(s.is_dragging());
        let lifted = s.actor_pos().1;
        s.handle(StageEvent::PointerReleased);
        // **松手不再瞬移**:先进入下落,一路掉到地面线才回待机
        assert!(
            matches!(activity(&s), Activity::Falling { .. }),
            "从半空松手该开始下落"
        );
        assert_eq!(s.actor_pos().1, lifted, "松手那一下不该跳位置");
        let ground = 600.0 - GROUND_MARGIN - 180.0;
        for _ in 0..120 {
            s.tick(1.0 / 60.0);
            if matches!(activity(&s), Activity::Idle { .. }) {
                break;
            }
            assert!(s.actor_pos().1 <= ground, "不该穿过地面");
        }
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "该落地回待机"
        );
        assert_eq!(s.actor_pos().1 + 180.0, 600.0 - GROUND_MARGIN);
    }

    #[test]
    fn far_target_runs_and_near_target_walks() {
        // 跑速比走速快,且只有远处才起跑
        let mut s = pet_stage();
        let max_x = 1000.0 - 200.0;
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => {
                pet.needs.boredom = 1.0;
                // 近处:目标就在旁边 ⇒ 走(或原地),不该是跑
                pet.choose_next(0.0, 10.0);
                assert!(
                    !matches!(pet.activity, Activity::Walk { running: true, .. }),
                    "近距离不该起跑"
                );
                // 远处:反复挑几次,总会挑到超过三个身位的目标
                let mut saw_run = false;
                for _ in 0..40 {
                    pet.needs.boredom = 1.0;
                    pet.choose_next(0.0, max_x);
                    if matches!(pet.activity, Activity::Walk { running: true, .. }) {
                        saw_run = true;
                        break;
                    }
                }
                assert!(saw_run, "远处目标该起跑(测试模型带 Run)");
            }
            _ => panic!("该是宠物"),
        }
    }

    #[test]
    fn rubbing_the_head_pets_it() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        let head_y = y - 60.0; // 落在上 45% 的头部区域
        // 只蹭一下不算:得来回换向够 PET_REVERSALS 次
        s.handle(StageEvent::PointerMoved { x, y: head_y });
        s.handle(StageEvent::PointerMoved {
            x: x + 30.0,
            y: head_y,
        });
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "单向划过不该算摸头"
        );
        for dx in [-30.0, 30.0, -30.0] {
            s.handle(StageEvent::PointerMoved {
                x: x + dx,
                y: head_y,
            });
        }
        assert!(
            matches!(activity(&s), Activity::React { .. }),
            "来回蹭够次数应触发反应(开心)"
        );
    }

    #[test]
    fn rubbing_the_body_does_not_count() {
        let mut s = pet_stage();
        let (x, y) = center(&s);
        let body_y = y + 60.0; // 头部区域之外
        for dx in [0.0, 30.0, -30.0, 30.0, -30.0, 30.0] {
            s.handle(StageEvent::PointerMoved {
                x: x + dx,
                y: body_y,
            });
        }
        assert!(
            matches!(activity(&s), Activity::Idle { .. }),
            "在身上蹭不算摸头"
        );
    }

    /// 帧率**不随台上在干什么变**。以前静止时会自动降频,取消了 ——
    /// 用户选了 30 帧就该一直是 30 帧。
    #[test]
    fn the_frame_rate_ignores_what_the_pet_is_doing() {
        let mut s = pet_stage();
        s.set_fps(30.0);
        let want = Duration::from_secs_f32(1.0 / 30.0);
        // 合成模型没有动画通道 → 姿势纹丝不动,这正是以前会被降频的情形
        s.tick(0.05);
        s.tick(0.05);
        assert_eq!(s.tick_interval(), want, "站着不动也该按 30 帧推进");
        // 逼它走起来:待机计时耗尽 + 无聊攒够才会挑目标点(中间可能先做几个表情)
        for _ in 0..600 {
            s.tick(0.1);
            if matches!(activity(&s), Activity::Walk { .. }) {
                break;
            }
        }
        assert!(
            matches!(activity(&s), Activity::Walk { .. }),
            "待机够久该开始走"
        );
        assert_eq!(s.tick_interval(), want, "走起来也还是 30 帧");
    }

    #[test]
    fn walking_reaches_its_target_and_faces_that_way() {
        let mut s = pet_stage();
        for _ in 0..600 {
            s.tick(0.1);
            if matches!(activity(&s), Activity::Walk { .. }) {
                break;
            }
        }
        let Activity::Walk { target_x, .. } = activity(&s) else {
            panic!("没走起来")
        };
        let going_right = target_x > s.actor_pos().0;
        // 朝向与目标方向一致(camera_yaw 的符号在 pet/target.rs 里有回归测试)
        match s.actor() {
            Actor::Pet(pet) => assert_eq!(pet.target_yaw, camera_yaw(going_right)),
            _ => panic!("不是宠物"),
        }
        for _ in 0..200 {
            s.tick(0.05);
            if matches!(activity(&s), Activity::Idle { .. }) {
                break;
            }
        }
        assert!((s.actor_pos().0 - target_x).abs() < 1.0, "该走到目标点");
    }
}

/// 感知与事件总线(design.md §9 Phase 5 第 5 步)。
mod perception_tests {
    use super::*;

    /// 两只宠物,画布 200、本体 120px(一个身位)。
    fn two_pets() -> Stage {
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "Run", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        for seed in 1..=2 {
            stage.spawn(Actor::Pet(PetActor::new(test_build(
                Arc::clone(&model),
                seed,
            ))));
        }
        stage
    }

    #[test]
    fn distance_is_in_body_lengths_not_pixels() {
        // 关键判据:阈值必须随宠物尺寸缩放。同样隔 240px,
        // 一个身位 120 的看是 2 身位(算近),身位 60 的看是 4 身位(算远)
        let mut stage = two_pets();
        stage.entities[0].pos = (0.0, 400.0);
        stage.entities[1].pos = (240.0, 400.0);
        let near = stage.perceive(0).nearest.expect("旁边有一只");
        assert_eq!(near.id, stage.entities[1].id());
        assert!((near.distance - 2.0).abs() < 1e-3, "实际 {}", near.distance);
        assert!(near.on_right, "它在右边");
        // 反过来看是对称的(身位取两只的均值)
        let back = stage.perceive(1).nearest.expect("旁边有一只");
        assert!((back.distance - 2.0).abs() < 1e-3);
        assert!(!back.on_right);

        // 把两只都缩小一半,像素距离不变 → 身位翻倍
        for entity in &mut stage.entities {
            if let Actor::Pet(pet) = &mut entity.actor {
                pet.body_px = TEST_BODY_PX / 2.0;
            }
        }
        let near = stage.perceive(0).nearest.expect("旁边有一只");
        assert!((near.distance - 4.0).abs() < 1e-3, "实际 {}", near.distance);
    }

    #[test]
    fn nearest_is_by_foot_point_and_ignores_self() {
        let mut stage = two_pets();
        stage.spawn(Actor::Sprite(Sprite::test_pattern(64)));
        stage.entities[0].pos = (0.0, 400.0);
        stage.entities[1].pos = (600.0, 400.0);
        // 精灵摆在 0 号右边一点:它比 1 号近,感知不该只认宠物
        stage.entities[2].pos = (100.0, 400.0);
        let near = stage.perceive(0).nearest.expect("旁边有东西");
        assert_eq!(near.id, stage.entities[2].id(), "该取最近的那一个");
        // 台上只剩自己时没有邻居
        let id = stage.entities[0].id();
        let others: Vec<EntityId> = stage
            .entities()
            .iter()
            .map(|e| e.id())
            .filter(|other| *other != id)
            .collect();
        for other in others {
            stage.despawn(other);
        }
        assert_eq!(stage.perceive(0).nearest, None, "自己不算自己的邻居");
    }

    #[test]
    fn neighbours_greet_once_then_go_on_cooldown() {
        let mut stage = two_pets();
        // 挨着站(1.5 身位,在 NOTICE_DISTANCE 之内)
        stage.entities[0].pos = (0.0, 400.0);
        stage.entities[1].pos = (TEST_BODY_PX * 1.5, 400.0);
        let (a, b) = (stage.entities[0].id(), stage.entities[1].id());

        // 待机结束时会发注意意图并当场落地。**判据是冷却表**:只有真走完
        // 「发意图 → dispatch → apply_notice」这条链才会有记录
        for _ in 0..200 {
            stage.tick(0.05);
            // 别让它们走开:这条测的是打招呼与冷却,不是走位
            stage.entities[0].pos.0 = 0.0;
            stage.entities[1].pos.0 = TEST_BODY_PX * 1.5;
        }
        for entity in stage.entities() {
            if let Actor::Pet(pet) = entity.actor() {
                let other = if entity.id() == a { b } else { a };
                assert!(!pet.notice_ready(other), "挨着站该互相打过招呼并进冷却");
            }
        }

        // 打招呼看得见的那一半:转过去 + 播一段动作
        stage.entities[0].pos = (0.0, 400.0);
        if let Actor::Pet(pet) = &mut stage.entities[0].actor {
            pet.notices.clear();
            pet.target_yaw = 0.0;
            pet.activity = Activity::Idle { remaining: 5.0 };
        }
        assert!(stage.apply_notice(Intent {
            from: a,
            kind: IntentKind::Notice,
            target: Some(b),
        }));
        match stage.entities[0].actor() {
            Actor::Pet(pet) => {
                assert!(matches!(pet.activity, Activity::React { .. }), "该播一段");
                assert_eq!(pet.target_yaw, camera_yaw(true), "该转向右边那只");
            }
            _ => panic!("不是宠物"),
        }
        // 对象已经不在台上:当没发生过,不能恐慌
        stage.despawn(b);
        assert!(!stage.apply_notice(Intent {
            from: a,
            kind: IntentKind::Notice,
            target: Some(b),
        }));
    }

    #[test]
    fn a_lone_pet_never_greets() {
        // 台上只有一只时,待机结束该照常去走动/做表情,而不是卡在「等意图落地」
        let mut stage = two_pets();
        let extra = stage.entities[1].id();
        stage.despawn(extra);
        for _ in 0..400 {
            stage.tick(0.05);
        }
        assert!(stage.intents.is_empty(), "没有邻居就不该发注意意图");
        if let Actor::Pet(pet) = stage.entities[0].actor() {
            assert!(pet.notices.is_empty(), "没打过招呼,冷却表该是空的");
        }
    }
}

/// 演出脚本(design.md §9 Phase 5 第 6 步)。
mod act_tests {
    use super::*;

    fn script() -> &'static Script {
        &act::SCRIPTS[0]
    }

    /// 两位正主,挨着站(1 身位),都闲着。
    fn cast_on_stage() -> Stage {
        let model = Arc::new(Model::for_test(&[
            "Idle", "Walk", "Run", "CallOut", "Alert", "Show", "Happy", "Shock",
        ]));
        let mut stage = Stage::new((2000, 600));
        for form_id in script().cast {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                form_id,
                ..test_build(Arc::clone(&model), form_id as u64)
            })));
        }
        stage.entities[0].pos = (400.0, 400.0);
        stage.entities[1].pos = (400.0 + TEST_BODY_PX, 400.0);
        stage
    }

    fn acting(stage: &Stage, index: usize) -> bool {
        match stage.entities[index].actor() {
            Actor::Pet(pet) => pet.acting,
            _ => false,
        }
    }

    /// 推进到开演,返回用掉的秒数。
    fn run_until_start(stage: &mut Stage) -> f32 {
        let mut t = 0.0;
        for _ in 0..400 {
            stage.tick(0.05);
            t += 0.05;
            if stage.performance.is_some() {
                return t;
            }
        }
        panic!("一直没开演");
    }

    #[test]
    fn casting_needs_both_and_close_enough() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        assert!(acting(&stage, 0) && acting(&stage, 1), "两位都该被占住");

        // 隔太远(> max_distance 身位)就不开演
        let mut far = cast_on_stage();
        far.entities[1].pos.0 = 400.0 + TEST_BODY_PX * (script().max_distance + 2.0);
        for _ in 0..400 {
            far.tick(0.05);
        }
        assert!(far.performance.is_none(), "隔太远不该开演");

        // 少一位也不开演
        let mut alone = cast_on_stage();
        let second = alone.entities[1].id();
        alone.despawn(second);
        for _ in 0..400 {
            alone.tick(0.05);
        }
        assert!(alone.performance.is_none(), "少一位不该开演");
    }

    #[test]
    fn same_form_twice_does_not_cast_itself() {
        // 台上两只**同一形态**时,不能拿同一只凑两个角色
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "CallOut"]));
        let mut stage = Stage::new((2000, 600));
        for seed in 0..2 {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                form_id: script().cast[0],
                ..test_build(Arc::clone(&model), seed)
            })));
        }
        assert_eq!(stage.cast_for(script()), None);
    }

    #[test]
    fn a_poke_ends_the_show() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        // 戳一下正在演的那只:受惊走 `react`,acting 被清掉,演出该收场
        let poked = stage.entities[1].id();
        if let Some(Actor::Pet(pet)) = stage.entity_mut(poked).map(|e| &mut e.actor) {
            pet.react(PetReaction::Startled, 0.5);
        }
        stage.tick(0.05);
        assert!(stage.performance.is_none(), "被打断该收场");
        assert!(!acting(&stage, 0) && !acting(&stage, 1), "两位都该放开");
        // 打断的也记冷却:松手之后不该立刻又演一遍
        for _ in 0..400 {
            stage.tick(0.05);
        }
        assert!(stage.performance.is_none(), "冷却里不该再开演");
    }

    #[test]
    fn a_removed_actor_ends_the_show() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        let gone = stage.entities[1].id();
        stage.despawn(gone);
        stage.tick(0.05);
        assert!(stage.performance.is_none(), "演员被撤下该收场");
        assert!(!acting(&stage, 0), "剩下那位该放开");
    }

    #[test]
    fn the_whole_show_runs_and_then_releases() {
        let mut stage = cast_on_stage();
        run_until_start(&mut stage);
        let length = script().length;
        // 演完之前不许有人自己溜达走(`acting` 期间待机不触发 choose_next)
        let mut ticks = 0.0;
        while ticks < length - 0.2 {
            stage.tick(0.05);
            ticks += 0.05;
            assert!(stage.performance.is_some(), "{ticks:.1}s 时不该提前收场");
        }
        // 到点收场,两位都放开
        for _ in 0..20 {
            stage.tick(0.05);
        }
        assert!(stage.performance.is_none(), "到点该收场");
        assert!(!acting(&stage, 0) && !acting(&stage, 1));
        // 所有拍子都放过了
        assert!(
            stage
                .script_cooldown
                .iter()
                .any(|(id, _)| *id == script().id)
        );
    }

    #[test]
    fn a_missing_clip_only_skips_that_beat() {
        // 全库动作覆盖不齐:缺 CallOut 的形态也得能把整场演完
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "Run", "Show"]));
        let mut stage = Stage::new((2000, 600));
        for form_id in script().cast {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                form_id,
                ..test_build(Arc::clone(&model), form_id as u64)
            })));
        }
        stage.entities[0].pos = (400.0, 400.0);
        stage.entities[1].pos = (400.0 + TEST_BODY_PX, 400.0);
        run_until_start(&mut stage);
        for _ in 0..(script().length / 0.05) as usize + 20 {
            stage.tick(0.05);
        }
        assert!(stage.performance.is_none(), "缺动作也该演到收场");
    }

    #[test]
    fn the_walker_ends_up_near_its_partner() {
        // 「跑过去」那一拍真的把它带到对方旁边:开演时隔 2 身位,`Approach{gap:1.3}` 之后
        // 该落在 1.3 身位附近
        let mut stage = cast_on_stage();
        stage.entities[1].pos.0 = 400.0 + TEST_BODY_PX * 2.0;
        run_until_start(&mut stage);
        // Approach 在第 2.0 秒,跑完给到 3.5 秒
        for _ in 0..70 {
            stage.tick(0.05);
        }
        let a = stage.entities[0].foot_point().0;
        let b = stage.entities[1].foot_point().0;
        let gap = (a - b).abs() / TEST_BODY_PX;
        assert!((gap - 1.3).abs() < 0.4, "该停在 1.3 身位左右,实际 {gap:.2}");
    }
}

/// 叫声(design.md §7 与 §9 Phase 6)。
mod voice_tests {
    use super::*;

    /// 一层假声音:字节内容无所谓,测试不解码。键是动作逻辑名。
    fn layer(keys: &[&str]) -> std::collections::HashMap<String, Arc<crate::audio::Pcm>> {
        keys.iter()
            .map(|k| (k.to_string(), Arc::new(crate::audio::Pcm::for_test())))
            .collect()
    }

    /// 一套只有叫声、没有动作音效的假库(音效层单独测)。
    /// 曲线取实测最常见的 ±300 音分。
    fn bank() -> Arc<VoiceBank> {
        Arc::new(VoiceBank {
            clips: layer(&["Happy", "Shock", "CallOut", "Relax"]),
            sfx: std::collections::HashMap::new(),
            cents_low: -300.0,
            cents_high: 300.0,
        })
    }

    fn pet_with_voice(voice_value: f32) -> Stage {
        let model = Arc::new(Model::for_test(&["Idle", "Walk", "Shock", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            ..test_build(model, 7)
        })));
        if let Actor::Pet(pet) = &mut stage.entities[0].actor {
            pet.voice_value = voice_value;
        }
        stage
    }

    #[test]
    fn pitch_follows_the_voice_attribute() {
        // 游戏里 voice ∈ −100..100 经 RTPC 曲线变调,而 Wwise 的 pitch 就是重采样,
        // 所以这里是「按 2^(音分/1200) 调播放速率」。0 = 原声、+1 = 婉转、−1 = 粗嗓门
        for (value, cents) in [(0.0, 0.0), (1.0, 300.0), (-1.0, -300.0), (0.5, 150.0)] {
            let mut stage = pet_with_voice(value);
            let id = stage.entities[0].id();
            stage.speak(id, "Happy");
            let cues = stage.take_sounds();
            assert_eq!(cues.len(), 1, "voice={value} 该出一声");
            assert!(
                (cues[0].speed - speed_for_cents(cents)).abs() < 1e-6,
                "voice={value} 该按 {cents} 音分放,实际速率 {}",
                cues[0].speed
            );
        }
    }

    #[test]
    fn cues_are_drained_once() {
        let mut stage = pet_with_voice(0.0);
        let id = stage.entities[0].id();
        stage.speak(id, "CallOut");
        assert_eq!(stage.take_sounds().len(), 1);
        assert!(stage.take_sounds().is_empty(), "收过就没了,不会一直重放");
    }

    /// 两层一起响,而**只有叫声那层变调** —— 动作音效来自 `Pet_Action_*` 库,
    /// 650 个里只有 1 个挂了 `Pet_Vo_Pitch` 曲线,那条 RTPC 是给嗓子的。
    #[test]
    fn both_layers_play_and_only_the_voice_is_pitched() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips: layer(&["Happy"]),
                sfx: layer(&["Happy"]),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            voice_value: Some(1.0),
            ..test_build(model, 11)
        })));
        let id = stage.entities[0].id();
        stage.speak(id, "Happy");
        let speeds: Vec<f32> = stage.take_sounds().iter().map(|c| c.speed).collect();
        assert_eq!(speeds.len(), 2, "叫声与音效该一起响");
        let want = speed_for_cents(300.0);
        assert!((speeds[0] - want).abs() < 1e-6, "叫声该变调: {speeds:?}");
        assert!((speeds[1] - 1.0).abs() < 1e-6, "音效不该变调: {speeds:?}");
    }

    /// 声音跟着**动作那张降级表**退。没有 `Shock` 只有 `Alert` 的形态,动作退到 Alert,
    /// 声音也得退到 Alert —— 两边各退各的就会出现「做着警觉的动作、叫着受惊的声」。
    #[test]
    fn sound_falls_back_along_the_same_table_as_the_clip() {
        let model = Arc::new(Model::for_test(&["Idle", "Alert"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips: layer(&["Alert"]),
                sfx: std::collections::HashMap::new(),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            ..test_build(model, 13)
        })));
        let id = stage.entities[0].id();
        assert_eq!(fallbacks("Shock"), ["Alert"], "这条测试依赖降级表的这一行");
        stage.speak(id, "Shock");
        assert_eq!(stage.take_sounds().len(), 1, "该退到 Alert 那段,而不是哑掉");
    }

    /// 自发的声音有冷却,**人点出来的没有**。
    ///
    /// 待机表情大约每 20~40 秒一个,做一次响一次的话桌上那只每半分钟叫你一嗓子;
    /// 而受惊/点动作是人要它出声的,连点就该连响。
    #[test]
    fn spontaneous_sounds_are_rationed_but_asked_for_ones_are_not() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            ..test_build(model, 19)
        })));
        // 这只没有 `Sad`(bank() 只有四段):**没出声就不该占掉这一分钟**
        let Actor::Pet(pet) = &mut stage.entities[0].actor else {
            unreachable!()
        };
        pet.speak_self("Sad");
        assert!(stage.take_sounds().is_empty(), "没这段声音,本来就没得响");

        let Actor::Pet(pet) = &mut stage.entities[0].actor else {
            unreachable!()
        };
        pet.speak_self("Happy");
        // 上一下没出声,冷却不该已经起来
        assert_eq!(stage.take_sounds().len(), 1, "该响");

        let Actor::Pet(pet) = &mut stage.entities[0].actor else {
            unreachable!()
        };
        pet.speak_self("Happy");
        assert!(stage.take_sounds().is_empty(), "冷却里不该再自己叫");

        // 人点的不受冷却管
        let id = stage.entities[0].id();
        stage.speak(id, "Happy");
        assert_eq!(stage.take_sounds().len(), 1, "点出来的该响");
    }

    /// 换键那天,已经下载过的旧包不该整只哑掉。旧包的键是小写的四个触发点名。
    #[test]
    fn packs_exported_before_the_key_change_still_make_a_sound() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips: layer(&["happy", "shock", "callout", "relax"]),
                sfx: std::collections::HashMap::new(),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            ..test_build(model, 23)
        })));
        let id = stage.entities[0].id();
        for name in ["Happy", "Shock", "CallOut", "Relax"] {
            stage.speak(id, name);
            assert_eq!(stage.take_sounds().len(), 1, "旧包的 {name} 该还认得出来");
        }
    }

    /// 配置窗口那张动作表点一下,**动作与声音一起来**。
    /// 原来点出来的动作是全程哑的:`play_clip` 压根不出声。
    #[test]
    fn the_action_table_makes_a_sound_too() {
        let model = Arc::new(Model::for_test(&["Idle", "Happy"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            ..test_build(model, 17)
        })));
        let id = stage.entities[0].id();
        assert!(stage.play_clip(id, "Happy"));
        assert_eq!(stage.take_sounds().len(), 1, "点动作该出声");
    }

    #[test]
    fn a_poke_cries_and_a_pickup_does_not() {
        // 受惊要出声;**被拎起来不出声** —— 拖动时指针一动就可能重入,
        // 叫起来会连成一串
        let mut stage = pet_with_voice(0.0);
        let (x, y) = (500.0f64, (600.0 - GROUND_MARGIN - 90.0) as f64);
        stage.handle(StageEvent::PointerPressed { x, y });
        stage.handle(StageEvent::PointerReleased);
        assert_eq!(stage.take_sounds().len(), 1, "点一下该受惊出声");

        stage.handle(StageEvent::PointerPressed { x, y });
        stage.handle(StageEvent::PointerMoved { x: x + 60.0, y });
        assert!(stage.take_sounds().is_empty(), "拎起来不该出声");
    }

    #[test]
    fn a_form_without_that_clip_stays_silent() {
        // 全库叫声覆盖不齐:缺哪一段就不出声,不能恐慌也不能放错的那段
        let model = Arc::new(Model::for_test(&["Idle"]));
        let clips = layer(&["Happy"]);
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(Arc::new(VoiceBank {
                clips,
                sfx: std::collections::HashMap::new(),
                cents_low: -300.0,
                cents_high: 300.0,
            })),
            ..test_build(model, 3)
        })));
        let id = stage.entities[0].id();
        stage.speak(id, "Sad");
        assert!(stage.take_sounds().is_empty(), "缺 Sad 就该没声");
        stage.speak(id, "Happy");
        assert_eq!(stage.take_sounds().len(), 1, "有 Happy 就该有声");
    }

    #[test]
    fn no_voice_bank_is_silent() {
        // 没导出叫声的形态(或者用户把音量设成 0)照样得能跑
        let model = Arc::new(Model::for_test(&["Idle", "Shock"]));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(Actor::Pet(PetActor::new(test_build(model, 5))));
        let id = stage.entities[0].id();
        stage.speak(id, "Shock");
        assert!(stage.take_sounds().is_empty());
    }

    /// 不设嗓音就是**原调**,四只同物种听着就该一样 —— 以前这里是上台随机掷,
    /// 于是同一只每次启动都换个嗓子;现在要变声得自己去配置窗口里重掷。
    #[test]
    fn pets_sound_the_same_until_someone_rerolls() {
        let model = Arc::new(Model::for_test(&["Idle"]));
        let mut stage = Stage::new((1000, 600));
        for seed in 1..=3u64 {
            stage.spawn(Actor::Pet(PetActor::new(PetBuild {
                voice: Some(bank()),
                ..test_build(Arc::clone(&model), seed * 7919)
            })));
        }
        // 第四只手动设过嗓音(配置窗口里重掷出来的那种)
        stage.spawn(Actor::Pet(PetActor::new(PetBuild {
            voice: Some(bank()),
            voice_value: Some(-0.6),
            ..test_build(Arc::clone(&model), 42)
        })));
        let ids: Vec<EntityId> = stage.entities().iter().map(|e| e.id()).collect();
        for id in ids {
            stage.speak(id, "Happy");
        }
        let speeds: Vec<f32> = stage.take_sounds().iter().map(|c| c.speed).collect();
        assert_eq!(speeds.len(), 4);
        assert!(
            speeds[..3].windows(2).all(|w| (w[0] - w[1]).abs() < 1e-6),
            "没设过嗓音的该是同一个音调: {speeds:?}"
        );
        assert!(
            (speeds[3] - speeds[0]).abs() > 1e-4,
            "重掷过的那只该听得出来不一样: {speeds:?}"
        );
    }
}

mod rate_tests {
    use super::*;

    /// 每一档都要真的落到推进间隔上,**空台也算** ——
    /// 台上没有宠物时也得有个合法的间隔,否则定时器排不出下一次。
    #[test]
    fn every_step_reaches_the_interval() {
        let mut stage = Stage::new((800, 600));
        for (fps, _) in crate::control::FPS_STEPS {
            stage.set_fps(*fps as f32);
            assert_eq!(
                stage.tick_interval(),
                Duration::from_secs_f32(1.0 / *fps as f32),
                "{fps} 帧没落到间隔上"
            );
        }
    }

    /// 默认值必须是配置里那个,否则「没配过」的台和「配了默认值」的台跑得不一样。
    #[test]
    fn a_fresh_stage_runs_at_the_configured_default() {
        let stage = Stage::new((800, 600));
        assert_eq!(
            stage.tick_interval(),
            Duration::from_secs_f32(1.0 / crate::config::DEFAULT_FPS as f32)
        );
    }
}

mod behaviour_tests {
    use super::*;

    fn pet_stage() -> Stage {
        let model = Model::for_test(&[
            "Idle",
            "Walk",
            "Run",
            "Shock",
            "SleepStart",
            "SleepLoop",
            "SleepEnd",
        ]);
        let actor = Actor::Pet(PetActor::new(test_build(Arc::new(model), 99)));
        let mut stage = Stage::new((1000, 600));
        stage.spawn(actor);
        stage
    }

    fn pet(stage: &Stage) -> &PetActor {
        match stage.actor() {
            Actor::Pet(pet) => pet,
            _ => panic!("不是宠物"),
        }
    }

    /// 推进 `seconds` 秒(按 30Hz 切片),中途 `stop` 成立就停。
    fn run(stage: &mut Stage, seconds: f32, stop: impl Fn(&Stage) -> bool) -> f32 {
        let dt = 1.0 / 30.0;
        let mut elapsed = 0.0;
        while elapsed < seconds {
            stage.tick(dt);
            elapsed += dt;
            if stop(stage) {
                break;
            }
        }
        elapsed
    }

    #[test]
    fn boredom_builds_while_idle_and_drains_while_busy() {
        let mut s = pet_stage();
        run(&mut s, 3.0, |_| false);
        let bored = pet(&s).needs.boredom;
        assert!(bored > 0.3, "待机该攒无聊,实际 {bored}");
        // 走起来之后无聊会被消掉
        run(&mut s, 60.0, |s| {
            matches!(pet(s).activity, Activity::Walk { .. })
        });
        run(&mut s, 1.0, |_| false);
        assert!(pet(&s).needs.boredom < bored, "动起来该消无聊");
    }

    #[test]
    fn sleeps_when_sleepy_and_runs_all_three_phases() {
        let mut s = pet_stage();
        // 直接把困倦顶到阈值,省去等 8 分钟
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| pet(s).is_sleeping());
        assert!(
            matches!(
                pet(&s).activity,
                Activity::Sleeping(SleepPhase::Falling { .. })
            ),
            "该先播入睡,实际 {:?}",
            pet(&s).activity
        );
        run(&mut s, 5.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        assert_eq!(
            pet(&s).activity,
            Activity::Sleeping(SleepPhase::Asleep),
            "该进入睡眠循环"
        );

        // 睡饱了会自己醒:困倦降到 SLEEPY_WAKE_AT 以下
        run(&mut s, SLEEPY_RECOVER_SECS + 5.0, |s| !pet(s).is_sleeping());
        assert!(
            pet(&s).needs.sleepiness <= SLEEPY_WAKE_AT + 0.05,
            "睡够该不困了"
        );
        assert!(
            matches!(pet(&s).activity, Activity::Idle { .. }),
            "醒来该回待机"
        );
    }

    #[test]
    fn poking_wakes_it_up_instead_of_startling() {
        let mut s = pet_stage();
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        let (x, y) = {
            let (px, py) = s.actor_pos();
            (px as f64 + 100.0, py as f64 + 100.0)
        };
        s.handle(StageEvent::PointerPressed { x, y });
        assert!(
            matches!(
                pet(&s).activity,
                Activity::Sleeping(SleepPhase::Waking { .. })
            ),
            "戳一下该转入醒来,实际 {:?}",
            pet(&s).activity
        );
        s.handle(StageEvent::PointerReleased);
        assert!(
            !matches!(pet(&s).activity, Activity::React { .. }),
            "叫醒的那一下不该再算受惊"
        );
        run(&mut s, 5.0, |s| {
            matches!(pet(s).activity, Activity::Idle { .. })
        });
        assert!(matches!(pet(&s).activity, Activity::Idle { .. }));
    }

    #[test]
    fn hovering_makes_it_glance_at_the_pointer() {
        let mut s = pet_stage();
        let (px, py) = s.actor_pos();
        let center_x = px as f64 + 100.0;
        let y = py as f64 + 100.0;
        // 指针在右侧:朝右瞥(幅度小于完整转身)
        s.handle(StageEvent::PointerMoved {
            x: center_x + 40.0,
            y,
        });
        let right = pet(&s).target_yaw;
        assert!((right - camera_yaw(true) * GLANCE_RATIO).abs() < 1e-5);
        assert!(
            right.abs() < camera_yaw(true).abs(),
            "瞥一眼的幅度该小于完整转身"
        );
        // 指针到左侧
        s.handle(StageEvent::PointerMoved {
            x: center_x - 40.0,
            y,
        });
        assert!(pet(&s).target_yaw * right < 0.0, "换边该反向");
        // 指针离开 → 转回正面
        s.handle(StageEvent::PointerLeft);
        assert_eq!(pet(&s).target_yaw, 0.0);
    }

    /// 睡着也照样按目标帧率推进。这里曾经降到 10Hz —— 那条优化取消了。
    #[test]
    fn sleeping_does_not_change_the_frame_rate() {
        let mut s = pet_stage();
        s.set_fps(60.0);
        match s.actor_mut_for_test() {
            Actor::Pet(pet) => pet.needs.sleepiness = 0.99,
            _ => unreachable!(),
        }
        run(&mut s, 30.0, |s| {
            matches!(pet(s).activity, Activity::Sleeping(SleepPhase::Asleep))
        });
        assert_eq!(s.tick_interval(), Duration::from_secs_f32(1.0 / 60.0));
    }
}
