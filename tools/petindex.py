#!/usr/bin/env python3
"""按图鉴号归并进化链,列出规范化之后的宠物包名称与形态构成。

导出器现在按**每条进化链**出一个包,名字取链首(`喵喵.rkpet`),重名时补链首 id
(`海盔虫.rkpet` / `海盔虫-3475.rkpet`)。那两个「海盔虫」其实是同一只宠物的两种外观,
分成两个包既难找也难维护 —— 这个脚本给的是归并之后的样子:

    076-海盔虫.rkpet:包含 海盔虫x2/刺盔虫x2/千棘盔x2/千棘海针 七种形态
                     其中 x2 = 本来的样子/磨损的样子

没有图鉴号的宠物(未实装居多)一律记 `000`,那一档靠资产名的词干分包。

数据都来自解包出来的 JSON(rocom-capture 的 scripts/unpack.sh 产物),四张表:

- `PET_EVOLUTION_CONF` —— **进化链的权威表**。`evolution_chain` 是普通形态(带 stage),
  `lordevo_chain` 是王者形态(那几个 5xxx id)。比顺着 `PETBASE_CONF.evolution_pet_id`
  自己爬可靠:那个字段在分支链上是**一串**(矿晶虫有 6 个),爬的时候只取第一个就会
  把另外五条外观整条丢掉。
- `PETBASE_CONF` —— 每个形态的 `pictorial_book_id`(图鉴号,包名前缀)、`stage`、`model_conf`。
- `MODEL_CONF` —— `model_conf` → 资产目录名(`Wat_HaiKuiChong1Ar_001`)。**判重就靠这个资产名**,
  不是 `model_conf` 自己:同一个王者形态在表里有三四行(千棘海针 4020/5012/8106,
  给图鉴、王者战斗等不同场合各配一行,`model_conf` 各不相同),但指的是同一份模型。
  换外观则是真换资产(`…1_001` vs `…1Ar_001`),所以 x6 那种数量不会被误并。
- `MEGAMAP_CONF` —— 外观标签。`genre` 写成「刺盔虫_本来的样子」,`icon` 就是 petbase_id。

用法:
    tools/petindex.py [--parsed 解包根] [--format text|tsv] [--check]
"""

import argparse
import json
import os
import re
import sys
from collections import OrderedDict
from pathlib import Path

BIN_DIR = Path("NRC/Content/ScriptC/Data/Bin/BinDataCompressed")

CN_NUM = "零一二三四五六七八九十"

# 补过「本来的样子」的形态(见 Form.fill_default_skin);出清单时一并报出来,别让它悄悄混进去
FILLED: list[str] = []
# 靠资产词干认回原包的形态(见 adopt_unbooked);同样要报,那是推断出来的,不是数据直说的
ADOPTED: list[str] = []


def cn_count(n: int) -> str:
    """1 → 「一」,11 → 「十一」,25 → 「二十五」。形态数最多也就二十来个,够用。"""
    if n == 2:
        return "两"  # 「两种形态」,不是「二种形态」;十二仍然念十二,所以只特判这一个
    if n <= 10:
        return CN_NUM[n]
    if n < 20:
        return "十" + CN_NUM[n - 10]
    if n < 100:
        tens, ones = divmod(n, 10)
        return CN_NUM[tens] + "十" + (CN_NUM[ones] if ones else "")
    return str(n)


def rows(parsed: Path, table: str) -> dict:
    path = parsed / BIN_DIR / f"{table}.json"
    if not path.is_file():
        sys.exit(
            f"缺配置表 {path}\n"
            "先在 rocom-capture 里跑 scripts/unpack.sh(会把 .bytes 解成 .json),"
            "再用 --parsed 指过来"
        )
    return json.loads(path.read_text(encoding="utf-8"))["RocoDataRows"]


