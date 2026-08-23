import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, RotateCcw, Sparkles, TriangleAlert, ZoomIn, ZoomOut } from "lucide-react";
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
  type GlassyCatalog,
  type Progress,
} from "@/lib/preview.ts";
import { previewUrl } from "@/lib/api.ts";
import { cn, formatBytes } from "@/lib/utils.ts";

interface Props {
  pack: Pack | null;
  /** 分享链接带来的现场:形态、表情、外观。都可省。 */
  initial?: { form?: string; face?: string; look?: string } | null;
  /** 现场变了就说一声,由外面写进地址栏。 */
  onState?: (state: { form?: string; face?: string; look?: string }) => void;
  onOpenChange: (open: boolean) => void;
}

/** 点一下按钮缩放多少。约等于滚轮滚两格,少了要点很多下,多了一下就到头。 */
const ZOOM_STEP = 1.5;

/**
 * 炫彩下拉里那两个不是隐藏款名字的档。Radix 的 `Select` 把空串留给 placeholder,
 * 所以「不上炫彩」也得有个非空的值。隐藏款用的是它自己的中文名(回头原样进
 * `mutation` 字符串),和这两个撞不上。
 */
const NO_GLASSY = "none";
const COMMON_GLASSY = "common";

/**
 * 拼 `mutation` 字符串。**和桌面版 `Mutation::to_config` 同一套写法** ——
 * 两个轴各写一段,同时带就用 `+` 接:`异色+炫彩:3/33`。
 * 常规炫彩写编号(和游戏 UI 上的编号一致),隐藏款写名字(那四条的 id 没有规律)。
 */
function mutationText(shiny: boolean, kind: string, particle: number, color: number): string {
  const parts: string[] = [];
  if (shiny) parts.push("异色");
  if (kind === COMMON_GLASSY) parts.push(`炫彩:${particle}/${color}`);
  else if (kind !== NO_GLASSY) parts.push(`炫彩:${kind}`);
  return parts.join("+");
}

/**
 * `mutationText` 的逆:把分享链接里那段字还原成四个选项。
 *
 * 认不得的部分**默默忽略**(不整条丢掉):链接是手打的、或者跨版本了,能还原多少算多少,
 * 比整只退回原样强。真正的把关在 wasm 那边 —— `set_mutation` 认不得会抛,
 * 底下那句红字会说出来。
 */
function parseLook(text: string): { shiny: boolean; kind: string; color: number; particle: number } {
  let shiny = false;
  let kind = NO_GLASSY;
  let color = 1;
  let particle = 1;
  for (const raw of text.split("+")) {
    const part = raw.trim();
    if (part === "异色") {
      shiny = true;
      continue;
    }
    if (!part.startsWith("炫彩:")) continue;
    const rest = part.slice("炫彩:".length);
    const slash = rest.indexOf("/");
    if (slash < 0) {
      kind = rest; // 隐藏/赛季款写的是名字
      continue;
    }
    kind = COMMON_GLASSY;
    particle = Number(rest.slice(0, slash)) || 1;
    color = Number(rest.slice(slash + 1)) || 1;
  }
  return { shiny, kind, color, particle };
}

