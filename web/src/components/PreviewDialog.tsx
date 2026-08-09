import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, RotateCcw, TriangleAlert, ZoomIn, ZoomOut } from "lucide-react";
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

/** 点一下按钮缩放多少。约等于滚轮滚两格,少了要点很多下,多了一下就到头。 */
const ZOOM_STEP = 1.5;

/** 两个触点之间的距离;不足两点记 0(= 还不能算捏合)。 */
function spanOf(pointers: Map<number, { x: number; y: number }>): number {
  if (pointers.size < 2) return 0;
  const [a, b] = [...pointers.values()];
  return Math.hypot(a.x - b.x, a.y - b.y);
}

/** 两个触点的中点;不足两点记 null(= 还不能算平移)。 */
function midOf(pointers: Map<number, { x: number; y: number }>): { x: number; y: number } | null {
  if (pointers.size < 2) return null;
  const [a, b] = [...pointers.values()];
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
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

  // 和常见的模型查看器(three.js 的 OrbitControls、<model-viewer>、Sketchfab)对齐:
  // 左键拖 = 转视角,右键 / 中键 / Shift+左键 拖 = 平移中心,滚轮 = 缩放;
  // 触屏单指转、双指同时缩放与平移(两点间距管缩放,中点位移管平移)。
  //
  // **用 pointer capture**:拖出画布外也不掉,松手才结束。按 id 记住每个触点的上一次位置
  // —— 双指时两个 move 事件是分开来的,不存位置就算不出间距与中点。
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef(0);
  const mid = useRef<{ x: number; y: number } | null>(null);
  /** 这一次单指/单键拖拽是不是平移。按下时定死,中途松开 Shift 不该让它变成转视角。 */
  const panning = useRef(false);

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    if (pointers.current.size === 0) panning.current = e.button === 1 || e.button === 2 || e.shiftKey;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    pinch.current = spanOf(pointers.current);
    mid.current = midOf(pointers.current);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const prev = pointers.current.get(e.pointerId);
    if (!prev) return;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    const session = sessionRef.current;

    // 双指期间不转视角:捏合时两根手指多少都会一起平移,同时转会甩得很难受
    if (pointers.current.size >= 2) {
      const span = spanOf(pointers.current);
      if (pinch.current > 0 && span > 0) session?.zoomBy(span / pinch.current);
      pinch.current = span;
      const now = midOf(pointers.current);
      if (mid.current && now) session?.pan(now.x - mid.current.x, now.y - mid.current.y);
      mid.current = now;
      return;
    }
    const [dx, dy] = [e.clientX - prev.x, e.clientY - prev.y];
    if (panning.current) session?.pan(dx, dy);
    else session?.drag(dx, dy);
  };
  const endDrag = (e: React.PointerEvent<HTMLCanvasElement>) => {
    pointers.current.delete(e.pointerId);
    // 抬起一根还剩一根时重算:留着旧的间距与中点,下次再按下去会当成一次突变
    pinch.current = spanOf(pointers.current);
    mid.current = midOf(pointers.current);
    if (pointers.current.size === 0) panning.current = false;
  };

  /**
   * 滚轮缩放。**必须是原生监听 + `passive: false`** —— React 的 `onWheel` 挂在根容器上
   * 且是被动的,里面 `preventDefault` 不生效(只会得到一句控制台警告),表现是缩放的同时
   * 弹窗内容跟着滚。
   */
  useEffect(() => {
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      // deltaMode:0=像素 1=行 2=页。Chrome 一格给 100px,Firefox 给 3 行 —— 都折成「格」
      const notches = e.deltaMode === 0 ? e.deltaY / 100 : e.deltaMode === 1 ? e.deltaY / 3 : e.deltaY;
      // 指数而非加减:放大再缩小能回到原处,一格进一格退是对称的
      sessionRef.current?.zoomBy(Math.exp(-notches * 0.2));
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, [canvas]);

  if (!pack) return null;

  const zoomable = clips.length > 0;

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(48rem,calc(100vw-2rem))]">
        <DialogHeader>
          <DialogTitle>预览 · {pack.name}</DialogTitle>
          <DialogDescription>
            {supported
              ? "拖动转视角,右键或 Shift+拖动平移,滚轮缩放(触屏:单指转、双指缩放与平移)。渲染的是桌面版那份运行时。"
              : "这个浏览器没有 WebGPU,预览用不了。"}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {/* 画布不透明(见 preview.ts 的 watchTheme),底色由它照这块的背景现算 */}
          <div className="relative overflow-hidden rounded-lg border bg-muted">
            <canvas
              ref={setCanvas}
              className={cn(
                // select-none:Shift+拖动本来是「扩选」,别让它把弹窗里的文字一路刷蓝
                "block h-[min(46svh,22rem)] w-full touch-none select-none",
                supported ? "cursor-grab active:cursor-grabbing" : "opacity-40",
              )}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={endDrag}
              onPointerCancel={endDrag}
              // 右键是拿来平移的,别让菜单弹出来打断;中键同理,不然 Windows 上会起自动滚动
              // (那个得拦 mousedown,拦 pointerdown 不管用)
              onContextMenu={(e) => e.preventDefault()}
              onMouseDown={(e) => e.button === 1 && e.preventDefault()}
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
            {/* 滚轮和双指都能缩放,但那两样都不显眼,也没法用键盘 —— 留一对按钮 */}
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="缩小"
              onClick={() => sessionRef.current?.zoomBy(1 / ZOOM_STEP)}
              disabled={!zoomable}
            >
              <ZoomOut />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="放大"
              onClick={() => sessionRef.current?.zoomBy(ZOOM_STEP)}
              disabled={!zoomable}
            >
              <ZoomIn />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => sessionRef.current?.recenter()}
              disabled={!zoomable}
            >
              <RotateCcw />
              复位
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
