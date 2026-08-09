import { Download, Eye, Flag, TriangleAlert } from "lucide-react";
import type { AssetStat, Pack, SpriteSheet } from "../../shared/types.ts";
import { PetAvatar } from "@/components/PetAvatar.tsx";
import { Sha256 } from "@/components/Sha256.tsx";
import { Button } from "@/components/ui/button.tsx";
import {
  Badge,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/primitives.tsx";
import { cnCount, formatBytes, formatCount } from "@/lib/utils.ts";

interface Props {
  pack: Pack | null;
  sheet: SpriteSheet;
  stat: AssetStat | undefined;
  onOpenChange: (open: boolean) => void;
  onPreview: (pack: Pack) => void;
  onDownload: (pack: Pack) => void;
  onReport: (pack: Pack) => void;
}

function stageLabel(stage: number): string {
  if (stage === 99) return "王者";
  return `${cnCount(stage)}阶`;
}

export function PackDialog({
  pack,
  sheet,
  stat,
  onOpenChange,
  onPreview,
  onDownload,
  onReport,
}: Props) {
  if (!pack) return null;
  const pending = !pack.sha256;

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <div className="flex items-center gap-3">
            <PetAvatar name={pack.name} sprite={pack.sprite} sheet={sheet} size={44} />
            <div className="min-w-0">
              <DialogTitle className="flex items-center gap-2">
                {pack.name}
                <Badge variant={pack.book === "000" ? "outline" : "secondary"} className="font-mono">
                  {pack.book === "000" ? "无图鉴号" : `#${pack.book}`}
                </Badge>
              </DialogTitle>
              <DialogDescription className="mt-0.5 font-mono text-xs">
                {pack.id}.rkpet
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <dt className="text-muted-foreground">文件大小</dt>
            <dd>{pending ? "尚未上传" : formatBytes(pack.size)}</dd>

            <dt className="text-muted-foreground">下载次数</dt>
            <dd>{formatCount(stat?.downloads ?? 0)}</dd>

            <dt className="text-muted-foreground">异常标记</dt>
            <dd>
              {stat?.reports ? (
                <span className="inline-flex items-center gap-1 text-[var(--warning)]">
                  <TriangleAlert className="size-3.5" />
                  {stat.reports} 次 —— 下载前留意校验 sha256
                </span>
              ) : (
                <span className="text-muted-foreground">暂无</span>
              )}
            </dd>

            <dt className="self-start pt-0.5 text-muted-foreground">sha256</dt>
            <dd className="min-w-0">
              <Sha256 value={pack.sha256} />
            </dd>
          </dl>

          <div className="mt-5">
            <h3 className="mb-2 text-sm font-medium">
              包内 {cnCount(pack.forms.length)} 种形态
              <span className="ml-1.5 font-normal text-muted-foreground">
                运行时可在托盘或配置窗口里切换
              </span>
            </h3>
            <ul className="divide-y rounded-lg border">
              {pack.forms.map((form) => (
                <li key={form.name} className="flex items-center gap-3 px-3 py-2">
                  <PetAvatar name={form.name} sprite={form.sprite} sheet={sheet} size={34} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-medium">{form.name}</div>
                    {form.asset && (
                      <div className="truncate font-mono text-[11px] text-muted-foreground">
                        {form.asset}
                      </div>
                    )}
                  </div>
                  {form.skins > 1 && <Badge variant="outline">{form.skins} 种外观</Badge>}
                  <Badge variant="secondary">{stageLabel(form.stage)}</Badge>
                </li>
              ))}
            </ul>
          </div>

          <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
            下载后到配置窗口里点击「导入包」，选择下载好的 .rkpet 文件即可。
          </p>
        </div>

        <DialogFooter>
          <Button variant="outline" disabled={pending} onClick={() => onReport(pack)}>
            <Flag />
            标记异常
          </Button>
          <Button variant="outline" disabled={pending} onClick={() => onPreview(pack)}>
            <Eye />
            预览
          </Button>
          <Button disabled={pending} onClick={() => onDownload(pack)}>
            <Download />
            下载 {!pending && formatBytes(pack.size)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
