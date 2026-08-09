/**
 * 预览的驱动:把 wasm 那边的 `Preview`(src/web.rs)接到一块 canvas 上。
 *
 * **wasm 是动态 import 的**,所以那 1.4MB(brotli 后约 380KB)只有真点开预览的人
 * 才会下 —— 首屏一个字节都不多。同样地,`.rkpet` 也是点开才按 Range 取,
 * 而且只取要看的那一个形态。
 */

import { RemoteZip, type ZipEntry } from "@/lib/rkpet.ts";

/** 这台机器有没有 WebGPU。骨骼矩阵走只读 storage buffer,WebGL2 顶不上。 */
export function canPreview(): boolean {
  return typeof navigator !== "undefined" && "gpu" in navigator;
}

export interface FormEntry {
  asset: string;
  name: string;
}

export interface ClipEntry {
  name: string;
  label: string;
}

interface Wasm {
  default: (init?: unknown) => Promise<unknown>;
  Preview: new () => WasmPreview;
  expressions: () => string[];
}

interface WasmPreview {
  attach(canvas: HTMLCanvasElement): Promise<void>;
  put(path: string, bytes: Uint8Array): void;
  reset(): void;
  load_pack(): { asset: string; name: string }[];
  load_form(asset: string): { name: string; label: string }[];
  play(name: string): boolean;
  set_face(name: string): void;
  drag(dx: number, dy: number): void;
  recenter(): void;
  set_background(r: number, g: number, b: number): void;
  resize(width: number, height: number): void;
  frame(dt: number): void;
  free(): void;
}

// 一个页面里只初始化一次 wasm 模块;`Preview` 实例可以有多个,但弹窗只开一个。
let wasmOnce: Promise<Wasm> | null = null;

function loadWasm(): Promise<Wasm> {
  wasmOnce ??= (async () => {
    const mod = (await import("@/wasm/rocom_pets.js")) as unknown as Wasm;
    await mod.default();
    return mod;
  })();
  return wasmOnce;
}

export interface Progress {
  /** 已经下了多少字节 / 一共要下多少。 */
  done: number;
  total: number;
  label: string;
}

/**
 * 一次预览会话。`open` 建、`close` 收;中途换形态走 `showForm`。
 *
 * 生命周期都挂在这上面(rAF、ResizeObserver、AbortController、wasm 实例),
 * 弹窗关掉时一次 `close` 全收干净 —— 少收一样,后台就会一直有一个 60fps 的循环在转。
 */
export class PreviewSession {
  private pv: WasmPreview | null = null;
  private zip: RemoteZip | null = null;
  private raf = 0;
  private last = 0;
  private abort = new AbortController();
  private observer: ResizeObserver | null = null;
  private theme: MutationObserver | null = null;
  /** 已经喂进 wasm 的形态,别重复下载。 */
  private loaded = new Set<string>();

  static async open(
    url: string,
    canvas: HTMLCanvasElement,
    onProgress: (p: Progress) => void,
  ): Promise<{ session: PreviewSession; forms: FormEntry[]; faces: string[] }> {
    const session = new PreviewSession();
    const { signal } = session.abort;
    onProgress({ done: 0, total: 1, label: "读取渲染器" });
    const wasm = await loadWasm();
    const pv = new wasm.Preview();
    session.pv = pv;

    await pv.attach(canvas);
    session.watchSize(canvas);
    session.watchTheme(canvas);

    onProgress({ done: 0, total: 1, label: "读取包目录" });
    const zip = await RemoteZip.open(url, signal);
    session.zip = zip;

    const manifest = zip.entries.get("manifest.toml");
    if (!manifest) throw new Error("包里没有 manifest.toml");
    pv.put("manifest.toml", await zip.read(manifest, signal));
    const forms = pv.load_pack().map((f) => ({ asset: f.asset, name: f.name }));
    return { session, forms, faces: wasm.expressions() };
  }

