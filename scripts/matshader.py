#!/usr/bin/env python3
"""把一个**材质**对上它的 shader:从 archive 里挑出该材质的 pixel/vertex shader 并导出 DXBC。

这是 docs/shader.md 里流水线的第 ② 步(认归属)。archive 里没有任何材质名,所以只能反向比对:
archive 的每条 `ShaderMapHashes` 是 20 字节 SHA1,而材质的 cooked resource 段里存着它用到的
shader map 的同一串哈希 —— 逐条 memmem 材质的 `.uexp`,命中的就是它的 shader map。
(CUE4Parse 不解那个段,所以材质必须用 `unpack.sh --raw` 导成原始字节。)

    ./scripts/unpack.sh --out <dir> --raw --no-exclude --no-post \\
        --filter "NRC/Content/ArtRes/AnimSequence/Pets/<资产>/Mat"
    uv run python scripts/matshader.py <archive> <dir>/…/MI_<材质>.uexp --out /tmp/s

一个材质会命中几十个 shader map(同一材质的不同排列:光照/雾/lightmap 组合),
去重后仍有上百条 shader。**要读公式就挑对排列**:按 uniform buffer 组合分组
(`--groups` 看分布),宠物在世界里跑的是带 `MobileDirectionalLight`、不带
`ClusteredForwardShading` 那组;同组里取解压后最大的那条,指令最全。
"""

from __future__ import annotations

import argparse
import os
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from shaderdump import ShaderArchive  # noqa: E402

# 宠物在世界里实际跑的那组:移动端 base pass + 平行光,没有聚簇前向着色
WORLD_BASE_PASS = (
    "View",
    "MobileBasePass",
    "MobileDirectionalLight",
    "Primitive",
    "MaterialCollection0",
    "MaterialCollection1",
    "Material",
)


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("archive", type=Path)
    ap.add_argument("uexp", type=Path, help="材质的 .uexp(必须是 unpack.sh --raw 导出的)")
    ap.add_argument("--freq", type=int, default=3, help="3=pixel(默认)、0=vertex")
    ap.add_argument("--groups", action="store_true", help="只按 uniform buffer 组合列出分布")
    ap.add_argument("--any-group", action="store_true", help="不限于世界 base pass 那组")
    ap.add_argument("--out", type=Path, help="导出目录(取该组最大的几条)")
    ap.add_argument("--limit", type=int, default=3, help="配合 --out")
    args = ap.parse_args()

    arc = ShaderArchive(args.archive)
    blob = args.uexp.read_bytes()
    hits = [i for i in range(arc.n_maps) if bytes.fromhex(arc.map_hash(i)) in blob]
    print(f"{args.uexp.name}:{len(blob)} 字节;命中 {len(hits)} / {arc.n_maps} 个 shader map")
    if not hits:
        raise SystemExit(
            "一条都没命中。材质是不是没用 --raw 导?(属性 JSON 里没有那段哈希)"
        )

    # 同一条 shader 会被多个 map 共用,按 shader 哈希去重
    seen: set[str] = set()
    rows: list[tuple[int, int, tuple[str, ...]]] = []
    for m in hits:
        for s in arc.shaders_of_map(m):
            h = arc.shader_hash(s)
            if h in seen:
                continue
            seen.add(h)
            if arc.entry(s)[3] == args.freq:
                rows.append((s, arc.entry(s)[2], tuple(arc.uniform_buffers(s))))
    print(f"freq={args.freq} 去重后 {len(rows)} 条")

    if args.groups:
        for ubs, n in Counter(r[2] for r in rows).most_common():
            sizes = sorted(r[1] for r in rows if r[2] == ubs)
            mark = "  ← 世界 base pass" if ubs == WORLD_BASE_PASS else ""
            print(f"  {n:4d} 条  {sizes[0] // 1024:3d}K~{sizes[-1] // 1024:3d}K  {ubs}{mark}")
        return

    picked = rows if args.any_group else [r for r in rows if r[2] == WORLD_BASE_PASS]
    if not picked:
        raise SystemExit("世界 base pass 那组是空的;用 --groups 看看有哪些组、或加 --any-group")
    picked.sort(key=lambda r: -r[1])
    print(
        "按解压后大小降序:"
        + ", ".join(f"{s}({sz // 1024}K)" for s, sz, _ in picked[: args.limit + 5])
    )

    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
        for s, _, _ in picked[: args.limit]:
            p = args.out / f"{s}.dxbc"
            p.write_bytes(arc.dxbc(s))
            print("  写出", p)
        print(f"反汇编: wine ./dxbcdis.exe {args.out}/*.dxbc")


if __name__ == "__main__":
    main()
