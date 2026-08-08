/** 前端与 Worker 共用的类型。catalog.json 由 scripts/gen_catalog.py 生成。 */

export interface PetForm {
  /** 形态名,如「魔力猫」 */
  name: string;
  /** 资产目录名,如 Gra_MiaoMiao3_001;同名多外观时取第一个 */
  asset: string;
  /** 进化阶段,王者形态记 99 */
  stage: number;
  /** 外观数;>1 时页面显示 xN */
  skins: number;
  /** 在 sprite.webp 中的序号,行主序;没有头像的记 null */
  sprite: number | null;
}

export interface Pack {
  /** 「002-喵喵」—— D1 主键、R2 key 主体、URL 锚点三处共用 */
  id: string;
  /** 图鉴号,补足三位;没有图鉴号的一律「000」 */
  book: string;
  /** 链首名 */
  name: string;
  /** R2 对象键,如 packs/002-喵喵.rkpet */
  key: string;
  size: number;
  sha256: string;
  forms: PetForm[];
  /** 包头像 = 链首形态的头像 */
  sprite: number | null;
}

export interface AppBuild {
  /** 「app-windows-x64」 */
  id: string;
  platform: "windows" | "linux";
  /** 「Windows 10+ (x64)」 */
  label: string;
  key: string;
  filename: string;
  size: number;
  sha256: string;
  version: string;
  note?: string;
}

export interface SpriteSheet {
  url: string;
  /** 列数 */
  cols: number;
  /** 源图格子边长(px) */
  cell: number;
  count: number;
}

export interface Catalog {
  generated_at: string;
  /** 宠物包的 source_version(游戏数据版本),取众数 */
  source_version: string;
  sprite: SpriteSheet;
  apps: AppBuild[];
  packs: Pack[];
}

export interface AssetStat {
  downloads: number;
  reports: number;
}

/** GET /api/stats 的响应:id → 计数 */
export type StatsResponse = Record<string, AssetStat>;

export const REPORT_REASONS = [
  { value: "download", label: "下载失败 / 文件损坏" },
  { value: "hash", label: "sha256 对不上" },
  { value: "model", label: "模型或贴图不对" },
  { value: "anim", label: "动作异常" },
  { value: "voice", label: "叫声缺失或错位" },
  { value: "crash", label: "导入后运行时崩溃" },
  { value: "other", label: "其他" },
] as const;

export type ReportReason = (typeof REPORT_REASONS)[number]["value"];
