#!/usr/bin/env python3
"""读材质 uexp 里 shader map 的 `FUniformExpressionSet`:cb 槽位的**结构与默认值**。

**「cb 槽位 → 参数名 → 参数值」整条链已经通了。** 现在能做的:

- 把 cb 槽位**准确**分成向量区与标量区(见「cb 布局」);
- 把某条 shader **唯一配对**到它的冻结块(见「配对到具体 shader」);
- 解出每个槽位的 opcode 流,并把 `标量参数(i)` / `向量参数(i)` 解成**值 + paramId**;
- 把 paramId 对到**名字**:`param_names()` 直接读 uexp 里 shader map 自带的名字表,
  `paramId` 就是它的稠密下标(锚点 13/13、根默认值复核 805/805)。
  成品输出用 `scripts/matparams.py`。

## cb 布局:已经查实,不是推算

一个冻结块里有**两条独立的 preshader 头链**,共用**一个** opcode 缓冲:

    UniformVectorPreshaders   V 条  → cb[0 .. V-1],每条占一个 float4
    UniformScalarPreshaders   S 条  → 从 cb[V] 起,4 条装一个 float4(标量 #i 在 cb[V + i//4] 的 i%4 分量)
    ── 末尾 UE 还追加一个 float4 ──

**所以向量槽的下标就是向量链的条目序号,`V` 就是分界,不需要任何推算。**

**两条链的区分:首 opcode 偏移为 0 的那条是「标量」链**,另一条接在它后面。
这里**认反过一次**,后果很大:向量链被当成整个 cb,分界永远算错;而且标量链里那一堆
`03` 被当成「向量槽引用了标量参数」,于是编出一个并不存在的「共享索引空间」。

## 配对到具体 shader:已经解决

判据两条,合起来实测 **32 条 shader 全部唯一定到块**:

1. `V` = 1 + 「材质 cb 里最后一个以 **≥3 分量 swizzle** 出现的槽位」——
   向量槽会整条当颜色用(`.xyzx`),标量槽只会单分量出现;
2. `V + ceil(S/4) + 1 >= 汇编 dcl_constantBuffer 声明的大小`。
   那个 **+1** 是实测的:声明大小 = 最大用到的下标 + 1 ≤ 总槽数,而
   `声明 − (V + ceil(S/4))` 的分布只有 `0` 和 `+1`,说明末尾确实多一个 float4。

**端到端验过一次**:宠物材质 `MI_Ill_XingGuang1_001_Fx1` 的 shader 19422(6 个 ub ⇒ 材质 cb5、
声明 cb5[109]、按判据 ① 得 V=79)唯一配到块 5(V=79, S=116, 79+29+1=109 ✓);
块 5 里 `cb5[34]` 的首操作数解出**向量参数 15 = (9, 0, 5, 0)、paramId 112**,
而根默认里 `FragmentsColor = (9, 0, 5, 0)`、名字表的 `[112]`
正是 `112 → FragmentsColor`。同块的 `cb5[35]`(id=106 →`Direction`,值 (0,-0.5,1,0))与
`cb5[36]`(id=109 → `FlowColor`,值 (0.5,0,0.6,0))也各自对上。

## opcode 编码:已经查实

    02 <16 字节>   Constant(float4)
    03 <uint16>    Parameter(下标)
    04/05/06/0b/18/1a …  运算(Add/Sub/Div/Max/… 具体对应还没逐个定)

判据不是猜的:某个槽位解出来是 `Constant(65503.86×4) Parameter(12) Constant(1e-5×4) 0b 06`
= `65503.86 / max(参数12, 1e-5)` —— 65503.86 是 half 的最大值、1e-5 是 epsilon,
这是 UE 里「限幅再取倒数」的固定写法。

**定 opcode 基址不能用「size==3 的条目能解成 Parameter(下标 < 向量参数条数)」打分** ——
那会把正确基址判负(实测向量链 106 条里 size==3 的有 70 条,而向量参数只有 18 条)。
判据要用「每一条的首字节都是合法 opcode」,且 0(Nop)不算合法,否则**全零区域会拿满分**。

## 为什么要它

反汇编读出的公式里到处是 `cb5[32]`、`cb5[59].x` 这种**编译顺序**的槽位,而材质资产给的是
「参数名 → 值」。两头对不上,读出来的公式就落不了地(见 rocom-pets/docs/design.md §1
「cb 槽位 ↔ 参数名」)。UE 里 cb 的布局是确定的:

    [UniformVectorPreshaders 每条一个 float4] [UniformScalarPreshaders 每条一个 float,按 4 个装一个 float4]

所以**向量槽数一确定,标量的起始槽位就确定了**。实测幽星光那两颗球的 shader:向量占 cb5[0..53]、
标量从 cb5[54] 起(标量 #k 在 `cb5[54 + k//4]` 的第 `k%4` 个分量)—— 与这里读出的条数可以互验。

## 冻结镜像里的记录格式(实测)

shader map 存成 **FMemoryImage**(UE4.26+ 的冻结布局,不走 FName 序列化),一个材质的 uexp 里
有多个冻结块(实测 12 个,对应不同 quality/feature level,uniform 条数各不相同)。块内:

    FScriptName 名字               12 字节 {ComparisonIndex, DisplayIndex, Number},**文件里全零、靠补丁填**
    int32 Index                     -1 = 非图层参数
    uint32 Association               2 = EMaterialParameterAssociation::GlobalParameter
    默认值                          float(标量,整条 24) / FLinearColor(向量,整条 36),都在 +20
    FMaterialUniformPreshaderHeader   8 字节: uint32 OpcodeOffset + uint32 OpcodeSize

**名字是 12 字节不是 8。** 按 8 找会让整个数组被识别在**偏移 4 字节**的位置 ——
默认值照样读得对(起点和值偏移一起挪了 4),但补丁偏移就永远对不上。踩过一次。

`Association == 2` 是 `EMaterialParameterAssociation::GlobalParameter`,`Index == -1` 是「非图层参数」——
这两个常量就是认出数组的特征。默认值可以**交叉验证**:根材质 `M_P_Object_Trans` 的
`FresnelColor` = (0.087,0.353,1,0)、`StarColor` = (0.333,0.667,2,0)、`FlowColor` = (0.5,0,0.6,0)、
`BlackMagicDarkColor` = (0.05,0.02,0.1,1) 都能在数组里原值找到,顺序也对得上。

preshader 头数组认起来更省:opcode 是**连续写**的,所以 `off[i+1] == off[i] + size[i]`,
一条长链就是一个数组;向量那条从 opcode 偏移 0 开始,标量那条紧接其后。

**条数要和汇编互验,别直接信。** 反汇编幽星光那两颗球的 shader 数出来是 54 个向量槽
(向量占 cb5[0..53]、标量从 cb5[54] 起),而这里对同一材质读出的向量链里有一条是 **55** ——
差 1,大概是链检测的起点多吃了一项(`{off,size}` 对在数组前后都可能凑巧接得上)。
所以拿数字下结论前先跟汇编对一次。

## 冻结块的边界:**已能精确定出**(不用猜、不用投票)

```
[int32 FrozenSize][冻结字节 FrozenSize 个]
[int32 NumVTables][int32 NumScriptNames][int32 NumMinimalNames]
[vtable 补丁][名字补丁][MinimalName 补丁]
```

名字补丁 = `{int32 名字下标, int32 Number, int32 补丁数 N, uint32 块内偏移[N]}`(实测 N 恒为 1,
所以看着像定长 16 字节)。于是**补丁表起点 − 12 = 冻结区末尾**,再往前找一个 int32 恰好等于
冻结区长度就锚住起点。

- **`FrozenSize` 的位置不是 4 字节对齐的**(根材质第一块的冻结区起点在 `0xf196`)。
  按 4 步进去找,12 个块一个都定不出来 —— 踩过。
- 锚对了有个很强的自检:**所有补丁偏移都应当正好落在「12 个零字节 + `Index = -1` +
  `Association = 2`」上**,也就是参数记录的头。实测 12 个块、每块全部补丁都满足。

## cb 槽位 ↔ 参数:**已打通**

    cb5[k]  →  UniformVectorPreshaders[k]  →  opcode 流  →  Parameter(下标)
            →  UniformVectorParameters[下标]  →  补丁给的「参数 id」  →  名字

三段的做法:

1. **preshader opcode 流**。头数组是 `{OpcodeOffset, OpcodeSize}`、opcode 连续写,所以
   `off[i+1] == off[i] + size[i]`,一条长链就是一个数组。opcode 字节块的基址靠**打分**定:
   对每个候选基址,数「`size == 3` 的条目里 opcode 恰为 3(`EMaterialPreshaderOpcode::Parameter`)
   且紧跟的 `uint16` 下标在参数总数内」的条数,减去不满足的×2,取最高分。
   实测幽星光 `Fx1` 的 12 个块**全部满分**(21/21、34/34、…、100/100)—— 这是整条链最硬的一处验证。
   **不能只按「首字节落在小集合里」选基址**:全零区域会碰巧通过(踩过,解出来 opcode 全是 0)。
2. **向量段与标量段的分界**:见开头「cb 布局」—— 两条链**分别定位**,`V` 就是向量链的条数,
   不需要任何推算。早先那版「取参数下标 < 向量参数条数的最后一条」是错的
   (只找到最后一个「单参数」向量槽,而 `Constant` 与算式也占向量槽、还能排在它后面),
   而且认反了哪条是标量链;两个错叠起来编出一个并不存在的「共享索引空间」。
3. **名字**:`param_names()` 直接读 uexp 里的名字表,`paramId` 就是它的稠密下标。
   注意它**不是包名字表下标**(硬证据:实例包名字表 139 条,而 `paramId` 稠密取到 167)。
   `paramId` **跨包稳定**,同一个参数在根材质与实例里完全一致。

## 块 ↔ shader map 的配对

见开头「配对到具体 shader:已经解决」。uexp 里每个 shader map 区域的排布是
(实测,幽星光 `Fx1`):

    [哈希 A] … 约 0x874 字节 … [哈希 B] +0x54 [int32 FrozenSize][冻结字节][计数][补丁表]

**一个区域两个哈希**,块起点在第二个哈希之后 0x54 字节。24 个哈希 / 12 个块。
按位置配对不行(「最近在前的哈希」拿到的是 B,而 34529 属于同区域的 A);
按 cb 声明大小配对要用**修正后的** `V`(早先用错的 V 估出 12/21/35/… 全对不上)。
另外注意 34529 那条的内容**不在这个 uexp 里**
(24 个哈希只有 12 个块有内联内容),所以要读带名字的公式,得挑一条内容确实内联的 shader。

    ./scripts/unpack.sh --out <dir> --raw --no-exclude --no-post --filter "<材质目录>"
    uv run python scripts/uniexpr.py <dir>/…/M_P_Object_Trans
    uv run python scripts/uniexpr.py <dir>/…/MI_<材质> --cb 11
"""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