def skin_labels(megamap: dict) -> dict:
    """petbase_id → 外观标签(「本来的样子」)。`genre` 里下划线前面那截是宠物名,丢掉。"""
    labels = {}
    for row in megamap.values():
        genre, icon = row.get("genre"), row.get("icon")
        if not (genre and icon and "_" in str(genre)):
            continue
        try:
            pid = int(icon)
        except (TypeError, ValueError):
            continue
        labels.setdefault(pid, str(genre).split("_", 1)[1])
    return labels


ASSET_RE = re.compile(r"^([A-Za-z]+_[A-Za-z]+?)(\d+|Bo)(Ar)?_(\d+)$")


def asset_stem(asset: str) -> str:
    """`Gra_RuoYeXi1_001` → `Gra_RuoYeXi`(一条链共用的那截)。

    资产名的构成是「元素_物种拼音 + 阶段 + 可选 Ar + _变体号」,阶段那位是 `Bo` 就是王者形态。
    没图鉴号的那批在 `evolution_pet_id` 里**一条边都没有**,靠这个词干才能把链重新拼起来
    (design.md §2 早就把「资源目录名数字后缀 = 阶段」列为交叉校验,这里是拿它当主依据)。
    """
    m = ASSET_RE.match(asset)
    return m.group(1) if m else asset


def asset_stage(asset: str) -> int | None:
    """资产名里的阶段位:`Gra_MiaoMiao2_001` → 2,`Gra_MiaoMiaoBo_001` → None(王者形态)。

    **排序优先用它,而不是配置里的 `stage` 字段**:那个字段是**相对本条链**的,
    半路起头的链会从 1 开始数(路路尼那条链只有它一个,写着 stage 1,可它明明是二阶),
    于是把散落的成员认回来之后,顺序就乱了。资产名里的数字是绝对的。
    """
    m = ASSET_RE.match(asset)
    if not m or m.group(2) == "Bo":
        return None
    return int(m.group(2))


def is_lord_asset(asset: str) -> bool:
    m = ASSET_RE.match(asset)
    return bool(m) and m.group(2) == "Bo"


def chain_label(chain_name: str) -> str | None:
    """「矿晶虫进化链(西瓜碧玺的样子)」→「西瓜碧玺的样子」。没括号就没有。

    **「…分支」不算外观**:果冻那三条链叫「果冻进化链(抹茶布丁分支)」,说的是分支去向、
    不是长相 —— 当外观标签用会写出「x3 表示有 抹茶布丁分支/…」这种莫名其妙的话。
    """
    left = chain_name.find("（")
    if left < 0 or not chain_name.rstrip().endswith("）"):
        return None
    label = chain_name[left + 1 : chain_name.rstrip().rfind("）")]
    return None if label.endswith("分支") else label or None


class Form:
    """归并之后的一个形态:一个名字 + 若干种外观(每种外观一个 model_conf)。"""

    def __init__(self, name: str, stage: int, lord: bool):
        self.name = name
        self.stage = stage
        self.lord = lord
        # 资产名 → 外观标签(可能为 None);**用资产名判重**,见模块头
        self.skins = OrderedDict()

    @property
    def count(self) -> int:
        return len(self.skins)

    def fill_default_skin(self) -> bool:
        """基础那一版没名字时补上「本来的样子」,返回补没补。

        游戏只给「特殊的那一版」登记名字:波波螺有「被污染的样子」,原样反而没条目。
        **这四个字是我们补的、数据里没有** —— 但它正是游戏自己在肯登记时用的说法
        (板板壳_本来的样子、冬羽雀_本来的样子),不是另造一套词。
        只补**孤零零一个**没名字的:两个都没名字就说明这不是「基础版 + 特殊版」那种结构,
        瞎补会把话说错。
        """
        blank = [model for model, label in self.skins.items() if not label]
        if self.count < 2 or len(blank) != 1:
            return False
        self.skins[blank[0]] = "本来的样子"
        return True

    def render(self) -> str:
        return f"{self.name}x{self.count}" if self.count > 1 else self.name

    def skin_note(self) -> str | None:
        """有多种外观、且标签齐全时,给一句「x2 表示有 …/… 两种外观」。

        标签缺一个就整条不给:「x3 表示有 A/B」比不解释更让人犯嘀咕。
        """
        if self.count < 2:
            return None
        labels = [lab for lab in self.skins.values() if lab]
        if len(labels) != self.count:
            return None
        note = f"x{self.count} 表示有 " + "/".join(labels)
        return note + " 两种外观" if self.count == 2 else note


