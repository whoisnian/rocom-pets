import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, RotateCcw, TriangleAlert } from "lucide-react";
import type { Pack } from "../../shared/types.ts";
import { Button } from "@/components/ui/button.tsx";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/primitives.tsx";
import {
  PreviewSession,
  canPreview,
  type ClipEntry,
  type FormEntry,
  type Progress,
} from "@/lib/preview.ts";
import { previewUrl } from "@/lib/api.ts";
import { cn, formatBytes } from "@/lib/utils.ts";

interface Props {
  pack: Pack | null;
  onOpenChange: (open: boolean) => void;
}

/**
 * 宠物预览。**点开才加载**:wasm 渲染器、包里的模型与贴图,都是这个组件挂上之后
 * 才开始下的 —— 首屏与只想下载的人一个字节都不多付。
 *
 * 画的是桌宠那份渲染器编成的 wasm(src/web.rs),不是另做的一套预览:
 * 动作清单、降级规则、表情图集都来自同一份代码。
 */
export function PreviewDialog({ pack, onOpenChange }: Props) {
  // **画布用回调 ref 存进 state,不是 useRef**:Radix 的 Portal 是在 layout effect 里
  // 才挂上的,首次提交时弹窗内容还是 null。用 useRef 的话下面这个 effect 第一次跑就看见
  // `current === null` 直接返回,而依赖没变也不会再跑一次 —— 表现是弹窗开着、画布停在
  // 300×150、既没有进度条也没有报错。
  const [canvas, setCanvas] = useState<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<PreviewSession | null>(null);
  const [forms, setForms] = useState<FormEntry[]>([]);
  const [asset, setAsset] = useState("");
  const [clips, setClips] = useState<ClipEntry[]>([]);
  const [faces, setFaces] = useState<string[]>([]);
  const [face, setFace] = useState("");
  const [progress, setProgress] = useState<Progress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const supported = canPreview();

  // 开:建会话 → 读 manifest → 装链首那个形态(默认待机)
  useEffect(() => {
    if (!pack || !supported || !canvas) return;
    let dead = false;
    setError(null);

    (async () => {
      try {
        const { session, forms, faces } = await PreviewSession.open(
          previewUrl(pack.id),
          canvas,
          setProgress,
        );
        if (dead) {
          session.close();
          return;
        }
        sessionRef.current = session;
        setForms(forms);
        setFaces(faces);
        setFace(faces[0] ?? "");
        const first = forms[0]?.asset ?? "";
        setAsset(first);
        setClips(await session.showForm(first, setProgress));
        setProgress(null);
      } catch (e) {
        if (!dead) {
          setError(e instanceof Error ? e.message : String(e));
          setProgress(null);
        }
      }
    })();

    return () => {
      dead = true;
      sessionRef.current?.close();
      sessionRef.current = null;
    };
  }, [pack, supported, canvas]);

  const switchForm = useCallback(async (next: string) => {
    const session = sessionRef.current;
    if (!session) return;
    setAsset(next);
    try {
      setClips(await session.showForm(next, setProgress));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setProgress(null);
    }
  }, []);

  // 拖拽转视角。**用 pointer capture**:拖出画布外也不掉,松手才结束
  const dragging = useRef<{ id: number; x: number; y: number } | null>(null);
  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    dragging.current = { id: e.pointerId, x: e.clientX, y: e.clientY };
  };
  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = dragging.current;
    if (!drag || drag.id !== e.pointerId) return;
    sessionRef.current?.drag(e.clientX - drag.x, e.clientY - drag.y);
    drag.x = e.clientX;
    drag.y = e.clientY;
  };
  const endDrag = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (dragging.current?.id === e.pointerId) dragging.current = null;
  };

  if (!pack) return null;

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(48rem,calc(100vw-2rem))]">
        <DialogHeader>
          <DialogTitle>预览 · {pack.name}</DialogTitle>
          <DialogDescription>
            {supported
              ? "拖动画面转视角,点下面的按钮让它做动作。渲染的是桌面版那份运行时。"
              : "这个浏览器没有 WebGPU,预览用不了。"}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {/* 画布不透明(见 preview.ts 的 watchTheme),底色由它照这块的背景现算 */}
          <div className="relative overflow-hidden rounded-lg border bg-muted">
            <canvas
              ref={setCanvas}
              className={cn(
                "block h-[min(46svh,22rem)] w-full touch-none",
                supported ? "cursor-grab active:cursor-grabbing" : "opacity-40",
              )}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={endDrag}
              onPointerCancel={endDrag}
            />
            {(progress || error || !supported) && (
              <div className="absolute inset-0 grid place-items-center bg-card/80 px-6 text-center backdrop-blur-[1px]">
                {error || !supported ? (
                  <div className="flex max-w-sm flex-col items-center gap-2 text-sm">
                    <TriangleAlert className="size-5 text-[var(--warning)]" />
                    <span>{error ?? "这个浏览器不支持 WebGPU"}</span>
                    {!supported && (
                      <span className="text-xs text-muted-foreground">
                        Chrome / Edge 113+、Safari 26+ 可用;Firefox 看版本。
                        下载下来在桌面上跑不受影响。
                      </span>
                    )}
                  </div>
                ) : (
                  <div className="flex flex-col items-center gap-2 text-sm">
                    <Loader2 className="size-5 animate-spin text-muted-foreground" />
                    <span>{progress?.label}</span>
                    {progress && progress.total > 1 && (
                      <span className="font-mono text-xs text-muted-foreground">
                        {formatBytes(progress.done)} / {formatBytes(progress.total)}
                      </span>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>

          <div className="mt-3 flex flex-wrap items-center gap-2">
            {forms.length > 1 && (
              <Select value={asset} onValueChange={switchForm}>
                <SelectTrigger className="w-40" aria-label="形态">
                  <SelectValue placeholder="形态" />
                </SelectTrigger>
                <SelectContent>
                  {forms.map((f) => (
                    <SelectItem key={f.asset} value={f.asset}>
                      {f.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            {faces.length > 0 && (
              <Select
                value={face}
                onValueChange={(v) => {
                  setFace(v);
                  sessionRef.current?.setFace(v);
                }}
              >
                <SelectTrigger className="w-32" aria-label="表情">
                  <SelectValue placeholder="表情" />
                </SelectTrigger>
                <SelectContent>
                  {faces.map((name) => (
                    <SelectItem key={name} value={name}>
                      {name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            <div className="flex-1" />
            <Button
              variant="ghost"
              size="sm"
              onClick={() => sessionRef.current?.recenter()}
              disabled={!clips.length}
            >
              <RotateCcw />
              正面
            </Button>
          </div>

          {clips.length > 0 && (
            <div className="mt-3 flex flex-wrap gap-1.5">
              {clips.map((clip) => (
                <Button
                  key={clip.name}
                  variant="outline"
                  size="sm"
                  onClick={() => sessionRef.current?.play(clip.name)}
                >
                  {clip.label}
                </Button>
              ))}
            </div>
          )}

          <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
            表情是人挑的那张,不过<strong>做动作时跟着动作走</strong> —— 和桌面上一样:
            生气时是生气眼,睡着时是困倦眼。预览只下当前这个形态的模型与贴图,不是整包。
          </p>
        </div>
      </DialogContent>
    </Dialog>
  );
}
