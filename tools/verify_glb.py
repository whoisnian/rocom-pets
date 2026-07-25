#!/usr/bin/env python3
"""渲染宠物包里的 glb,肉眼验证网格/骨架/蒙皮/动画/贴图是否都对。

导出器(exporter/)把网格 glb 与 AnimSequence 的关键帧合并成一个 glb,合并要求动画的
坐标转换与网格完全一致(见 exporter/GlbBuilder.cs)。一旦上游 CUE4Parse 改了转换约定,
动画就会与网格错开——所以这里不用第三方查看器,而是自己按 glTF 规范采样+蒙皮+光栅化,
既能出对比图,也能当回归检查。纯 numpy/PIL,不引渲染依赖。

用法:
    uv run --with numpy --with pillow python tools/verify_glb.py <包目录> [-o 输出.png]
    python tools/verify_glb.py packs/喵喵 --form Gra_MiaoMiao1_001 --clips Idle,Walk,Happy
"""

import argparse
import json
import re
import struct
import sys
from pathlib import Path

import numpy as np
from PIL import Image

# glTF 组件类型 → numpy dtype
DTYPES = {5120: "<i1", 5121: "<u1", 5122: "<i2", 5123: "<u2", 5125: "<u4", 5126: "<f4"}
COMPONENTS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


class Glb:
    """够用的 glTF 读取:accessor、节点树、skin、animation 采样。"""

    def __init__(self, path: Path):
        blob = path.read_bytes()
        magic, _version, total = struct.unpack_from("<III", blob, 0)
        if magic != 0x46546C67:
            raise ValueError(f"{path} 不是 glb")
        offset, chunks = 12, {}
        while offset < total:
            length, kind = struct.unpack_from("<II", blob, offset)
            offset += 8
            chunks[kind] = blob[offset : offset + length]
            offset += length
        self.json = json.loads(chunks[0x4E4F534A].decode("utf8"))
        self.bin = chunks[0x004E4942]

    def accessor(self, index: int) -> np.ndarray:
        acc = self.json["accessors"][index]
        count, comps = acc["count"], COMPONENTS[acc["type"]]
        dtype = np.dtype(DTYPES[acc["componentType"]])
        view = self.json["bufferViews"][acc["bufferView"]]
        start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
        stride = view.get("byteStride") or dtype.itemsize * comps
        if stride == dtype.itemsize * comps:
            flat = np.frombuffer(self.bin, dtype=dtype, count=count * comps, offset=start)
            data = flat.reshape(count, comps)
        else:  # 交错存放:按 stride 逐条抠出来
            raw = np.frombuffer(self.bin, dtype=np.uint8, count=stride * count, offset=start)
            data = np.stack([
                np.frombuffer(raw[i * stride : i * stride + dtype.itemsize * comps].tobytes(), dtype=dtype)
                for i in range(count)
            ])
        out = data.astype(np.float64)
        if acc.get("normalized"):
            info = np.iinfo(dtype)
            out = out / info.max if info.min == 0 else np.maximum(out / info.max, -1.0)
        return out


def trs_matrix(t, r, s) -> np.ndarray:
    x, y, z, w = r
    rot = np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])
    m = np.eye(4)
    m[:3, :3] = rot * np.asarray(s)  # 列缩放
    m[:3, 3] = t
    return m


def node_local(node: dict) -> tuple:
    """节点的 TRS(matrix 形式也拆回 TRS 之外的路径不需要,glb 里骨骼都是 TRS)。"""
    if "matrix" in node:
        m = np.array(node["matrix"], dtype=np.float64).reshape(4, 4).T
        return None, m
    t = np.array(node.get("translation", [0, 0, 0]), dtype=np.float64)
    r = np.array(node.get("rotation", [0, 0, 0, 1]), dtype=np.float64)
    s = np.array(node.get("scale", [1, 1, 1]), dtype=np.float64)
    return (t, r, s), None