class Pack:
    """一个图鉴号 = 一个包。**没有图鉴号的一律记 `000`**,那一档靠资产词干彼此分开
    (见 `build` 里 key2 的说明),所以 `000` 上会挂着好几十个包。"""

    def __init__(self, book: int, name: str):
        self.book = book
        self.name = name
        self.forms: OrderedDict[str, Form] = OrderedDict()
        self.chains: list[int] = []

    @property
    def file_name(self) -> str:
        return f"{self.book:03d}-{self.name}.rkpet"

    def ordered(self) -> list[Form]:
        # 普通形态按 stage,王者形态一律排最后(它们的 stage 是 4,但同为 4 的普通形态
        # 也有,排序键里显式分开更稳)
        return sorted(self.forms.values(), key=lambda f: (f.lord, f.stage))

    @property
    def total(self) -> int:
        return sum(f.count for f in self.ordered())


def build(parsed: Path) -> list[Pack]:
    petbase = rows(parsed, "PETBASE_CONF")
    evolution = rows(parsed, "PET_EVOLUTION_CONF")
    model_conf = rows(parsed, "MODEL_CONF")
    labels = skin_labels(rows(parsed, "MEGAMAP_CONF"))

    def base(pid: int) -> dict:
        return petbase.get(str(pid)) or {}

    def asset_of(pid: int) -> str:
        """这个形态用哪份模型资产。**认不出就返回空串** —— 那种行不该进清单。

        `MODEL_CONF.path` 里没有 `/Pets/…` 的只有四行(幸运惊喜盒 ×3、随机精灵),
        是界面上的占位条目,不是宠物;没有资产目录也就无从导出。
        """
        model = base(pid).get("model_conf")
        path = (model_conf.get(str(model)) or {}).get("path") or ""
        _, _, rest = path.partition("/Pets/")
        return rest.split("/", 1)[0] if rest else ""

    packs: dict[tuple[int, str], Pack] = {}
    # 有图鉴号的先来:它们说了算。**没图鉴号的那批里有一堆借着别人的模型占位** ——
    # `Com_YaJiJi1_001` 是鸭吉吉的模型,却被 51 个还没做模型的宠物拿去顶着,
    # 不先把有主的资产占掉,那 51 个会连成一个二十几形态的怪包
    taken: set[str] = set()
    ordered_chains = sorted(
        ((int(k), v) for k, v in evolution.items()),
        key=lambda kv: (
            base(((kv[1].get("evolution_chain") or [{}])[0]).get("petbase_id", 0)).get(
                "pictorial_book_id"
            )
            is None,
            kv[0],
        ),
    )
    # 有图鉴号的包占着哪些资产。**判断某个没图鉴号的链首是不是在「顶别人的模型」就靠它**
    booked_assets = {
        asset_of(m["petbase_id"])
        for _, chain in ordered_chains
        for m in (chain.get("evolution_chain") or [])
        if base((chain["evolution_chain"] or [{}])[0].get("petbase_id", 0)).get(
            "pictorial_book_id"
        )
        is not None
    }
    for key, chain in ordered_chains:
        members = chain.get("evolution_chain") or []
        # 链名自己写着「废弃」的不要(两条:野外首领梦想三三/雪影娃娃)。
        # 它们仨成员指的是同一份 BOSS 模型,收进来会白白多出两个只有一份模型的包
        if not members or any(m in (chain.get("name") or "") for m in ("废弃", "占位", "测试")):
            continue
        root = members[0]
        book = base(root["petbase_id"]).get("pictorial_book_id")
        # 键是 (图鉴号, 次序) —— 没图鉴号的那批都排在 0 号,靠资产词干彼此分开。
        # **链首没图鉴号也照收**:那条链在 PET_EVOLUTION_CONF 里是全的(连王者形态一起),
        # 比后面 adopt_unbooked 靠词干拼出来的强;词干当键,好让散落的成员认回同一个包
        # 没图鉴号的用**链首资产的词干**分包:同根的几条分支链(菌宝那四条)并成一个,
        # 换外观的几条(雪毛角羚牛那三条,`…1_001` / `…1Ar_001` / `…1Ar_002`)也并成一个 ——
        # 有图鉴号的那批靠图鉴号并,这批只剩词干可用。
        # **链首顶着别人的模型时不能用词干**:五条毫不相干的链都指着鸭吉吉的
        # `Com_YaJiJi1_001`,按词干会并成一个二十几形态的怪包;那时退回按链首 id 各归各的
        root_asset = asset_of(root["petbase_id"])
        key2 = ""
        if book is None:
            own = root_asset and root_asset not in booked_assets
            key2 = asset_stem(root_asset) if own else f"pet{root['petbase_id']}"
        pack = packs.setdefault((book or 0, key2), Pack(book or 0, root["pet_name"]))
        pack.chains.append(key)
        # 链名自己就带着外观:「矿晶虫进化链(西瓜碧玺的样子)」。**当 MEGAMAP 的补漏**——
        # 那张表只登记了「特殊的那一版」,基础版往往没有条目(脆筒甜甜的樱桃巧克力口味
        # 就只在链名里);而链名对整条链有效,正好补上
        chain_skin = chain_label(chain.get("name") or "")

        listed = [(m["petbase_id"], m["pet_name"], m.get("stage", 0), False) for m in members]
        listed += [
            (m["lord_petbase_id"], m["lord_pet_name"], base(m["lord_petbase_id"]).get("stage", 4), True)
            for m in chain.get("lordevo_chain") or []
        ]
        for pid, pet_name, stage, lord in listed:
            asset = asset_of(pid)
            if not asset:
                continue
            # 没图鉴号的不许抢已经有主的资产(见上面 taken 的说明);
            # 有图鉴号的照收 —— 同一份模型在几个包里各占一格是正常的(千棘海针那种)
            if book is None and asset in taken:
                continue
            taken.add(asset)
            form = pack.forms.get(pet_name)
            if form is None:
                # 王者形态不套资产阶段位:它们的资产名写的是 `…Bo_001`(没有数字),
                # 认不出就会退回配置里的 stage,而同一条链的几个王者 stage 全一样 ——
                # 那时候该保持 `lordevo_chain` 里的先后(叶冕魔力猫在武斗酷猫前面)
                form = pack.forms[pet_name] = Form(
                    pet_name, stage if lord else (asset_stage(asset) or stage), lord
                )
            # **同名同资产 = 同一个形态**:钻石蜗在六条链里各挂一个 id,
            # 指的都是 Lig_KuangChongBo_001,只该算一种
            form.skins.setdefault(asset, labels.get(pid) or chain_skin)

    # 整条链的形态都被上面那条规则挡掉的话,包就空了 —— 别留个没形态的壳
    for k in [k for k, p in packs.items() if not p.forms]:
        del packs[k]
    adopt_unbooked(packs, petbase, asset_of)

    result = list(packs.values())
    for pack in result:
        # 链首被挡掉时(它顶着别人的模型),包名得改成留下来的头一环 ——
        # 不然会出现一个叫「呆火鸟」、里面根本没有呆火鸟的包
        if pack.name not in pack.forms:
            pack.name = pack.ordered()[0].name
        for form in pack.forms.values():
            if form.fill_default_skin():
                FILLED.append(f"{pack.file_name} 的 {form.name}")
    # **排序放在改名之后**:`000` 那一档的包名要等收养完才定下来。
    # 与 exporter/Config.cs 的 `Packs()` 同序(图鉴号,然后按名字的码位序),两边好逐行对账
    result.sort(key=lambda p: (p.book, p.name))
    return result


