# web · 下载站

应用本体与宠物包的下载页,整站跑在 Cloudflare 上:

| 件 | 干什么 |
| --- | --- |
| **Workers**(Static Assets) | 出静态页,并接管 `/api/*` |
| **R2** | 存 `.rkpet` 与应用本体 |
| **D1** | 记下载次数与异常标记次数 |
| **KV** | 按 IP + 日期去重,防止刷计数 |

前端是 Vite + React 19 + TypeScript + Tailwind v4,组件按 shadcn/ui 的路子直接抄进
`src/components/ui/`(那套东西本来就是拿来抄的,省一个依赖)。

**素材版权属原发行方。** 这个目录只有代码 —— `catalog.json` 与 `sprite.webp` 都是生成物,
和宠物包本身一样不入仓库(见根 `README.md` 与 `docs/design.md` §11)。要上线得自己出包、
自己传 R2。

## 一次性准备

```sh
npm install

wrangler d1 create rocom-pets           # 把返回的 uuid 填进 wrangler.jsonc
wrangler kv namespace create DEDUPE     # 把返回的 id   填进 wrangler.jsonc
wrangler r2 bucket create rocom-pets

wrangler d1 execute rocom-pets --remote --file schema.sql
wrangler secret put DEDUPE_SALT         # 随便一串随机字符
```

**给 R2 挂自定义域**(桶设置 → Custom Domains,比如 `files.你的域`),再把它填进
`wrangler.jsonc` 的 `PUBLIC_BASE`。这一步不能省:

- 官方明说 `*.r2.dev` **不用于生产** —— 有可变速率限制,超了返 429,带宽也会被限流;
- 走自定义域才进你自己的 zone,才有边缘缓存、WAF 与速率限制规则可用;
- 配上之后 `/api/dl/:id` 只回一个 302,**字节不经过 Worker**:Range、断点续传、
  CDN 缓存全部由 R2 原生处理,Worker 的每日请求配额也不会被下载量吃掉。

没配 `PUBLIC_BASE` 时 Worker 会自己从 R2 读字节回给客户端(Range 与条件请求都实现了),
本地开发够用,生产不要这么跑。

## 出目录、传文件、上线

```sh
# 1. 扫包目录,算 sha256,读 manifest 里的形态构成,顺便从解包数据拼头像精灵图
npm run catalog -- --packs ~/Downloads/rocom/packs-all \
                   --apps  ~/Downloads/rocom/dist-bin --version 0.1.0

# 2. 传 R2。用 rclone(配 R2 的 S3 endpoint),不要用 wrangler r2 object put ——
#    它一次只传一个,而且会把中文对象键 percent-encode,和 Worker 查的键对不上。
rclone copy ~/Downloads/rocom/packs-all r2:rocom-pets/packs/ --progress
rclone copy ~/Downloads/rocom/dist-bin     r2:rocom-pets/app/0.1.0/ --progress

# 3. 上线
npm run deploy
```

`gen_catalog.py` 还有个**演示模式**,给没有那 1.6GB 包的机器用 —— 从
`docs/petindex.md` 的清单造一份只有名字、没有大小与哈希的目录,页面会把这些条目
标成「待上传」:

```sh
npm run catalog -- --index
```

## 本地开发

```sh
cp .dev.vars.example .dev.vars
npm run catalog -- --index                       # 先得有 catalog.json
npm run db:init:local                            # 建本地 D1 表
npm run dev                                      # Vite + workerd,/api/* 是真的
```

`npm run build` 会先跑 `tsc --noEmit` 再打包,两步都过才算过。

`npm run dev` **不跑 `npm run wasm`**(只有 `build` 跑)。改了 `src/web.rs` 或清过
`src/wasm/`,得自己先出一遍,否则点开预览时那个动态 import 会 404。

### 本地要看预览,得先往本地 R2 里塞包

