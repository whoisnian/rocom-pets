import { cn } from "@/lib/utils.ts";
import type { SpriteSheet } from "../../shared/types.ts";

/** 名字 → 稳定的色相。没有头像的宠物用首字 + 这个底色,不至于一片灰。 */
function hueOf(name: string): number {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return h % 360;
}

interface Props {
  name: string;
  /** 精灵图序号,行主序;null 表示游戏里没给这只出头像图标 */
  sprite: number | null;
  sheet: SpriteSheet;
  /** 显示边长(px) */
  size?: number;
  className?: string;
}

export function PetAvatar({ name, sprite, sheet, size = 56, className }: Props) {
  const shared = "shrink-0 rounded-full ring-1 ring-border/70 bg-muted";

  if (sprite === null) {
    const hue = hueOf(name);
    return (
      <div
        className={cn(shared, "grid place-items-center font-semibold select-none", className)}
        style={{
          width: size,
          height: size,
          fontSize: size * 0.4,
          background: `oklch(0.9 0.07 ${hue})`,
          color: `oklch(0.36 0.13 ${hue})`,
        }}
        aria-hidden
      >
        {name.slice(0, 1)}
      </div>
    );
  }

  // 源图 cols 列、每格 cell px。缩放到 size 时整张图跟着按同一比例缩,
  // 于是背景尺寸 = (cols * size, rows * size),偏移 = -(列 * size, 行 * size)。
  // **行数得自己算**:图不一定是正方的(591 张 = 25 列 × 24 行),
  // 拿 cols 当高度会把竖向拉伸,越靠下的头像偏得越多。
  const rows = Math.ceil(sheet.count / sheet.cols);
  const col = sprite % sheet.cols;
  const row = Math.floor(sprite / sheet.cols);
  return (
    <div
      className={cn(shared, "sprite-cell", className)}
      style={
        {
          width: size,
          height: size,
          "--sp-img": `url(${sheet.url})`,
          "--sp-bg": `${sheet.cols * size}px`,
          "--sp-bh": `${rows * size}px`,
          "--sp-x": `${-col * size}px`,
          "--sp-y": `${-row * size}px`,
        } as React.CSSProperties
      }
      aria-hidden
    />
  );
}
