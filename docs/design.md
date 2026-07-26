# 设计方案

跨平台桌面宠物：把《洛克王国：世界》的宠物模型/动作/叫声做成本地生成的「宠物包」，
由一个原生运行时在桌面上播放、交互、互动。本文是实现前的方案定稿，含待验证项与分阶段计划。

- 目标平台：**Windows 10+** 与 **KDE Plasma Wayland**(kwin_wayland)。**不支持** GNOME/Mutter 等
  不实现 wlr-layer-shell 的合成器，也不做 X11 回退——只维护两个后端，省下的复杂度换取实现深度。
- 资产提取链路已在 [rocom-capture](../../rocom-capture) 里验证过(CUE4Parse 解包 + 骨骼网格/动画导出)，
  本仓库只做**运行时**与**打包导出器**。
- 原始素材与生成的宠物包**都不入仓库、不分发**，见 §11。

## 0. 目标与非目标

**做**：宠物在桌面上待机/行走/奔跑/睡觉/情绪动作；鼠标交互(点击受惊、摸头、拖放)；
点击穿透可开关；多宠物同时在场并有跨物种互动；部分宠物叫声；按需启用的宠物包。

**不做**：还原游戏的战斗/技能演出、场景与 BGM；1:1 复刻游戏的自研卡通着色器；
移植游戏的行为树；任何联网/账号功能(抓包统计是 rocom-capture 的事)。

## 1. 已验证的数据事实

以下都在解包数据上实测过，是方案的地基(2026-07-25，客户端 pak 对应 `GAME_RocoKingdomWorld`)。