  /** 切到某个形态。第一次会把它那一份资产下下来。 */
  async showForm(asset: string, onProgress: (p: Progress) => void): Promise<ClipEntry[]> {
    const { pv, zip } = this;
    if (!pv || !zip) throw new Error("预览已经关了");
    if (!this.loaded.has(asset)) {
      // 一个形态的东西全在 `forms/<资产>/` 下面 —— glb、贴图、叫声都不越界,
      // 所以按前缀取就够,不必先解 manifest 才知道要哪些文件
      const entries = zip.under(`forms/${asset}/`).filter(keepForRender);
      const total = RemoteZip.bytesOf(entries);
      let done = 0;
      onProgress({ done, total, label: "下载模型与贴图" });
      for (const entry of entries) {
        pv.put(entry.name, await zip.read(entry, this.abort.signal));
        done += entry.compressedSize;
        onProgress({ done, total, label: "下载模型与贴图" });
      }
      this.loaded.add(asset);
    }
    const clips = pv.load_form(asset).map((c) => ({ name: c.name, label: c.label }));
    this.start();
    return clips;
  }

  play(clip: string) {
    this.pv?.play(clip);
  }

  setFace(name: string) {
    this.pv?.set_face(name);
  }

  drag(dx: number, dy: number) {
    this.pv?.drag(dx, dy);
  }

  recenter() {
    this.pv?.recenter();
  }

  close() {
    this.abort.abort();
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    this.observer?.disconnect();
    this.observer = null;
    this.theme?.disconnect();
    this.theme = null;
    this.pv?.free();
    this.pv = null;
    this.zip = null;
  }

  private start() {
    if (this.raf) return;
    this.last = performance.now();
    const tick = (now: number) => {
      const dt = (now - this.last) / 1000;
      this.last = now;
      this.pv?.frame(dt);
      this.raf = requestAnimationFrame(tick);
    };
    this.raf = requestAnimationFrame(tick);
  }

  /**
   * 画布跟着容器走。**按设备像素配**(`devicePixelRatio`),不然在 2 倍屏上是一张
   * 放大的糊图;上限 2 是因为再高对一只巴掌大的宠物已经看不出来,而像素是平方涨的。
   */
  private watchSize(canvas: HTMLCanvasElement) {
    const apply = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
      const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
      if (canvas.width === w && canvas.height === h) return;
      canvas.width = w;
      canvas.height = h;
      this.pv?.resize(w, h);
    };
    apply();
    this.observer = new ResizeObserver(apply);
    this.observer.observe(canvas);
  }

  /**
   * 画布底色跟着主题走。
   *
   * **网页上的 WebGPU 画布只能是不透明的**(wgpu 的后端只报 `Opaque`),清屏色得自己给,
   * 不然深色主题下弹窗中间就是一块纯黑。取的是画布**父元素**算出来的背景色 ——
   * 那正好是弹窗那块底,于是画布和卡片融成一片。主题一切(根节点 class 变)就重取。
   */
  private watchTheme(canvas: HTMLCanvasElement) {
    const apply = () => {
      const host = canvas.parentElement ?? canvas;
      const rgb = toRgb(getComputedStyle(host).backgroundColor);
      if (rgb) this.pv?.set_background(rgb[0] / 255, rgb[1] / 255, rgb[2] / 255);
    };
    apply();
    this.theme = new MutationObserver(apply);
    this.theme.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  }
}

/**
 * 任意 CSS 颜色 → sRGB 三元组。
 *
 * **不要拿正则去抠数字**:Tailwind v4 的调色板是 `oklch()`,Chrome 的
 * `getComputedStyle` 就原样回 `oklch(0.21 0.006 286 / 0.4)` —— 按 `rgb()` 读出来是
 * 0.21/255,画出来近乎纯黑(踩过)。让 2D 画布替我们解析,什么语法都认。
 */
function toRgb(css: string): [number, number, number] | null {
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return null;
  ctx.fillStyle = css;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return [r, g, b];
}

/**
 * 渲染要用的条目。**叫声与动作音效跳过** —— 预览不出声,而那是每个形态 250KB 左右,
 * 占单形态下载量的一成。
 */
function keepForRender(entry: ZipEntry): boolean {
  return !/\/(voice|sfx)\//.test(entry.name);
}