VEC_STRIDE = 36
SCALAR_STRIDE = 24
PRESHADER_STRIDE = 8


def name_map(uasset: bytes) -> list[str]:
    """.uasset 的名字表:`NameOffset`(实测恒为 0xc1)起,每条 `int32 长度 + 字符 + NUL + 4 字节`。

    **长度为负 = UTF-16 条目**(字符数 = -n)。只认正长度会在第一个中文名处截断 ——
    本作的静态开关名是中文(`是否使用MatCap` 一类),恰好排在字典序末尾,所以少几条。
    条数可以拿包头的 `NameCount` 对(实测 139 / 418,与这里一致)。
    """
    out: list[str] = []
    o = 0xC1
    while o + 4 <= len(uasset):
        n = struct.unpack_from("<i", uasset, o)[0]
        if n > 0:
            if n > 512:
                break
            out.append(uasset[o + 4 : o + 4 + n - 1].decode("ascii", "replace"))
            o += 4 + n + 4
        elif n < 0:
            m = -n
            if m > 512:
                break
            out.append(uasset[o + 4 : o + 4 + 2 * m - 2].decode("utf-16-le", "replace"))
            o += 4 + 2 * m + 4
        else:
            break
    return out


def _read_name_table(d: bytes, off: int, count: int, limit: int = 256):
    """`count` 条 `int32 长度 + 字符 + NUL + uint32 哈希`;长度为负 = UTF-16。不自洽就抛。"""
    out: list[str] = []
    o = off
    for _ in range(count):
        n = struct.unpack_from("<i", d, o)[0]
        o += 4
        if n > 0:
            if n > limit or o + n > len(d) or d[o + n - 1] != 0:
                raise ValueError("长度前缀不自洽")
            s = d[o : o + n - 1].decode("utf8")
            o += n
        elif n < 0:
            if -n > limit or o - 2 * n > len(d):
                raise ValueError("长度前缀不自洽")
            s = d[o : o - 2 * n - 2].decode("utf-16-le")
            o += -2 * n
        else:
            raise ValueError("长度为 0")
        if not s or any(c < " " for c in s):
            raise ValueError("不像名字")
        out.append(s)
        o += 4
    return out, o


