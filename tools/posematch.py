"""扫 `--at` 与 `--yaw`,挑出**轮廓最像实机截图**的那一帧。

    uv run --with numpy --with pillow python tools/posematch.py 春兔 0,15,25,35,340,350

为什么要它:并排比对一直是「我们随手取 `--at 0.4`」对上「实机某个未知姿势」。
姿势一差,按屏幕框取样就取到不同部位 —— **逐像素的半透明度比较会被这一条直接毁掉**:
春兔耳朵那次,同一个框在实机侧落在耳膜上、在我们这侧落在耳后的背景上,
于是「我们比实机透 3.6 倍」这个结论整个不成立(换姿势后重测差距小得多)。

判据:两边各按宠物包围盒归一化到同一张 256×256 的二值轮廓,取 IoU 最大的一帧。
实测春兔:`yaw 25 / at 0.4`(以前随手取的)IoU 只有 0.66,扫过之后
`yaw 35 / at 0.55` 能到 **0.74**。

**两个已知的坑**:
① 实机截图常常是**裁过的**(春兔那张脚被裁掉了),包围盒归一化会把两边竖向拉伸得不一样
   —— 要比头/耳朵这类局部,按**宽度**锚定比按包围盒锚定可靠;
② IoU 到不了 1:实机那一帧的动作相位在我们的 Idle 里不一定存在,而且实机有抗锯齿。
   0.7 上下就是当前能做到的最好,别当成「姿势对上了」。
"""
import os, re, subprocess, sys
from pathlib import Path
import numpy as np
from PIL import Image
sys.path.insert(0, "tools")
from gamemask import game_mask

SHOTS = Path.home()/"Downloads/rocom/screenshot-pets"
PACKS = Path(os.environ.get("CMP_PACKS", Path.home()/"Downloads/rocom/packs-back"))
BIN = Path("target/release/rocom-pets").resolve()
OUT = Path("/tmp/posematch"); OUT.mkdir(exist_ok=True)


def pick(name):
    for pat in (rf'^name = "{re.escape(name)}"$', rf'^name = "[^"]*\({re.escape(name)}\)"$'):
        best = None
        for man in sorted(PACKS.glob("*/manifest.toml")):
            m = re.search(pat + r'\n(?:.*\n)*?^asset = "([^"]+)"$', man.read_text(), re.M)
            if m:
                k = (1 if "Ar_" in m.group(1) else 0, m.group(1))
                if best is None or k < best[0]:
                    best = (k, man.parent, m.group(1))
        if best:
            return best[1], best[2]
    return None, None


def norm(mask):
    ys, xs = np.where(mask)
    box = mask[ys.min():ys.max()+1, xs.min():xs.max()+1]
    return np.array(Image.fromarray((box*255).astype(np.uint8)).resize((256, 256))) > 127


name = sys.argv[1]
ref = SHOTS/"widnows"/f"{name}.png"
if not ref.exists():
    ref = SHOTS/f"{name}.png"
_, sel = game_mask(str(ref))
want = norm(sel)
pack, asset = pick(name)
best = None
for yaw in (sys.argv[2].split(",") if len(sys.argv) > 2 else ["25"]):
    for i in range(21):
        at = i / 20
        png = OUT/f"{name}_{yaw}_{at:.2f}.png"
        if not png.exists() or png.stat().st_mtime < BIN.stat().st_mtime:
            subprocess.run([str(BIN), "--render", str(pack), "--form", asset, "--clips", "Idle",
                            "--yaw", yaw, "--size", "600", "--at", f"{at:.2f}", "--time", "0.7",
                            "--no-fade", "-o", str(png)], capture_output=True)
        if not png.exists():
            continue
        m = np.array(Image.open(png).convert("RGBA"))[..., 3] > 100
        if m.sum() < 100:
            continue
        got = norm(m)
        iou = (got & want).sum() / (got | want).sum()
        if best is None or iou > best[0]:
            best = (iou, yaw, at, png)
        print(f"  yaw={yaw} at={at:.2f}  IoU {iou:.4f}")
print(f"\n最佳: yaw={best[1]} at={best[2]:.2f}  IoU {best[0]:.4f}  → {best[3]}")
