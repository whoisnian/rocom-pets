import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatBytes(n: number): string {
  if (!n) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

export function formatCount(n: number): string {
  if (n < 10000) return String(n);
  return `${(n / 10000).toFixed(1)} 万`;
}

const CN_NUM = ["零", "一", "两", "三", "四", "五", "六", "七", "八", "九", "十"];
export function cnCount(n: number): string {
  return n <= 10 ? CN_NUM[n] : String(n);
}