def param_names(uexp: bytes, scan_limit: int = 1 << 17):
    """**冻结块补丁表里的 `paramId` 就是这张表的下标** —— 材质参数名,直接读出来。

    `FMaterialShaderMap` 以 FMemoryImage 存,里面的 `FName` 字段被写成全零 + 一条补丁;
    补丁记录的头 4 字节(这里叫 `paramId`)长期对不上任何东西,原因是它**不是包名字表下标**,
    而是 uexp 里 shader map 自带的一张名字表的下标。判据很硬:实测 `paramId` 取值是
    **稠密的 0..167**,而包名字表只有 139 条 —— 根本不可能是它。

    表就在 uexp 前部,格式与包名字表一致(`int32 长度 + 字符 + NUL + uint32 哈希`,
    负长度 = UTF-16,本作的中文参数名如 `黑魔法or噩梦污染` 排在后段),前面一个 `uint32` 是条数。
    这里靠「条数在合理区间、且整张表逐条自洽」扫出来,取最长的一张。

    实测:`MI_Ill_XingGuang1_001_Fx1` 168 条、`MI_P_Object_Trans_MatCap` 168 条、
    根图 `M_P_Object_Trans` 166 条,末条都是 `05FX_FlowV_Tiling`。
    13 个此前用「按值钉死」独立确认过的锚点(87 `06FX_SmearDirectionY`、112 `FragmentsColor`、
    132 `StarUVScale`、137 `HighLightSpecPow` …)**逐条命中 13/13**。
    """
    best: list[str] = []
    for p in range(0, min(len(uexp), scan_limit) - 8):
        count = struct.unpack_from("<I", uexp, p)[0]
        if not (8 <= count <= 4096) or len(best) >= count:
            continue
        try:
            names, _ = _read_name_table(uexp, p + 4, count)
        except (ValueError, UnicodeDecodeError, struct.error):
            continue
        best = names
    return best


NAME_BYTES = 12
VALUE_OFF = NAME_BYTES + 8
PATCH = 16
VTABLE_PATCH = 16      # FMemoryImageVTablePointer:{uint64 哈希, uint32 vtable 偏移, uint32 块内偏移}


def _patch_run(d: bytes, o: int, n: int, id_limit: int):
    """解 `n` 条变长补丁 `{int32 名字下标, int32 Number, int32 补丁数 N, uint32 偏移[N]}`。

    → (末尾偏移, {块内偏移: 名字下标});不自洽返回 (None, None)。
    """
    m = {}
    for _ in range(n):
        if o + 12 > len(d):
            return None, None
        i, num, cnt = struct.unpack_from("<3I", d, o)
        if not (num == 0 and 1 <= cnt <= 8 and i < id_limit):
            return None, None
        o += 12
        if o + 4 * cnt > len(d):
            return None, None
        for k in range(cnt):
            off = struct.unpack_from("<I", d, o + 4 * k)[0]
            if not 0 < off < (1 << 24):
                return None, None
            m.setdefault(off, i)
        o += 4 * cnt
    return o, m


def patch_tables(d: bytes, id_limit: int = 1 << 20):
    """名字补丁表,**按冻结块**返回 [(冻结区起点, 冻结区末尾, {块内偏移: 名字下标})]。

    块尾的布局(见模块头「冻结块的边界」):

        [int32 NumVTables][int32 NumScriptNames][int32 NumMinimalNames]
        [vtable 补丁 16 字节/条][名字补丁 变长][MinimalName 补丁 变长]

    **名字补丁是变长的,不是定长 16 字节。** `{名字下标, Number, 补丁数 N, 偏移[N]}` ——
    幽星光那批材质里 N 恒为 1,所以一开始按定长 16 写、也跑通了;但**用材质图层的那一族
    (`M_P_Object` 图:`MI_P_Object_NoMetal` 等)里 N 会是 2**(同一个名字打两处),
    按定长扫就从那一条开始失步,整块认不出来 —— 那一族的冻结块因此一直报 0。

    这里改成从计数三元组正向变长解析,自洽了才收;起点仍靠往前找 `FrozenSize` 锚定。
    """
    out = []
    o = 0
    while o + 12 <= len(d):
        nv, ns, nm = struct.unpack_from("<3i", d, o)
        if not (0 <= nv < 4096 and 8 <= ns < 8192 and 0 <= nm < 4096):
            o += 4
            continue
        q = o + 12 + nv * VTABLE_PATCH
        q, m = _patch_run(d, q, ns, id_limit)
        if q is None or (nm and _patch_run(d, q, nm, id_limit)[0] is None):
            o += 4
            continue
        start = _anchor(d, o, m)
        if start is not None:
            out.append((start, o, m))
            o = q
        else:
            o += 4
    return out


