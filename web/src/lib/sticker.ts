/**
 * 把 wasm 抓回来的帧编成 GIF(表情包)。
 *
 * 帧由 `PreviewSession.capture` **分批**给过来,每批是一整块紧排 RGBA、**预乘 alpha**
 * (整条渲染管线的约定,见 `src/pet/gpu/mod.rs` 里那句 `PREMULTIPLIED_ALPHA_BLENDING`)。
 * 这里先按选的背景还原成直通 alpha,再交给 `gifenc`。
 *
 * **GIF 只有 1 位透明**,所以「透明」那一档必须自己先把 alpha 二值化:
 * 半透的边缘像素要么整个留下、要么整个丢掉。留下的那些还得**去预乘**
 * (`rgb / a`),否则边缘会压着一圈黑 —— 预乘的 rgb 本来就是乘过 alpha 的。
 * 纯色背景那一档没有这个问题:`rgb + 底色 × (1 − a)` 正是预乘合成公式,
 * 边缘能正常羽化,这也是它观感更好的原因。
 */

import type { Palette } from "gifenc";

/**
 * 编码器**按需取**:`gifenc` 只有点了「下载 GIF」才用得上,静态引进来就是让所有人
 * (包括只想下包的)先付这十几 KB。和 wasm、`.rkpet` 同一条规矩。
 */
const encoder = () => import("gifenc");

/**
 * 可挑的帧率。
 *
 * **只放能整除 100 的**:GIF 的帧延迟以**百分之一秒**为单位,`1000/fps` 除不尽的话
 * 每帧都要凑整,一个循环下来会偏几个百分点。20/25/50 对应 5/4/2 厘秒,都是准的。
 *
 * 最高那档是给「存下来收藏、不在乎体积」用的(512 那档一段长动作能到 2MB 上下);
 * 没有低于 20 的档 —— 实机反馈的「掉帧严重」就是低帧率,不该再给一个更差的选项。
 */
export const STICKER_RATES = [20, 25, 50] as const;

/** 默认帧率。 */
export const STICKER_FPS: number = STICKER_RATES[0];

/** 可选的贴纸边长。240 是微信表情那档,512 是 Telegram 那档。 */
export const STICKER_SIZES = [240, 360, 512] as const;

/**
 * 一个循环最多多少帧。**和 `web.rs` 的 `MAX_STICKER_FRAMES` 是同一个数** ——
 * 那边也会夹,这边先算好只是为了帧延迟能跟着调准(见 [`plan`])。
 */
export const MAX_STICKER_FRAMES = 400;

/** 调色板取样的目标像素数。256 色用不着几十万个样本,而量化是这一步里最慢的。 */
const SAMPLE_PIXELS = 300_000;

/** 一批帧:`data` 是紧排的预乘 RGBA,`frames` 是这批实际有几帧。 */
export interface Batch {
  data: Uint8Array;
  frames: number;
}

/** 要第 `first` 帧起的一批。返回的 `frames` 由 wasm 那边按显存预算定。 */
export type Grab = (first: number) => Promise<Batch>;

export interface StickerOptions {
  /** 每帧边长(像素)。 */
  size: number;
  /** 一个循环切成多少帧([`plan`] 算的)。 */
  total: number;
  /** 每帧停多少毫秒([`plan`] 算的,一定是 10 的倍数)。 */
  delayMs: number;
  /** 透明背景;false 时贴到 `background` 那个纯色上。 */
  transparent: boolean;
  /** 纯色背景(0~255)。`transparent` 为 true 时不看。 */
  background: [number, number, number];
  /** 干到哪儿了(0~1),给进度条用。 */
  onProgress?: (ratio: number) => void;
}

/**
 * 「这段动作按这个帧率该切成多少帧、每帧停多久」。
 *
 * **从厘秒倒着算**,而不是先定帧数再除:GIF 的延迟只能是 10ms 的整数倍,
 * 先定帧数的话余数会摊成整个循环的时长误差。这样算出来
 * `总时长 = total × delayMs` 与动作本身最多差半帧。
 *
 * 两条夹子:
 * - **延迟最低 20ms**。1 厘秒(10ms)会被不少渲染器当成「没设」而按 100ms 放
 *   (GIF 的老约定),那反倒成了慢动作。
 * - 帧数顶到 [`MAX_STICKER_FRAMES`] 时**回头把延迟拉长**,于是循环时长仍是这段动作的时长,
 *   只是帧率降下来 —— 截断的话首尾就接不上了。
 */
export function plan(seconds: number, fps: number): { total: number; delayMs: number } {
  let cs = Math.max(2, Math.round(100 / fps));
  let total = Math.max(2, Math.round((seconds * 100) / cs));
  if (total > MAX_STICKER_FRAMES) {
    total = MAX_STICKER_FRAMES;
    cs = Math.max(2, Math.round((seconds * 100) / total));
  }
  return { total, delayMs: cs * 10 };
}

/**
 * 预乘 RGBA → 直通 RGBA(写进 `out`)。
 *
 * alpha 的门限取 128:GIF 只有「透明」和「不透明」两种,盖住一半以上就算数。
 */