def unbooked_rows(petbase: dict, taken: set[str], asset_of) -> list[tuple[int, dict, str]]:
    """没有图鉴号、资产也还没被收走的那些行。

    过滤掉三类:`首领-xxx`(BOSS 行,和本体同一份模型)、名字带「占位」的(数据自己说的)、
    以及 `legal_petbase == 0`(明确作废)。**id 不在 1000~99999 的**是影子行,也不要。
    """
    out = []
    for key, row in petbase.items():
        try:
            pid = int(key)
        except ValueError:
            continue
        name = row.get("name") or ""
        if not (1000 <= pid <= 99999) or not name:
            continue
        if any(mark in name for mark in ("测试", "Test", "占位")) or name.startswith("首领"):
            continue
        if row.get("legal_petbase") == 0 or row.get("pictorial_book_id") is not None:
            continue
        asset = asset_of(pid)
        if not asset or asset in taken:
            continue
        out.append((pid, row, asset))
    return out


def adopt_unbooked(packs: dict, petbase: dict, asset_of) -> None:
    """把没图鉴号的形态收进来:词干对得上的并进原包,剩下的自成 `000-xxx.rkpet`。

    **词干对得上就不是新宠物**,是原包缺的那一环 —— 实测八处:赤毛鸡仔那条链缺三阶
    (伊丽莎白 `Fir_JiZai3_001`)、路路尼缺一阶和三阶、小鼠獭缺王者(卷发巨獭 `…ShuTaBo_001`)
    等等。丢进 000 会把一条链劈成两个包,正好和「规范化合并」反着来。
    """
    taken = {a for p in packs.values() for f in p.forms.values() for a in f.skins}
    by_stem = {
        asset_stem(a): pack
        for pack in packs.values()
        for f in pack.forms.values()
        for a in f.skins
    }
    orphans: dict[str, Pack] = {}
    for pid, row, asset in sorted(unbooked_rows(petbase, taken, asset_of)):
        if asset in taken:
            continue  # 同一份模型挂着好几行(古路尼 3365/8024),头一行说了算
        taken.add(asset)
        name = row.get("name") or str(pid)
        lord = is_lord_asset(asset)
        stage = row.get("stage") or 0 if lord else (asset_stage(asset) or row.get("stage") or 0)
        stem = asset_stem(asset)
        home = by_stem.get(stem)
        if home is not None:
            ADOPTED.append(f"{home.file_name} ← {name}({asset})")
        else:
            # 000 里同一条链的成员靠词干聚在一起(它们的 evolution_pet_id 是空的)
            home = orphans.get(stem)
            if home is None:
                home = orphans[stem] = Pack(0, name)
                packs[(0, stem)] = home
        # 包名跟着最靠前的那一环走:路路尼(二阶)收进 路路(一阶)之后该改叫 路路。
        # **要在挂进去之前比**,不然拿自己和自己比,永远不会更靠前
        earlier = min((f.stage for f in home.forms.values() if not f.lord), default=99)
        if not lord and stage < earlier:
            home.name = name
        form = home.forms.get(name)
        if form is None:
            form = home.forms[name] = Form(name, stage, lord)
        form.skins.setdefault(asset, None)