演示模式的目录里 `sha256` 是空的,前端据此把这些条目判成「待上传」,预览、下载、
标记异常三个按钮一起禁掉;本地 R2 也是空的,`/api/preview/:id` 只会回
`404 R2 里没有 packs/….rkpet`。所以要真在浏览器里转一圈,得有真目录 + 本地对象:

```sh
npm run catalog -- --packs ~/Downloads/rocom/packs-all --version 0.1.0   # 带 size/sha256

# dev 开着,往它那份本地 R2 里灌。键里的 / 必须写成 %2F(不然按路径分段,404),
# 中文原样即可
API=http://localhost:5173/cdn-cgi/local/explorer/api/r2/buckets/rocom-pets/objects
for f in ~/Downloads/rocom/packs-all/*.rkpet; do
    curl -s -o /dev/null -X PUT --data-binary @"$f" "$API/packs%2F$(basename "$f")"
done
```

**别用 `wrangler r2 object put --local`**:它回显的是解码后的名字(`packs/002-喵喵.rkpet`),
落进 miniflare 的键却是 percent-encode 过的 `packs/002-%E5%96%B5%E5%96%B5.rkpet` ——
和 Worker 拿 `catalog.json` 里 `key` 去查的对不上,照样 404(和上面 R2 那节是同一个毛病,
`--local` 躲不掉;把键预先编码再传也没用,存进去还是同一个编码后的键)。
上面那个 Local Explorer API 存的是字面 UTF-8 键,目录不用动。

**rclone 在这儿用不上** —— 它要的是 S3 endpoint,而本地 R2 根本不是服务,就是
`.wrangler/state/v3/r2/` 下的 sqlite + blobs;rclone 的 http backend 又只读。R2 那节里
用 rclone 是往**真桶**传,和这里两码事。

Local Explorer 是 `@cloudflare/vite-plugin` 的 dev 期接口,`X_LOCAL_EXPLORER` 默认开
(设 `X_LOCAL_EXPLORER=false` 关掉);miniflare 那侧的选项名叫 `unsafeLocalExplorer`,
插件升级时这套路由可能变。同一个 API 也能列/删:

```sh
curl -s "$API?prefix=packs/"                                  # 列
curl -s -X DELETE -H 'Content-Type: application/json' \
     -d '["packs/002-喵喵.rkpet"]' "$API"                      # 批量删(键放 body,原样 UTF-8)
```

写进去立刻生效,不用重启 —— dev 起的 workerd 读的就是这份状态。但 Worker 侧的 catalog
缓存 5 分钟,**改了 `catalog.json`** 没立刻生效的话等一会儿或重启 dev。

预览还要浏览器支持 WebGPU;检测不到 `navigator.gpu` 时那个 chunk 根本不加载,弹窗里只有
一句说明。

## 接口

| 路由 | 说明 |
| --- | --- |
| `GET /api/stats` | `{ id: { downloads, reports } }`,边缘缓存 60 秒 |
| `GET /api/config` | 前端要的 Turnstile sitekey 与「是否直连」 |
| `GET /api/dl/:id` | 计数后 302 到 R2(或代理字节);id 就是 `002-喵喵` / `app-windows-x64` |
| `POST /api/report` | `{ id, reason, note?, token? }`,记异常标记 |

`/api/dl/:id` **不接受客户端传对象键** —— Worker 自己读 `catalog.json` 把 id 解析成 R2 key
并缓存 5 分钟。让客户端传键等于把整个桶开放给任意路径。

## 计数是怎么防刷的

三层,各管各的:

1. **KV 去重** —— `sha256(盐 + IP + 日期)` 当键,同一 IP 当天对同一个 id 只计一次。
   **原始 IP 不落盘**,KV 里只有哈希,异常标记明细也只留哈希前 12 位。
   键的 TTL 到当天 UTC 结束,自己过期,不用清理。
2. **异常标记的 IP 日配额** —— 全站每天 20 次,和上面那条是两码事(去重管「同一个包」,
   配额管「一个人一天能标多少个包」)。
3. **Turnstile**(可选)—— 配了 `TURNSTILE_SITEKEY` + `TURNSTILE_SECRET` 才启用。