def _anchor(d: bytes, table: int, m: dict, min_hit: float = 0.6):
    """往前找冻结区起点:`FrozenSize` 那个 int32 恰好等于到 `table` 的距离。

    **不能取第一个匹配** —— 随便一个 uint32 都可能碰巧等于距离。用自检打分挑:
    补丁偏移应当落在参数记录头上(12 个零 + `Index = -1` + `Association = 2`)。

    **自检不会是 100%**:有些补丁指向贴图参数一类别的记录。实测
    `MI_Ill_XingGuang1_001_By` 的三个块是 87/98、104/107、125/136(≈90%),
    而幽星光 `Fx1` 是满分 —— 所以门槛取 0.6,只用来把假锚点筛掉。

    **`FrozenSize` 的位置不是 4 字节对齐的**,要逐字节找。
    """
    best = None
    for back in range(4, 1 << 22):
        p = table - back
        if p < 4:
            break
        if struct.unpack_from("<I", d, p - 4)[0] != back:
            continue
        hit = sum(1 for off in m
                  if p + off + 20 <= len(d) and d[p + off:p + off + 12] == b"\0" * 12
                  and struct.unpack_from("<iI", d, p + off + 12) == (-1, 2))
        if best is None or hit > best[1]:
            best = (p, hit)
        if hit == len(m):
            break
    if best is None or best[1] < min_hit * len(m):
        return None
    return best[0]


GLOBAL_PARAM = 2       # EMaterialParameterAssociation::GlobalParameter
LAYER_PARAM = 0        # LayerParameter
BLEND_PARAM = 1        # BlendParameter


def param_arrays(d: bytes, stride: int, lo: int = 0, hi: int | None = None,
                 exclude: list[tuple[int, int]] | None = None, layers: bool = False):
    """参数记录数组 → [(起点, 条数)]。记录是

        FScriptName 名字   12 字节(文件里全零、靠补丁填)
        int32 Index        -1 = 非图层参数;≥ 0 = 图层下标
        uint32 Association 2 = GlobalParameter、0 = LayerParameter、1 = BlendParameter
        值                 向量 16 字节(stride 36)/ 标量 4 字节(stride 24)

    默认只收 `Association == 2`(全局)。`layers=True` 时也收图层作用域的 ——
    **那才是大头**:`MI_P_Object_NoMetal` 里 `Index=0 / Association=0` 的记录有 29076 条,
    而全局的只有 1228 条;实测 92 个有冻结块的材质**每一个**块内都有图层作用域的数组。

    **`UniformVectorParameters` 是一个数组、条目的作用域可以混**(UE 里它就是一个
    `TMemoryImageArray<FMaterialParameterInfo>`),所以**不能**要求一段数组内
    `(Index, Association)` 一致 —— 那会把真数组切成碎片,`max(条数)` 挑到的是中间一块,
    于是 preshader 里的 `Parameter(下标)` 全部错位。判据只能是「每条都像参数记录」。

    **但图层参数不一定进得了冻结块。** 水体预设(`ML_P_StylizedWater`,参数是
    `Color1`/`Color2`/`CausticsInt`/`FlowDistort`/`FresnelInt`/`FresnelPower`)在
    `MI_Wat_ShuiLanLan2_001_Fx` 里只出现在 `CachedExpressionData`(0x1238,
    第一个块 0x589c 之前),27 个块里一次都没有 —— 那个材质没有自己的内联 shader map。

    **`exclude` 要用上。** 向量数组的判据(12 零 + Index=-1 + Association=2)在**标量数组内部
    也能对上** —— 实测 `MI_P_Object_Trans_MatCap` 块 3 里,它锁在 0x2365a,那正是标量数组
    第 70 条的起点(143306 + 70×24),比真起点(标量数组末尾 + 12 字节)早了 36 字节,
    于是读出来的 19 条全是错位的值。所以扫向量时要把标量数组占的字节段排除掉。
    """
    hi = len(d) if hi is None else hi
    ex = exclude or []

    def blocked(o: int) -> bool:
        return any(a <= o < b for a, b in ex)

    def scope(o: int):
        """→ (Index, Association),不像参数记录就 None。"""
        if o + stride > hi or blocked(o) or any(b != 0 for b in d[o:o + 12]):
            return None
        idx, assoc = struct.unpack_from("<iI", d, o + 12)
        if assoc == GLOBAL_PARAM:
            return (idx, assoc) if idx == -1 else None
        if layers and assoc in (LAYER_PARAM, BLEND_PARAM) and 0 <= idx < 1024:
            return (idx, assoc)
        return None

    out, o = [], lo
    while o < hi:
        if scope(o) is None:
            o += 1
            continue
        n = 1
        while scope(o + stride * n) is not None:
            n += 1
        if n >= 4:
            out.append((o, n))
            o += stride * n
        else:
            # **短串被丢弃时只能前进 1 个字节。** 原来这儿也 `o += stride * n`,
            # 于是一条 1~3 条的假串会把后面**合法数组的起点跳过去**:
            # `MI_Wat_ShuiLanLan2_001_Fx1` 块 0 的向量数组真起点是 0x7c76,被跳到 0x7cbe,
            # 少认了 2 条 —— 而 preshader 里正好引用到 `向量[13]`,解出来是 `向量[13]?`。
            o += 1
    return out