def notes_of(forms: list[Form]) -> list[str]:
    """一个包里的外观说明。

    **同一套外观说一遍就够**:一条链上每一环往往共用同一组外观(海盔虫/刺盔虫/千棘盔
    都是「本来的/磨损的」),逐个形态各说一句是三份一模一样的话。
    去重之后要是还剩不止一条(一个包里几个形态各有各的外观),就把形态名带上,
    否则光看「x2 表示有…」不知道说的是哪一个。
    """
    seen = OrderedDict()
    for form in forms:
        if note := form.skin_note():
            seen.setdefault(note, []).append(form.name)
    if len(seen) <= 1:
        return list(seen)
    return [f"{names[0]} 的 {note}" for note, names in seen.items()]


def as_text(packs: list[Pack]) -> str:
    out = []
    for pack in packs:
        forms = pack.ordered()
        line = (
            f"{pack.file_name}:包含 "
            + "/".join(f.render() for f in forms)
            + f" {cn_count(pack.total)}种形态"
        )
        if notes := notes_of(forms):
            line += ",其中 " + ";".join(notes)
        out.append(line + ";")
    return "\n".join(out)


def as_tsv(packs: list[Pack]) -> str:
    out = ["图鉴号\t包名\t形态数\t形态构成\t进化链 id"]
    for pack in packs:
        out.append(
            "\t".join(
                [
                    f"{pack.book:03d}",
                    pack.file_name,
                    str(pack.total),
                    "/".join(f.render() for f in pack.ordered()),
                    ",".join(str(c) for c in pack.chains),
                ]
            )
        )
    return "\n".join(out)


