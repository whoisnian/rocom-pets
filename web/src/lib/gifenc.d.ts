/**
 * `gifenc` 没带类型声明,这里补一份**只覆盖我们用到的那三样**的。
 * 上游 API 见 node_modules/gifenc/README.md。
 */
declare module "gifenc" {
  /** 调色板:每项是 `[r, g, b]` 或 `[r, g, b, a]`,取决于 `format`。 */
  export type Palette = number[][];

  export function quantize(
    rgba: Uint8Array | Uint8ClampedArray,
    maxColors: number,
    options?: {
      format?: "rgb565" | "rgb444" | "rgba4444";
      /** 量化完把 alpha 一律推到 0 或 255;给数字就是自定阈值。 */
      oneBitAlpha?: boolean | number;
      clearAlpha?: boolean;
      clearAlphaThreshold?: number;
      clearAlphaColor?: number;
    },
  ): Palette;

  export function applyPalette(
    rgba: Uint8Array | Uint8ClampedArray,
    palette: Palette,
    format?: "rgb565" | "rgb444" | "rgba4444",
  ): Uint8Array;

  export interface GifStream {
    writeFrame(
      index: Uint8Array,
      width: number,
      height: number,
      options?: {
        palette?: Palette;
        first?: boolean;
        transparent?: boolean;
        transparentIndex?: number;
        /** 毫秒。GIF 里存的是百分之一秒,所以实际会被取整到 10 的倍数。 */
        delay?: number;
        /** 0 = 一直循环,-1 = 只放一遍。 */
        repeat?: number;
        /** GIF 的 disposal 方式;2 = 放完这一帧先擦回背景。 */
        dispose?: number;
      },
    ): void;
    finish(): void;
    bytes(): Uint8Array;
  }

  export function GIFEncoder(options?: {
    auto?: boolean;
    initialCapacity?: number;
  }): GifStream;
}