def param_pair(d: bytes, start: int, end: int, layers: bool = False):
    """一个块里的向量参数数组与标量参数数组 → ((向量起点, 条数), (标量起点, 条数))。

    **两个数组会互相冒充,必须一起定。** 36 字节记录的前 24 字节本身就是一条合法的
    24 字节记录(12 零 + `Index` + `Association` + 4 字节值 + 4 字节对齐),反之亦然,
    所以单独按 `max(条数)` 挑会让**在前的那个吃掉后一个的开头**:
    `MI_Wat_ShuiLanLan2_001_Fx1` 块 0 的标量数组认成 33 条(0x798e..0x7ca6),
    把向量数组真起点 0x7c76 盖住,向量只剩 13 条 —— 而 preshader 里引用到 `向量[13]`,
    解出来就是个 `向量[13]?`。

    判据:两个数组在文件里**相邻**,所以谁在前就把它裁到另一个的起点。
    这里 `31 = (0x7c76 − 0x798e) / 24`,向量 15 条 —— 两边都对上了。
    """
    vas = param_arrays(d, VEC_STRIDE, start, end, layers=layers)
    sas = param_arrays(d, SCALAR_STRIDE, start, end, layers=layers)
    if not vas or not sas:
        return (max(vas, key=lambda v: v[1]) if vas else None,
                max(sas, key=lambda v: v[1]) if sas else None)
    vo, nv = max(vas, key=lambda v: v[1])
    so, ns = max(sas, key=lambda v: v[1])
    if so < vo < so + ns * SCALAR_STRIDE:
        ns = (vo - so) // SCALAR_STRIDE
    elif vo < so < vo + nv * VEC_STRIDE:
        nv = (so - vo) // VEC_STRIDE
    return (vo, nv), (so, ns)


MAX_PRESHADER_SIZE = 4096


def preshader_arrays(d: bytes, lo: int = 0, hi: int | None = None, min_len: int = 6):
    """`{OpcodeOffset, OpcodeSize}` 链(offset 连续)→ [(文件偏移, 条数, 首 opcode 偏移)]。

    **要按块限定范围**:全文件扫会把相邻块的链接到一起,或在块边界断开。

    **单条 preshader 的字节数上限不能卡得太小。** 这里原来写 64,而暮星辰裙子那个块
    (`MI_Ill_XingGuang3_001_Fx1` 块 10)的标量链末尾两条是 97 / 115 字节 —— 链在那儿被截断,
    `S` 少认 2 条、总槽算成 137,于是它的 shader 51729(`dcl cb6[142]`)配不上任何块,
    `GLASS_RIM_GAIN` 一直卡着。放宽到 4096 之后那条链是完整的 141 条
    (末条 `off + size = 1364`,正好接上向量链的首偏移 —— 这是链认对了的强自检)。
    真正起作用的判据是**连续性**(`off[i+1] == off[i] + size[i]`),它本身就够强。

    **注意**:放宽之后那个块仍然配不上 51729(`101 + ⌈141/4⌉ + 1 = 138 < dcl 142`),
    向量链也确实到 101 条为止(末条 `2307 + 13 = 2320`,之后是另一条从 0 起的新链)——
    所以 `GLASS_RIM_GAIN` 卡住的原因是**那条 shader 的内容不内联**,不是链没认全。
    """
    hi = len(d) if hi is None else hi
    runs, o = [], lo
    while o + 8 <= hi:
        off, size = struct.unpack_from("<2I", d, o)
        if not (0 < size <= MAX_PRESHADER_SIZE and off < (1 << 20)):
            o += 4
            continue
        cnt, co, cs, p = 1, off, size, o
        while p + 16 <= hi:
            o2, s2 = struct.unpack_from("<2I", d, p + 8)
            if o2 != co + cs or not 0 < s2 <= MAX_PRESHADER_SIZE:
                break
            cnt += 1
            p += 8
            co, cs = o2, s2
        if cnt >= min_len:
            runs.append((o, cnt, off))
        o = p + 8
    return runs

# 根材质 `M_P_Object_Trans` 已命名的向量默认值,用来给「参数 id」标定名字。
# 只收有辨识度的(纯白/全零对不上号)。名字来自 rocom-pets 的 GUID 桥(param-guids.tsv)。
ANCHOR_VECTORS = {
    "BlackMagicDarkColor": (0.05, 0.02, 0.1, 1.0),
    "BlackMagicDarkRimColor": (0.1473, 0.0996, 1.0, 1.0),
    "BlackMagicRimColor": (1.0, 0.011, 0.7991, 1.0),
    "FlowColor": (0.5, 0.0, 0.6, 0.0),
    "FresnelColor": (0.0873, 0.3534, 1.0, 0.0),
    "RimColor": (0.8438, 0.9611, 1.0, 1.0),
    "StarColor": (0.3333, 0.6667, 2.0, 0.0),
    "HighLight Offset": (0.0, 0.0, 0.0, 1.0),
    "HighLight SpecCol": (1.0, 1.0, 1.0, 0.0),
    "RotatorCenter": (0.0, 0.0, 1.0, 1.0),
}

PARAMETER_OPCODE = 3

