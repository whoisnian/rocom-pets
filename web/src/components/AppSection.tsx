import { Download, Flag, MonitorSmartphone, TriangleAlert } from "lucide-react";
import type { AppBuild, StatsResponse } from "../../shared/types.ts";
import { Sha256 } from "@/components/Sha256.tsx";
import { Button } from "@/components/ui/button.tsx";
import { Badge, Tooltip } from "@/components/ui/primitives.tsx";
import { formatBytes, formatCount } from "@/lib/utils.ts";

interface Props {
  apps: AppBuild[];
  stats: StatsResponse;
  onDownload: (app: AppBuild) => void;
  onReport: (app: AppBuild) => void;
}

export function AppSection({ apps, stats, onDownload, onReport }: Props) {
  if (!apps.length) {
    return (
      <div className="rounded-xl border border-dashed p-5 text-sm text-muted-foreground">
        <div className="mb-1 flex items-center gap-2 font-medium text-foreground">
          <MonitorSmartphone className="size-4" />
          应用本体还没上传
        </div>
        编译出产物后跑{" "}
        <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">
          npm run catalog -- --packs &lt;包目录&gt; --apps &lt;产物目录&gt; --version x.y.z
        </code>
        ,再把文件同步到 R2 的 <code className="font-mono text-xs">app/</code> 前缀下。
      </div>
    );
  }

  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {apps.map((app) => {
        const stat = stats[app.id];
        return (
          <div key={app.id} className="flex flex-col rounded-xl border bg-card p-4 shadow-xs">
            <div className="flex items-start gap-2">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <h3 className="font-semibold">{app.label}</h3>
                  <Badge variant="secondary" className="font-mono">
                    v{app.version}
                  </Badge>
                  {stat?.reports ? (
                    <Badge variant="warning">
                      <TriangleAlert />
                      {stat.reports}
                    </Badge>
                  ) : null}
                </div>
                <div className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
                  {app.filename} · {formatBytes(app.size)}
                </div>
              </div>
            </div>

            {app.note && <p className="mt-2 text-xs text-muted-foreground">{app.note}</p>}

            <div className="mt-3">
              <div className="mb-1 text-xs text-muted-foreground">sha256</div>
              <Sha256 value={app.sha256} />
            </div>

            <div className="mt-3 flex items-center gap-2 border-t pt-3">
              <span className="flex items-center gap-1 text-xs text-muted-foreground">
                <Download className="size-3.5" />
                {formatCount(stat?.downloads ?? 0)}
              </span>
              <div className="flex-1" />
              <Tooltip label="标记异常">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="标记异常"
                  onClick={() => onReport(app)}
                >
                  <Flag />
                </Button>
              </Tooltip>
              <Button size="sm" onClick={() => onDownload(app)}>
                <Download />
                下载
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
