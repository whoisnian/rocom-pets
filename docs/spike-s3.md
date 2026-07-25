# Spike S3 — 导出器:一条进化链 → 带动画的宠物包

验收标准见 [design.md](design.md) §9:把动画合并进 glb、跑通喵喵整条链、确认
walk/run 是否原地循环与单位朝向换算、产出填好的 manifest。**结论:全部达成。**

## 跑法

```sh
dotnet run --project exporter -- --species 3001 --out /tmp/packs [--zip]
uv run --with numpy --with pillow python tools/verify_glb.py /tmp/packs/喵喵 --clips Idle,Walk,Happy
```

导出器要两个输入:游戏 pak(资产)+ rocom-capture 解包出的配置 JSON(表结构)。
后者的原因见 [design.md](design.md) §8:CUE4Parse 只提供 `FRocoBinData` 解码器而**不解 `.non`
schema**,全仓唯一的 `.non` 实现在 rocom-capture 的 `scripts/bin2json.py` 里,不重复造。

## 实测结果(喵喵链 3001 → 3025 → 3007)

| 形态 | 资产 | 动作 | glb | 贴图 | 包围盒高 |
| --- | --- | --- | --- | --- | --- |
| 喵喵 | `Gra_MiaoMiao1_001` | 16/46 | 2.1MB | 5 张 | 80.5cm |
| 喵呜 | `Gra_MiaoMiao2_001` | 16/45 | 3.0MB | 3 张 | 104.2cm |
| 魔力猫 | `Gra_MiaoMiao3_001` | 16/52 | 5.0MB | 3 张 | 204.0cm |

「16/46」= 桌宠动作白名单 16 个全部命中,ANIM_CONF 里其余 30 条是战斗/技能/CG 演出,不导。
整链目录 13MB,`--zip` 后 `.rkpet` 6.9MB。

渲染验证(`tools/verify_glb.py` 自己按 glTF 规范采样 + 蒙皮 + 光栅化,不依赖第三方查看器,
可当回归):三个形态的 Idle/Walk/Run/Happy/Show/SleepLoop/Anger 姿态全部结构正确,无拉伸。

## 关键发现

### 1. CUE4Parse 的 glTF 骨骼旋转约定是错的(已在导出器里修正)

UE 是 Z-up 左手系、glTF 是 Y-up 右手系,CUE4Parse 的转换是交换 Y/Z——**这是个反射**
(det = -1)。位置按 `(x, z, y)` 交换没问题,但旋转不能照抄:反射共轭 `M R M` 会把旋转方向
反过来,正确的四元数是 `(-x, -z, -y, w)`,而上游 `Gltf.cs` 写的是 `(x, z, y, w)`,恰好是
它的共轭(即逆旋转)。

上游一直没暴露,是因为**绑定姿势下蒙皮矩阵 = world × inverse(world) = I**,骨骼旋转错了
也照样渲染正确,而 CUE4Parse 根本不导 glTF 动画。我们一加动画就露馅:整只宠物的耳朵和尾巴
被拉成面条(这就是第一版渲出来的样子)。

修正要做两件事,只改一件都不行:

1. 用正确映射改写所有骨骼节点的局部旋转;
2. **按改过的绑定姿势重算 `inverseBindMatrices`**——IBM 是上游按错旋转烘出来的,
   不重算的话 `world_anim × IBM_bind` 依然是乱的。

实现在 `exporter/GlbBuilder.cs` 的 `FixBindPose`,值得给上游提 PR。
**这条是回归重点**:上游哪天改了约定,`SwapYz` 与 `FixBindPose` 都要跟着改,改完必须重跑
`tools/verify_glb.py` 肉眼确认。

### 2. root motion 逐 clip 不一致,不能假设

位移方向恒为 glTF **+Z**(即 UE +Y,这些骨架朝 +Y 而不是常见的 +X)。但同一条链里
带位移和原地循环是混着的:

| 形态 | Walk | Run |
| --- | --- | --- |
| 喵喵 | 53cm / 1.13s → **47cm/s** | 180cm / 0.6s → **300cm/s** |
| 喵呜 | 60cm / 1.07s → **56cm/s** | **原地**(0cm) |
| 魔力猫 | **原地**(0cm) | 800cm / 1.07s → **750cm/s** |

所以 manifest 逐 clip 给 `in_place` / `root_motion_cm` / `speed_cm_s`,运行时:
有位移就用这个速度推进位置并原地循环播放(不必解析 root motion 曲线),没有就按
locomotion 类型取默认速度;并且要对离谱值钳制(魔力猫 Run 的 7.5m/s 显然是冲刺演出)。

### 3. 单位与朝向

glb 是米制(UE 厘米 × 0.01)。`height_cm` 取 `USkeletalMesh.ImportedBounds` 的 Z 向全高,
链上三个形态 80/104/204cm,与进化阶段一致,可直接用来换算屏幕像素。

### 4. 体积高于设计估算

设计估每形态 ≈2MB,实测 2.1–5.0MB(动画通道是主要占比,魔力猫 105 个骨骼节点 × 16 段动画)。
已做的优化:恒定轨道不写通道、恒定但非绑定值只写一个关键帧(这一步把最大的形态从 6.5MB 压到 5MB)。
仍待做(排 Phase 4):关键帧精简(丢掉能被线性插值还原的键)、贴图降到 512、KTX2/BasisU、
按需只导当前启用的形态。683 个形态全量仍在 GB 级,与设计的分包结论一致。

## 未做/欠账

- 贴图没塞进 glb,独立成文件 + manifest 里给映射(材质名后缀 `_By/_Es/_Mh` ↔ `T_*_<槽>_D`)。
  等 Phase 4 定了 toon shader 要哪些贴图再决定要不要内嵌基色。
- 贴图仍是 PNG,没转 KTX2。
- 叫声(Phase 6)与行为脚本(Phase 5)还不在包里。
- 只跑了喵喵链;全量 683 形态的覆盖率报告要等 Phase 4 批量导。