function straighten(
  src: Uint8Array,
  offset: number,
  out: Uint8ClampedArray,
  transparent: boolean,
  bg: [number, number, number],
) {
  for (let i = 0; i < out.length; i += 4) {
    const a = src[offset + i + 3];
    if (transparent) {
      if (a < 128) {
        out[i] = out[i + 1] = out[i + 2] = out[i + 3] = 0;
        continue;
      }
      const k = 255 / a;
      out[i] = src[offset + i] * k;
      out[i + 1] = src[offset + i + 1] * k;
      out[i + 2] = src[offset + i + 2] * k;
      out[i + 3] = 255;
      continue;
    }
    const uncovered = (255 - a) / 255;
    out[i] = src[offset + i] + bg[0] * uncovered;
    out[i + 1] = src[offset + i + 1] + bg[1] * uncovered;
    out[i + 2] = src[offset + i + 2] + bg[2] * uncovered;
    out[i + 3] = 255;
  }
}

/**
 * 跨帧取样、量化出**全片共用的一张**调色板。
 *
 * 逐帧各量化一次会让同一块颜色在相邻帧上落到不同的色号,放起来整只宠物在闪 ——
 * 所以量化一次,之后每帧只做 `applyPalette`。
 */
function paletteOf(
  quantize: Awaited<ReturnType<typeof encoder>>["quantize"],
  sample: Uint8ClampedArray,
  transparent: boolean,
): { palette: Palette; format: "rgb565" | "rgba4444"; transparentIndex: number } {
  const format = transparent ? "rgba4444" : "rgb565";
  const palette = quantize(sample, 256, {
    format,
    // 量化器会在 rgba4444 下自己造出中间 alpha,而 GIF 表达不了 —— 直接压成两档
    oneBitAlpha: transparent,
  });
  if (!transparent) return { palette, format, transparentIndex: -1 };
  let transparentIndex = palette.findIndex((c) => c[3] === 0);
  if (transparentIndex < 0) {
    // 整段动作一个透明像素都没有(取景顶满了)——补一格,免得 writeFrame 没得指
    transparentIndex = palette.length < 256 ? palette.length : 255;
    palette[transparentIndex] = [0, 0, 0, 0];
  }
  return { palette, format, transparentIndex };
}

/**
 * 编码。**帧是分两遍抓的**:
 *
 * 1. 第一遍只为凑调色板的取样(跨全片,几十万像素封顶);
 * 2. 第二遍边抓边 `applyPalette`、边写进 GIF 流。
 *
 * 为什么不抓一遍留着:512 那档 400 帧的原始 RGBA 是 420MB,标签页扛不住。
 * 而重抓一遍很便宜(实测 60 帧 512² 回读 103ms),且**结果逐位相同** ——
 * 抓帧是 `seek` 到定点、`time` 冻住、相机不动,没有任何随机量。
 *
 * 每帧之后让出一次主线程,不然进度条一格都不会动。
 */
export async function encodeGif(
  grab: Grab,
  { size, total, delayMs, transparent, background, onProgress }: StickerOptions,
): Promise<Blob> {
  const stride = size * size * 4;
  const frame = new Uint8ClampedArray(stride);
  const breathe = () => new Promise((done) => setTimeout(done));

  // 取样步长按「一共多少像素」算,于是不论多长多大,取样量都在 SAMPLE_PIXELS 上下
  const step = Math.max(1, Math.floor((total * size * size) / SAMPLE_PIXELS));
  const sample = new Uint8ClampedArray(Math.ceil((total * size * size) / step) * 4);
  let at = 0;
  let pixel = 0;
  for (let done = 0; done < total; ) {
    const batch = await grab(done);
    if (!batch.frames) throw new Error("抓帧中断了");
    for (let f = 0; f < batch.frames; f++) {
      straighten(batch.data, f * stride, frame, transparent, background);
      for (let i = 0; i < frame.length; i += 4, pixel++) {
        if (pixel % step) continue;
        sample[at++] = frame[i];
        sample[at++] = frame[i + 1];
        sample[at++] = frame[i + 2];
        sample[at++] = frame[i + 3];
      }
    }
    done += batch.frames;
    onProgress?.((done / total) * 0.35);
    await breathe();
  }

  const { GIFEncoder, applyPalette, quantize } = await encoder();
  const { palette, format, transparentIndex } = paletteOf(
    quantize,
    sample.subarray(0, at),
    transparent,
  );

  const gif = GIFEncoder();
  let written = 0;
  for (let done = 0; done < total; ) {
    const batch = await grab(done);
    if (!batch.frames) throw new Error("抓帧中断了");
    for (let f = 0; f < batch.frames; f++) {
      straighten(batch.data, f * stride, frame, transparent, background);
      gif.writeFrame(applyPalette(frame, palette, format), size, size, {
        // 头一帧带上全局调色板,后面的沿用
        palette: written === 0 ? palette : undefined,
        delay: delayMs,
        repeat: 0,
        transparent,
        transparentIndex: transparent ? transparentIndex : undefined,
        // **透明必须配 dispose = 2**:默认是「留着上一帧」,宠物一动就会拖出一串残影
        dispose: transparent ? 2 : undefined,
      });
      written++;
      onProgress?.(0.35 + (written / total) * 0.65);
      await breathe();
    }
    done += batch.frames;
  }
  gif.finish();
  // `bytes()` 给的是 wasm 之外的普通数组,直接进 Blob
  return new Blob([gif.bytes() as BlobPart], { type: "image/gif" });
}

/** 触发一次下载。`name` 会被清掉文件名里不能有的字符。 */
export function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name.replace(/[\\/:*?"<>|]+/g, "_");
  a.click();
  // 立刻 revoke 会让 Firefox 的下载拿不到内容,推到下一轮
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