# 你给的六条样例,原样抄进来当回归基线
EXPECTED = {
    2: ("002-喵喵.rkpet", "喵喵/喵呜/魔力猫/叶冕魔力猫/武斗酷猫", 5),
    39: ("039-矿晶虫.rkpet", "矿晶虫/晶石蜗x6/钻石蜗", 8),
    76: ("076-海盔虫.rkpet", "海盔虫x2/刺盔虫x2/千棘盔x2/千棘海针", 7),
    116: ("116-小独角兽.rkpet", "小独角兽/白金独角兽/彩虹独角兽", 3),
    313: ("313-果冻.rkpet", "果冻/抹茶布丁/椰浆布丁/熔岩布丁", 4),
    322: ("322-月牙雪熊.rkpet", "月牙雪熊", 1),
}


def check(packs: list[Pack]) -> int:
    by_book = {p.book: p for p in packs}
    bad = 0
    for book, (file_name, forms, total) in EXPECTED.items():
        pack = by_book.get(book)
        got = (
            (pack.file_name, "/".join(f.render() for f in pack.ordered()), pack.total)
            if pack
            else None
        )
        if got == (file_name, forms, total):
            print(f"  ok  {file_name}")
        else:
            bad += 1
            print(f"  ✗   期望 {(file_name, forms, total)}\n      实得 {got}")
    print(f"\n{len(EXPECTED) - bad}/{len(EXPECTED)} 条样例对上了")
    return bad


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument(
        "--parsed",
        type=Path,
        default=Path(os.environ.get("ROCOM_PARSED", Path.home() / "Downloads/rocom/parsed")),
        help="解包根(默认 $ROCOM_PARSED,再默认 ~/Downloads/rocom/parsed)",
    )
    ap.add_argument("--format", choices=["text", "tsv"], default="text")
    ap.add_argument("--check", action="store_true", help="只跑样例回归,不出清单")
    args = ap.parse_args()

    packs = build(args.parsed)
    if args.check:
        return 1 if check(packs) else 0

    print(as_text(packs) if args.format == "text" else as_tsv(packs))
    print(
        f"\n# {len(packs)} 个包 / {sum(p.total for p in packs)} 个形态"
        f"(含外观变体);归并前是 {sum(len(p.chains) for p in packs)} 条进化链",
        file=sys.stderr,
    )
    if FILLED:
        print(
            f"# 有 {len(FILLED)} 个形态的基础外观在数据里没名字,按惯例补了「本来的样子」:"
            + "、".join(FILLED),
            file=sys.stderr,
        )
    if ADOPTED:
        print(
            f"# 有 {len(ADOPTED)} 个没图鉴号的形态**按资产词干认回了原包**(推断,不是数据直说的):"
            + "、".join(ADOPTED),
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
