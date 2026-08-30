#!/usr/bin/env python3
"""从解包出来的配置表生成 `src/pet/glassy_table.rs` —— 炫彩(`MDT_GLASS`)的配色/粒子/隐藏款三张表。

三张表都很小(39 + 4 + 4 行),但字段多、全是浮点,**手抄一定会错**,所以照 `web/scripts/
gen_catalog.py` 的先例用生成器出。生成物**入仓库**(和 `persona.rs` 里那七条性格同一个道理:
是从游戏配置里搬出来的少量常量,不是素材),这样编译不依赖解包树。

    uv run python scripts/gen_glassy.py                 # 默认读 $ROCOM_PARSED
    uv run python scripts/gen_glassy.py --check         # 只比对,不写(CI/回归用)

来源表:

| 表 | 用途 |
| --- | --- |
| `COLOR_RANDOM_CONF` | 常规炫彩的 39 组配色(`mat_color_1/2` → `RedChannel`/`GreenChannel`) |
| `PARTICLE_RANDOM_CONF` | 4 种粒子(贴图 + `StarStickTiling`) |
| `HIDDEN_GLASS_CONF` | 隐藏(常驻)与赛季炫彩,各带一整套要覆盖的参数 |

客户端那边打包成 `glass_value = (粒子id << 20) | 配色id`,见
`PetMutationUtils.DecodeShineColorId`;本文件保持同一套编号,好和游戏里的展示对得上。
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# 隐藏款要覆盖的标量,顺序即 `GlassyParams` 的字段顺序。值缺省时沿用根材质默认(见下)。
SCALARS = [
    ("StarIntensity", "star_intensity"),
    ("GlobalRefraction", "global_refraction"),
    ("GlobalDepth", "global_depth"),
    ("MainTexFlowSpeedX", "main_tex_flow_x"),
    ("MainTexFlowSpeedY", "main_tex_flow_y"),
    ("MainTexTiling", "main_tex_tiling"),
    ("NormalEffectAmount", "normal_effect_amount"),
    ("BaseColorDetail", "base_color_detail"),
]

# 根材质 `M_P_Object` 的默认值(`exporter/RootDefaults.cs` 那条路读出来的,幽星光
# `MI_..._By` 的冻结块逐条复核过)。**现在只留作参考**:隐藏款没列到的标量沿用
# 材质实例自己那份(见下面 `GlassyOverrides` 那段),运行时的兜底在 `glassy::ROOT_PARAMS`。
# 根材质 `M_P_Object` 的 `StickRandomColor01..04` —— 星贴层四段渐变的色标。
# 与 pet.wgsl 的 `STICK_RAMP_0..3` 是同一组数(那边是既有星贴层用的,同一族同一条公式)。
ROOT_STICK_COLORS = [
    [0.9462, 0.0636, 0.0214, 1.0],
    [0.9601, 0.1603, 0.9074, 1.0],
    [0.0489, 0.1545, 0.9774, 1.0],
    [0.9253, 0.7416, 0.0273, 1.0],
]

ROOT_DEFAULTS = {
    "star_intensity": 1.0,
    "global_refraction": 2.0,
    "global_depth": 30.0,
    "main_tex_flow_x": 0.0,
    "main_tex_flow_y": 0.1,
    "main_tex_tiling": 1.5,
    "normal_effect_amount": 0.1,
    "base_color_detail": 0.35,
}


def rows(parsed: Path, name: str) -> dict:
    path = parsed / "NRC/Content/ScriptC/Data/Bin/BinDataCompressed" / f"{name}.json"
    if not path.exists():
        sys.exit(f"找不到 {path}\n先在 rocom-capture 里跑 scripts/unpack.sh,再用 --parsed 指过来")
    return json.loads(path.read_text(encoding="utf-8"))["RocoDataRows"]


def asset_name(long_path: str | None) -> str:
    """`Texture2D'/Game/…/Tex_X.Tex_X'` → `Tex_X`。取包名(点号前那半)而不是对象名,
    两者在本作里恒等,但包名才是解包后的文件名。"""
    if not long_path:
        return ""
    inner = long_path.split("'")[1] if "'" in long_path else long_path
    return inner.split(".")[0].rsplit("/", 1)[-1]


def hex_rgb(s: str) -> int:
    return int(s.lstrip("#")[:6], 16)


def f(x: float) -> str:
    """浮点字面量。整数也带小数点,免得 Rust 把它当整型。"""
    return f"{x!r}" if "." in repr(x) or "e" in repr(x) else f"{x}.0"


def vec3(v: list[float]) -> str:
    return "[" + ", ".join(f(x) for x in v[:3]) + "]"


def vec4(v: list[float]) -> str:
    padded = list(v[:4]) + [1.0] * (4 - len(v[:4]))
    return "[" + ", ".join(f(x) for x in padded) + "]"


def strip_rich(s: str) -> str:
    """`<span color="#eebf31">暗夜拾光</>` → `暗夜拾光`。配置表里的名字带富文本标签。"""
    out, depth = [], 0
    for ch in s:
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth = max(0, depth - 1)
        elif depth == 0:
            out.append(ch)
    return "".join(out)


def build(parsed: Path) -> str:
    colors = rows(parsed, "COLOR_RANDOM_CONF")
    particles = rows(parsed, "PARTICLE_RANDOM_CONF")
    hidden = rows(parsed, "HIDDEN_GLASS_CONF")

    out: list[str] = []
    w = out.append
    w("// 本文件由 scripts/gen_glassy.py 生成,请勿手改。")
    w("//")
    w("// 数据来自游戏配置表 COLOR_RANDOM_CONF / PARTICLE_RANDOM_CONF / HIDDEN_GLASS_CONF;")
    w("// 字段含义与渲染公式见 src/pet/glassy.rs 与 docs/design.md「炫彩」那节。")
    w("")
    w("use super::glassy::{GlassyColor, GlassyOverrides, GlassyParticle, HiddenGlass};")
    w("")

    # ---- 配色
    w(f"/// 常规炫彩的配色表:`mat_color_1` → `RedChannel`、`mat_color_2` → `GreenChannel`。")
    w(f"/// 全表 {len(colors)} 条,`ratio` 全部相等 ⇒ 等概率;`shine_strength` 全表恒为 10。")
    w(f"pub static COLORS: [GlassyColor; {len(colors)}] = [")
    for k in sorted(colors, key=lambda x: int(x)):
        c = colors[k]
        w("    GlassyColor {")
        w(f"        id: {c['id']},")
        w(f"        name: \"{c['name']}\",")
        w(f"        red_channel: {vec3(c['mat_color_1'])},")
        w(f"        green_channel: {vec3(c['mat_color_2'])},")
        w(f"        ui_color_1: 0x{hex_rgb(c['ui_color_1']):06x},")
        w(f"        ui_color_2: 0x{hex_rgb(c['ui_color_2']):06x},")
        w(f"        shine_strength: {f(float(c['shine_strength']))},")
        w("    },")
    w("];")
    w("")

    # ---- 粒子
    w("/// 常规炫彩的粒子表。`tex` 是共享贴图名(不带目录),运行时到炫彩素材目录里找。")
    w(f"pub static PARTICLES: [GlassyParticle; {len(particles)}] = [")
    for k in sorted(particles, key=lambda x: int(particles[x].get("sort_id", 0))):
        p = particles[k]
        w("    GlassyParticle {")
        w(f"        id: {p['id']},")
        w(f"        name: \"{p['name']}\",")
        w(f"        tex: \"{asset_name(p['particle_res'])}\",")
        w(f"        star_stick_tiling: {f(float(p['StarStickTiling']))},")
        w("    },")
    w("];")
    w("")

    # ---- 隐藏 / 赛季
    w("/// 隐藏炫彩。`type=1` 是常驻款(任何宠物都能上),`type=2` 是赛季款 ——")
    w("/// 赛季款只有 `season_pets` 里那几只有专属贴图,其余宠物走与常驻款相同的通用覆盖。")
    w(f"pub static HIDDEN: [HiddenGlass; {len(hidden)}] = [")
    for k in sorted(hidden, key=lambda x: int(x)):
        h = hidden[k]
        tex = {t["tex_param_name"]: asset_name(t["tex_param_path"]) for t in h.get("tex_param", [])}
        # **列在表里 = 覆盖了;而值那一栏缺席 = 值是 0。** RocoBinData 序列化时把零值字段
        # 整个丢掉,所以「有 `num_param_name` 却没有 `num_param_value`」不是「没设」,
        # 是「设成了 0」。原来这里读成 None 再退回根默认,于是铅字幻梦(把
        # `MainTexFlowSpeedY` 与 `NormalEffectAmount` 都设成 0)在我们这儿照样在流动,
        # 而实机是完全静止的。狂欢怪谈两个流速也都是 0,同样受影响。
        # 参数**根本没列**才是「没设」,那时才落到根默认。
        col = {c["color_param_name"]: c.get("color_param_value", [0.0, 0.0, 0.0, 0.0])
               for c in h.get("color_param", [])}
        num = {n["num_param_name"]: n.get("num_param_value", 0.0)
               for n in h.get("num_param", [])}
        # 粒子颜色 1..4 是**四段渐变的四个色标**(不是四个离散色),按每颗粒子自己的
        # `k` 取值 —— 所以粒子一边涨缩一边换色。没列出来的退回**根材质默认**
        # (`ROOT_STICK_COLORS`),不是白:白会把那一段渐变冲淡成灰。
        sticks = [col.get(f"StickRandomColor{i:02d}", ROOT_STICK_COLORS[i - 1])
                  for i in range(1, 5)]
        w("    HiddenGlass {")
        w(f"        id: {h['id']},")
        w(f"        name: \"{strip_rich(h['name'])}\",")
        w(f"        season: {'true' if h['type'] == 2 else 'false'},")
        w(f"        red_channel: {vec4(col.get('RedChannel', h['glass_color_1']))},")
        w(f"        green_channel: {vec4(col.get('GreenChannel', h['glass_color_2']))},")
        w(f"        main_tex: \"{tex.get('MainTex', '')}\",")
        w(f"        star_tex: \"{tex.get('StarStickTex', '')}\",")
        w("        stick_colors: [")
        for s in sticks:
            w(f"            {vec4(s)},")
        w("        ],")
        # **没列到的那几条 = 不覆盖**,沿用材质自己的值(不是回根默认)——
        # lua 的 `num_param` 只写清单里那几个名字,别的参数在材质实例上原封不动。
        # 狂欢怪谈就没列 `MainTexTiling`,而加油海葵那种材质自己写着 0.2,回根默认(1.5)
        # 会把花纹凭空细 7.5 倍。
        w("        params: GlassyOverrides {")
        for conf_name, field in SCALARS:
            val = num.get(conf_name)
            w(f"            {field}: {'None' if val is None else f'Some({f(float(val))})'},")
        w("        },")
        pets = h.get("season_pet") or []
        w(f"        season_pets: &{list(pets)!r}".replace("[", "[").replace("]", "]") + ",")
        w("    },")
    w("];")
    w("")
    return "\n".join(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--parsed", type=Path,
                    default=Path(os.environ.get("ROCOM_PARSED",
                                                Path.home() / "Downloads/rocom/parsed")))
    ap.add_argument("--out", type=Path,
                    default=Path(__file__).resolve().parent.parent / "src/pet/glassy_table.rs")
    ap.add_argument("--check", action="store_true", help="只比对已有产物,不写")
    args = ap.parse_args()

    text = build(args.parsed)
    if args.check:
        old = args.out.read_text(encoding="utf-8") if args.out.exists() else ""
        if old != text:
            sys.exit(f"{args.out} 与配置表不一致,重跑 scripts/gen_glassy.py")
        print(f"{args.out} 与配置表一致")
        return
    args.out.write_text(text, encoding="utf-8")
    print(f"写出 {args.out}({len(text.splitlines())} 行)")


if __name__ == "__main__":
    main()