| 事实 | 结论对方案的影响 |
| --- | --- |
| 宠物资产在 `NRC/Content/ArtRes/AnimSequence/Pets/<Asset>/`：1001 个目录、683 个带 `SKM_*`(蒙皮网格)，Pets 下 AnimSequence 共 2.9 万个 | 导出器的输入根；`ArtRes` 默认被 unpack.sh 排除，要 `--filter` 单独导 |
| 喵喵 `SKM_Gra_MiaoMiao1_001_Skin`：4 级 LOD、LOD0 3095 顶点/4826 三角、44 骨骼、3 材质槽、8 套 UV + 顶点色 | 单只宠物的量级极小，多实体常驻可行；LOD1 可作为省内存档 |
| 每只宠物 `Animation/` 约 60 个序列，含 `World_Idle/Walk/Run/Jump_Fall/Hide_*`、`Common_Happy/Sad/Anger/Fear/Relax/Show/Sleep_{Start,Loop,End}`、`Fight_*`、`Ride_Hug`、LookAt BlendSpace | 桌宠要的动作全都有，不需要自己做动画 |
| 动作是**配置驱动**的：`MODEL_CONF.anim_conf_id` → `ANIM_CONF` 给出逻辑动作名 + 毫秒时长，`ANIM_ID_CONF` 给 id→名。喵喵 30+ 条 | 包 manifest 的「逻辑动作 → clip」可自动生成，并能出每只宠物的动作覆盖率报告 |
| 逻辑名到资产名有规律：`Idle→World_Idle`、`Walk→World_Walk`、`SleepLoop→Common_Sleep_Loop` | 去前缀 + 忽略下划线大小写即可对齐，对不上的进报告人工兜 |
| `anim_conf_id` 可以不等于 `model_conf` 的 id(珀尔鼬 model 14765 / anim_conf 14641) | 必须从 MODEL_CONF 读，不能拿 model id 当动作表 id |
| 动画是 **ACL 压缩**的 | 导出器依赖 CUE4Parse-Natives 带 ACL 编译，见 §8 |
| 进化链可从配置归组：`PETBASE_CONF.stage / evolution_pet_id`，且资源目录名数字后缀 = 阶段。喵喵链 = 3001 喵喵(`Gra_MiaoMiao1_001`) → 3025 喵呜(`…2_001`) → 3007 魔力猫(`…3_001`) | 「一条链一个包」可完全自动切分 |
| `PETBASE_CONF` 含测试行与重复行(`9901 测试喵喵1`、`32000001 喵喵`) | 导出器要过滤：名称含「测试」、id 段异常、`legal_petbase` 等字段 |
| `INTERACTIONTREE_CONF` 有「摸头」「亲昵」「查看信息」并带动作键 | 交互动作有官方对应关系可循 |
| `NRC_AI_BEHAVIOR_CONF`(3077 行)只是指向 `Modules/AI/BehaviorTree/MFBT/…` 的自研 Dots 行为树资产 | **不移植**；但 `editor_name` 是中文可读的(如「【毛头小蛛】主动清扫」)，可作为「这只宠物该有什么行为」的选型参考 |
| 捕尘长绒的资产家族是 `Wor_MaoTouXiaoZhu2_001`(毛头小蛛)，AI 表里正有「【毛头小蛛】主动清扫」 | §6 的第一个互动样例(珀尔鼬 × 捕尘长绒)有据可依 |
| 叫声在 `WwiseAudio` 的 `Pet_Vo_*.bnk` + 流式 wem，`PetData.voice` 选组；粗嗓门/婉转声是运行时 Wwise pitch RTPC，wem 本身中性 | 复用 rocom-petvo 的提取管线；变调用播放速率复刻 |
| CUE4Parse 的 BC7 解码有 R/B 通道对调的上游 bug | 导出贴图时必须换回，参照 rocom-capture 的 `FixBc7ChannelOrder` |
| **法线一直是错的:上游把切线写进了 NORMAL。** `FPackedNormal(FVector)` 少了两层括号(`vector.X + 1 * 127.5` 是 `X+127.5`,要的是 `(X+1)*127.5`),又踩了 C# 里 `+` 比 `<<` 紧的优先级 —— `a + b << 8 + c << 16` 实际被解析成 `((a+b) << (8+c)) << 16`,三个分量搅成一个数。本作的切线基是**高精度**的(`FPackedRGBA16N`),而它降到 8 位**只经这个构造函数**,于是 TangentX 与 TangentZ 出来是同一个值。实测:glb 里 `NORMAL · TANGENT` 中位 **+1.000**(本该正交)、面法线·顶点法线 中位 **−0.12 ~ −0.71**(平面四边形的眼/嘴 100% 的面都反);修好后分别是 **≈0** 与 **+0.996~+0.999** | 打补丁:`exporter/patches/0001-fix-FPackedNormal-quantize.patch`(降到 8 位有 ≤0.45° 量化误差,可接受)。**导出器启动时硬拦一道**(`PackedNormalRoundTrips`):未打补丁直接退出,因为模型看着仍然正常、只有光照错,静默产出坏包比失败更糟。这条是**一大类观感问题的共同根因**——matcap 闪烁、两段明暗、边缘光、以及我为了压住「整只发白」加的那些底与增益,全是在错法线上调出来的 |
| **材质实例是能正常读的**——「`UMaterialInstance.Deserialize` 抛 OverflowException」这条旧结论**是错的**。实测本作 2792 个宠物材质全部强类型加载成功、参数一条不少。当初大概是把 CUE4Parse 为**别的**资产刷的 OverflowException 日志当成了材质的 | 贴图**不按命名约定猜**,直接读材质的 `TextureParameterValues`:`BaseTex`=本体基色、`EyeTex`=眼/嘴基色,`MaskTex/MainTex/StarStickTex/MatCap/Noise` 是别的通道。参数是继承的,要顺 `Parent` 链合并(子覆盖父) |
| **alpha 的含义分两种,由基色参数名决定**:`EyeTex`(眼/嘴)的贴图是**带透明背景的表情图集**,alpha 是真遮罩;`BaseTex`(本体)的 alpha 是美术塞的遮罩通道(813 张 `_By_D` 里 160 张通过率 <95%、60 张 <5%) | 导出器把这个区分写进 manifest 的 `mask_alpha`:载入时本体贴图的 alpha 刷成 255、表情图集原样保留,shader 里一个统一的 alpha 测试就够。阈值用材质给的 `OpacityMaskClipValue`,全量实测都是 0.3333 |
| **材质名在「资产文件名」与「对象名」之间大小写会漂,方向还不一致**:喵呜的文件是 `MI_Gra_MiaoMiao2_001_By`、对象名是 `…Miaomiao2…`,魔力猫正好反过来 | glb 里的材质名取的是**对象名**,所以 manifest 的键也要用对象名,运行时查表再统一小写。两头对不上的表现是**整只宠物一片都画不出来**,而且报错是 wgpu 深处一句 `buffer slice can not be empty`——现在载入时加了空网格守卫,直接说清楚 |
| **材质要从网格的 `Materials` 数组拿,不能去列 `<资产>/Mat/` 目录**:小浣蛋的 `Mat/` 里只有描边材质,本体材质不在那儿;还有资产把材质放在 `Yise/Mat/`(异色变体)。网格声明的槽是权威来源 | 改按网格取后,13 个「材质表为空」的形态全部有解 |
| **shader 反编译那条链的操作细节不在这儿**:取 shader / 认归属 / 反汇编 / 对语义的四步流水线、archive 的二进制布局、踩过的坑,全在 rocom-capture 的 [docs/shader.md](https://github.com/whoisnian/rocom-capture/blob/master/docs/shader.md) | 这张表只记**结论**(读出来的公式与判据),工具和格式那边归 rocom-capture(它管解包) |
| **公式的离线来源是 shader library,而且它就在包里**:解包数据来自 **Windows 客户端**,所以 `NRC/Content/ShaderArchive-NRC-PCD3D_ES31.ushaderbytecode`(229MB)里是 D3D 的 DXBC —— 5715 个 shader map、72407 条 shader(pixel 52453 / vertex 19936),裸 LZ4 压,已能全部解出(rocom-capture 的 `scripts/shaderdump.py`) | cooked 包里材质图被剥掉了(editor-only),只剩参数值与静态开关;而编译产物里公式是全的、**静态开关也已在编译期定死** = 「这个材质实际跑的是什么」。这是不碰设备、不碰反作弊就能拿到 ground truth 的路子 |
| **shader 反过来认不出材质,但绕过去了**:archive 里没有任何材质名(搜 `M_P_Object`/`XingGuang` 零命中);`.stable.upipelinecache` 里也只有哈希;DXBC 的反射段 `RDEF` 被剥了,只剩 `ISGN`/`OSGN`/`SHEX` | 归属靠**反向比对**:把材质导成原始字节(rocom-capture 的 `unpack.sh --raw`,因为哈希在 CUE4Parse 不解的 cooked resource 段里),再拿 archive 的 5715 条 `ShaderMapHashes` 逐条 memmem 那个 `.uexp`。实测幽星光 `Fx1`(那两颗球)命中 24 个 shader map、`By` 18 个、眼睛 1 个,材质之间基本不重叠 |
| **反汇编本机就能做,不用 Windows**:wine 自带的 `d3dcompiler_47.dll` 就导出 `D3DDisassemble`(wine 11 由 vkd3d-shader 实现),编译宿主用 wine 自己的 `winegcc` | 工具是 rocom-capture 的 `scripts/dxbcdis.c`,抽查 31 条全部成功。所以**不需要抓帧**:安卓要 root、iOS 要越狱、Windows 要赌 ACE,而这条完全离线、零封号风险 |
| **DXBC 的 `ISGN` 签名段能把 `vN` 寄存器对回语义名** —— 这是把汇编接回资产的关键一环。幽星光球的片元着色器:`v2` = TEXCOORD0、`v3` = TEXCOORD1;配 UE 的 UV 打包规则(TEXCOORD0 = UV0.xy + UV1.xy、TEXCOORD1 = UV2.xy + UV3.xy),汇编里那句 `r4.xy = v2.zw; r4.z = v3.x` 就是 **`(UV1.x, UV1.y, UV2.x)`** | 内部层的采样起点由此确定,不用猜。工具是 `sig.py` 那 40 行(解 ISGN/OSGN)。实测幽星光那两颗球 UV1 恒为 0、UV2.x 每颗球一个区间 —— 起点几乎是每颗球一个常量,空间变化全来自「折射方向 × 深度」,所以每颗球看到的是星场里**以某点为心的一个圆盘**,这才是「每颗球都稳定居中一颗星、两颗球各是星和圆点」的机制 |
| **同一族的两个变体可以是完全不同的 shader,必须逐个反汇编确认。** 暮星辰那两颗球的直接父是 `..._Trans_XingGuang_Fresnel`(祖父才是 `Trans_MatCap`),它的片元着色器只有 223 行 / 4 张贴图,是 `N·L` + 遮罩 → **一维 `RampTex` 行查**(采样 v 是常数 1/256),**既没有折射也没有三向投影** | 所以内部层的判据只能是「**直接父**就是 `MI_P_Object_Trans_MatCap`」。按「父链里含 Trans_MatCap」判会把暮星辰的球也算进来,给它套上一套它压根没有的画法(踩过) |
| **卷动色带是「混色」不是「相乘」**:色带图本身就是成品颜色(暮星辰那张是青↔粉竖条纹),而基色图里环带那一条是**纯粉**。相乘等于「粉 × 青」→ 出来是蓝,实机是真青。`FlowPower`(暮星辰 0.8)就是混色权重 | 改成 `mix(固有色, 色带, FlowPower)`。这条是用户看出来的:「实机是青粉渐变,现在更像蓝粉渐变」 |
| **色带有 ID 遮罩,不是整片材质都卷**:`MaskTex` 的 **alpha 是离散的材质 ID 台阶**(暮星辰实测 0.0 / 0.27 / 0.50 / 0.72 / 1.0),材质给的 `MaskID Min/Max` 划出该卷的那一档。环带那片是 **0.72**(68.5% 落在 [0.6,0.8] 内),额头与身体中央的黄色装饰是 **0.502**(0% 落在区间内) | 不门控就是黄装饰跟着在黄绿之间来回变,而实机里它们是固定黄。加上门控后实测五个时刻的黄装饰稳定在 (228,184,27),通道差 ≤1 |
| **球「转起来白闪」的根因是法线被写成切线**(见上面法线那条),不是任何一层特效。逐层关掉量「球区域亮度随帧的摆幅」定的位:关星点遮罩数字**一点没动**(它在球上只贡献 +0.5 亮度)、关 matcap 摆幅从 51.7 掉到 30.4 —— 说明 matcap 只是放大器,底下还有 30 的摆动。法线修好后:动作轴摆幅 **51.7 → 2.1**(单帧最大跳 48.8 → 1.9)、观察角轴 **51.6 → 8.5**,且此时开不开 matcap 对摆幅已无差别 | **量闪烁不能用帧间逐像素差** —— 球本身在动,那个数字量的是运动。要量「整片的亮度随帧摆动多少」(每帧取球区域的平均/p99 亮度,看跨帧振幅)。工具是 `flick.py`(复制包 → 只留一个材质槽 → 按需摘掉 `star_tex`/`matcap_tex` → 两条轴各扫一遍) |
| **视线相关的采样层容易「区域白闪」**:玻璃内部层的采样圆盘随视线扫过星场,扫到亮星就是一块白斑 | 这类效果**要么参数全对、要么别开**。内部层仍按此关着导出(代码与查实的采样起点都留着),等 cb 槽位↔参数名解出来再开 |
| **星点遮罩的平铺按 `StarStickTiling` 直接算偏大**:2.5~4 折算下来是「整只宠物上 2~4 格」,星点比实机大一倍以上,看着像「一张图拉伸后投上去」;实机更像原图小尺寸密铺 | 额外乘一个 3 倍(对着截图挑的)。这条是用户看出来的,渲图并排比过:×1 星点明显偏大、×3 与实机的密度和大小最接近、×5 开始发噪 |
| **球内那个形状是 `StarTex`,默认贴图 `T_EMeng003`** —— 一张四角星场:绿色细胞里各一颗蓝青四角星,**alpha 是干净的稀疏四角星遮罩**。它被沿折射光线做三向投影采样、采样坐标再叠上「时间 × 每轴速度」 | 这就是实机里球内那颗星,以及「它自己在动、和球的自转无关」的全部来源。同族的 `T_EMeng002`(彩色细胞 + 三角形 alpha)是另一张三向投影贴图,`T_EMeng004` 是漩涡流场图 |
| **根材质的参数默认值读得到,而且必须读**:`UMaterial` 的 `CachedExpressionData.Parameters` 里有 149 个标量 / 43 个向量 / 13 张贴图的**有序默认值**。顺父链只能合并到根**之前**(根不是 `UMaterialInstance`),所以只在根上给默认、实例没覆盖的参数,导出器至今完全看不见 | 那两颗球的固有色就在这儿:根默认里有 `F94728` 红橙、`64358B` 紫、`FF1BE7` 品红、`FB6FF5` 粉 —— 正好覆盖三个阶段六颗球的颜色,而我之前只能从基色图集的 UV footprint 里取近似色 |
| **名字被剥了,但 `ExpressionGuids` 是桥**:`CachedExpressionData` 只留参数名的**哈希**;但它有一份与值数组同序的 `ExpressionGuids`,而**实例**那边每条参数同时带名字和 `ExpressionGUID` | 全量扫一遍实例收 GUID→名字(实测 395 条),再和根材质的 `ExpressionGuids` 对齐,就给根默认值配上了名字 —— 向量 23/43、贴图 10/13。剩下没名字的是**没有任何宠物实例覆盖过**的参数 |
| **仍未解决:cb 槽位 ↔ 参数名。** shader 里 `cb5[32]`/`cb5[33]` 这种是**编译顺序**的 `VectorExpressions`,和 `CachedExpressionData` 的顺序是两套 | 要对上得解 shader map 里的 `FUniformExpressionSet`。~~里头引用的也是哈希名~~ **这条改了**:参数名在 uexp 里是**可读字符串**(见下面「FMemoryImage」那条),难点是冻结镜像的布局而不是名字丢失。所以「哪个 cb 槽是哪个参数」目前还得靠**从汇编读出的语义**去反推(如「唯一的折射比 = `GlobalRefraction` = 1.3」) |
| **那两颗球是「折射 + 物体内部体积」,不是「平色球 + 高光」**:读 `MI_P_Object_Trans_MatCap` 的 pixel shader 汇编,开头就是 `refract()` 的教科书实现(`k = 1 - eta²(1-cos²)`、`eta*I - (eta*cos + sqrt(k))*N`,eta 来自一个 cb 标量 —— 材质里那个人人都有的 `GlobalRefraction = 1.3`),然后沿折射向量在**物体空间**march(用 `Primitive` 的局部包围盒归一化,对应 `GlobalDepth`) | 这解释了「球内有形状」。我原来那套「基色图集里的平色圆盘 + MatCap 高光」在**机制上**就不对,不是调参能补的 |
| **球内那个形状是三向投影采样两张贴图**:t4 与 t5 各按 `r.yz`/`r.xz`/`r.xy` 采三次,权重是 `pow(\|N\|, 锐度)` 归一化 —— 标准 triplanar。采样坐标里叠了 `View` 的时间乘一个速度参数,还有 `frac`/`sin` 的相位动画 | **正好对上「球内的形状会单独旋转」**:它随时间动,和球自身的自转无关。之前判定「不在导出资产里、是运行时挂的粒子」是**错的** —— 它一直在 shader 里,只是我没往「折射进物体内部」这个方向想 |
| **球的固有色是两个 cb 向量槽按物体空间高度做的渐变**:`lerp(cbA, cbB, smoothstep(高度))`,高度取 `物体空间 y - 包围盒 min`,再过一个对比度重映射。这是从汇编读出的**结构**,可靠 | 一颗材质喂两颗球:两颗球在物体空间高度不同,于是各取渐变的一段 —— 这正好解释「一个材质两颗球两个颜色」 |
| **但那两个 cb 槽是什么,查不出来,而且不能猜。** 判死性反例:曜星光实机两颗球是**橙 (235,125,55) 与紫 (85,45,190)**,而它 `Fx1` 能拿到的颜色(实例覆盖 + 根默认)里**一个橙色都没有** —— `Rim Light/Dark` 是绿 (0,255,103) 和青,`BlackMagicColor` 是极暗紫 (17,9,34),`FlowColor` 是紫红。拿 `Rim LightColor` 当渐变端点在幽星光身上像(它那对正好都是红),换到曜星光就成了两颗绿球 | 所以**固有色这一步先不做**。要落地得先打通 cb 槽位 ↔ 参数名(解 shader map 里的 `FUniformExpressionSet`);另外 cb 向量槽也**不一定是参数** —— UE 的 VectorExpressions 里还混着图里的常量与计算结果,可能压根没有对应的参数名 |
| 另外读到的:t1 是切空间法线贴图(UV0 采样、`*2-1`、重建 z),t2 用 UV0 采样后过一串多项式(还没认出是什么);整条 shader 还有第二层「按三向投影通道再 lerp 一个颜色」和一个总强度权重 | cb 槽位与参数名的对应还没做完(要靠 `CachedReferencedTextures` 的顺序 + 已知参数值反推),所以上面只写结构、不写「哪个参数是哪个」 |
| **那两颗球的 pixel shader 长这样**:24 个 shader map 去重后 140 条 pixel shader,大的 29–36KB、6 张贴图 / 6 sampler / 6 cbuffer,能看到三向投影(triplanar)的采样模式 | cbuffer 与 shader 尾部那串 uniform buffer 名一一对应(`View`/`MobileBasePass`/`Primitive`/`MaterialCollection0`/`MaterialCollection1`/`Material`),**材质参数在最后那个 cb**;贴图槽顺序对得上材质属性里的 `CachedReferencedTextures`(有序数组)。剩下的活是读那几百行汇编、把 cb 槽位与参数名对起来 |
| **材质实例里的静态开关是能读的,而且名字多半是中文**:`StaticParameters.StaticSwitchParameters` 给 `是否使用MatCap`、`GlassySwitch`、`开启黑魔法效果`、`使用顶点色`、`Opacity or OpacityMask`、`是否需要BaseColor流动` 这类,每条带 True/False | 这是「**这个特性到底开没开**」的明写答案,取代原来「美术有没有显式写某个参数」那种间接推断。`bOverride` 一律是 false 而 `Value` 各不相同,说明本作存的是**合并后的有效值**(和 BasePropertyOverrides 一个套路),所以照样顺父链合并、近的覆盖远的 |
| **开关只在美术真的打开时才写进实例**:全量普查里基本都是「N 个开 / 0 个关」(只有 `OpenMetallic` 是 592 关 / 1 开) | **「查不到这一条」≠「关」**,只能理解为「用父材质的默认」。而根材质是 `UMaterial` 不是 `UMaterialInstance`,父链走到它就停了,默认值读不到 —— 所以开关只能拿来**确认「开」**,不能拿来判「关」 |
| **全量开关普查**(2792 个材质):`是否透贴` 192 开(全是眼/嘴)、`OpenNRCMask` 151、`UseDepthOffset` 19、**`是否使用MatCap` 17**、`OpenCustomDepth` 11、**`Opacity or OpacityMask` 11**、`UseNormalDirection` 6、`EyeDistortion` 6、**`是否需要BaseColor流动` 3**、`OpenMetallic` 1(暮星辰的环带);而 `GlassySwitch`(898)、`开启黑魔法效果`(944)、`OpenPetFX`(915)、`是否开启Xray`(343)、`使用顶点色` 一个都没开 | 后面这批印证了几处决定:`MainTex`(`Tex_PetGlassy_007_D`,红绿双通道斑点噪声)那条路没启用、`BlackMagicColor` 没参与、顶点色不用 —— 都是之前靠观察得出的,现在有明写的依据 |
| **`是否使用MatCap` 才是 matcap 的判据**:17 个材质开着,而原来的启发式「美术显式设了 `MatCapColor`」数目也正好 17 个,但**对错各有两处**(多算了果冻与翡翠水母、漏了莫比乌乌与风铃鲨三阶) | 改用开关。渲出来几乎没变(那四只平均差 ≤0.02/255)—— 这不是观感修复,是把判据从推断换成明写 |
| **`是否需要BaseColor流动` 只有 3 个开**(空空颅一族),而按「美术给了 `Flow_U_Speed`」会判出 35 个 | 卷动色带的判据改成「`UVFlow` 族(公式写死在父材质里,18 个)或这个开关」。那多出来的 17 个是火焰族(火花/迪莫/守夜烛/黑猫密探):它们的 `Flow_U_Speed` 是给**特效层自己的噪声卷动**用的,不是给固有色叠色带 |
| **`Opacity or OpacityMask` 指出真正半透的只有 11 个**:暮星辰的裙子、果冻、牵线木偶、春团、小甲虫与蜜蜂的翅膀(`By1`)。而我按「不透明度真的小于 1」挑出来的那批和它**零交集**(那批全是纯特效层,本来就要混合) | 这条与实机一致:幽星光那两颗球**不在**名单里,所以当不透明画是对的(也就不会闪)。但暂时没落实成真半透 —— 不透明度的来源推不出来,见下一条 |
| **拿基色 alpha 当那 11 个的不透明度,行不通**:蜜蜂/甲虫翅膀的 alpha 是双台阶(131 与 255、112 与 255),很像「翅膀半透、其余实心」;但暮星辰裙子的 alpha 中位数只有 27、99.4% 低于 250,当不透明度会让裙子几乎消失(它那张是线条遮罩) | 同一组开关里 alpha 的语义并不统一,不敢一刀切。留在横向待办 |
| **材质资产悬空 = 这只宠物没做完**:13 个形态的材质包在 pak 里根本不存在,而它们全是未实装的——4 个名字直接带「占位」,全部 `legal_petbase`/`completeness` 皆空,id 集中在最新未上线段 | 照「这个形态这版本没做」跳过(和缺 SKM 那条一样),**不要**猜贴图硬渲。全量 530→517 条链、832→819 个形态 |
| **本体 `_By_D` 的 alpha 是「线条/细节遮罩」**,不是不透明度也不是垃圾:RGB 是完整的固有色图集,alpha 里画着身上的纹路——水灵身上那一道道竖向浅色条纹,白线正好压在纹路位置上(alpha >200 占 21%) | 渲染要**照着它提亮**(实测 1.55 倍贴近实机),而不是刷成 255 忽略掉。刷 255 那版纹路全丢,拿它当不透明度剔像素更糟(身体被啃掉) |
| **有基色的材质也可能挂着 `BLEND_Translucent`**:暮星辰的裙子(`Fx1`)与那两个球(`Fx2`)、幽星光那两个球(`Fx1`)都是 `MI_P_Object_Trans_*` 家族 | 混合模式对**所有**材质生效,不能只看纯特效层。这一族要额外叠 MatCap 高光并把固有色往 `Rim LightColor` 上拉 |
| **但「标着半透」≠「要混合」**:这批材质压根没有 `Opacity` 参数(缺省 1),输出与不透明一模一样 | 判据是「不透明度真的小于 1」。按 `BLEND_Translucent` 一律塞进不写深度的混合通道,幽星光那两颗球就会**闪**——两颗球绕着转、谁盖谁只由索引序决定,前后关系隔一会儿突然对调 |
| **`OpenOpacityAdd`(暮星辰的球 = 0.15)不是「中心不透明度」**:照那个读法算菲涅尔 alpha,球中心只剩 15% 实,渲出来是两团白幽灵;实机截图里这两颗球是**看不透背景的** | 不拿它当透明度。名字里的 Add 大概是「在别处算出的不透明度上再加一点」,单独一个数推不出公式,与其猜错不如不用 |
| **星点/MatCap 层几乎每个材质都挂着贴图,但绝大多数并没有真的启用**——游戏靠静态开关与遮罩通道决定 | 判据取「美术是否**显式**设了 `StarStickTiling` / `MatCapColor`」。无条件叠过一版,整只宠物被冲白(幽星光的 `By` 的 MatCap 槽绑的压根是 `Fx_ID` 描边图)。全量只有 10 / 16 个形态真的启用星点 / matcap |
| **本体 alpha 的线条层不是每只都有**:喵喵/鸭吉吉/治愈兔/大耳帽兜的 `By_D` alpha **恒等于 1**(100% 覆盖),而水灵是 23% | alpha 恒定 = 没有线条信息,提亮必须是空操作。照 alpha 一律提亮会把**整只宠物均匀调亮 55%**——雪影娃娃就是这么被冲淡的,而这个错误藏在「看起来更亮更好看」里,过了一轮才发现 |
| **顶点色存在但是遮罩通道**:四个通道各自 0–1、均值只有 0.2,配合材质里的 `UseVertexColorG` / `RedChannel`/`GreenChannel`/`BlueChannel` 使用 | 直接当颜色乘上去会整体发黑。**不用它**——环带渐变的来源另有其人(下一条) |
| **环带的青↔粉渐变是「卷动色带」,一张现成的渐变图**:暮星辰的 `By` 是 `MI_P_Object_XingGuang_UVFlow_Morph`,给了 `FlowTexture` = `T_..._Fx_D`(青↔粉竖条纹)+ `Flow_U_Speed` = 0.25 + `FlowPower` = 0.8;基色图里环带那一条是**纯粉的**,渐变一点不在里面 | 按 `uv * 平铺 + 速度 * t` 采那张图、按亮度归一化后乘进固有色,渐变就绕着环跑起来了。判据取「美术真给了流速」——`FlowTexture` 槽几乎人人都挂着,只有 UVFlow 族在用 |
| **星点图的形状不在 alpha 里**:`Tex_PetGlassyStar_004` 是张区域图集,红/橙/黄的随机色块、每块中间一颗浅蓝白小星,**alpha 恒为 255**(整张 rgb 均值 (218,119,31));「假半透」族那张则是黑底 + 粉白星点 | 强度取 **min(r,g,b)**:两族的底都是饱和的(色块 / 纯黑),至少一个通道贴近 0,而星芒是浅色的、三通道都高。按 `rgb * a` 算等于把整张橙图糊到表面——暮星辰的裙子从饱和蓝被冲成彩虹糖就是这么来的 |
| **`StarStick` 是「贴在镜头上」而不是「贴在表面上」**:实机里星点不随模型转动,像镜头前挂了一层遮罩投到宠物身上 | 采样坐标取 **NDC**(取景视体是正方的,格子不会被拉扁,平铺数就是「横跨模型几格」) |
| **一只宠物只有一份星点遮罩,而且盖在整只身上**:各材质自己写的贴图与平铺数并不一致(暮星辰:裙子是共享的 `Tex_PetGlassyStar_004` 4×4、身体是自己的 `Fx_D` 1.8×1.8),照各自的画就成了两种星点两种密度叠在一起 | 导出时统一成一份写到**所有**材质上,优先用「假半透」族给的那张(那是宠物自己的星点图,幽星光一族 = `T_Ill_XingGuang1_001_Fx_D`)。全量 18 个形态带这层 |
| **「假半透」族(`..._FakeTrans*`)也是这层星点,而且不流动**:给的是 `NoiseTex`(黑底星点)+ `NoiseTilingSpeed` + HDR 的 `Color02`。幽星光一族的身体看着半透、身上有星星靠它 | 和 `StarStickTex` 走同一条路(按屏幕位置贴的遮罩),只换贴图与着色;`Color02` 的 15 是配着别处衰减用的,只取它的**色相**(按最大通道归一化)。当成会卷动的「体内星光」画过一版,实机里并没有那个流动 |
| **MatCap 实机只取一张图的单通道当标量**,不是 rgb 查表:汇编里是 `sample r2.w, (u, 1-v), t3.yzwx, s3`(目标只写 `.w`、资源 swizzle 第 4 位是 `x` ⇒ 取 **R**),紧接着 `mul r4.xyz, r2.w, cb5[4]`(= `MatCapColor`)。两张 matcap 图实测都是灰度(三通道与亮度相关系数 ≈ 1.000),所以取 R 与取 rgb 数值等价。UV 也对得上:实机 `r4.z = 1 - r4.y`,与 `-dot(n, up)*0.5+0.5` 同一个式子 | ~~「减掉 0.35 的底再归一化」~~ 这条是**猜的,已删**:它把整张图的暗区削成 0,球大部分时间不吃 matcap、高光块扫过来时又猛地一亮,**反而放大了闪烁**。当初那个「不减底就冲成一团白」的现象是**法线错了**的次生症状(见法线那条),不是 matcap 本身的问题 |
| **加上去的几层光是 `max` 合的,不是相加**:汇编里连着两条 `max r2.yzw, matcap*MatCapColor, spec*SpecColor` → `max r2.xyz, 上一步, rim` | 相加会让高光与边缘光在轮廓处叠成一圈白边;取 max 是「哪层亮听哪层」。玻璃族已照改 |
| **那两颗球的颜色就是基色图集里的一片平色圆盘**:把 `Fx1`/`Fx2` 的 UV 三角形栅格化到基色图上一看,每颗球对应的就是一个纯色圆盘(幽星光 = 朱红 (255,96,33) + 琥珀 (255,161,32)、曜星光 = 琥珀 + 蓝 (0,75,241)、暮星辰 = 品红 (205,51,199) + 深藏青 (31,28,78)),`By_M` 那边也是平色圆盘 | 球的固有色直接用它;暮星辰两颗与实机几乎一致,曜星光方向对(琥珀↔橙、蓝↔紫)。UV1/UV2/UV3 都试过,指到的是别处、更不像 |
| **材质里的 `Rim Intensity` 普遍写着 1,那是「没动过的默认值」不是「开了边缘光」** | 只认强度**大于 1** 的(全量 946 个带边缘光的材质里只有 3 个,暮星辰的裙子 = 3 的青色边)。曜星光那两颗球写着强度 1 + 绿色 `Rim LightColor` + `Rim Power` 0.3(= 整颗泛色),而实机里它们是橙的和紫的 —— 拿去混固有色成两颗绿球,当加色叠也把琥珀色球顶成黄绿 |
| **自转的玻璃小件要压成平色**:幽星光那两颗球是单骨骼刚体、一个 Idle 净转 700° 以上,而它们在基色图集里的 UV 落脚处横跨几块**不相干**的色块(橙 (255,123,60)、奶油 (255,248,172)、粉 (222,125,201)、黄 (255,255,63))—— 逐像素采样再让它自转,亮度就在 101↔158 之间来回跳 | 把这种小件的 UV 钉到「颜色最接近本件平均色」的那个顶点上:整件一片平色,转身不再变,两颗球又各留自己的颜色。判据三条并列:玻璃族 + 单骨骼刚体 + 那根骨骼**净转**超一圈。「净转」必须按轴×角向量累加,用转角绝对值累计会把扇翅膀也算进去(实测误判圣羽翼王 71 件) |
| ~~**玻璃不吃两段明暗**~~ **反了,玻璃也吃。** 同一个 pixel shader 里 `mul r3.xyz, 基色, lerp(暗色, 亮色, smoothstep(N·L))` 就在折射/matcap 那些分支的**下游**,没有任何开关把玻璃排除掉 | 撤掉 `lambert = 1.0` 那个特例。原来给的理由是「开口薄壳自转时露出的面一直在换,分两段就整颗球在 0.72↔1.0 之间跳」——**那个跳动来自法线被写成切线**(见法线那条),法线修好后不复存在。撤掉后全库 752 个可比形态:平均亮度中位 **±0.00**(只有玻璃族变),最大变暗 9.7(风铃鲨),莫比乌乌的过曝反而从 0.50 降到 0.45 |
| **两段明暗的两端是「颜色对」而不是灰度系数**,暗部会偏色:`mad r4.xyz, r0.x, cb5[24]-cb5[25], cb5[25]`。而且这是**四对同构槽**之一 —— (24,25)/(28,29)/(32,33)/(36,37) 全部由同一个两段因子 lerp,步长 4,像是四个图层各一对明/暗色 | **还没做**:这四对的参数名是 cb 槽位问题的一部分(见下条)。暂时保留灰度对 (0.72, 1.0),**不猜颜色**。顺带证伪一条:实机写的是 `smoothstep(thr, hi, (N·L+1)*0.5)`,但 `smoothstep(a,b,(x+1)/2)` 恒等于 `smoothstep(2a-1,2b-1,x)` —— **半兰伯特只是换参数,不是结构差异**,原来直接对 `N·L` 取阈值那一步本来就对 |
| **材质参数名在 uexp 里是可读字符串**(`strings` 就能看到 `GlobalRefraction`/`StarStickTex`/`RimColor`…),但**没有 `FMaterialUniformExpression*` 类型名** —— UE4.26+ 的 `FMaterialShaderMap` 是 **FMemoryImage 冻结布局**,不走 FName 序列化 | 所以 cb 槽位↔参数名不是「没有数据」而是「要解冻结镜像」。**cb5 的分区已经定了**:向量表达式占 `cb5[0..53]`、标量表达式紧跟其后按每 float4 装 4 个(`cb5[54..69]`,故标量 #k 在 `cb5[54+k/4]` 的第 `k%4` 个分量)。据此 `cb5[58].w`/`cb5[59].x` 是**相邻的标量 #19/#20**,即两段明暗的 (hi, thr) 是一对连续参数 |
| ~~**那两颗球里那颗单独转动的小星星,在导出的资产里根本找不到**~~ **这条是错的**,见上面折射那几条 —— 它在 shader 里(沿折射光线的三向投影 + 时间动画)。下面这些排查过程仍然成立(贴图里确实没有星形),错在由此推出「是运行时挂的粒子」:`Fx1` 只有 2 个连通分量(两颗球各 129 顶点、都是正球),材质挂的每一张都查过了 —— `BaseTex` 与 `MaskTex` 在球的 UV 处都是平色圆盘、`MainTex`(`Tex_PetGlassy_007_D`)是红绿双通道的斑点噪声、`MatCap`(matcap26/35)是玻璃球高光图、`StarStickTex` 是五角星色块图集;资产目录下也只有 SKM + 材质 + 动画 | 那是运行时另外挂上去的特效组件(粒子/socket 挂件)—— 也正好解释「球内的形状会单独转」:精灵始终朝着镜头,球在它后面自转。要支持得走蓝图/VFX 导出那条路,**目前画不出来**,已记入横向待办 |
| **UV 大量落在 [0,1] 之外**(水灵实测 u/v 都到 -1.0);UE 贴图默认 wrap | 采样器必须 `AddressMode::Repeat`。wgpu 默认是 `ClampToEdge`,会把区间外全压到贴图边缘,图案摊平成纯色 |
| **MatCap 类遮罩要按视空间法线采样**(它是球面反射查找表),不是网格 UV | 拿 UV 采会糊成一块块的斑(水灵的水膜)。相机右/上向量可以直接从 `view_proj` 的行向量取——正交投影没有透视错切,归一化即得。8 个形态用 matcap 遮罩 |
| **特效层的固有色写在颜色参数里,不在贴图里**:火焰是 `Color01`(火花 = (6, 0.8, 0),R>1 的 HDR ⇒ 加色发光)+ Mask/Noise + 流速;水壳是 `MainColor`(水蓝蓝 = (0.19, 0.65, 1)) + `Opacity=0.8` + MatCap | 特效层能近似画出来:主色 × 遮罩 × 卷动噪声。输出**预乘 alpha** 就能一条管线覆盖两种模式——alpha 输出 0 = 加色(dst+rgb),输出不透明度 = 常规半透 |
| **「这个材质画不画固有色」看它有没有 `BaseTex`/`EyeTex`**。纯特效层(火焰、水壳、光晕)压根没有:火花的 `Fx` 父材质是 `M_FX_Fire_Mat`、只有 Mask+Noise;水蓝蓝的 `Fx` 父材质是 `M_Wat_ShuiLanLan_PP`、只有一张 MatCap | 这个判据取代了原来按**几何占比**和**贴图亮度**猜的两套启发式。猜法在幽星光一阶上是错的:它的 Fx 壳占 79% 几何、贴图是黑底粉星点,而材质里 `BaseTex` 指的是**粉色本体贴图**,黑星点绑在 `NoiseTex` 上——我把噪声当基色了 |
| **召唤/落地类动作会整只挪到别处**：喵喵 `CallOut` 起始几帧悬在 y=1.44..2.47m(其余动作都在 0..1.0)，单帧形体只有 1.19 倍，但并集一下取景盒高度就从 0.8m 撑到 2.48m | 量取景盒时丢掉「中心偏离绑定中心超过一个身高」的姿势。阈值不能太紧:**悬浮类宠物**是正常的(空空颅幽灵的 `Alert` 常态浮在 45–56%)，而落地是 160–197%，中间很宽 |
| CUE4Parse 把空 morph target 写成**没有 bufferView 的 accessor**;按 glTF 规范那等价「全零」是合法的，但 Rust `gltf` crate 判 `Missing data` 直接拒绝加载。826 个形态里 32 个带 morph target 且**全部**加载失败 | 导出器 `ExportMorphTargets = false`——我们从不驱动它们(既无 morph 通道也无 `mesh.weights`) |
| 伸展/张翅/小跳的姿势明显超出**绑定姿势**包围盒：120 个抽样形态 × 四个动作各查一次，按绑定盒取景 11 个会裁掉肢体 | 取景用各动作采样的并集包围盒(`Model::motion_bounds`)→ 剩 1 个被裁，代价是画布面积平均 1.64 倍；尺寸换算仍用绑定盒(站姿高度不能随动作变)。采样时剥的位移要**和运行时一模一样**(只剥 root 的 X/Z、保留 Y)，否则带纵向起伏的动作会顶出画布 |
| **CUE4Parse 的 glTF 骨骼旋转约定是错的**：Y/Z 交换是反射，正确四元数是 `(-x,-z,-y,w)`，上游写的 `(x,z,y,w)` 是它的共轭。绑定姿势下 world × IBM = I 掩盖了它，上游又不导 glTF 动画，故从未暴露 | 导出器必须改写骨骼旋转**并重算 inverseBindMatrices**，见 [spike-s3.md](spike-s3.md) §1；这是动画正确性的单点故障，也是回归重点 |
| 走跑动画的 root motion 方向恒为 glTF +Z(= UE +Y，这些骨架朝 +Y)，但**逐 clip 不一致**：同一条链里有的带位移、有的原地 | manifest 逐 clip 给 `in_place`/`speed_cm_s`，运行时两种都要能处理 |
| CUE4Parse 只有 `FRocoBinData` 解码器，**不解 `.non` schema**；全仓唯一实现是 rocom-capture 的 `scripts/bin2json.py` | 导出器读 rocom-capture 产出的配置 JSON，不重复实现，见 §8 |

已验证的端到端结果：喵喵的 LOD0 网格 + 骨架 + `World_Idle/World_Walk/Common_Happy/Common_Sleep_Loop`
经 CPU 蒙皮 + 软件光栅化渲出正确形体、贴图与姿态，说明**网格、骨架、蒙皮权重、动画关键帧、贴图全部可用**。

## 2. 技术选型

**结论：Rust + wgpu + 自写平台窗口层。**

项目的成败不在渲染，而在两个平台集成点：Wayland 的置顶/定位/输入区，Windows 的逐像素
alpha 置顶窗口 + 命中穿透。现成引擎恰好都在这两点撞墙：

| 方案 | 优 | 致命处 |
| --- | --- | --- |
| **Rust + wgpu + 自写窗口层** | 两个平台集成点都能精确控制；单二进制、低内存、多实体便宜；包加载就是读 zip | 场景/骨骼动画/混合/toon 着色要自己写(工作量可控)；无编辑器 |
| Godot 4 | glTF、AnimationTree、PCK 资源包、音频、导出全免费，出原型最快 | Wayland 后端无置顶与定位；`window_set_mouse_passthrough` 不覆盖 Wayland → Linux 只能退回 XWayland |
| Electron/Tauri + three.js | Web 技术栈熟，Windows 上 `setIgnoreMouseEvents` 可用 | Wayland 透明+置顶不可靠；常驻多实体内存代价大；GB 级资产在 JS 侧流式加载别扭 |
| Go(现有栈) | 与 rocom-capture 同语言 | 无可用的 wayland layer-shell 绑定，GPU 生态太薄 |

选定栈的组件：`smithay-client-toolkit`(wlr-layer-shell) / `windows-rs`(Win32 + DirectComposition) /
`wgpu`(Vulkan+DX12，`CompositeAlphaMode::PreMultiplied`) / `gltf` / `kira` 或 `rodio`(带播放速率，
正好复刻叫声变调) / `mlua` 或 `rhai`(行为脚本) / `egui`(配置与包管理 UI，与 wgpu 同栈)。

## 3. 运行时架构

### 3.1 窗口模型：一屏一个透明 stage，宠物是其中的实体

不采用「一只宠物一个窗口」：跨宠物互动、互相拖放、遮挡排序都需要同一个坐标空间与同一个场景，
单 stage 让这些几乎免费；代价(全屏 alpha 合成)可以用提交策略压掉，见 §3.3。

```
 ┌─ stage(每个显示器一个透明置顶表面) ────────────────────────┐
 │  ECS/slotmap: 实体 = {物种/形态, 位置, 状态机, 需求, 脚本VM}  │
 │  ├ 场景更新 → 骨骼动画采样/混合 → wgpu 渲染(premultiplied)  │
 │  ├ 每 N 帧渲一张 64×64 alpha mask → 命中测试 + 输入区        │
 │  └ 事件总线: 鼠标 / 邻近 / 屏幕边界 / 定时器 / 脚本 Intent    │
 └───────────────┬──────────────────────────┬────────────────┘
       平台层 trait│                          │
   ┌───────────────▼────────┐   ┌─────────────▼──────────────┐
   │ KDE Wayland:           │   │ Windows:                   │
   │ wlr-layer-shell        │   │ layered 窗口 + DComp       │
   └────────────────────────┘   └────────────────────────────┘
```

### 3.2 平台层

| 关注点 | KDE Plasma Wayland | Windows |
| --- | --- | --- |
| 表面 | 每 output 一个 layer surface，`layer=top`(不用 `overlay`，那会盖住菜单/通知)，四边 anchor，**`exclusive_zone=0`**，`keyboard_interactivity=none` | 每显示器一个 `WS_EX_LAYERED|TOPMOST|TOOLWINDOW|NOACTIVATE` 窗口 + DirectComposition 交换链 |
| 逐像素 alpha | wgpu `CompositeAlphaMode::PreMultiplied` | 必须 `CreateSwapChainForComposition`(GDI 的 `UpdateLayeredWindow` 路径不适合 GPU 渲染) |
| 命中/穿透 | `wl_surface.set_input_region` = 宠物轮廓并集；全局穿透 = 置空区域 | `WM_NCHITTEST` 返回 `HTTRANSPARENT`；全局穿透 = 加 `WS_EX_TRANSPARENT` |
| 定位 | layer surface 的 anchor + margin | `SetWindowPos`(整屏窗口，宠物坐标在窗口内) |
| 多屏 | `zwlr_layer_shell_v1.get_layer_surface` 指定 `wl_output`，跟随 output 热插拔重建 | 枚举显示器，每个一个窗口 |
| 缩放 | `wp_fractional_scale_v1` 拿精确 scale(1/120 单位) + `wp_viewporter` 把物理像素 buffer 映射回逻辑尺寸;**此时 `set_buffer_scale` 必须留 1**,且要忽略 `wl_output` 的整数 scale 事件 | DPI 感知 + `GetDpiForWindow` |

已确认：开发环境 KDE Plasma 6.7.3 / kwin_wayland，`libkwin.so` 导出 `zwlr_layer_shell_v1` 与
`zwlr_layer_surface_v1`，layer-shell 可用。KWin 相关注意点：

- `zwlr_layer_shell_v1` 是 wlroots 系的**非正式协议**，KWin 只是兼容实现，跨大版本可能变化；
  Phase 0 S1 要记录实测的 KWin 版本，升级 Plasma 后重跑 S1 的验收项。
- `layer=top` 与全屏窗口、锁屏、通知/OSD 的叠放次序由 KWin 决定，不可假设，S1 里逐项实测。
- KWin 的窗口规则/脚本(KWin Script、`kwriteconfig` 规则)可作为定位与置顶的**备选**手段，
  但那是 xdg-toplevel 路线，交互不如 layer surface 干净，仅在 S1 失败时才考虑。
- **`exclusive_zone` 取 0 而不是 -1**:0 是「自己不占地方，但尊重别人占的地方」，合成器给的
  configure 就是**去掉任务栏后的工作区**(实测 2560×1440 → 2560×1368)，宠物正好踩在任务栏
  上沿；-1 是「连别人的独占区一起无视」，那样宠物的脚会藏到面板后面。两者都不会挤压其他窗口布局。

### 3.3 渲染与帧率

- 每形态一个 glb：mesh + skin + 全部所需 clip。骨骼动画在 GPU(或 CPU 蒙皮 + 顶点缓冲上传，
  实体数少时都够)；clip 间做交叉淡入淡出。
- 卡通着色：base color + ramp 光照 + 描边(法线外扩或屏幕空间)。**目标是「像」不是「同」**——
  游戏是自研 shader，含 RampTex/MatCap/描边/StarStick/Fragments 等几十个参数，且材质实例参数还解不全。
- 提交策略(全屏透明层的合成开销主要靠这些压掉)：
  - 无动画/交互时不提交帧；
  - 用 `wl_surface.damage_buffer` / DXGI dirty rect 只提交宠物所在矩形；
  - 空闲降帧(待机 15fps、睡觉 5fps)，交互中 60fps；
  - 前台全屏窗口(游戏/视频)不用自己处理:KWin 会把全屏窗口排在 `layer=top` 之上,
    宠物自然被遮住(S1 实测,见 spike-s1.md W1)。
- 命中测试与输入区共用**低分辨率 alpha mask**：每隔几帧把宠物渲到 64×64 离屏 RT 回读，
  延迟一帧无感，避免每帧 CPU 侧算轮廓。

## 4. 宠物包(插件)

### 4.1 分包原则

**一条进化链一个包**，包内多形态，启用后可在 UI 切换形态(不重导)。链的切分完全由
`PETBASE_CONF.stage / evolution_pet_id` 推出，资源目录名的数字后缀作为交叉校验。

### 4.2 结构

```
<链名>.rkpet                     # zip 归档
├── manifest.toml
├── forms/<asset>/model.glb      # mesh + skin + 已合并的全部 clip
├── forms/<asset>/tex/*.ktx2     # 基色/遮罩，已修正 BC7 通道序
├── voice/*.opus
└── behaviors/*.lua              # 可选:该物种特有行为/互动
```

### 4.3 manifest schema(草案;实际产物见 spike-s3.md 与导出器 `Manifest.cs`)

```toml
schema = 1            # manifest 格式版本
runtime_abi = 1       # 需要的运行时 ABI，运行时拒绝不兼容包
source_version = "…"  # 导出时的游戏版本/pak 指纹，便于排查
generated_at = "2026-07-25"

[species]
id      = 3001        # 链首 PETBASE_CONF.id
name    = "喵喵"
chain   = [3001, 3025, 3007]

[[forms]]
id        = 3001
name      = "喵喵"
stage     = 1
asset     = "Gra_MiaoMiao1_001"
model     = "forms/Gra_MiaoMiao1_001/model.glb"
scale     = 1.00      # MODEL_CONF.model_scale / 100
height    = 80        # 绑定姿势包围盒高度(cm)，用于换算屏幕像素
locomotion= "ground"  # ground|hover|swim ← PETBASE_CONF.move_type
voice     = "voice/vo_3001"
tags      = []        # 互动能力标签，如 ["cleaner"]/["commander"]

  [forms.clips]              # 由 ANIM_CONF 自动生成
  idle   = { clip = "World_Idle",   ms = 1333, loop = true }
  walk   = { clip = "Walk", ms = 1133, frames = 35, in_place = false, root_motion_cm = 53.06, speed_cm_s = 46.8 }
  run    = { clip = "Run",  ms =  600, frames = 19, in_place = false, root_motion_cm = 180,   speed_cm_s = 300 }
  happy  = { clip = "Common_Happy", ms = 1500 }
  anger  = { clip = "Common_Anger", ms = 1500 }
  shock  = { clip = "Common_Shock", ms = 1500 }
  sleep  = { start = "Common_Sleep_Start", loop = "Common_Sleep_Loop", end = "Common_Sleep_End" }
  callout= { clip = "Common_Show",  ms = 1500, voice = "callout" }

  [forms.materials]          # 从游戏材质实例解出来的「这个槽画什么」,见 §1
  # base_color 缺失 = 纯特效层(材质里没有 BaseTex/EyeTex),运行时整片跳过;
  # 这类条目额外记下父链与全部贴图参数,留给将来的特效通道用。
  MI_Gra_Miaomiao1_001_By = { base_color = "forms/…/tex/T_…_By_D.png", mask_alpha = false, mask_clip = 0.3333, blend = "BLEND_Opaque" }
  MI_Gra_Miaomiao1_001_Es = { base_color = "forms/…/tex/T_…_Es_D.png", mask_alpha = true,  mask_clip = 0.3333, blend = "BLEND_Opaque" }

[report]              # 导出覆盖率,缺失动作让运行时降级而不是报错
missing_clips = ["hide"]
```

实际产物比这份草案更细(每 clip 带 `frames`/`root_motion_cm`/`speed_cm_s`，贴图带槽位与尺寸)，
见 [spike-s3.md](spike-s3.md) 与导出器的 `Manifest.cs`。

### 4.4 加载与体积

- 发现路径 `~/.local/share/rocom-pets/packs/`、`%APPDATA%\rocom-pets\packs\`；
  启动只读各包 manifest(轻)，**启用某形态时**才流式读该形态的 glb 与贴图。
- 体积(S3 实测，喵喵链 16 个动作 + 1024 贴图)：每形态 **2.1–5.0MB** glb + 贴图，
  一条链目录 13MB、`.rkpet` 6.9MB。比原估的 2MB/形态高一倍,动画通道是主要占比
  (骨骼数 × clip 数 × 帧数)。
- 压体积手段(已做)：只导桌宠动作白名单、恒定轨道不写通道/只写单帧。
  (待做，Phase 4)：关键帧精简、贴图降到 512、KTX2/BasisU、只导当前启用的形态。

## 5. 动作与行为

- **逻辑动作层**：运行时只认 `idle/walk/run/happy/anger/sad/fear/shock/show/relax/sleep/callout/…`，
  具体 clip 由 manifest 映射，缺失则降级(如无 `run` 就用 `walk` 提速)。
- **三段式(Start/Loop/End)是一等公民**：睡觉、隐藏、技能都是这个结构；Loop 时长由状态机需求决定。
- 每实体一个状态机 + 需求值(困倦/心情/无聊) + 作息时钟；转移由事件驱动：鼠标、邻近实体、
  屏幕边界、定时器、脚本 Intent。
- 进阶：LookAt BlendSpace → 视线跟随鼠标。
- **已由 S3 定论**(详见 [spike-s3.md](spike-s3.md))：走跑动画**逐 clip 不一致**——同一条链里
  有的带 root motion 有的原地，方向恒为 glTF +Z(= UE +Y)。故 manifest 逐 clip 给
  `in_place`/`speed_cm_s`；运行时有速度就用它推进位置并原地循环播放，没有就按 locomotion
  取默认值，并对离谱值钳制(魔力猫 Run 反推出 7.5m/s)。单位：glb 米制，`height_cm` 取
  `ImportedBounds` 全高(喵喵链 80/104/204cm)。
- **待验证**：
  - `MODEL_CONF.SMR`、`PET_SHOW_SPEED_CONF` 各自的含义(现在速度直接从 root motion 反推，
    够用；要与游戏内手感对齐再查)；
  - `INTERACTIONTREE_CONF` 的 `anim_key*` 到动作表的确切映射(「摸头」指向的 id 20 在
    `ANIM_ID_CONF` 里叫 `Sad`，字面对不上，需实机核对)；
  - stage 0 目录(如 `Gra_MiaoMiao0_001`)只有 Mat/Tex 没有 SKM，是蛋还是共享皮，包里怎么表达。

## 6. 多实体与跨宠物互动

- 同一 stage 内多实体：同物种可多开(一个包创建多个实体)，不同包可同时启用。
- **事件总线**：`Intent{from, kind, target}` + `Perception{邻近实体, 鼠标, 屏幕边界}`。
- **互动包(interaction pack)**声明依赖，双方都在场且距离够近才可触发：

  ```toml
  [interaction]
  id = "peel_commands_cleaner"
  requires = [{ species = 3758 }, { species = 3604 }]   # 珀尔鼬 × 捕尘长绒
  trigger  = { kind = "proximity", max_distance = 200, cooldown = "3m" }
  ```

- 编排用**演出脚本(时间轴)**而非让两个状态机自发协商：谁在第几秒播哪个 clip、走到哪、
  何时出声，可靠且可调。脚本用 Lua(自产包)，若将来接受第三方包则换 WASM 沙箱。
- 诚实的限制：「清扫」这类游戏里由行为树驱动、没有独立 clip 的行为，只能用现成动作拼近似
  (`walk` 往返 + `show`/`attack1` 当动作)。

## 7. 音频

- 来源：复用 rocom-petvo 已跑通的 `Pet_Vo_*.bnk` + wem → vgmstream 管线，转 opus 进包；
  `PetData.voice` 决定用哪一组。
- 粗嗓门/婉转声是运行时 pitch RTPC，用播放速率/变调复刻，不需要额外音频文件。
- 触发点：启用召唤、受惊、摸头满意、睡醒。默认低音量、可静音、可全局关。
- **不做 BGM**(体积、版权、干扰)。

## 8. 导出器

`pak → 宠物包` 的本地工具，输入是用户自己的游戏安装。

1. 读配置(`PETBASE_CONF`/`MODEL_CONF`/`ANIM_CONF`/`ANIM_ID_CONF`)，过滤测试与重复行，
   按 `stage/evolution_pet_id` 归成链，输出待导清单。
2. 用 CUE4Parse 导每个形态的 `SKM_*`(glb) + 所需 `AnimSequence`(psa) + `Tex/*`(png)。
3. **把 psa 动画合并进 glb**：glTF 导出器不产动画，且 glb 做过 UE→glTF 轴转换而 psa 保持
   UE 空间，合并时要补变换(或统一走 psk+psa / UEFormat 对再转)。psa 结构简单
   (BONENAMES + 逐帧 quat/pos)，已在 rocom-capture 侧验证过这条数据通路正确。
4. 贴图修正 BC7 通道序、按材质名后缀 `_By/_Es/_Mh` 接槽位、转 KTX2/webp。
5. 叫声转码，生成 manifest 与覆盖率报告，打包 zip。

依赖与坑：
- **CUE4Parse 克隆必须先打补丁**,否则法线会被静默写成切线(§1「法线」那条):
  `git -C "$CUE4PARSE_DIR" apply <本仓库>/exporter/patches/0001-fix-FPackedNormal-quantize.patch`。
  导出器启动时会自检并拦住,不打补丁跑不起来。
- CUE4Parse-Natives **必须带 ACL 编译**，否则动画解压报 `nAllocate` 找不到：
  `git submodule update --init --recursive CUE4Parse-Natives/ACL/external/acl`，
  再 `cmake -B builddir -DCMAKE_BUILD_TYPE=RelWithDebInfo . && cmake --build builddir`。
  build type **必须避开 Debug/Release**——那两个会命中 `install(TARGETS … RUNTIME DESTINATION)`，
  Linux 上 SHARED 库属 LIBRARY 产物无 destination，cmake 报错会让 `dotnet build` 挂在 MSB3073。
- 语言：导出器留在 C#(CUE4Parse 在那边)，运行时是 Rust；两者只通过包格式耦合。

## 9. 实施阶段

### 9.0 原始需求对照(2026-07-26)

立项时定下的九条需求，逐条对当前代码核过一遍：

| # | 需求 | 状态 |
| --- | --- | --- |
| 1 | 独立新仓库、不限定语言 | ✅ Rust 运行时 + C# 导出器 |
| 2 | 至少支持 Windows 与 Linux Wayland | ⚠️ **KDE Wayland 已跑通，Windows 后端 0%**(`src/platform/windows.rs` 是空壳) |
| 3 | GB 级数据 → 主程序 + 宠物插件、按需启用 | ✅ 包目录 + `--list`/`--pack`，启动只读 manifest |
| 4 | 同一进化链形态封装进一个包、启用时可切 | ✅ 一链一包，托盘「形态」子菜单单选切换 |
| 5 | 同时启用多个包 / 一个包开多个实体 | ❌ `Stage` 目前恒定持有**一个** actor |
| 6 | 跨宠物互动(珀尔鼬指挥捕尘长绒) | ❌ 依赖 #5 |
| 7 | 普通动作：睡觉/行走/奔跑/生气 | ⚠️ 睡觉、行走、生气(表情池)都有，**`Run` 从未接进状态机** |
| 8 | 部分支持宠物叫声 | ❌ 提取管线在 rocom-petvo 已通，运行时侧未开工 |
| 9 | 穿透开关 + 点击受惊 + 摸头 + 把一只拖到另一只旁边 | ⚠️ 前三项 ✅；「拖到另一只旁边」依赖 #5 |

结论：**多实体是最大的一块缺口，且同时卡着 #5/#6/#9**，所以它是下一步；`Run` 是几小时的小
补丁，顺手在同一阶段做掉。剩下的 Windows 与叫声都是独立块，不互相阻塞。

### Phase 0 — 技术验证(spike，各 1–2 天，失败即换路线)

必须先做，因为结论会改架构。

| # | 内容 | 验收标准 |
| --- | --- | --- |
| S1 | 平台层：KDE Wayland(layer-shell) 与 Windows(DComp) 各画一张半透明贴图 **(Wayland ✅ / Windows 未开始,见 [spike-s1.md](spike-s1.md))** | 两平台都能：置顶于普通窗口之上、指定坐标、逐像素 alpha 正确(无黑边/无不透明底)、贴图内点击被自己接到而贴图外点击落到下层窗口、运行时切换全局穿透生效、多显示器各自一个 stage 且 output 热插拔不崩；另记录 KWin 下 `layer=top` 与全屏窗口/锁屏/通知的实际叠放次序，以及空闲与活动时的 CPU/GPU 占用。**这是全项目成败点。** |
| S2 | 渲染：wgpu 加载 glb 播骨骼动画 + toon 着色 | **✅ 见 [spike-s2.md](spike-s2.md)**：形体与 CPU 参考实现(`tools/verify_glb.py`)一致；淡化中点是平滑中间态；单只 0.040–0.054ms/帧(60fps 预算的 0.3%) |
| S3 | 导出器：动画合并进 glb，跑通喵喵整条链(3001/3025/3007) | **✅ 见 [spike-s3.md](spike-s3.md)**：三形态动画正确(途中修掉 CUE4Parse 的骨骼旋转 bug)；root motion/朝向/单位已定论；manifest 已产出 |

### Phase 1 — 单宠物 MVP

**已完成**:`--pack` 载入宠物包(读 manifest)、宠物站在工作区底边(踩任务栏上沿)、
`Idle` 循环、随机挑目标点用 `Walk` 走过去并平滑转身、拖放(松手落回地面)、
`SIGUSR1` 切穿透、宠物按 `height_cm × --px-per-cm` 换算屏幕尺寸。
实测:自身 CPU **1.2% 单核**(30fps 推进动画)、RSS 152MB(debug 依赖 + NVIDIA Vulkan)。

**已补**:配置文件(`~/.config/rocom-pets/config.toml`,首次运行生成带注释模板,
命令行参数优先)、托盘菜单(StatusNotifierItem:鼠标穿透勾选 / 召回宠物 / 退出)、
全局热键、自己的 D-Bus 控制接口。

全局热键有两条路,都实测通过:

1. **XDG GlobalShortcuts portal**(`org.freedesktop.portal.GlobalShortcuts`)。
   应用只能*建议*按键,KDE 会**弹窗让用户确认**——在用户点之前 portal 不回应,
   所以代码里放了看门狗提示去看弹窗(一开始误判成「KDE 丢弃了请求」,实机确认是等确认)。
2. **`org.rocom.Pets` D-Bus 接口** + `rocom-pets --toggle-passthrough|--recall|--quit`。
   在 KDE「自定义快捷键」里把任意键绑到这条命令即可,不依赖 portal,顺带让宠物可脚本化。

**待做**:落地用 `JumpFall` 动作、damage 局部提交、开机自启(`packaging/rocom-pets.desktop`
复制到 `~/.config/autostart/`)。

### Phase 2 — 鼠标交互

**已完成**:轮廓命中与轮廓输入区(离屏画布 alpha 异步回读成 8 物理像素的格子掩码,
腿与尾之间的空隙能点穿,实测输入区 60–87 个矩形随动画变化)、点击受惊(`Shock`)、
摸头(指针在头部区域来回蹭够 3 次换向 → `Happy`)、拎起来害怕(`Fear`)/放下落地、
**按姿势变化速度**自适应降频。行为逻辑有 10 个单测(用 `Model::for_test` 的合成模型,
不碰 GPU 也不需要宠物包)。
实测:CPU **1.3% 单核**、RSS 219MB。

降频这条踩过一次:一开始按状态硬分档(「待机」→ 12Hz),实机反馈**明显发顿**——
待机动画本身带起伏,实测关节最大速度约 6m/s(行走 4.7m/s),根本不算静止。
改成用关节速度连续映射成帧率(1m/s 以上跑满 30Hz,越接近静止越省,下限 10Hz):
待机/行走都稳稳跑满,睡觉那类真正近乎静止的动作会自动落到下限,不需要给每段动作手工标注。

**待做**:多显示器(手上只有单屏,没法验)、HiDPI 分数缩放(见下)、
掩码回读的内存开销(比 Phase 1 多 ~65MB,疑似 wgpu 的可映射缓冲内存池;
若要抠可以改成渲一张 64×64 的专用掩码附件而不是回读整张画布)。

### Phase 3 — 行为引擎

**已完成**:需求值(困倦/无聊)驱动的状态机、睡觉三段式(入睡 `SleepStart` → 睡着 `SleepLoop`
循环到睡饱 → 醒来 `SleepEnd`)、被戳会醒(而不是原地受惊)、待机时随手做表情
(`Happy/Sad/Anger/Show/Relax/Alert` 里随机)、指针悬在身上时侧身「瞥一眼」。
时间尺度是手感常量(困倦 8 分钟攒满、睡 90 秒睡饱、无聊 6 秒攒满),
`ROCOM_PETS_NEEDS_SPEED=20` 可整体加速,几十秒看完一轮作息。
睡着时姿势几乎不动 → Phase 2 的自适应帧率自动把它降到 10Hz,不需要额外标注。
新增 5 个行为单测(作息三段、戳醒不受惊、无聊消涨、瞥视方向、睡着降频)。

**不做**:真正的视线跟随。它要 LookAt BlendSpace(没导出),而且 Wayland 下**输入区之外
根本收不到指针事件**——要追全屏光标就得把输入区扩大到吃掉点击,代价不划算。
现在只在指针落在身上时侧身,读起来已经像在瞥。

**待做**:按真实时钟的作息(std 没有时区,要引依赖)、心情影响表情选择、饥饿/喂食。

### Phase 4 — 包格式定稿与导出器成品

**已完成**:

- 导出器 `--all`:遍历全部宠物、**按进化链去重**、写 `report.txt`(每个形态的动作命中/缺失、
  体积、警告 + 汇总);`--limit` 试跑、`--skip-existing` 分批续跑
  (过滤在计数**之前**,否则 `--limit` 永远只覆盖前 n 条链);
- manifest 加 `source_version`(pak 文件名+长度+挂载文件数的短哈希):换版本重导后会变,
  便于排查「这包是哪版导的」;
- 包目录 `~/.local/share/rocom-pets/packs`:`--list` 列出包与形态、`--pack` 既接受路径
  也接受包名/物种名;
- **运行时形态切换**:托盘「形态」子菜单(单选),切换时重建模型与那套 GPU 资源
  (管线/画布/合成四边形/掩码缓冲全跟形态绑),位置重新落地。实测喵喵 161px ↔ 魔力猫 481px。

**全量导出暴露的问题(单只喵喵试跑时看不出来,都已修)**:

1. **一个形态缺资产不该拖垮整条链**。有些进化阶段这版本根本没做资产目录
   (如 `Roc_MeiQiu1_001` 不存在,只有 `2_001`),原来整条链直接失败;
   现在按形态 try/catch,跳过并记进报告,全链皆缺才跳过整条(实测 110 条链属于这种)。
2. **网格名不能硬编码 `SKM_<资产>_Skin`**:改成枚举目录直属的 `SKM_*` 并优先 `_Skin` 结尾
   (`LOD_`/`ABP_` 前缀的不是网格)。
3. **197/827 个形态自己没有 `Animation/` 目录**(24%)。它们是变体资产
   (`Win_ShiJiu1Ar_001` 的 `Ar`)或换了属性前缀的同族(`Gra_DiMo2_001` vs `Lig_DiMo2_001`),
   与同族基础资产共用骨架与动画。现在两级回退:先找同 `anim_conf_id` 的资产(配置层面的显式共享),
   再按**族名 + 阶段**找(族名 = 资产名中段去掉末尾 `Ar` 与阶段数字)。
   实测圣草迪莫 0/16 → 16/16,借自 `Lig_DiMo2_001`,渲染姿态正确。
   借错也安全:骨骼名对不上时 `GlbBuilder` 会跳过那段动画,只会少动作而不会渲出乱形。
4. **部分材质槽指向共享贴图**:890 个资产目录里 352 个至少缺一张 `<槽>_D`
   (眼睛等用的是 CommonTexture 里的共享图集),而「用哪张」只写在材质实例参数里,
   那份参数在本作解不出来(§1 的 OverflowException)。运行时现在退用本体槽贴图
   而不是留一块纯白——**是权宜之计,不是正确解**。真要修得先解出材质参数,
   这也是目前**性价比最高的保真度改进项**。

**并行化**:导出按链并行(`Parallel.For`,默认并行度 = CPU 核数,`-j` 可调)。
链之间没有共享可变状态(各写自己的包目录),provider 的并行只读在 rocom-capture 的解包脚本里
已经压过;控制台与报告文本按链攒着、跑完按原顺序合并,否则并行下输出会交错。
实测 16 核:全量 **10.6 分钟 → 2.4 分钟**(采样 CPU ≈1000%,峰值 RSS 2.3GB);
产物与单核跑**逐字节一致**(`diff -r` 验证),报告内容与顺序也一致。

并行顺带暴露一个原本就存在的 bug:**72 个物种名被多条链共用**(「棋契陛下」有 10 条),
而包目录直接拿名字命名 → 互相覆盖(530 条链成功却只剩 395 个目录),并行下还可能两条链
交错写同一个目录。重名的现在追加链首 id(`名字-3001`)。

**全量回归的做法(值得固化)**:光看导出器「0 失败」不算数——它只证明写出了文件。
真正的检查是**拿运行时把每个形态都载入并渲一帧**,再看两个指标:
① 退出码(能不能加载),② 渲出来的不透明像素覆盖率(**是不是真画出了东西**)。
就是这一步逮到下面三个 bug——它们全都能在「导出成功」的产物里安静地待着。
两个踩过的坑:`-o /dev/null` 会让 PNG 编码器报「格式判不出来」,看着像 831 个全崩,
其实是渲完才失败;以及只测喵喵一只是不够的,喵喵恰好是唯一躲过 alpha bug 的宠物。
当前基线:**767 个形态加载+渲染全部成功、0 失败**,另 64 个形态没有任何动作(素材本身不全)。

**全量渲染回归逮到的三个 bug(2026-07-26,导出器报告全绿也照样存在)**:

1. **32 个形态根本加载不了**:CUE4Parse 把空 morph target 写成没有 bufferView 的 accessor,
   Rust `gltf` crate 拒收(见 §1)。导出器关掉 `ExportMorphTargets` 即可——我们从不驱动它们。
   修完重导了受影响的 26 个包。
2. **贴图 alpha 被当成不透明度**:`if tex.a < 0.35 { discard; }` 把 160 张 `_By_D` 里的身体
   啃掉一部分,火花只剩眼睛、迪莫整只消失(见 §1)。
   **这条改了两轮。** 先整个去掉 alpha 测试:身体是回来了,但眼/嘴的**表情图集**没人剔,
   菊花梨的眼睛糊成一块方斑、学院呱呱的圆眼镜黏成一团——**是拿实机截图逐只比才看出来的**,
   全量渲染回归不会报错(它只看「有没有画出东西」)。
   正解按槽区分:载入时把**本体贴图**的 alpha 刷成 255,shader 保留统一的 alpha 测试。
   判据要看**最终用的是哪张贴图**而不是槽名——火神的肌肉是 Fx 槽退用本体贴图,
   按槽名判会被整片剔光。全量 A/B 验过:开/不开 alpha 测试,764 个形态**没有一个**
   掉覆盖率超过 25%。
3. **取景按绑定姿势包围盒**,伸展类动作被裁(120 个抽样 × 四个动作,11 个被裁)。
   改成按动作包围盒(`Model::motion_bounds`)取景后剩 1 个。
   这条也绕了一圈:动作并集里混着**召唤落地**类动作(喵喵 `CallOut` 从 1.5m 高处掉下来),
   一并算进去会让画布白涨——33 个形态实测总面积 2.08 倍;丢掉「整只挪走」的姿势后降到 1.64 倍。
   中心偏移阈值要取一整个身高:取 0.4 会误伤**悬浮类宠物**(空空颅的 `Alert` 常态浮在
   45–56%,而它就在表情池里,于是运行时照样顶出画布)。
   另一个坑:采样时剥的位移必须**和运行时一模一样**(只剥 root 的 X/Z、保留 Y),
   否则量出来的盒子比实际渲的低,带纵向起伏的动作会顶出去。

**分批重导踩的一个坑**:包目录的重名后缀原来是按「这次要导的链」统计的,于是单独重导「迪莫」时
这批里没有同名链、目录就叫 `迪莫`,而全量导时它叫 `迪莫-3004`——增量重导会另起一个目录、
把原来的孤立掉(实测重导 14 条链后 3 个包名变了)。改成按**全部宠物**统计重名,与批次无关。

**待做**:直接读 `.rkpet`(zip)而不是解开的目录——现在导出器能打 zip、运行时只读目录,
资产访问要先抽一层;egui 的包管理 GUI;贴图转 KTX2 与关键帧精简(体积);
材质参数解析(见上面第 4 点)。

### Phase 5 — 多实体与跨宠物互动

一次性补掉 §9.0 的 #5/#6/#7/#9，是剩下最大的一块。**先做它再做 Windows**：多实体会改动
`Stage` 与平台层的接口(单 actor → 实体集合)，接口定稿后 Windows 后端只写一遍。

按依赖排的子步骤：

1. **`Stage` 单 actor → 实体集合**。`Stage` 现在恒定持一个 `Actor`(`stage.rs:527` 的字段 +
   `replace_actor`)，改成 slotmap/Vec + `EntityId`。要一起改的：命中测试**取最上面的那只**
   (需要 z 序，简单起点是按脚底 y 排序，后面的挡前面的)、输入区取**各实体掩码的并集**、
   拖动状态从 `Stage` 挪进实体。行为单测已有 20 个，refactor 靠它们兜。
2. **同物种多实体必须共享资产**。`Model`(网格/动画/贴图)与那套 GPU 资源现在和形态一对一
   绑在 `PetSurfaces` 上(`wayland.rs:259`)。多开同一只如果各带一份 `Model`，内存直接翻 N 倍
   ——RSS 已经 219MB。改成 `Arc<Model>` + 按 (包, 形态) 缓存的 GPU 资源，实体只持自己的
   `Player`(姿势)与状态。**每实体独立的是画布**(各自渲各自)。
3. **掩码回读要错峰**。单只时是 140ms 节流一次回读；N 只如果同帧全回读，会把 Phase 2 好不容易
   压下去的开销乘回来。按实体轮转，一帧最多回读一只。
   取景改按动作包围盒后画布面积平均涨了 1.64 倍(见 §1)，回读量按画布面积走，
   所以这条和上一条(共享资产)的收益都比原来更大。
4. **补 `Run` 与落地 `JumpFall`**(需求 #7 的缺口)。manifest 里 `Run` 的 `speed_cm_s` 已经导出且
   已知有离谱值(魔力猫反推 7.5m/s)，所以要钳制；触发条件：目标点远 → 跑，近 → 走，
   受惊逃跑用跑。`JumpFall` 接在拖放松手之后(Phase 1 的遗留待做)。
5. **感知与事件总线**。`Perception{邻近实体, 鼠标, 屏幕边界}` + `Intent{from, kind, target}`，
   见 §6。proximity 用脚底点的屏幕距离，阈值随宠物尺寸缩放(161px 的喵喵和 481px 的魔力猫
   不能用同一个绝对像素阈值)。
6. **演出脚本 + 第一个互动样例**(珀尔鼬 3758 × 捕尘长绒 3604)。按 §6 定的走**时间轴**而非
   两个状态机自发协商。第一版把时间轴硬编码在 Rust 里跑通编排与打断语义，**确认可行后**再
   决定要不要抬到 Lua——先引 mlua 会把「编排怎么写才好用」和「脚本 VM 怎么接」两个问题搅在一起。
   诚实的限制仍在 §6：「清扫」没有独立 clip，只能用 `walk` 往返 + `show` 拼近似。
7. **托盘与配置**：加一只/移除一只/同时启用哪些包，配置里存实体清单(重启后恢复在场阵容)。

### Phase 6 — 音频

叫声 + 变调，自成一块、不阻塞任何东西，也可以插在 Phase 5 中间当换脑子的活。
管线在 rocom-petvo 已经跑通(`Pet_Vo_*.bnk` + wem → vgmstream)，这里做的是：导出器把
`PetData.voice` 选中的组转 opus 进包、运行时用 kira/rodio 按播放速率复刻粗嗓门/婉转声、
触发点接到召唤/受惊/摸头满意/睡醒。默认低音量、可静音、可全局关；不做 BGM。
Phase 5 的互动演出正好需要出声，两者一起做手感最完整。

### Phase 8 — Windows 后端

S1 只做完了一半，这是**剩下风险最高的一块**，且需要用户的 Windows 机器实测，不能留到最后。
放在 Phase 5 之后是为了对着定稿的平台层 trait 写一遍。验收项直接复用 S1 那份清单
(置顶、逐像素 alpha 无黑边、命中/穿透、多显示器、叠放次序、空闲/活动占用)。
已知要点：必须 `CreateSwapChainForComposition`(GDI 的 `UpdateLayeredWindow` 不适合 GPU 渲染)，
wgpu 侧大概要 `SurfaceTargetUnsafe::CompositionVisual` 自建 surface；穿透是
`WS_EX_TRANSPARENT` + `WM_NCHITTEST` 返回 `HTTRANSPARENT`——**没有 Wayland 那种输入区**，
轮廓命中要在 `WM_NCHITTEST` 里查掩码。

### Phase 7 — 打磨与分发

egui 的配置与包管理 GUI、开机自启(KDE autostart 已有 `.desktop` / Windows 启动项)、
N 只宠物的性能与内存实测、Windows 安装包 / Linux AppImage。

### 横向待办(不属于某个阶段，随时可插)

按性价比排：

| 事项 | 为什么 | 代价 |
| --- | --- | --- |
| ~~材质实例参数解析~~ **已完成** | 见 §1:材质能正常读,贴图改按 `TextureParameterValues` 接。全量 2043 个材质槽里修正了 **258 个**(246 个原来猜不到→退用本体色、12 个猜错)。幽星光一阶从「一坨黑」变成正确的粉色;水蓝蓝一族不再是噪声 | 已做 |
| ~~特效层通道(火焰/水壳/光晕)~~ **已完成(近似)** | 主色 × 遮罩 × 卷动噪声,加色/半透用预乘 alpha 统一;参数全部取自游戏材质。37 个形态带特效层。**是近似不是复刻**:没有折射、没有 MatCap 反射、菲涅尔只用 N·V 粗略代替 | 已做 |
| ~~玻璃/薄纱层与卷动色带~~ **已完成(近似)** | 半透族叠 MatCap 高光 + 边缘光混色,环带按 `FlowTexture` 卷动出渐变,星点按屏幕位置贴。暮星辰的裙子回到饱和蓝、环带有青↔粉渐变、幽星光那两颗球是红玻璃且不再闪 | 已做 |
| ~~球旋转时**仍有区域白闪**~~ **已修**:根因是上游把切线写进了 NORMAL(见 §1 法线那条),不是任何一层特效 —— 我原先排的两条嫌疑(屏幕空间星点遮罩、MatCap 高光)**都不是**:星点在球上只贡献 +0.5 亮度,matcap 只是放大器 | 摆幅 51.7 → 2.1。顺带把 matcap 按汇编改成「单通道 × MatCapColor」并让几层光用 `max` 合 | 已做 |
| 玻璃内部层继续对齐实机 | 机制已经照汇编实现了(折射 + 沿折射线三向投影采 `StarTex` + 时间卷动,见 §1),但**观感还没对上**:实机是一颗又大又干净、居中的四角星,我们这边是一团偏软的亮斑。差在 march 深度/平铺的归一化(实机用包围盒配 `GlobalDepth` = 100,我按最长边缩放后手挑了 0.7),以及固有色还没换成那个按高度的两色渐变 | 中:知道该调什么,但每一版都得对着截图看 |
| **解 `FUniformExpressionSet` 的 FMemoryImage 布局** —— 这是剩下全部材质问题的**总闸** | 卡着的具体条目:两段明暗那四对明/暗色 (24,25)/(28,29)/(32,33)/(36,37)、球的固有色两色渐变、11 个真半透材质的不透明度、星点强度的确切系数、内部层的深度与平铺。**已知有利条件**:参数名在 uexp 里是可读字符串;cb5 分区已定(向量 0..53、标量 54..69 每 float4 装 4 个) | 中偏大:要摸 UE4.26+ 冻结镜像的布局。不打通就只能继续「像但不对」 |
| 那 11 个「真半透」材质做成真半透 | `Opacity or OpacityMask` 开关点名了它们(蜜蜂/小甲虫的翅膀、果冻、暮星辰的裙子……),现在一律当不透明画。缺的是不透明度**从哪来**:翅膀那种像是基色 alpha,暮星辰的 alpha 却是线条遮罩,同一组里语义不统一 | 中:要么再找一个判据区分两种 alpha,要么等 shader 反编译 |
| 附加特效组件(粒子 / socket 挂件) | 幽星光与暮星辰那两颗球里**单独转动的小星星**不在骨骼网格里(见 §1),同类的还有各种拖尾/光环。要走蓝图 → 组件树 → 粒子系统/附加网格这条导出链 | 大:等于开第二条资产管线 |
| 直接读 `.rkpet`(zip) | 导出器已能打 zip，运行时只读解开的目录；分发前必须补，也是体积优化的前提 | 中：资产访问要先抽一层 |
| 贴图 KTX2 + 关键帧精简 | 全量 3.0GB，单形态 2.1–5.0MB 里动画通道占大头 | 中 |
| 64 个零动作形态 | 同族里也找不到带动画的资产，属素材本身不全；可能得放弃或用同阶段近亲代播 | 小(调查)，修不一定可行 |
| 真实时钟作息 / 心情影响表情 / 喂食 | Phase 3 的遗留，纯手感 | 小 |
| damage 局部提交 | §3.3 的提交策略只做了降频这一条 | 小 |
| 多显示器实测 | 代码按 per-output 写的，但手上只有单屏 | 需要第二块屏 |

**推荐执行顺序：Phase 5 → Phase 6 → Phase 8 → Phase 7**，横向待办里的材质参数建议在
Phase 5 之后单独插一轮(它是纯导出器侧的活，和运行时不冲突)。

## 10. 风险与未决问题

| 风险 | 缓解 |
| --- | --- |
| Windows 逐像素 alpha + GPU 交换链 | 必须走 `CreateSwapChainForComposition`；wgpu 可能要用 `SurfaceTargetUnsafe` 自建 surface，S1 定论 |
| KWin 对 wlr-layer-shell 的支持随 Plasma 版本变化(非正式协议) | 平台层抽象成 trait；S1 的验收项固化成回归清单，升级 Plasma 后重跑；实测的 KWin 版本写进 README 支持矩阵 |
| 全屏透明层的合成开销 | §3.3 的提交策略；S1 里就要量一次空闲/活动时的 CPU/GPU 占用 |
| 材质只能近似 | 明确目标是「像」；把 ramp/描边参数做成包内可调 |
| 游戏版本更新改路径/命名 | 导出器带版本适配与覆盖率报告，缺失动作降级而非报错 |
| 第三方包的脚本安全 | 自产包用 Lua；一旦开放第三方，换 WASM 沙箱 + 能力白名单 |

## 11. 法务与分发

- 素材版权属腾讯/发行方。仓库**只有代码、schema 与导出器**；原始解包数据、生成的宠物包
  都不入仓库、不随发布分发，用户用自己的游戏安装本地生成(沿用 rocom-capture / rocom-petvo 的约定)。
- 运行时不读游戏内存、不注入进程、不联网上报。
