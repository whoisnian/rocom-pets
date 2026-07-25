# Spike S2 — 渲染:wgpu 播骨骼动画 + toon 着色

验收标准见 [design.md](design.md) §9:离屏渲出的形体要与 CPU 参考实现一致、clip 间淡化
无跳变、单只宠物的出帧开销可忽略。**结论:全部达成。**

## 跑法

```sh
# 离屏渲染:每个动作一格,末尾附一格「淡化中点」
cargo run --release -- --render packs/喵喵 --form Gra_MiaoMiao1_001 \
    --clips Idle,Walk,Happy,SleepLoop,Anger -o s2.png
# 顺带测出帧耗时
cargo run --release -- --render packs/喵喵 --bench 600
```

离屏模式不开窗口(不需要桌面环境),所以它同时是 CI 友好的回归入口。

## 实现要点

- `pet/model.rs`:读包里的 glb → 顶点(位置/法线/UV/关节/权重)、骨架(绑定 TRS + 父子 +
  拓扑序 + 逆绑定矩阵)、动画通道、材质。贴图按材质名后缀从 `tex/` 找(`_By` → `T_*_By_D.png`)。
- `pet/anim.rs`:采样 + 交叉淡化 + 蒙皮矩阵。**混合在 TRS 分量上做**(旋转 slerp),
  不在矩阵上插值——矩阵插值会把旋转插成剪切。淡化默认 0.18s。
- `pet/gpu.rs` + `pet.wgsl`:蒙皮在顶点着色器里做(CPU 每帧只算每关节一个矩阵,经
  storage buffer 上传);toon = 基色 + 两段明暗(`smoothstep` 过渡带避免锯齿)+ 边缘光;
  描边是第二遍绘制,法线外扩 + 只画背面,颜色取基色暗版而非纯黑。
- 深度用 `Depth32Float`,投影用 glam 的 DirectX 约定(0..1 深度)配 `CompareFunction::Less`。

## 实测

| 形态 | 顶点/三角 | 关节 | 出帧耗时 |
| --- | --- | --- | --- |
| 喵喵 `Gra_MiaoMiao1_001` | 3079 / 4826 | 44 | **0.040ms/帧** |
| 魔力猫 `Gra_MiaoMiao3_001` | 6366 / 9380 | 103 | **0.054ms/帧** |

耗时含「CPU 采样动画 → 上传矩阵 → 描边 + 本体两遍绘制」,不含表面呈现。相对 60fps 的
16.7ms 预算约 0.3%,单只宠物的开销确实可忽略(RTX 3070 / Vulkan)。

形体正确性:同一个 clip、同一时刻,GPU 渲染与 `tools/verify_glb.py`(纯 CPU、独立按 glTF
规范实现的一版)姿态一致——Idle 站立、Walk 迈步、Happy 起跳后仰、SleepLoop 蜷坐、Anger 张臂、
Show 双爪举起。两套实现互相校验,任一边写错都会立刻看出来。淡化探针那一格是两个姿势之间
的平滑中间态,没有跳变。

## 欠账

- toon 只用了基色贴图;`_M`(遮罩)与 `_ID`(分色)还没接,ramp/描边参数也还是硬编码,
  等 Phase 4 决定包内可调参数时一起做。
- 形变目标(morph target)忽略:导出器带了出来但运行时不用,表情可能会需要(Phase 3+)。
- 离屏取景是按**绑定姿势**包围盒 × 1.35 算的,跳跃类动作仍可能超出画面上沿(纯属这个
  工具的取景问题;stage 上是全屏表面,不存在裁切)。
- 还没接进 stage——那是 Phase 1(把测试精灵换成真宠物、沿屏幕底边行走、状态机)。