D1 的自增走 `INSERT … ON CONFLICT DO UPDATE SET downloads = downloads + 1`,**单语句原子**。
读出来加一再写回会在并发下丢计数 —— 这也是计数没用 KV 的原因,KV 没有原子自增,
而且同一个键有每秒 1 次写的软限制,热门包正好撞上。

**没有做的事**:Cloudflare 自带的无限量 DDoS 缓解拦的是流量形态异常,拦不住「有人写个循环
一直点下载」。真被盯上了,在 `/api/*` 上加一条速率限制规则;**别在下载域上开 Bot Fight Mode**,
它会挑战 `wget` / `curl` / `aria2c`,正好把下大文件的人全挡了。

## 预览

卡片与详情页上的「预览」点开是一块 WebGPU 画布,里面跑的是**桌面版那份运行时**:
`src/` 的渲染与动画代码编成 wasm(`src/web.rs` 是那层胶水),动作清单、降级规则、
表情图集全来自同一份源码,不是照着做的第二套。

**点开才付钱**:wasm 是动态 `import` 的,单独一个 chunk(1.4MB,brotli 后约 380KB);
`.rkpet` 也是点开才按 HTTP Range 取,而且**只取当前那个形态**(`forms/<资产>/` 前缀,
中位 2.9MB)—— 不是整包(中位 6.8MB)。首屏与只想下载的人一个字节都不多付。

- `src/lib/rkpet.ts` —— zip over HTTP Range:尾部找 EOCD → 中央目录 → 按条目取字节 →
  `DecompressionStream('deflate-raw')` 解。**不用现成 zip 库**:它们都要先拿到整个文件。
- `src/lib/preview.ts` —— 会话:装 wasm、喂资产、rAF 循环、跟随画布尺寸与主题色。
- `src/components/PreviewDialog.tsx` —— 弹窗:形态/表情下拉 + 动作按钮 + 相机操作。
  相机绑定照 `OrbitControls` 那套:左键拖转视角,右键 / 中键 / Shift+左键拖平移,
  滚轮缩放(0.5×~5×);触屏单指转、双指同时缩放与平移。「复位」把角度、缩放、平移一起还原。
- 取包走 `/api/preview/:id`,和 `/api/dl/:id` 分开:**预览不计下载数**,也不 302 到
  R2 自定义域(`fetch` 跨源要 CORS,自己代理反而简单)。

出 wasm 要 `rustup target add wasm32-unknown-unknown` 与 `cargo install wasm-bindgen-cli`
(版本对齐 `Cargo.lock` 里的 `wasm-bindgen`;版本不对时它会直接告诉你该装哪个)。
`npm run build` 会先跑 `npm run wasm`,产物在 `src/wasm/`(生成物,不入仓库);
`npm run dev` 不跑,本地怎么把预览跑起来见〈本地开发〉。

**只支持 WebGPU**:骨骼矩阵走只读 storage buffer,WebGL2 没有这东西。
检测不到 `navigator.gpu` 就不加载那个 chunk,弹窗里给一句说明,下载照旧。

## 头像

**自己从解包数据拼**,不引外部仓库的成品图。游戏自带 `Icon/HeadIcon/<conf_id>.png`
(128px 一张,解包树里 634 张),而 manifest 里 `[species].id` 与 `[[forms]].id` 就是
conf_id —— **按 id 直接对上**。用得上的那些拼成一张 webp(当前 573 张,24 列 × 128px,
1.9MB),其余不进图:多拼一张就是白让访客下载一张的字节。

按 id 对比按中文名好在两处:同名不同图鉴号的宠物各有各的头像(按名字只能给一个),
以及不受「哪些宠物被收录过」的限制。**实测命中率:包 196/201、形态 573/607**
(此前按中文名查外部表是 168/201 与 369/607)。剩下的是游戏里本来就没出图标的,
记 `null`,前端退回「首字 + 按名字哈希出来的底色」,不留空位。

没有解包数据也能跑:`--parsed` 指不到就整站文字头像,不报错。
