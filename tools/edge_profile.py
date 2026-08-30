#!/usr/bin/env python3
"""量**描边**:把实机截图与渲图的「边界剖面」摆在一起比,并判宽度按哪条律走。

素材不入仓库:实机截图放 `~/Downloads/rocom/screenshot-pets/<形态名>.png`,
宠物包放 `~/Downloads/rocom/packs-all/`(`exporter --all` 的产物)。

    uv run --with numpy --with pillow --with scipy python tools/edge_profile.py
    EDGE_PACKS=~/Downloads/rocom/packs-F uv run … tools/edge_profile.py --profile 学院呱呱

**为什么要单独一个工具:`cmp_shots.py` 的「描边比」判不了描边。** 那个比值是
`(我们的描边环 ÷ 主体) ÷ (实机的同一个比)`,而两边的边缘锐度不同(我们硬、实机抗锯齿),
描边一变深这个差异就被放大。更坑的是**它的中位会因为「一半太粗一半太细」而好看** ——
2026-08-29 把宽度律改对的那一轮,中位反而从 1.02 走到 1.05,而均方偏离 1 从
0.268 降到 0.176、离群从 7 只降到 3 只。**判描边只看离散,或者用这里的剖面。**

## 两个混淆项必须先堵上,否则会得出反向结论

1. **抗锯齿**:实机截图是抗锯齿的,我们是硬边 ⇒ 我们这侧渲 4 倍再盒式下采样(4×SSAA)。
2. **尺度**:两边取景不同 ⇒ 按**宠物像素高**把我们缩到实机同一档再比。

## 判据:暗环面积

`Σ (参考电平 − 剖面)`,里侧比主体、外侧比背景。颜色按汇编导对之后,这个面积 ≈ 宽度,
而且**对模糊稳健**(高斯模糊不改积分)。三条注意:

- **外侧那几像素也要算**:糊到轮廓外的部分在按颜色抠图的实机侧不算宠物,只统计里侧
  会系统性少算实机。(实测实机那圈**没有**明显外溢,但判据里留着,免得换宠物就失效。)
- **不能砍底部水印**。`tools/gamemask.py` 那条 `m[0.86h:] = False` 是给 shrink=4 的
  粗略抠图用的;在这里砍会把脚剪掉 ⇒ 宠物高度算错 ⇒ 整条缩放跟着错(踩过:学院呱呱
  571 → 509px,我们的描边凭空细了 12%)。
- **暗环太浅的不可比**:实机面积 < 0.30 时这个比值全是噪声,直接跳过。

## 三条候选律

宽度到底是「世界空间常数」还是「屏幕空间常数」,汇编里由 `clamp(clip.w, Min, Max)` 决定
(见 `exporter/Materials.cs` 的 `OutlineOf`)。拿实机反推的宽度换三种归一化,
**哪一列最像常数就是哪条律**:

    世界空间常数   实机px × height_cm / 宠物px    ← 实机描边的厘米数
    屏幕空间常数   实机px                        ← 同一张画面里的像素数
    占宠物大小     实机px / 宠物px               ← 我们的正交取景下与前者等价

2026-08-29 那次 12 只的结果是 7.06 / 4.05 / 3.42(max/min),原来的世界空间常数明显最差。

**这个估计量不干净**:姿势、落地投影、特效层都会掺进来,变异系数在 0.4 上下。
它只够分辨「哪条律更像」,**别拿它去推更细的结论**(比如标定系数的第二位)。
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

SHOTS = Path.home() / "Downloads/rocom/screenshot-pets"
PACKS = Path(os.environ.get("EDGE_PACKS", Path.home() / "Downloads/rocom/packs-all"))
BIN = Path(os.environ.get(
    "EDGE_BIN", Path(__file__).resolve().parent.parent / "target/release/rocom-pets"))
# 渲图缓存。**和 cmp_shots 同一条规矩**:缓存比二进制旧就重渲,否则改完 shader
# 再跑比的还是旧图(见 cmp_shots.py 的坑 4)。
OUT = Path(os.environ.get("EDGE_OUT", "/tmp/edge_profile"))
SS = 4              # 超采样倍数
SIZE = 1024         # 下采样后的画布边长
MIN_GAME_PX = 200   # 实机宠物太小就别比了
MIN_AREA = 0.30     # 暗环太浅 ⇒ 比值是噪声

# **那条深色带根本不是描边的**,照常打印但不计进汇总。判据不是「数字难看」,
# 而是查实了它的成因;每条都要写清楚查到了什么。
KNOWN_NOT_OUTLINE = {
    "莫比乌乌": "尾巴是半透的(不画壳时 alpha 中位 0.30),那条带是**背面的描边壳透过正面**"
                "显出来的 —— 与外扩量无关,把宽度降 5.6 倍它几乎不变。见 docs/design.md",
}


def forms():
    """→ [(截图名, asset, height_cm, 包目录, 描边宽度(米))],按 `height_cm` 升序。

    截图名的匹配和 `cmp_shots.pick_asset` 同一套:先精确名,再试「XX(截图名)」,
    最后试「截图名(XX)」—— manifest 里既有「鸭吉吉(蓬松的样子)」也有别的变体写法。
    """
    have = {p.stem for p in SHOTS.glob("*.png")}
    out, seen = [], set()
    for man in sorted(PACKS.glob("*/manifest.toml")):
        txt = man.read_text()
        for m in re.finditer(
                r'^name = "([^"]+)"\nstage = \d+\nasset = "([^"]+)"(?:.*\n)*?'
                r'^height_cm = ([0-9.]+)', txt, re.M):
            name, asset, hcm = m.group(1), m.group(2), float(m.group(3))
            paren = re.search(r"\((.+)\)$", name)
            cands = [name, re.sub(r"\(.*", "", name)] + ([paren.group(1)] if paren else [])
            key = next((k for k in cands if k in have), None)
            if key is None or key in seen:
                continue
            seen.add(key)
            out.append((key, asset, hcm, man.parent, outline_width(txt, asset)))
    return sorted(out, key=lambda r: r[2])


def outline_width(manifest_text: str, asset: str) -> float:
    """这个形态的描边宽度(**米**)。取材质表里最大的那个 —— 轮廓由本体材质决定,
    而同一形态里眼/嘴那类槽经常是 0。

    **必须从 manifest 读,不能写死 0.0039**:宽度自 2026-08-29 起随宠物大小走
    (见 `exporter/Materials.cs` 的 `OutlineOf`),写死会让这个工具的反推整个失真。
    材质行里带着 `forms/<asset>/` 的贴图路径,拿它认这个形态的那几行。
    """
    widths = [float(w.group(1))
              for line in manifest_text.splitlines() if f"forms/{asset}/" in line
              for w in [re.search(r"outline_width = ([0-9.]+)", line)] if w]
    return max(widths) if widths else 0.0


def game(name: str):
    """实机截图 → (RGB 图, 宠物遮罩, 背景色)。抠不出来返回 None。"""
    a = np.asarray(Image.open(SHOTS / f"{name}.png").convert("RGB"), np.float32) / 255.0
    bg = np.median(np.concatenate([a[:, :20], a[:, -20:]], axis=1), axis=1)
    m = np.linalg.norm(a - bg[:, None, :], axis=2) > 0.10
    lab, n = ndimage.label(m)
    if n == 0:
        return None
    sizes = ndimage.sum(m, lab, range(1, n + 1))
    m = ndimage.binary_fill_holes(lab == (np.argmax(sizes) + 1))
    if m.mean() > 0.55:              # 抠图失败,见 gamemask.py
        return None
    ys, _ = np.nonzero(m)
    if ys.max() >= a.shape[0] - 3 or ys.min() <= 2:
        return None                  # 宠物被画面裁掉,高度不可信
    return a, m, np.median(bg, axis=0).astype(np.float32)


def render(pack: Path, asset: str) -> Path | None:
    png = OUT / f"{asset}.png"
    if png.exists() and BIN.exists() and png.stat().st_mtime > BIN.stat().st_mtime:
        return png
    OUT.mkdir(parents=True, exist_ok=True)
    r = subprocess.run([str(BIN), "--render", str(pack), "--form", asset, "--clips", "Idle",
                        "--yaw", "25", "--size", str(SIZE * SS), "--time", "0.7",
                        "--no-fade", "-o", str(png)], capture_output=True)
    return png if r.returncode == 0 and png.exists() else None


def ours(png: Path, bgcol, target_h: int):
    """渲图 → (合成到实机背景色的 RGB, 遮罩)。4×SSAA + 按宠物像素高缩到实机同尺度。"""
    a = np.asarray(Image.open(png).convert("RGBA"), np.float32) / 255.0
    h, w, _ = a.shape
    a = a.reshape(h // SS, SS, w // SS, SS, 4).mean(axis=(1, 3))
    comp = a[..., :3] * a[..., 3:4] + bgcol * (1 - a[..., 3:4])   # 渲图是预乘 alpha 的
    m = a[..., 3] > 0.5
    if m.sum() < 500:
        return None
    ys, _ = np.nonzero(m)
    s = target_h / (ys.max() - ys.min() + 1)
    if abs(s - 1) > 0.02:
        nh, nw = max(int(comp.shape[0] * s), 8), max(int(comp.shape[1] * s), 8)
        comp = np.asarray(Image.fromarray((np.clip(comp, 0, 1) * 255).astype(np.uint8))
                          .resize((nw, nh), Image.LANCZOS), np.float32) / 255.0
        m = np.asarray(Image.fromarray((m * 255).astype(np.uint8))
                       .resize((nw, nh), Image.LANCZOS)) > 128
    return comp, m


def profile(a, m):
    """→ (剖面 −3..6, 主体, 背景, 暗环面积);量不了返回 None。

    只取宠物竖直方向中段 15%~85%,避开头顶与脚下(落地投影会污染外侧那几像素)。
    """
    sd = np.where(m, ndimage.distance_transform_edt(m), -ndimage.distance_transform_edt(~m))
    lum = a @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    ys, _ = np.nonzero(m)
    ylo, yhi = ys.min(), ys.max()
    band = np.zeros_like(m)
    band[ylo + int(.15 * (yhi - ylo)):ylo + int(.85 * (yhi - ylo))] = True
    body = lum[(sd > 8) & (sd < 25) & band]
    bg = lum[(sd < -8) & (sd > -25) & band]
    if body.size < 200 or bg.size < 200:
        return None
    body, bg = float(np.median(body)), float(np.median(bg))
    if abs(body - bg) < 0.08:        # 主体与背景太接近,暗环量不出来
        return None
    ks = list(range(-3, 7))
    prof, area = [], 0.0
    for k in ks:
        sel = (sd > k - .5) & (sd <= k + .5) & band
        v = float(np.median(lum[sel])) if sel.sum() >= 80 else float("nan")
        prof.append(v)
        if not np.isnan(v):
            area += max((body if k > 0 else bg) - v, 0.0)
    return prof, body, bg, area


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--profile", metavar="形态名", nargs="*",
                    help="只打这几只的逐档剖面(实机 / 我们),不跑全量")
    ap.add_argument("--law", action="store_true",
                    help="额外打三条宽度律的离散度。**只在「当前实现明显不对」时才有意义**,"
                         "见下面那段警告")
    args = ap.parse_args()

    if not BIN.exists():
        sys.exit(f"先 cargo build --release:{BIN} 不存在")
    todo = forms()
    if not todo:
        sys.exit(f"{PACKS} 下没有能和截图对上的形态 —— 这里要的是**解开后的包目录**,"
                 f"不是 .rkpet;`EDGE_PACKS` 可以指到别处")
    if args.profile:
        todo = [r for r in todo if r[0] in args.profile]

    print(f"{'宠物':10s} {'height_cm':>9s} {'实机px':>6s} {'实机面积':>8s} {'我们面积':>8s} "
          f"{'比值':>6s} {'我们描边px':>9s} {'反推实机px':>9s}")
    res = []
    for name, asset, hcm, pack, wid in todo:
        g = game(name)
        if g is None:
            print(f"{name:10s} 抠图不可用,跳过")
            continue
        ga, gm, bgc = g
        ys, _ = np.nonzero(gm)
        gh = ys.max() - ys.min() + 1
        if gh < MIN_GAME_PX:
            print(f"{name:10s} 实机宠物只有 {gh}px,跳过")
            continue
        png = render(pack, asset)
        if png is None:
            print(f"{name:10s} 渲染失败,跳过")
            continue
        o = ours(png, bgc, gh)
        if o is None:
            print(f"{name:10s} 渲图为空,跳过")
            continue
        pg, po = profile(ga, gm), profile(*o)
        if pg is None or po is None or pg[3] < MIN_AREA:
            print(f"{name:10s} 暗环量不出来(面积 {pg[3]:.2f} 或主体≈背景),跳过"
                  if pg else f"{name:10s} 暗环量不出来,跳过")
            continue
        if args.profile:
            print(f"\n== {name}  实机宠物高 {gh}px  描边宽度 {wid} 米")
            for tag, p in (("实机", pg), ("我们", po)):
                print(f"   {tag}  外3..1 / 内1..6: "
                      + " ".join("  n/a" if np.isnan(v) else f"{v:.3f}" for v in p[0])
                      + f"   主体={p[1]:.3f} 背景={p[2]:.3f} 暗环面积={p[3]:.3f}")
            continue
        ratio = po[3] / pg[3]
        ourpx = wid * gh / (hcm / 100.0)      # 世界宽度 ÷ 宠物世界高 = 描边px ÷ 宠物px
        imp = ourpx / ratio
        res.append((name, hcm, gh, ratio, ourpx, imp))
        print(f"{name:10s} {hcm:9.1f} {gh:6d} {pg[3]:8.3f} {po[3]:8.3f} "
              f"{ratio:6.2f} {ourpx:9.2f} {imp:9.2f}")

    if args.profile or len(res) < 5:
        return
    good = [x for x in res if x[0] not in KNOWN_NOT_OUTLINE]
    r = np.array([x[3] for x in good])
    print(f"\n{len(good)} 只计进汇总(另有 {len(res) - len(good)} 只不计,见下)。"
          f"**面积比**(我们 / 实机,目标 1.00):")
    print(f"  中位 {np.median(r):.2f}   均方偏离 1 {float(np.sqrt(((r - 1) ** 2).mean())):.3f}   "
          f"max/min {r.max() / r.min():.2f}")
    print("  **看离散,别看中位** —— 过粗与过细会互相抵消,中位好看不代表对(见模块头)。")
    for name, why in KNOWN_NOT_OUTLINE.items():
        if any(x[0] == name for x in res):
            print(f"  [不计] {name}:{why}")

    if not args.law:
        return
    print("\n⚠ **三条律的离散度只在「当前实现明显不对」时才判得动。** 一旦宽度已经改对,"
          "\n  `反推实机px = 我们px ÷ 面积比` 里我们那一项也跟着按新律走了,三列会一起被"
          "\n  面积比的噪声主导 —— 这时候数字变差**不代表**律选错了。要重判就先把实现"
          "\n  换回旧律再跑。2026-08-29 那次的判定值是 7.06 / 4.05 / 3.42(世界/屏幕/占体高)。")
    for tag, col in (("世界空间常数 厘米", lambda x: x[5] * x[1] / x[2]),
                     ("屏幕空间常数 画面px", lambda x: x[5]),
                     ("占宠物大小 占体高%", lambda x: x[5] / x[2] * 100)):
        v = np.array([col(x) for x in good])
        print(f"  {tag:22s} 中位 {np.median(v):8.3f}   max/min {v.max() / v.min():5.2f}   "
              f"变异系数 {v.std() / v.mean():.2f}")


if __name__ == "__main__":
    main()