def slerp(a: np.ndarray, b: np.ndarray, f: float) -> np.ndarray:
    if np.dot(a, b) < 0:
        b = -b
    dot = float(np.clip(np.dot(a, b), -1.0, 1.0))
    if dot > 0.9995:
        out = a + f * (b - a)
    else:
        theta = np.arccos(dot) * f
        perp = b - a * dot
        perp /= max(np.linalg.norm(perp), 1e-12)
        out = a * np.cos(theta) + perp * np.sin(theta)
    return out / max(np.linalg.norm(out), 1e-12)


def sample(glb: Glb, animation: dict, time: float) -> dict:
    """采样一个 animation 在 `time` 处的节点 TRS 覆盖值。"""
    out: dict[int, dict] = {}
    for channel in animation["channels"]:
        target = channel["target"]
        node, path = target.get("node"), target["path"]
        if node is None or path == "weights":
            continue
        sampler = animation["samplers"][channel["sampler"]]
        times = glb.accessor(sampler["input"])[:, 0]
        values = glb.accessor(sampler["output"])
        interp = sampler.get("interpolation", "LINEAR")

        if len(times) == 1 or time <= times[0]:
            value = values[0]
        elif time >= times[-1]:
            value = values[-1]
        else:
            i = int(np.searchsorted(times, time) - 1)
            span = times[i + 1] - times[i]
            f = 0.0 if span <= 0 else float((time - times[i]) / span)
            if interp == "STEP":
                value = values[i]
            elif path == "rotation":
                value = slerp(values[i], values[i + 1], f)
            else:
                value = values[i] * (1 - f) + values[i + 1] * f
        out.setdefault(node, {})[path] = value
    return out


def globals_of(glb: Glb, overrides: dict) -> np.ndarray:
    """所有节点的世界变换(按场景树自顶向下)。"""
    nodes = glb.json["nodes"]
    result = [None] * len(nodes)

    def visit(index: int, parent: np.ndarray):
        node = nodes[index]
        trs, matrix = node_local(node)
        if matrix is not None:
            local = matrix
        else:
            t, r, s = trs
            over = overrides.get(index, {})
            t = over.get("translation", t)
            r = over.get("rotation", r)
            s = over.get("scale", s)
            local = trs_matrix(t, r, s)
        world = parent @ local
        result[index] = world
        for child in node.get("children", []):
            visit(child, world)

    roots = glb.json["scenes"][glb.json.get("scene", 0)]["nodes"]
    for root in roots:
        visit(root, np.eye(4))
    # 不在场景树里的节点(理论上不该有)兜个单位矩阵
    return np.array([m if m is not None else np.eye(4) for m in result])


def skinned_positions(glb: Glb, primitive: dict, skin: dict, world: np.ndarray) -> np.ndarray:
    pos = glb.accessor(primitive["attributes"]["POSITION"])
    joints = glb.accessor(primitive["attributes"]["JOINTS_0"]).astype(int)
    weights = glb.accessor(primitive["attributes"]["WEIGHTS_0"])
    ibm = glb.accessor(skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1)
    joint_nodes = skin["joints"]

    # 蒙皮矩阵 = 关节世界变换 @ 逆绑定矩阵(glTF 规范;跳过网格节点自身变换)
    skin_mats = np.array([world[joint_nodes[j]] @ ibm[j] for j in range(len(joint_nodes))])
    homo = np.concatenate([pos, np.ones((len(pos), 1))], axis=1)
    out = np.zeros((len(pos), 3))
    for slot in range(joints.shape[1]):
        w = weights[:, slot]
        active = w > 0
        if not active.any():
            continue
        mats = skin_mats[joints[active, slot]]
        transformed = np.einsum("nij,nj->ni", mats, homo[active])[:, :3]
        out[active] += transformed * w[active, None]
    return out


