#!/usr/bin/env python3
"""拿实机截图对照渲图,给出可复现的差距数字。

素材不入仓库:实机截图放 `~/Downloads/rocom/screenshot-pets/<形态名>.png`,
宠物包放 `~/Downloads/rocom/packs-all/`(`exporter --all` 的产物)。

    uv run --with numpy --with pillow python tools/cmp_shots.py

## 三个坑,都踩过一次,判据里都堵上了

1. **实机侧抠图不能只按「与角落背景色的距离」**:好几张截图里宠物很小、背景是带花纹的卡片,
   按距离判会把大片背景算成宠物(菊花梨的色偏因此虚高到 1.46,修正后 0.16)。
   见 `gamemask.py`:取最大连通块 + 面积占比 > 55% 视为抠图失败。
2. **同名形态散在几十个包里、asset 还不一样**(鸭吉吉 6 个变体、岚鸟 4 个),随便挑会挑到
   异地/配色变体 —— 挑错了色偏能差 30 倍(鸭吉吉 `Ar_003` 0.200 vs `_001` **0.025**)。
   规则:优先不带 `Ar`、后缀数字最小的 asset。
3. **整只轮廓上的「中位颜色」不可比**:两边取景不同(红绒十字那张裁掉了腿、白身占主导),
   中位色能差一大截 —— 那是构图差异不是颜色错。所以这里同时给**形状相对**的量
   (描边环 ÷ 主体、`p75−p25` ÷ 中位),那两个才稳。

另外:**有些宠物有异色形态**(`MutationColorSwitch` + `RedChannel`/`GreenChannel`/`BlueChannel`
+ `MaskTex` 分区),我们没实现;拿截图当参考前要先确认那张不是异色版。
"""
import os
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

sys.path.insert(0, str(Path(__file__).parent))
from gamemask import game_mask  # noqa: E402

SHOTS = Path.home() / "Downloads/rocom/screenshot-pets"
PACKS = Path.home() / "Downloads/rocom/packs-all"
BIN = Path(__file__).parent.parent / "target/release/rocom-pets"


def pick_asset(name: str):
    """→ (包目录, asset);见模块头第 2 条。"""
    best = None
    for man in sorted(PACKS.glob("*/manifest.toml")):
        txt = man.read_text()
        m = re.search(rf'^name = "{re.escape(name)}"$\n(?:.*\n)*?^asset = "([^"]+)"$',
                      txt, re.M)
        if not m:
            continue
        asset = m.group(1)
        key = (1 if "Ar_" in asset else 0, asset)
        if best is None or key < best[0]:
            best = (key, man.parent, asset)
    return (best[1], best[2]) if best else (None, None)


def ours(path: Path, down=4, erode=2):
    im = Image.open(path).convert("RGBA")
    im = im.resize((im.width // down, im.height // down), Image.LANCZOS)
    a = np.array(im).astype(float)
    m = a[..., 3] > 200
    inner = np.array(Image.fromarray((m * 255).astype(np.uint8))
                     .filter(ImageFilter.MinFilter(erode * 2 + 1))) > 127
    return a[..., :3], inner, m & ~inner


def main() -> None:
    out = Path(os.environ.get("CMP_OUT", "/tmp/cmp_shots"))
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    for shot in sorted(SHOTS.glob("*.png")):
        name = shot.stem
        pack, asset = pick_asset(name)
        if pack is None:
            print(f"  {name}: 包里找不到同名形态,跳过")
            continue
        png = out / f"{name}.png"
        if not png.exists():
            subprocess.run([str(BIN), "--render", str(pack), "--form", asset,
                            "--clips", "Idle", "--yaw", "25", "--size", "2400",
                            "--time", "0.7", "--no-fade", "-o", str(png)],
                           capture_output=True)
        if not png.exists():
            continue
        a, sel = game_mask(str(shot))
        if sel is None:
            print(f"  {name}: 实机侧抠图失败(背景占比过大),跳过")
            continue
        gin = np.array(Image.fromarray((sel * 255).astype(np.uint8))
                       .filter(ImageFilter.MinFilter(5))) > 127
        gring = sel & ~gin
        A, inner, ring = ours(png)
        if gin.sum() < 300 or gring.sum() < 100:
            continue
        ma, mb = np.median(A[inner], axis=0), np.median(a[gin], axis=0)
        la, lb = A[inner].mean(1), a[gin].mean(1)
        iqr = lambda x: (np.percentile(x, 75) - np.percentile(x, 25)) / np.median(x)
        rows.append((name, ma.mean() / mb.mean(),
                     float(np.abs(ma / ma.mean() - mb / mb.mean()).sum()),
                     (np.median(A[ring].mean(1)) / np.median(la))
                     / (np.median(a[gring].mean(1)) / np.median(lb)),
                     iqr(la) / iqr(lb), asset))
    rows.sort(key=lambda r: -r[2])
    print(f'{"形态":12} {"亮度比":>7} {"色偏":>6} {"描边比":>7} {"对比比":>7}  asset')
    for r in rows:
        print(f"{r[0]:12} {r[1]:7.2f} {r[2]:6.3f} {r[3]:7.2f} {r[4]:7.2f}  {r[5]}")
    if rows:
        med = lambda i: np.median([r[i] for r in rows])
        print(f"\n中位: 亮度 {med(1):.2f}  色偏 {med(2):.3f}  描边 {med(3):.2f}  对比 {med(4):.2f}")
        print("目标都是 1.00(色偏是 0.00)。亮度中位偏低是**实机的场景雾**,不要去追,见 docs/design.md §1.1")


main()