# opcode 编码**已经查实**:
#   02 <16 字节>   Constant(float4)
#   03 <uint16>    **标量**参数(下标)
#   04 <uint16>    **向量**参数(下标)
#   23 <n> <i0..i3>  ComponentSwizzle:n = 分量数(1..4),后 4 字节是下标(0xff = 不用)
#   24 <n>           AppendVector(n = 已累积的分量数)
#   05/06/0b/18/1a …  其余运算(具体对应还没逐个定)
#
# 23/24 也是查实的:块内所有 `23` 后面 5 字节**无一例外**都是「分量数 1..4 + 下标 0..3 或 0xff」,
# 实测取值只有 xyz / w / xy / zw 四种;`24` 的操作数只有 1 和 2。
#
# 判据是**稠密完整枚举**:实测宠物材质某块里,标量链的单条 `03` 共 80 个、下标 0..79,
# 全部 < 标量参数数 81;向量链的单条 `04` 共 27 个、下标 0..26,全部 < 向量参数数 27。
# 两条各自把自己那张表不重不漏地点了一遍 —— 不可能是巧合。
#
# **原来把 03/04 都当成同一个 `Parameter`,于是被迫编出「向量与标量共享一个索引空间」**,
# 那是错的(而且当时还把两条链认反了,见 `chains`)。
#
# 另一条独立佐证:某槽解出 `Constant(65503.86×4) 标量参数(12) Constant(1e-5×4) 0b 06`
# = `65503.86 / max(参数12, 1e-5)` —— half 最大值 + epsilon,UE 里「限幅再取倒数」的固定写法。
VALID_OPCODES = frozenset(range(1, 48))


def solve_opcode_base(d: bytes, lo: int, hi: int, hdrs, nparams: int):
    """定 preshader opcode 缓冲的基址。

    **判据是「每一条的首字节都是合法 opcode」**,不是「size==3 的条目能解成 Parameter」。
    两个坑:
    ① 只看「首字节落在一个小集合里」时,**全零区域会拿满分**(踩过,解出来 opcode 全是 0);
       所以 0(Nop)不算合法首字节。
    ② 按「Parameter 下标 < 向量参数条数」打分**会把正确基址判负**:实测那个材质的向量链
       106 条里 size==3 的有 70 条,而向量参数只有 19 条(见下面「参数下标空间」那条)。
    """
    best, best_score = None, -(1 << 30)
    total = max(off + size for off, size in hdrs)
    for base in range(lo, max(lo + 1, hi - total)):
        score = 0
        for off, size in hdrs:
            op = d[base + off]
            if op not in VALID_OPCODES:
                score -= 4
                continue
            score += 1
            if size == 3 and op == PARAMETER_OPCODE:
                idx = struct.unpack_from("<H", d, base + off + 1)[0]
                score += 2 if idx < nparams else -4
        if score > best_score:
            best, best_score = base, score
    return best, best_score


def chains(d: bytes, start: int, end: int):
    """一个块里的两条 preshader 头链 → (向量链, 标量链),各为 [(opcode 偏移, 字节数)]。

    **一个块有两条独立的头链、共用一个 opcode 缓冲。** 标量链从 opcode 偏移 0 起,
    向量链**紧接其后** —— 所以判据是硬的:`向量链首偏移 == 标量链所有 size 之和`。

    早先只按「首偏移为 0 的那条是标量」挑前两条,一个块里出现**三条**链就会挑错
    (`MI_P_Object_NoMetal` 那一族每块有 3 条、其中两条都从 0 起,挑出来 V 恒等于 17)。
    再早先还**认反过**:向量链被当成整个 cb,分界永远算错,而且标量链里那一堆 `03`
    被当成「向量槽引用了标量参数」,于是编出一个并不存在的「共享索引空间」。

    认向量/标量的独立佐证是拿汇编反推的:`V` 应当等于「材质 cb 里最后一个以 ≥3 分量
    swizzle 出现的槽位 + 1」,实测这个数总是等于**不从 0 起**的那条链的长度。
    """
    runs = preshader_arrays(d, start, end)
    if len(runs) < 2:
        return None, None

    def hdrs(o: int, cnt: int):
        return [struct.unpack_from("<2I", d, o + 8 * k) for k in range(cnt)]

    best = None
    for so, scnt, sfirst in runs:
        if sfirst != 0:
            continue
        sh = hdrs(so, scnt)
        span = sum(size for _off, size in sh)
        for vo, vcnt, vfirst in runs:
            if vo == so or vfirst != span:
                continue
            if best is None or vcnt + scnt > best[2]:
                best = (hdrs(vo, vcnt), sh, vcnt + scnt)
    if best is not None:
        return best[0], best[1]
    # 兜底:**首偏移非零的那条就是向量链**,从 0 起的取最长的那条当标量链。
    # `M_P_Object` 那一族每块有**三条**链(如 63@989 / 72@0 / 17@0,第三条 17 条是别的东西),
    # 上面那条「向量首偏移 == 标量 opcode 总长」的强判据在它们身上凑不上(777+95 ≠ 989),
    # 所以要有这条兜底 —— 它给出的 V 与汇编数出来的完全一致(63/71/90/90/104/117)。
    vec = max((r for r in runs if r[2] != 0), key=lambda r: r[2], default=None)
    sca = max((r for r in runs if r[2] == 0), key=lambda r: r[1], default=None)
    if vec is None or sca is None:
        return None, None
    return hdrs(vec[0], vec[1]), hdrs(sca[0], sca[1])


SCALAR_PARAM_OPCODE = 3
VECTOR_PARAM_OPCODE = 4
CONSTANT_OPCODE = 2
SWIZZLE_OPCODE = 0x23
APPEND_OPCODE = 0x24
# 操作数字节数(不含 opcode 本身);没列的当 0 字节
OPERAND_BYTES = {CONSTANT_OPCODE: 16, SCALAR_PARAM_OPCODE: 2, VECTOR_PARAM_OPCODE: 2,
                 SWIZZLE_OPCODE: 5, APPEND_OPCODE: 1}