def rasterize(prims: list, size: int = 320, view: str = "iso") -> Image.Image:
    """正交投影 + z-buffer + 贴图采样;prims = [(positions, uv, indices, texture)]"""
    allpos = np.concatenate([p[0] for p in prims])
    if view == "front":  # glTF: +Y 上,-Z 前
        axes = lambda p: (p[:, 0], p[:, 1], -p[:, 2])
    elif view == "side":
        axes = lambda p: (p[:, 2], p[:, 1], p[:, 0])
    else:
        c = np.sqrt(0.5)
        axes = lambda p: (c * (p[:, 0] + p[:, 2]), p[:, 1], c * (p[:, 2] - p[:, 0]))
    u_all, v_all, _ = axes(allpos)
    cx, cy = (u_all.min() + u_all.max()) / 2, (v_all.min() + v_all.max()) / 2
    extent = max(u_all.max() - u_all.min(), v_all.max() - v_all.min(), 1e-6)
    scale = 0.85 * size / extent

    img = np.ones((size, size, 3))
    zbuf = np.full((size, size), -1e30)
    for pos, uv, indices, tex in prims:
        u, v, depth = axes(pos)
        sx = (u - cx) * scale + size / 2
        sy = size / 2 - (v - cy) * scale
        th, tw = tex.shape[:2]
        for tri in indices:
            xs, ys, zs = sx[tri], sy[tri], depth[tri]
            x0, x1 = int(max(0, np.floor(xs.min()))), int(min(size - 1, np.ceil(xs.max())))
            y0, y1 = int(max(0, np.floor(ys.min()))), int(min(size - 1, np.ceil(ys.max())))
            if x1 < x0 or y1 < y0:
                continue
            ax, ay = xs[0], ys[0]
            e1, e2 = (xs[1] - ax, ys[1] - ay), (xs[2] - ax, ys[2] - ay)
            den = e1[0] * e2[1] - e1[1] * e2[0]
            if abs(den) < 1e-9:
                continue
            gx, gy = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
            px, py = gx - ax, gy - ay
            b1 = (px * e2[1] - py * e2[0]) / den
            b2 = (py * e1[0] - px * e1[1]) / den
            b0 = 1 - b1 - b2
            inside = (b0 >= 0) & (b1 >= 0) & (b2 >= 0)
            if not inside.any():
                continue
            z = b0 * zs[0] + b1 * zs[1] + b2 * zs[2]
            normal = np.cross(pos[tri[1]] - pos[tri[0]], pos[tri[2]] - pos[tri[0]])
            norm = np.linalg.norm(normal)
            lam = 0.55 + 0.45 * abs(np.dot(normal / norm, [0.3, 0.5, 0.8])) if norm > 0 else 1.0
            uu = b0 * uv[tri[0], 0] + b1 * uv[tri[1], 0] + b2 * uv[tri[2], 0]
            vv = b0 * uv[tri[0], 1] + b1 * uv[tri[1], 1] + b2 * uv[tri[2], 1]
            tx = np.clip((uu % 1.0 * tw).astype(int), 0, tw - 1)
            ty = np.clip((vv % 1.0 * th).astype(int), 0, th - 1)
            texel = tex[ty, tx]
            ok = inside & (z > zbuf[y0 : y1 + 1, x0 : x1 + 1]) & (texel[:, :, 3] > 0.3)
            region = img[y0 : y1 + 1, x0 : x1 + 1]
            region[ok] = (texel[:, :, :3] * lam)[ok]
            zbuf[y0 : y1 + 1, x0 : x1 + 1][ok] = z[ok]
    return Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8))


