import { memo } from "react";
import { Download, Eye, Flag, TriangleAlert } from "lucide-react";
import type { AssetStat, Pack, SpriteSheet } from "../../shared/types.ts";
import type { PackHit } from "@/lib/search.ts";
import { PetAvatar } from "@/components/PetAvatar.tsx";
import { Button } from "@/components/ui/button.tsx";
import { Badge, Tooltip } from "@/components/ui/primitives.tsx";
import { cn, cnCount, formatBytes, formatCount } from "@/lib/utils.ts";

interface Props {
  hit: PackHit;
  sheet: SpriteSheet;
  stat: AssetStat | undefined;
  onOpen: (pack: Pack) => void;
  onPreview: (pack: Pack) => void;
  onDownload: (pack: Pack) => void;
  onReport: (pack: Pack) => void;
}

/**
 * 形态标签那块的高度:**正好两行**。
 * 一个标签是 `text-[11px] leading-4`(16px)+ `py-0.5`(上下各 2)= 20px,行距 `gap-1` = 4px,
 * 两行就是 `20 + 4 + 20 = 44px` = `h-11`。
 */
const CHIPS_H = "h-11";

/**
 * 一张卡片的占位高度,给 `contain-intrinsic-size` 用 —— **必须等于真实高度**。
 *
 * 对不上就会漂:视口外的卡片按这个数算进文档高,进了视口换成真实高度,差多少文档就变多少,
 * **而拖着的滚动条滑块跟着挪**。曾经占位给 13rem、真实高度却在 176~248px 之间参差,
 * 整页系统性高估 9%(15245 → 13978):往下拖时滑块一路追不上鼠标,拖到底之后再拖才正常
 * (那时每张都量过了),回到顶上一看滑块比刚打开时更长。所以卡片钉成定高
 * (形态标签那块固定两行,见 `CHIPS_H`)。
 *
 * **它量的是内容盒,边框另算在外面。** 卡片自己那圈 `border` 会加在这个数之上
 * (`box-sizing: border-box` 在这儿不作数),所以填的是「卡片高度减掉上下两条边框」
 * 而不是卡片高度:实测渲出来的卡片 199.83px、上下边框各 0.667px(1px 边框在 1.5 倍屏
 * 上就是这个用值),内容盒 **198.5px**。填 200 的话每张被跳过的卡片会多算 1.33px,
 * 六十多行下来就是七十几像素的漂移。
 *
 * 边框那一项在两边是同一个数(跳过的和画出来的都要加),所以按内容盒填之后
 * **换个屏幕缩放也仍然对得上**。
 *
 * 改动卡片里任何一块的高度都要重新量:打开页面记下 `scrollHeight`,滚到底再记一次,
 * 两个数要一样。
 */
// **写成整串字面量**:Tailwind 是扫源码文本认 class 的,拼出来的名字它看不见,
// 那条 utility 根本不会生成(表现是占位值悄悄失效、滚动条照旧漂)。
const CARD_H = "[contain-intrinsic-size:auto_198.5px]";

/**
 * **memo 过**:换排序时两百张卡片的内容一个字都没变,变的只是顺序。不 memo 的话
 * 每张卡都要重跑一遍(每张里有三个 Radix Tooltip,各自带 context 与 effect),
 * 一次换序就是六百个 tooltip 重建 —— 那才是切排序时卡住的地方,排序本身不到 1ms。
 *
 * 代价是所有回调都必须在上游稳住引用(见 App.tsx 里的 `reportPack`)。
 */
