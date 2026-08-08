#!/usr/bin/env python3
"""生成下载站的目录数据 public/catalog.json,以及头像精灵图 public/sprite.webp。

两种数据源,按需选一个:

  --packs DIR   真实模式。扫目录下的 *.rkpet,算 size/sha256,读包内 manifest.toml
                取形态构成。出来的目录和你要上传到 R2 的那批文件逐字对应。

  --index PATH  演示模式。读 docs/petindex.md 的「## 清单」一节,只有包名与形态名,
                size 填 0、sha256 留空。用来在没有 1.6GB 包的机器上把页面跑起来,
                前端会把这类条目标成「待上传」。

头像**自己从解包数据拼**(--parsed,默认 $ROCOM_PARSED 或 ~/Downloads/rocom/parsed):
游戏自带 `Icon/HeadIcon/<conf_id>.png`,128px 一张,按 conf_id 命名 —— 而 manifest 里
`[species].id` 与 `[[forms]].id` 就是 conf_id,所以**按 id 直接对上**,不必按中文名查表
(同名不同图鉴号的宠物按名字只能给一个头像)。用得上的那些拼成一张 webp,其余不进图。

取不到解包数据(或某只没有图标)就记 null,前端退回「首字 + 按名字哈希出来的底色」。

用法:
  scripts/gen_catalog.py --packs ~/Downloads/rocom/packs-all \
                         --apps dist-bin --version 0.1.0
  scripts/gen_catalog.py --index ../docs/petindex.md          # 无包时的演示目录

要 Pillow:`uv run --with pillow python scripts/gen_catalog.py …`(npm run catalog 已经带上)。
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import math
import os
import re
import sys
import tomllib
import zipfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
WEB = HERE.parent
REPO = WEB.parent

# 解包树里的两个位置(rocom-capture 的 scripts/unpack.sh 产物)
HEAD_ICONS = Path("NRC/Content/NewRoco/Modules/System/Common/Icon/HeadIcon")
BIN_DIR = Path("NRC/Content/ScriptC/Data/Bin/BinDataCompressed")

#: 精灵图一格的边长。前端最大按 56px 显示,128 正好够 2 倍屏。
CELL = 128

# 应用本体的文件名 → (id, 平台, 展示名)。认不出来的文件会被跳过并打一行警告。
APP_PATTERNS: list[tuple[re.Pattern[str], str, str, str]] = [
    (re.compile(r"\.exe$", re.I), "app-windows-x64", "windows", "Windows 10+ (x64)"),
    (re.compile(r"\.(AppImage|tar\.(gz|xz|zst))$", re.I), "app-linux-x64", "linux", "Linux (x64)"),
]


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        # 1.4GB / 200 个文件,一次读 4MB 比默认的 8KB 快一个量级
        for chunk in iter(lambda: f.read(4 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------- 头像精灵图


def head_icons(parsed: Path) -> dict[int, Path]:
    """conf_id → 头像 png。缺解包数据就是空表(全站退回文字头像,不算错)。

    目录里还有 `3001_2.png` 这种带后缀的(同一只的备用图),**不收** ——
    manifest 给的是纯 conf_id,收进来也对不上。
    """
    d = parsed / HEAD_ICONS
    if not d.is_dir():
        print(f"  [跳过] 找不到 {d} —— 全站用文字头像;要头像就 --parsed 指到解包根")
        return {}
    icons = {int(p.stem): p for p in d.glob("*.png") if p.stem.isdigit()}
    print(f"  解包数据里有 {len(icons)} 张头像({d})")
    return icons


def build_sprite(icons: dict[int, Path], wanted: list[int], out_dir: Path) -> tuple[dict[int, int], dict]:
    """把用得上的头像拼成一张 webp,返回 conf_id → 序号 与几何参数。

    **只拼用得上的**:解包里有 819 张,而这批包只用到六百来张;多拼的每一张都是
    白让访客下载的字节。行主序,列数取接近正方形的那个 —— 单边太长会撞上
    浏览器的贴图上限,而且方图压得更小。
    """
    have = [i for i in wanted if i in icons]
    dst = out_dir / "sprite.webp"
    if not have:
        dst.unlink(missing_ok=True)
        return {}, {"cols": 1, "cell": CELL, "count": 0}

    from PIL import Image

    cols = max(1, math.ceil(math.sqrt(len(have))))
    rows_n = math.ceil(len(have) / cols)
    sheet = Image.new("RGBA", (cols * CELL, rows_n * CELL), (0, 0, 0, 0))
    index: dict[int, int] = {}
    for n, conf_id in enumerate(have):
        im = Image.open(icons[conf_id]).convert("RGBA")
        if im.size != (CELL, CELL):
            im = im.resize((CELL, CELL), Image.LANCZOS)
        sheet.paste(im, ((n % cols) * CELL, (n // cols) * CELL))
        index[conf_id] = n
    # 头像是硬边卡通色块,`method=6` 多花几秒换几个百分点,值
    sheet.save(dst, "WEBP", quality=88, method=6)
    print(f"  头像精灵图 {dst} — {len(have)} 张 / {cols}×{rows_n} 格 / "
          f"{dst.stat().st_size / 1e6:.1f}MB")
    return index, {"cols": cols, "cell": CELL, "count": len(have)}


def names_to_ids(parsed: Path) -> dict[str, int]:
    """中文名 → conf_id,只给**演示模式**用(那条路只有名字)。

    同名多条取 id 最小的:同名不同图鉴号的宠物按名字本来就分不开,取哪个都是近似,
    取最小的至少稳定。真实模式不走这里 —— manifest 里直接有 id。
    """
    f = parsed / BIN_DIR / "PETBASE_CONF.json"
    if not f.is_file():
        return {}
    rows = json.loads(f.read_text("utf-8")).get("RocoDataRows", {})
    out: dict[str, int] = {}
    for row in rows.values():
        name, pid = row.get("name"), row.get("id")
        if name and isinstance(pid, int) and pid < out.get(name, 1 << 30):
            out[name] = pid
    return out


# ---------------------------------------------------------------- 真实模式


def stage_of(form: dict) -> int:
    """王者形态排最后。资产名以 Bo_001 结尾的是王者,配置里的 stage 不可靠
    (它是相对本条链的,见 docs/petindex.md 规则 4)。"""
    if re.search(r"Bo_\d+$", form.get("asset", "")):
        return 99
    return int(form.get("stage", 1) or 1)


def read_pack(path: Path) -> dict:
    """`sprite` 这一步先填 **conf_id**;等收齐全部 id 拼完图再换成格子序号。"""
    stem = path.stem  # 002-喵喵
    book, _, name = stem.partition("-")
    with zipfile.ZipFile(path) as z:
        manifest = tomllib.loads(z.read("manifest.toml").decode("utf-8"))

    # 同名多条 = 同一形态的多种外观(晶石蜗x6),合成一条并记 skins
    grouped: dict[str, dict] = {}
    for raw in manifest.get("forms", []):
        fname = raw["name"]
        if fname in grouped:
            grouped[fname]["skins"] += 1
            continue
        grouped[fname] = {
            "name": fname,
            "asset": raw.get("asset", ""),
            "stage": stage_of(raw),
            "skins": 1,
            "sprite": raw.get("id"),
        }
    forms = sorted(grouped.values(), key=lambda f: (f["stage"], f["name"]))

    return {
        "id": stem,
        "book": book if book.isdigit() else "000",
        "name": name or stem,
        "key": f"packs/{path.name}",
        "size": path.stat().st_size,
        "sha256": sha256_of(path),
        "forms": forms,
        "sprite": manifest.get("species", {}).get("id"),
        "_source_version": manifest.get("source_version", ""),
    }


def scan_packs(packs_dir: Path) -> tuple[list[dict], str]:
    files = sorted(packs_dir.glob("*.rkpet"))
    if not files:
        sys.exit(f"{packs_dir} 里没有 .rkpet —— 先用导出器出包:\n"
                 f"  dotnet run --project exporter -- --all --zip-only --out packs")
    out: list[dict] = []
    versions: collections.Counter[str] = collections.Counter()
    for i, f in enumerate(files, 1):
        pack = read_pack(f)
        versions[pack.pop("_source_version")] += 1
        out.append(pack)
        print(f"\r  [{i}/{len(files)}] {f.name}".ljust(60), end="", file=sys.stderr)
    print(file=sys.stderr)
    if len(versions) > 1:
        print(f"  [警告] 这批包来自 {len(versions)} 个不同的 source_version:"
              f"{dict(versions)} —— 混版本会让「宠物包内包含宠物」和实际对不上")
    return out, (versions.most_common(1)[0][0] if versions else "")


# ---------------------------------------------------------------- 演示模式

# 000-三叶草龙.rkpet:包含 三叶草龙/花叶龙 两种形态;
LINE = re.compile(r"^(?P<file>[^:]+\.rkpet):包含 (?P<forms>\S+) ")


def scan_index(index_md: Path, by_name: dict[str, int]) -> list[dict]:
    """同真实模式,`sprite` 先填 conf_id —— 只是这一路要拿名字去换。"""
    packs: list[dict] = []
    body = index_md.read_text("utf-8")
    body = body[body.index("## 清单"):]
    for line in body.splitlines():
        m = LINE.match(line.strip())
        if not m:
            continue
        stem = Path(m.group("file")).stem
        book, _, name = stem.partition("-")
        forms = []
        for i, token in enumerate(m.group("forms").split("/")):
            fname, _, n = token.partition("x")
            forms.append({
                "name": fname,
                "asset": "",
                "stage": i + 1,
                "skins": int(n) if n.isdigit() else 1,
                "sprite": by_name.get(fname),
            })
        packs.append({
            "id": stem,
            "book": book if book.isdigit() else "000",
            "name": name or stem,
            "key": f"packs/{stem}.rkpet",
            "size": 0,
            "sha256": "",
            "forms": forms,
            "sprite": by_name.get(name),
        })
    if not packs:
        sys.exit(f"{index_md} 里没解出条目 —— 「## 清单」一节的格式变了?")
    return packs


# ---------------------------------------------------------------- 应用本体


def scan_apps(apps_dir: Path, version: str) -> list[dict]:
    apps: list[dict] = []
    for f in sorted(apps_dir.iterdir()):
        if not f.is_file():
            continue
        for pattern, app_id, platform, label in APP_PATTERNS:
            if pattern.search(f.name):
                apps.append({
                    "id": app_id,
                    "platform": platform,
                    "label": label,
                    "key": f"app/{version}/{f.name}",
                    "filename": f.name,
                    "size": f.stat().st_size,
                    "sha256": sha256_of(f),
                    "version": version,
                })
                print(f"  应用本体 {f.name} ({f.stat().st_size / 1e6:.1f}MB)")
                break
        else:
            print(f"  [跳过] 认不出的文件 {f.name}")
    return apps


# ----------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--packs", type=Path, help="宠物包目录(*.rkpet)")
    src.add_argument("--index", type=Path, nargs="?", const=REPO / "docs/petindex.md",
                     help="演示模式:从 docs/petindex.md 的清单造目录")
    ap.add_argument("--apps", type=Path, help="应用本体所在目录(.exe / .AppImage / .tar.*)")
    ap.add_argument("--version", default="0.0.0", help="应用本体版本号,进 R2 key")
    ap.add_argument("--parsed", type=Path,
                    default=Path(os.environ.get("ROCOM_PARSED",
                                                Path.home() / "Downloads/rocom/parsed")),
                    help="rocom-capture 的解包根,取 Icon/HeadIcon 里的头像")
    ap.add_argument("--out", type=Path, default=WEB / "public",
                    help="输出目录,默认 web/public")
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)

    print("头像:")
    icons = head_icons(args.parsed)

    source_version = ""
    if args.packs:
        print("宠物包(算 sha256,1.6GB 大约要一分钟):")
        packs, source_version = scan_packs(args.packs)
    else:
        print(f"宠物包(演示模式,读 {args.index}):")
        packs = scan_index(args.index, names_to_ids(args.parsed))

    # 收齐用得上的 conf_id 再拼图,然后把各处的 id 换成格子序号。
    # **顺序按包列表出现的先后**:同一批包重跑出来的图逐字节一致,便于比对。
    wanted: list[int] = []
    seen: set[int] = set()
    for p in packs:
        for cid in [p["sprite"]] + [f["sprite"] for f in p["forms"]]:
            if isinstance(cid, int) and cid not in seen:
                seen.add(cid)
                wanted.append(cid)
    sprite_index, sprite_geom = build_sprite(icons, wanted, args.out)
    for p in packs:
        p["sprite"] = sprite_index.get(p["sprite"])
        for f in p["forms"]:
            f["sprite"] = sprite_index.get(f["sprite"])

    apps: list[dict] = []
    if args.apps:
        print("应用本体:")
        apps = scan_apps(args.apps, args.version)

    # 排序只影响没有统计数据时的兜底顺序 —— 页面默认按下载次数排,那份数据来自 D1。
    packs.sort(key=lambda p: (p["book"] == "000", p["book"], p["name"]))

    catalog = {
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "source_version": source_version,
        "sprite": {"url": "/sprite.webp", **sprite_geom},
        "apps": apps,
        "packs": packs,
    }
    out_file = args.out / "catalog.json"
    out_file.write_text(json.dumps(catalog, ensure_ascii=False, separators=(",", ":")), "utf-8")

    total_forms = sum(len(p["forms"]) for p in packs)
    with_avatar = sum(1 for p in packs if p["sprite"] is not None)
    forms_with = sum(1 for p in packs for f in p["forms"] if f["sprite"] is not None)
    total_bytes = sum(p["size"] for p in packs)
    print(f"\n{out_file} ({out_file.stat().st_size / 1024:.0f}KB)")
    print(f"  {len(packs)} 个包 / {total_forms} 个形态 / {total_bytes / 1e9:.2f}GB")
    why = "游戏里就没出图标" if icons else "这次没读到解包数据"
    print(f"  有头像:包 {with_avatar}/{len(packs)}、形态 {forms_with}/{total_forms};"
          f"剩下的{why},页面退回文字头像")
    if apps:
        print(f"  应用本体 {len(apps)} 个,版本 {args.version}")

    print("\n下一步:")
    print(f"  rclone copy {args.packs or '<包目录>'} r2:rocom-pets/packs/ --progress")
    if args.apps:
        print(f"  rclone copy {args.apps} r2:rocom-pets/app/{args.version}/ --progress")
    print("  npm run build && wrangler deploy")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