def decode_expr(d: bytes, base: int, off: int, size: int, ctx) -> str:
    """把一条 preshader 的 opcode 流译成可读串。

    `ctx` = (voff, nvec, soff, nsca, patches, start, names) —— `names` 是 `param_names()`
    读出的名字表,`paramId` 是它的下标,于是输出直接带**参数名**。
    """
    voff, nvec, soff, nsca, patches, start, names = ctx

    def tag(pid):
        if pid is None:
            return ""
        return f"#{pid}={names[pid]}" if pid < len(names) else f"#{pid}"

    out, i = [], 0
    b = d[base + off:base + off + size]
    while i < len(b):
        op = b[i]
        n = OPERAND_BYTES.get(op, 0)
        arg = b[i + 1:i + 1 + n]
        if op == VECTOR_PARAM_OPCODE and n == len(arg):
            pi = struct.unpack_from("<H", arg, 0)[0]
            if pi < nvec:
                v = struct.unpack_from("<4f", d, voff + VEC_STRIDE * pi + VALUE_OFF)
                pid = patches.get(voff + VEC_STRIDE * pi - start)
                out.append(f"向量[{pi}]{tag(pid)}"
                           f"({v[0]:g},{v[1]:g},{v[2]:g},{v[3]:g})")
            else:
                out.append(f"向量[{pi}]?")
        elif op == SCALAR_PARAM_OPCODE and n == len(arg):
            pi = struct.unpack_from("<H", arg, 0)[0]
            if pi < nsca:
                v = struct.unpack_from("<f", d, soff + SCALAR_STRIDE * pi + VALUE_OFF)[0]
                pid = patches.get(soff + SCALAR_STRIDE * pi - start)
                out.append(f"标量[{pi}]{tag(pid)}({v:g})")
            else:
                out.append(f"标量[{pi}]?")
        elif op == CONSTANT_OPCODE and n == len(arg):
            v = struct.unpack_from("<4f", arg, 0)
            out.append(f"常量({v[0]:g},{v[1]:g},{v[2]:g},{v[3]:g})")
        elif op == SWIZZLE_OPCODE and n == len(arg):
            out.append("." + "".join("xyzw"[c] for c in arg[1:1 + arg[0]] if c < 4))
        elif op == APPEND_OPCODE and n == len(arg):
            out.append(f"append{arg[0]}")
        else:
            out.append(f"op{op:#04x}")
        i += 1 + n
    return " ".join(out)


def decode_cb(d: bytes, block, vec_arr, scalar_arr, vec_hdrs, sca_hdrs, names=()):
    """→ ([(cb 槽位串, 条目序号, 类型, 值/说明)], opcode 基址, 得分)

    **cb 的布局已经查实**:

        [向量 preshader,每条一个 float4] [标量 preshader,4 条装一个 float4] [UE 追加的一个 float4]

    所以 **`cb[k]`(k < V)就是向量链的第 k 条**,标量 #i 在 `cb[V + i // 4]` 的第 `i % 4` 分量。
    末尾那多出来的一个 float4 是实测出来的:32 条 shader 的 `dcl_constantBuffer` 声明大小
    减去 `V + ceil(S/4)`,差值只有 0 或 +1(声明大小是「最大用到的下标 + 1」,所以 ≤ 总槽数)。
    """
    start, end, patches = block
    voff, nvec = vec_arr
    soff, nsca = scalar_arr
    base, score = solve_opcode_base(d, start, end, vec_hdrs + sca_hdrs, max(nvec, nsca))
    nv = len(vec_hdrs)

    def tag(pid):
        if pid is None:
            return "id=?"
        return f"{names[pid]}" if pid < len(names) else f"id={pid}"

    out = []
    for kind, hdrs in (("vector", vec_hdrs), ("scalar", sca_hdrs)):
        for i, (off, size) in enumerate(hdrs):
            slot = f"cb[{i}]" if kind == "vector" else f"cb[{nv + i // 4}].{'xyzw'[i % 4]}"
            op = d[base + off]
            raw = d[base + off:base + off + size]
            if size == 3 and op in (SCALAR_PARAM_OPCODE, VECTOR_PARAM_OPCODE):
                pi = struct.unpack_from("<H", d, base + off + 1)[0]
                if op == VECTOR_PARAM_OPCODE and pi < nvec:
                    v = struct.unpack_from("<4f", d, voff + VEC_STRIDE * pi + VALUE_OFF)
                    pid = patches.get(voff + VEC_STRIDE * pi - start)
                    out.append((slot, i, kind, f"{tag(pid)} = "
                                f"({v[0]:.4f}, {v[1]:.4f}, {v[2]:.4f}, {v[3]:.4f})"))
                    continue
                if op == SCALAR_PARAM_OPCODE and pi < nsca:
                    v = struct.unpack_from("<f", d, soff + SCALAR_STRIDE * pi + VALUE_OFF)[0]
                    pid = patches.get(soff + SCALAR_STRIDE * pi - start)
                    out.append((slot, i, kind, f"{tag(pid)} = {v:.4f}"))
                    continue
            if op == CONSTANT_OPCODE and size >= 17:
                v = struct.unpack_from("<4f", d, base + off + 1)
                out.append((slot, i, kind,
                            f"常量({v[0]:.4f}, {v[1]:.4f}, {v[2]:.4f}, {v[3]:.4f})"))
                continue
            out.append((slot, i, kind, decode_expr(
                d, base, off, size,
                (voff, nvec, soff, nsca, patches, start, names))))
    return out, base, score


