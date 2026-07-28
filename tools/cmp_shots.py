#!/usr/bin/env python3
"""拿实机截图对照渲图,给出可复现的差距数字。

素材不入仓库:实机截图放 `~/Downloads/rocom/screenshot-pets/<形态名>.png`,
宠物包放 `~/Downloads/rocom/packs-all/`(`exporter --all` 的产物)。

    uv run --with numpy --with pillow python tools/cmp_shots.py

## 五个坑,都踩过一次,判据里都堵上了

1. **实机侧抠图会把「颜色贴近背景」的部位整段丢掉**,而且**填充率检测不出来**。
   红绒十字那张背景是橙色卡片、红腿距背景只有 39,抠图只剩乳白上身 ——
   调色板虚高到 0.239、`对比比` 飙到 12.20。判据见 `dropped_by_bg`:
   「我们有一大块 + 色近背景 + 实机选区里几乎没有」三条同时成立就判不可比。
2. **实机侧抠图不能只按「与角落背景色的距离」**:好几张截图里宠物很小、背景是带花纹的卡片,
   按距离判会把大片背景算成宠物(菊花梨的色偏因此虚高到 1.46,修正后 0.16)。
   见 `gamemask.py`:取最大连通块 + 面积占比 > 55% 视为抠图失败。
3. **同名形态散在几十个包里、asset 还不一样**(鸭吉吉 6 个变体、岚鸟 4 个),随便挑会挑到
   异地/配色变体 —— 挑错了色偏能差 30 倍(鸭吉吉 `Ar_003` 0.200 vs `_001` **0.025**)。
   规则:优先不带 `Ar`、后缀数字最小的 asset。
4. **渲图缓存会骗人**:`/tmp/cmp_shots`(或 `CMP_OUT`)里已有同名 png 就直接复用,
   于是改完 shader 再跑,比的还是旧图、数字一行都不变。判据已改成「缓存比二进制旧就重渲」。
5. **整只轮廓上的「中位颜色」不可比**:两边取景不同(红绒十字那张裁掉了腿、雪影娃娃那张
   是特写),中位色能差一大截 —— 那是构图差异不是颜色错。**所以主指标用「调色板距离」**:
   两边各取主色再做加权最近邻匹配,构图只改各色的占比、不改调色板成员。实测三只被构图
   带偏的立刻归位:红绒十字 1.666 → **0.236**、雪影娃娃 0.743 → **0.098**、
   暮星辰 0.650 → **0.085**。中位色偏仍然打印,但只当参考。
   形状相对的量(描边环 ÷ 主体、`p75−p25` ÷ 中位)同样不受构图影响。

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


def palette(px, k=6, bins=8):
    """粗量化取前 k 个主色 → [(按亮度归一化的色, 占比)]。

    归一化是为了把整体明暗差(实机有场景雾)从色度比较里剔掉;分母有下限,见下面。
    """
    q = (px // (256 // bins)).astype(int)
    key = q[:, 0] * bins * bins + q[:, 1] * bins + q[:, 2]
    u, c = np.unique(key, return_counts=True)
    out = []
    for i in np.argsort(-c)[:k]:
        col = px[key == u[i]].mean(0)
        # **分母要有下限**:直接除以自身均值时,近黑色会被归一化成一个**噪声方向**。
        # (注:这不是下面那条「指标骗人」的原因 —— 加下限后波波拉的数字没变。)
        out.append((col / max(col.mean(), 16.0), c[i] / len(px)))
    return out


def dropped_by_bg(ours_px, game_px, bg, share=0.25, near=50.0, present=0.02, bins=8):
    """实机侧抠图是不是把「颜色贴近背景」的一大块整段丢了 → 该只不可比。

    判据:我们渲图里某个主色簇占 ≥ `share`,它的颜色距实机背景色 < `near`,
    而在实机选区里占比 < `present`。三条同时成立才判。

    **为什么需要它**:红绒十字那张的背景是**橙色卡片**,而它的红腿距背景只有 39 ——
    抠图把整条红腿丢了,于是实机侧只剩乳白上身(选区均色 (223,204,184)、亮度 p25/p75
    只有 190/223 这么窄)。后果是调色板距离虚高到 0.239、`对比比` 直接飙到 **12.20**,
    而实际上我们渲的颜色和实机很接近。

    **填充率(选区面积 ÷ bbox 面积)不能当判据** —— 量过:红绒十字 0.357,
    而健康的岚鸟 0.263、雪影娃娃 0.354,完全分不开。要用「丢掉的那块是否贴近背景色」。
    实测这条能干净分开:红绒十字 43.1% 的红距背景 39、实机里 0.0%(判失败);
    波波拉最大簇距 56、实机里 21.3%,岚鸟三簇都在(都判通过)。
    """
    q = (ours_px // (256 // bins)).astype(int)
    key = q[:, 0] * bins * bins + q[:, 1] * bins + q[:, 2]
    gq = (game_px // (256 // bins)).astype(int)
    gkey = gq[:, 0] * bins * bins + gq[:, 1] * bins + gq[:, 2]
    u, c = np.unique(key, return_counts=True)
    for i in np.argsort(-c)[:4]:
        if c[i] / len(ours_px) < share:
            continue
        col = ours_px[key == u[i]].mean(0)
        if np.linalg.norm(col - bg) >= near:
            continue
        if (gkey == u[i]).mean() < present:
            return float(np.linalg.norm(col - bg))
    return None


def background(path: Path, shrink=4):
    """四角的中位色 —— 与 `gamemask.game_mask` 用的同一套。"""
    im = Image.open(path).convert("RGB")
    im = im.resize((im.width // shrink, im.height // shrink), Image.LANCZOS)
    a = np.asarray(im).astype(float)
    h, w = a.shape[:2]
    return np.median(np.concatenate([
        a[:h // 8, :w // 8].reshape(-1, 3), a[:h // 8, -w // 8:].reshape(-1, 3),
        a[-h // 8:, :w // 8].reshape(-1, 3), a[-h // 8:, -w // 8:].reshape(-1, 3)]), axis=0)


def palette_dist(pa, pb):
    """加权最近邻的对称距离 —— 对构图稳健(见模块头第 3 条)。"""
    def one(x, y):
        return sum(w * min(float(np.abs(c - c2).sum()) for c2, _ in y) for c, w in x)
    return 0.5 * (one(pa, pb) + one(pb, pa))


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
        # **缓存必须比二进制新**,否则改了 shader 跑出来的还是旧图 —— 踩过一次:
        # 改星贴层前后两轮 17 只的数字**逐行完全相同**,才发现比的是同一批渲图。
        if not png.exists() or png.stat().st_mtime < BIN.stat().st_mtime:
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
        if (d := dropped_by_bg(A[inner], a[gin], background(shot))) is not None:
            print(f"  {name}: 实机侧抠图丢了一大块(色距背景仅 {d:.0f}),不可比,跳过")
            continue
        ma, mb = np.median(A[inner], axis=0), np.median(a[gin], axis=0)
        la, lb = A[inner].mean(1), a[gin].mean(1)
        iqr = lambda x: (np.percentile(x, 75) - np.percentile(x, 25)) / np.median(x)
        rows.append((name, ma.mean() / mb.mean(),
                     palette_dist(palette(A[inner]), palette(a[gin])),
                     (np.median(A[ring].mean(1)) / np.median(la))
                     / (np.median(a[gring].mean(1)) / np.median(lb)),
                     iqr(la) / iqr(lb), asset,
                     (A[inner].mean(1) < 26).mean(), (a[gin].mean(1) < 26).mean()))
    rows.sort(key=lambda r: -r[2])
    print(f'{"形态":12} {"亮度比":>7} {"调色板":>7} {"描边比":>7} {"对比比":>7} {"很暗(我们/实机)":>16}  asset')
    for r in rows:
        print(f"{r[0]:12} {r[1]:7.2f} {r[2]:7.3f} {r[3]:7.2f} {r[4]:7.2f} "
              f"{r[6]:8.3f}/{r[7]:<7.3f}  {r[5]}")
    if rows:
        med = lambda i: np.median([r[i] for r in rows])
        print(f"\n中位: 亮度 {med(1):.2f}  调色板 {med(2):.3f}  描边 {med(3):.2f}  对比 {med(4):.2f}")
        print("比值类目标都是 1.00,调色板距离目标 0.00。"
              "亮度中位偏低是**实机的场景雾**,不要去追,见 docs/design.md §1.1")


main()
