import { Download, Flag, TriangleAlert } from "lucide-react";
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
  onDownload: (pack: Pack) => void;
  onReport: (pack: Pack) => void;
}

export function PackCard({ hit, sheet, stat, onOpen, onDownload, onReport }: Props) {
  const { pack, formHits } = hit;
  const pending = !pack.sha256;
  const downloads = stat?.downloads ?? 0;
  const reports = stat?.reports ?? 0;

  // 卡片只放得下 4 个形态。搜「魔力猫」时它要是排在第 5 位,卡片上就只剩一个「+3」,
  // 看不出为什么这张卡会被搜出来 —— 所以命中的形态一律提到前面。
  const MAX_CHIPS = 4;
  const shown = formHits.size
    ? [...pack.forms].sort((a, b) => Number(formHits.has(b.name)) - Number(formHits.has(a.name)))
        .slice(0, MAX_CHIPS)
    : pack.forms.slice(0, MAX_CHIPS);

  return (
    <div
      className={cn(
        "group flex flex-col rounded-xl border bg-card shadow-xs transition-shadow hover:shadow-md",
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

          <div className="mt-2 flex flex-wrap gap-1">
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
            {pack.forms.length > MAX_CHIPS && (
              <span className="px-1 py-0.5 text-[11px] leading-4 text-muted-foreground">
                +{pack.forms.length - MAX_CHIPS}
              </span>
            )}
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
        <Button size="sm" disabled={pending} onClick={() => onDownload(pack)}>
          <Download />
          {pending ? "待上传" : "下载"}
        </Button>
      </div>
    </div>
  );
}