def anchor_name(v, values=None) -> str | None:
    """按「值」标定名字。**值不唯一就不标** —— 否则会把两个不同参数标成同一个名字
    (踩过:`cb5[2]` 与 `cb5[3]` 的默认值都是 (1,1,1,0),都被标成 `HighLight SpecCol`)。
    `values` 给整块的向量默认值列表用于判唯一。"""
    hit = None
    for nm, want in ANCHOR_VECTORS.items():
        if max(abs(a - b) for a, b in zip(v, want)) < 0.003:
            hit = nm
            break
    if hit is None:
        return None
    if values is not None:
        same = sum(1 for u in values
                   if max(abs(a - b) for a, b in zip(u, v)) < 0.003)
        if same > 1:
            return None
    return hit


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("stem", type=Path, help="材质路径(不带后缀,同名 .uasset/.uexp 都要在)")
    ap.add_argument("--names", action="store_true", help="附带打印参数名字表")
    ap.add_argument("--cb", type=int, metavar="块号", help="解出该块的 cb 槽位 → 参数名 = 值")
    ap.add_argument("--slot", type=int, nargs="*", default=None, metavar="槽位",
                    help="配合 --cb,只打印这几个 cb 槽位")
    ap.add_argument("--layers", action="store_true",
                    help="参数数组也收材质图层作用域的条目(水体那类预设必须开)")
    ap.add_argument("--gaps", action="store_true",
                    help="列出**空隙区**(块之外)的 preshader 链对 —— 用来判断某个排列到底存不存在")
    args = ap.parse_args()

    ua = args.stem.with_suffix(".uasset").read_bytes()
    ue = args.stem.with_suffix(".uexp").read_bytes()
    pkg_names = name_map(ua)
    names = param_names(ue)             # **参数名在这张表里,不是包名字表**
    print(f"{args.stem.name}:参数名 {len(names)} 条(包名字表 {len(pkg_names)} 条),"
          f"uexp {len(ue)} 字节")
    if args.names:
        for i, s in enumerate(names):
            print(f"  {i:4d} {s}")

    blocks = patch_tables(ue)
    if args.gaps:
        # **块之外还有成对的链**:结构齐全(V、S、opcode 流、参数记录的值),只是没有补丁表
        # 给名字。判断「某条 shader 的排列在这个材质里到底有没有」时,光看块是不够的。
        # **首偏移是大数(上万)的链是假阳性** —— 真链从 0 或一千上下起。
        covered = [(s, e) for s, e, _ in blocks]
        runs = sorted((r for r in preshader_arrays(ue, 0, len(ue))
                       if not any(a <= r[0] < b for a, b in covered)), key=lambda r: r[0])
        print(f"块之外的链 {len(runs)} 条;成对的:")
        # 文件序是「向量链(首偏移非零)→ 标量链(首偏移 0)→ 一条 17 项的第三链」
        for i in range(len(runs) - 1):
            v, sc = runs[i], runs[i + 1]
            if v[2] == 0 or v[2] > 1 << 14 or sc[2] != 0:
                continue
            tot = v[1] + -(-sc[1] // 4) + 1
            print(f"  向量 V={v[1]:4} @0x{v[0]:06x}  标量 S={sc[1]:4}  总槽={tot:4}")
        return
    if args.cb is not None:
        start, end, patches = blocks[args.cb]
        # **两个数组一起定**(它们会互相冒充,见 param_pair)
        va, sa = param_pair(ue, start, end, layers=args.layers)
        vh, sh = chains(ue, start, end)
        if vh is None:
            print(f"块 {args.cb}:没找到成对的 preshader 头链")
            return
        rows, base, score = decode_cb(ue, (start, end, patches), va, sa, vh, sh, names)
        print(f"块 {args.cb}:向量槽 {len(vh)} 条、标量 {len(sh)} 条;"
              f"向量参数 {va[1]}、标量参数 {sa[1]}")
        print(f"  opcode 基址 0x{base:x}(得分 {score});"
              f"**向量槽 cb[0..{len(vh) - 1}],标量从 cb[{len(vh)}] 起**")
        want = set(args.slot) if args.slot else None
        for slot, i, kind, desc in rows:
            if want is not None and int(slot[3:].split("]")[0]) not in want:
                continue
            print(f"  {slot:<14} [{kind[:3]} {i:3d}] {desc}")
        return

    # **逐块扫、且扫向量时排除标量段**(见 param_arrays 文档:不排除会锁在标量数组内部)
    vecs, scals = [], []
    for bstart, bend, _m in blocks:
        sa = param_arrays(ue, SCALAR_STRIDE, bstart, bend)
        scals += sa
        ex = [(o, o + n * SCALAR_STRIDE) for o, n in sa]
        vecs += param_arrays(ue, VEC_STRIDE, bstart, bend, exclude=ex)
    pres = preshader_arrays(ue)
    print(f"\n冻结块 {len(blocks)} 个(边界已精确锚定)")
    print(f"向量参数数组 {len(vecs)} 段,条数 {[n for _, n in vecs]}")
    print(f"标量参数数组 {len(scals)} 段,条数 {[n for _, n in scals]}")
    print(f"preshader 头链 {len(pres)} 条:")
    for o, c, first in pres:
        # **从 0 起的是标量链**(见 chains());这行的标签一度写反,而块 5 的
        # 79 项(从 1263)/ 116 项(从 0)对上汇编的 V=79、S=116,反着就说不通
        kind = "标量(从 0 起)" if first == 0 else f"向量(从 {first} 起)"
        print(f"  0x{o:06x}  {c:4d} 项  {kind}")

    # 补丁能告出「哪条记录该有名字」,但下标的索引空间还没认出来(见模块头),
    # 所以这里只标出「有补丁 / 没补丁」,不写名字 —— 写了就是错的。
    named = {}
    for start, end, m in blocks:
        for off, n in vecs:
            if start <= off < end:
                named[off] = sum(1 for k in range(n) if (off + VEC_STRIDE * k - start) in m)
    print("\n各向量参数数组的有序默认值(名字的索引空间还没认出来,见模块头):")
    for off, n in vecs:
        print(f"  --- 0x{off:06x}  {n} 条,其中 {named.get(off, 0)} 条有名字补丁")
        for k in range(n):
            v = struct.unpack_from("<4f", ue, off + VEC_STRIDE * k + VALUE_OFF)
            print(f"    [{k:2d}] ({v[0]:8.4f},{v[1]:8.4f},{v[2]:8.4f},{v[3]:8.4f})")


if __name__ == "__main__":
    main()