/** 0xRRGGBB → CSS。配色下拉里那两个小方块。 */
function swatch(rgb: number): string {
  return `#${rgb.toString(16).padStart(6, "0")}`;
}

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
export function PreviewDialog({ pack, initial, onState, onOpenChange }: Props) {
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
  // 外观:两个互不影响的轴(游戏里就是两个位标志,既有异色炫彩也有原色炫彩)
  const [glassy, setGlassy] = useState<GlassyCatalog | null>(null);
  const [shiny, setShiny] = useState(false);
  const [kind, setKind] = useState(NO_GLASSY);
  const [color, setColor] = useState(1);
  const [particle, setParticle] = useState(1);
  /** 炫彩取不到素材(部署时没传 `glassy/`)。出现过一次就把那几档一直禁着。 */
  const [glassyError, setGlassyError] = useState<string | null>(null);
  const supported = canPreview();
  const hasShiny = forms.find((f) => f.asset === asset)?.shiny ?? false;

  // 开:建会话 → 读 manifest → 装链首那个形态(默认待机)
  useEffect(() => {
    if (!pack || !supported || !canvas) return;
    let dead = false;
    setError(null);

    (async () => {
      try {
        const { session, forms, faces, glassy } = await PreviewSession.open(
          await previewUrl(pack),
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
        setGlassy(glassy);

        // 分享链接带来的现场。**认不出来的就用默认**:形态可能已经改名、
        // 表情可能是手打的 —— 那种情况下打开一只默认样子的宠物,好过报错不给看
        const wanted = forms.find((f) => f.asset === initial?.form)?.asset;
        const first = wanted ?? forms[0]?.asset ?? "";
        const wantFace = faces.includes(initial?.face ?? "") ? initial!.face! : (faces[0] ?? "");
        setFace(wantFace);
        setAsset(first);
        setClips(await session.showForm(first, setProgress));
        if (wantFace) session.setFace(wantFace);

        // 外观放在**装完形态之后**:`setMutation` 改的是「当前这只」,没有当前这只就没处改。
        // 炫彩共享贴图要现取,所以这一步也可能有一小段进度
        if (initial?.look) {
          const want = parseLook(initial.look);
          setShiny(want.shiny);
          setKind(want.kind);
          setColor(want.color);
          setParticle(want.particle);
          try {
            setProgress({ done: 0, total: 1, label: "下载炫彩素材" });
            await session.setMutation(initial.look);
          } catch (e) {
            setGlassyError(e instanceof Error ? e.message : String(e));
          }
        }
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
    // `initial` 是**开场那一份**,故意不进依赖:它由地址栏来,而地址栏是我们自己写的
    // —— 进了依赖就成了「改一下选项 → 重开一次会话」的死循环
  }, [pack, supported, canvas]);

  const switchForm = useCallback(
    async (next: string) => {
      const session = sessionRef.current;
      if (!session) return;
      setAsset(next);
      // 异色是**每个形态各自有没有**的:同一条进化链里常有前两阶有、末阶没有。
      // 切到没有的那一阶就把这个轴放下 —— 留着开关亮着却什么也不换更糟
      const drop = shiny && !forms.find((f) => f.asset === next)?.shiny;
      if (drop) setShiny(false);
      try {
        setClips(await session.showForm(next, setProgress));
        // 界面上放下了,wasm 那边也要跟着放下 —— 不然两边记的不是同一身,
        // 下次改配色时才「顺手」纠正过来,中间这段时间是对不上的
        if (drop) await session.setMutation(mutationText(false, kind, particle, color));
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setProgress(null);
      }
    },
    [forms, shiny, kind, particle, color],
  );

  /**
   * 换外观。**共享贴图是现取的**,所以这是个异步操作:进度条打「下载炫彩素材」
   * (常规炫彩两张约 250KB,铅字幻梦那张噪声自己就有 2MB)。
   *
   * 失败**不改回界面上的选择**:wasm 那边已经退回上一身了,而把下拉也拨回去会让人
   * 以为自己没点到。底下那句红字说清楚是取不到素材。
   */
  const applyMutation = useCallback(
    async (next: { shiny?: boolean; kind?: string; color?: number; particle?: number }) => {
      const want = {
        shiny: next.shiny ?? shiny,
        kind: next.kind ?? kind,
        color: next.color ?? color,
        particle: next.particle ?? particle,
      };
      setShiny(want.shiny);
      setKind(want.kind);
      setColor(want.color);
      setParticle(want.particle);
      const session = sessionRef.current;
      if (!session) return;
      const text = mutationText(want.shiny, want.kind, want.particle, want.color);
      try {
        if (want.kind !== NO_GLASSY) setProgress({ done: 0, total: 1, label: "下载炫彩素材" });
        await session.setMutation(text);
        setGlassyError(null);
      } catch (e) {
        setGlassyError(e instanceof Error ? e.message : String(e));
      } finally {
        setProgress(null);
      }
    },
    [shiny, kind, color, particle],
  );

  // 现场变了就往上报一次,由外面写进地址栏(见 lib/share.ts)。
  //
  // **默认值一律不写**:链首形态、默认表情、原样外观都省掉,于是随手点开一只得到的是
  // 干净的 `?pet=011-鸭吉吉`,而不是一串等于没说的参数。和 `roster.toml` 那条
  // 「默认值不落盘」是同一条规矩。
  const look = mutationText(shiny, kind, particle, color);
  useEffect(() => {
    if (!asset || !onState) return;
    onState({
      form: asset === forms[0]?.asset ? undefined : asset,
      face: face === faces[0] ? undefined : face,
      look: look || undefined,
    });
    // `onState` 故意不进依赖:它每次渲染都是新的箭头函数,进去就是每帧写一次地址栏
  }, [asset, face, look, forms, faces]);

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
                {/* 按最长的形态名给宽度:「晶石蜗(西瓜碧玺的样子)」十一个汉字,再窄就要省略号 */}
                <SelectTrigger className="w-56" aria-label="形态">
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

          {/*
            外观**自己一行**:上面那行已经有形态、表情和三个视角按钮,再塞四个控件
            要挤成两行,而且「形态」和「炫彩」挨着容易让人以为是一回事。
            两个轴互不影响 —— 游戏里就是两个位标志,既有异色炫彩,也有原色炫彩。
          */}
          {glassy && clips.length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {hasShiny && (
                <Button
                  variant={shiny ? "default" : "outline"}
                  size="sm"
                  aria-pressed={shiny}
                  onClick={() => void applyMutation({ shiny: !shiny })}
                >
                  <Sparkles />
                  异色
                </Button>
              )}
              <Select
                value={kind}
                onValueChange={(v) => void applyMutation({ kind: v })}
                disabled={glassyError !== null}
              >
                <SelectTrigger className="w-32" aria-label="炫彩">
                  <SelectValue placeholder="炫彩" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NO_GLASSY}>无炫彩</SelectItem>
                  {/* 常驻那款(黑白)缀个「隐藏」—— 光写「黑白」会被当成一组配色名
                      (常规那 39 组就叫「亮X暗 - 浅蓝蓝」这种)。赛季款自带专名,不用缀 */}
                  {glassy.hidden
                    .filter((h) => !h.season)
                    .map((h) => (
                      <SelectItem key={h.name} value={h.name}>
                        {h.name}隐藏
                      </SelectItem>
                    ))}
                  <SelectItem value={COMMON_GLASSY}>常规炫彩</SelectItem>
                  {glassy.hidden
                    .filter((h) => h.season)
                    .map((h) => (
                      <SelectItem key={h.name} value={h.name}>
                        {h.name}
                      </SelectItem>
                    ))}
                </SelectContent>
              </Select>
              {/* 配色与粒子只有常规炫彩才挑;隐藏/赛季款是配好的一整套 */}
              {kind === COMMON_GLASSY && (
                <>
                  <Select
                    value={String(color)}
                    onValueChange={(v) => void applyMutation({ color: Number(v) })}
                  >
                    <SelectTrigger className="w-56" aria-label="配色">
                      <SelectValue placeholder="配色" />
                    </SelectTrigger>
                    <SelectContent>
                      {glassy.colors.map((c) => (
                        <SelectItem key={c.id} value={String(c.id)}>
                          <span className="flex items-center gap-2">
                            <span className="flex gap-0.5">
                              {[c.color1, c.color2].map((rgb, i) => (
                                <span
                                  key={i}
                                  className="size-3 rounded-[2px]"
                                  style={{ backgroundColor: swatch(rgb) }}
                                />
                              ))}
                            </span>
                            {c.name}
                          </span>
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Select
                    value={String(particle)}
                    onValueChange={(v) => void applyMutation({ particle: Number(v) })}
                  >
                    <SelectTrigger className="w-32" aria-label="粒子">
                      <SelectValue placeholder="粒子" />
                    </SelectTrigger>
                    <SelectContent>
                      {glassy.particles.map((p) => (
                        <SelectItem key={p.id} value={String(p.id)}>
                          {p.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </>
              )}
              {glassyError && (
                <span className="text-xs text-[var(--warning)]">
                  这个站点没上传炫彩素材,炫彩用不了
                </span>
              )}
            </div>
          )}

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

        </div>
      </DialogContent>
    </Dialog>
  );
}