def load_textures(glb: Glb, tex_dir: Path) -> list:
    """材质 index → 贴图数组。材质名后缀(By/Es/Mh)对应贴图 `T_*_<槽>_D`(见 design §1)。"""
    fallback = np.ones((4, 4, 4))
    out = []
    for material in glb.json.get("materials", []):
        name = material.get("name", "")
        slot = name.rsplit("_", 1)[-1] if "_" in name else ""
        found = None
        for candidate in sorted(tex_dir.glob("*.png")):
            parts = candidate.stem.rsplit("_", 2)
            if len(parts) == 3 and parts[1].lower() == slot.lower() and parts[2] == "D":
                found = candidate
                break
        if found is None:
            out.append(fallback)
            continue
        arr = np.asarray(Image.open(found).convert("RGBA"), dtype=np.float64) / 255.0
        out.append(arr)
    return out or [fallback]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pack", type=Path, help="包目录(含 manifest.toml)或直接给 glb")
    parser.add_argument("--form", help="形态资产名,默认第一个")
    parser.add_argument("--clips", default="Idle,Walk,Happy,SleepLoop", help="逗号分隔的动作名")
    parser.add_argument("--at", type=float, default=0.4, help="采样时刻占动作时长的比例")
    parser.add_argument("--view", default="iso", choices=["iso", "front", "side"])
    parser.add_argument("-o", "--out", type=Path, default=Path("verify.png"))
    args = parser.parse_args()

    if args.pack.suffix == ".glb":
        glb_path = args.pack
    else:
        forms = sorted((args.pack / "forms").iterdir())
        if args.form:
            forms = [f for f in forms if f.name == args.form]
        if not forms:
            print(f"在 {args.pack} 里找不到形态", file=sys.stderr)
            return 1
        glb_path = forms[0] / "model.glb"
    tex_dir = glb_path.parent / "tex"

    glb = Glb(glb_path)
    textures = load_textures(glb, tex_dir)
    animations = {a.get("name", f"#{i}"): a for i, a in enumerate(glb.json.get("animations", []))}
    print(f"{glb_path}: {len(glb.json['nodes'])} 节点, {len(animations)} 段动画")
    print("  动画:", ", ".join(animations))

    mesh_nodes = [n for n in glb.json["nodes"] if "mesh" in n and "skin" in n]
    if not mesh_nodes:
        print("没有带蒙皮的网格节点", file=sys.stderr)
        return 1
    mesh = glb.json["meshes"][mesh_nodes[0]["mesh"]]
    skin = glb.json["skins"][mesh_nodes[0]["skin"]]

    tiles = []
    for clip in args.clips.split(","):
        clip = clip.strip()
        if clip not in animations:
            print(f"  跳过 {clip}(glb 里没有)")
            continue
        animation = animations[clip]
        duration = max(
            float(glb.accessor(s["input"])[:, 0].max()) for s in animation["samplers"]
        )
        time = duration * args.at
        world = globals_of(glb, sample(glb, animation, time))
        prims = []
        for primitive in mesh["primitives"]:
            pos = skinned_positions(glb, primitive, skin, world)
            uv = glb.accessor(primitive["attributes"]["TEXCOORD_0"])
            indices = glb.accessor(primitive["indices"]).astype(int).reshape(-1, 3)
            tex = textures[primitive.get("material", 0) % len(textures)]
            prims.append((pos, uv, indices, tex))
        tiles.append((f"{clip} @{time:.2f}s/{duration:.2f}s", rasterize(prims, view=args.view)))
        low, high = np.concatenate([p[0] for p in prims]).min(0), np.concatenate([p[0] for p in prims]).max(0)
        print(f"  {clip}: {duration:.2f}s, 采样 {time:.2f}s, 包围盒 {low.round(2)} .. {high.round(2)}")

    if not tiles:
        print("没有可渲染的动作", file=sys.stderr)
        return 1
    sheet = Image.new("RGB", (sum(t[1].width for t in tiles), tiles[0][1].height), "white")
    x = 0
    for _, tile in tiles:
        sheet.paste(tile, (x, 0))
        x += tile.width
    sheet.save(args.out)
    print(f"写出 {args.out} {sheet.size}:{[t[0] for t in tiles]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