export const PackCard = memo(function PackCard({
  hit,
  sheet,
  stat,
  onOpen,
  onPreview,
  onDownload,
  onReport,
}: Props) {
  const { pack, formHits } = hit;
  const pending = !pack.sha256;
  const downloads = stat?.downloads ?? 0;
  const reports = stat?.reports ?? 0;

  // 标签最多摆 4 个,再多也放不进那两行。搜「魔力猫」时它要是排在第 5 位,卡片上就一个
  // 都看不见了 —— 所以命中的形态一律提到前面。
  //
  // **不再缀「+N」**:标签那块会按两行切,切了之后「+N」要么跟着被切掉、要么剩下一个
  // 对不上实际显示条数的数字。总数上面「12 种形态」那行本来就写着,不必在这儿说第二遍。
  const MAX_CHIPS = 4;

  const shown = formHits.size
    ? [...pack.forms].sort((a, b) => Number(formHits.has(b.name)) - Number(formHits.has(a.name)))
        .slice(0, MAX_CHIPS)
    : pack.forms.slice(0, MAX_CHIPS);

  return (
    <div
      className={cn(
        "group flex flex-col rounded-xl border bg-card shadow-xs transition-shadow hover:shadow-md",
        // 视口外的卡片跳过样式/布局/绘制。一屏放得下六七张,其余一百九十多张不必参与
        // 每次重排的布局计算(实测全量重排 2.5~4.2ms,不跳过是 11~13.6ms)。
        // 代价是占位高度必须和真实高度分毫不差,所以这张卡是定高的 —— 见 `CARD_H`。
        CARD_H,
        "[content-visibility:auto]",
        reports > 0 && "border-[color-mix(in_oklab,var(--warning)_45%,var(--border))]",
      )}
    >
      <button
        type="button"
        onClick={() => onOpen(pack)}
        className="flex flex-1 items-start gap-3 rounded-t-xl p-3.5 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40"
      >
        <PetAvatar name={pack.name} sprite={pack.sprite} sheet={sheet} size={52} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <Badge variant={pack.book === "000" ? "outline" : "secondary"} className="font-mono">
              {pack.book === "000" ? "无图鉴号" : `#${pack.book}`}
            </Badge>
            {reports > 0 && (
              <Badge variant="warning">
                <TriangleAlert />
                {reports}
              </Badge>
            )}
          </div>
          <div className="mt-1 truncate text-[15px] font-semibold">{pack.name}</div>
          <div className="mt-0.5 text-xs text-muted-foreground">
            {cnCount(pack.forms.length)} 种形态
            {!pending && <> · {formatBytes(pack.size)}</>}
          </div>

          {/*
            **定高两行,放不下就切掉。** 形态名长短差很多(「兽花蕾」一行装得下四个,
            「海盔虫(本来的样子)」这种一行只装得下一个),原来任它撑,卡片高度就在
            176~248px 之间参差 —— 而 `content-visibility` 的占位值只能给一个数,
            对不上滚动条就漂(见上面那段)。
            切掉的那部分不算丢信息:上面「六 种形态」那行本来就写着总数,
            而命中搜索的形态已经被提到最前面了。
          */}
          <div
            className={cn(
              // `items-start content-start`:定高的 flex 容器默认会把标签**拉高填满**
              // (单行时每个标签都变成 44px 的方块,四行字高的空盒子)。两条都要,
              // 一条管单行内的对齐、一条管多行之间的分布。
              "mt-2 flex flex-wrap content-start items-start gap-1 overflow-hidden",
              CHIPS_H,
            )}
          >
            {shown.map((form) => (
              <span
                key={form.name}
                className={cn(
                  "rounded px-1.5 py-0.5 text-[11px] leading-4",
                  formHits.has(form.name)
                    ? "bg-primary/15 font-medium text-primary"
                    : "bg-muted text-muted-foreground",
                )}
              >
                {form.name}
                {form.skins > 1 && <span className="opacity-60">×{form.skins}</span>}
              </span>
            ))}
          </div>
        </div>
      </button>

      <div className="flex items-center gap-2 border-t px-3.5 py-2.5">
        <Tooltip label="下载次数(同一 IP 每天只记一次)">
          <span className="flex items-center gap-1 text-xs text-muted-foreground">
            <Download className="size-3.5" />
            {formatCount(downloads)}
          </span>
        </Tooltip>
        <div className="flex-1" />
        <Tooltip label={pending ? "这个包还没上传到 R2" : "标记异常"}>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="标记异常"
            disabled={pending}
            onClick={() => onReport(pack)}
          >
            <Flag />
          </Button>
        </Tooltip>
        <Tooltip label={pending ? "这个包还没上传到 R2" : "在浏览器里看看它长什么样"}>
          <Button
            variant="outline"
            size="sm"
            disabled={pending}
            onClick={() => onPreview(pack)}
          >
            <Eye />
            预览
          </Button>
        </Tooltip>
        <Button size="sm" disabled={pending} onClick={() => onDownload(pack)}>
          <Download />
          {pending ? "待上传" : "下载"}
        </Button>
      </div>
    </div>
  );
});
