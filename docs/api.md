# 本地接口

桌宠客户端(Linux / Windows)可以在本机开一个 HTTP 口,让别的程序控制它:查在场宠物、让它说话、播动作、
冒表情、走动,增删改宠物,改全局设置,开预览与配置窗口。同一个口上有两套协议:

- **REST**:`/v1/...`,JSON 进、JSON 出;
- **MCP**:`/mcp`(Model Context Protocol 的 Streamable HTTP 传输),AI agent 可以直接把它接成工具。

两套协议背后是**同一张工具表**,MCP 的每个工具都对应一条 REST 路由,参数相同、结果相同。
本文的接口名、路由、参数与返回字段是对外契约:后续只增加,不改名、不改含义。网页端没有这个接口。

## 开启

在配置窗口的「常用配置 › 本地接口」里填:

| 设置 | config.toml 键 | 说明 |
|---|---|---|
| 本地监听 | `listen` | 监听地址,如 `127.0.0.1:47831`。不填就不监听。只想给本机用就绑 `127.0.0.1` |
| 允许跨域 | `allow_origin` | 允许哪些网页从浏览器里调:逗号分隔的 Origin(如 `http://localhost:5173, https://a.b`),`*` = 都允许。不填 = 只收不带 `Origin` 的请求 |
| 鉴权令牌 | `api_token` | 每个请求都要带。填了监听地址而令牌空着时自动生成(`rkp_` + 32 位十六进制);可显示、复制、重新生成,也能手填 |

改完立即生效,桌宠不用重启。令牌空着时不开监听(日志里有一行警告)。

## 鉴权与跨域

- **每个请求都要带令牌**:`Authorization: Bearer <api_token>`。缺了或不对回 `401`(带 `WWW-Authenticate: Bearer`)。
  唯一的例外是浏览器的 CORS 预检(`OPTIONS`),它不带令牌。
- **带 `Origin` 的请求**(浏览器发的跨域请求都带)必须在「允许跨域」的单子上,否则一律 `403`,令牌对了也不行;
  预检同样会被挡。通过的响应带 `Access-Control-Allow-Origin: <原样回显>`,预检放行
  `Authorization`、`Content-Type`、`Mcp-Protocol-Version`、`Mcp-Session-Id` 头与 `GET / POST / PATCH / DELETE`。
- **不带 `Origin` 的请求**(curl、脚本、别的程序的后端)只看令牌。

令牌等同于这台桌宠的遥控器:不要写进公开的页面或仓库。重新生成后,旧令牌立即失效。

## 约定

- 请求体是 JSON 对象,`Content-Type: application/json`;不需要参数的写操作可以不带请求体。请求体上限 64 KB。
  参数名拼错、多了不认得的参数会回 `400`。
- 成功回 `200` 与一个 JSON 对象。失败回对应的状态码与:

  ```json
  { "error": { "code": "not_found", "message": "没有 uid 为 abc 的宠物(见 list_pets)" } }
  ```

  程序按 `code` 判断,`message` 是给人看的中文说明。
- **宠物用 `uid` 指认**:每只宠物上桌时随机生成,12 位小写十六进制,之后不变(换形态、换外观、孵化、重启都不变),
  撤下后作废。写在阵容存档 `roster.toml` 里。插槽号 `slot` 只是当前的排序,撤掉前面那只后会变,不要拿它当标识。
- **也可以按插槽指认**:凡是要 `uid` 的地方(路径里的 `{uid}`、参数 `pet`)都可以写 `@<插槽号>`:
  `@0` = 阵容里第一只、`@1` = 第二只,负数从末尾数,`@-1` = 最后一只(比如刚 `add_pet` 加上的那只)。
  给不知道 uid 的简易脚本用(`curl …/v1/pets/@0/say`);插槽号会随增删变,连着几步的调用还是先取 uid 再用。
  `@` 后面不是整数回 `400`,超出只数回 `404`。返回里的 `pet` / `removed` 总是真正的 uid。
- MCP 工具的 `pet` 参数不填时作用于阵容里第一只(`remove_pet`、`update_pet` 必须填)。
- 坐标:屏幕用平台给的显示器名(`DP-1`、`\\.\DISPLAY2`),横向位置是脚底在那块屏上的比例,0 = 左边、1 = 右边;
  `foot` 是脚底在全部屏幕拼成的那张坐标里的逻辑像素。
- 每个连接只处理一个请求(`Connection: close`),同时最多处理 8 个。一个调用在桌宠里最多等 15 秒。

## REST 接口

| 方法 | 路径 | MCP 工具 | 作用 |
|---|---|---|---|
| GET | `/v1`(或 `/`) | `get_info` | 版本、平台、接口版本 |
| GET | `/v1/status` | `get_status` | 全局状态 |
| PATCH | `/v1/settings` | `update_settings` | 改全局设置 |
| POST | `/v1/recall` | `recall_pets` | 把宠物都召回主屏 |
| POST | `/v1/reload` | `reload` | 重读配置与阵容 |
| POST | `/v1/quit` | (无) | 退出桌宠 |
| GET | `/v1/pets` | `list_pets` | 在场宠物 |
| POST | `/v1/pets` | `add_pet` | 加一只 |
| GET | `/v1/pets/{uid}` | `get_pet` | 一只的详情,含动作表与形态 |
| PATCH | `/v1/pets/{uid}` | `update_pet` | 改一只 |
| DELETE | `/v1/pets/{uid}` | `remove_pet` | 撤一只 |
| POST | `/v1/pets/{uid}/say` | `say` | 说一句话 |
| POST | `/v1/pets/{uid}/play` | `play_action` | 播动作 |
| POST | `/v1/pets/{uid}/emote` | `show_emote` | 头顶表情 |
| POST | `/v1/pets/{uid}/move` | `move_pet` | 走过去 |
| GET | `/v1/catalog` | `search_catalog` | 查图鉴 |
| GET | `/v1/packs` | `list_packs` | 已导出的包 |
| GET | `/v1/vocab` | `get_vocabulary` | 各参数的可选值 |
| POST | `/v1/preview` | `open_preview` | 开预览窗口 |
| POST | `/v1/windows/settings` | `open_settings` | 开配置窗口 |

路径对、方法不对回 `405`(带 `Allow`);路径不存在回 `404`(`no_route`)。`GET` 接口的参数放查询串
(`/v1/catalog?q=喵喵&exported=true`),其余放 JSON 请求体;路径里的 `{uid}` 优先于请求体里的 `pet`。

### 全局

**`GET /v1/status`** → 全局状态:

```json
{
  "fps": 30,
  "size_percent": 100.0,
  "volume": 0.6,
  "muted": false,
  "audio": true,
  "passthrough": false,
  "balloon_talk": true,
  "battle_fx": true,
  "pets": 2,
  "screens": [{ "name": "DP-1", "x": 0.0, "y": 0.0, "width": 2560.0, "height": 1400.0 }]
}
```

`fps` 是整数,或 `"display"`(跟随屏幕刷新)。`muted` 在没有音频输出时是 `null`(`audio: false`)。
`screens` 是去掉任务栏后的工作区,逻辑像素。

**`PATCH /v1/settings`**:只改给出的那几项,返回改完后的全局状态。先校验全部参数,有一项不对就一项都不改。

| 参数 | 类型 | 说明 |
|---|---|---|
| `fps` | 整数 10~240 或 `"display"` | 目标帧率,写回配置 |
| `size_percent` | 50~300 | 整体大小百分比,写回配置;台上每只都按新比例重建 |
| `volume` | 0~1 | 叫声音量,写回配置 |
| `muted` | 布尔 | 静音,只管这次运行;没有音频输出时回 `409 no_audio` |
| `passthrough` | 布尔 | 点击穿透(鼠标点不到宠物),只管这次运行 |
| `balloon_talk` | 布尔 | 宠物自己找话说,写回配置。`say` 接口不受它影响 |
| `battle_fx` | 布尔 | 特效(头顶表情、蓄力光芒等),写回配置 |

**`POST /v1/reload`**:`{"packs": true}` 时连包目录一起重扫(刚导出或删除了包之后用)。返回 `{"pets": 在场只数, "packs": 包目录里的包数}`。

**`POST /v1/recall`** → `{"recalled": 只数}`。**`POST /v1/quit`** → `{"quitting": true}`,随后桌宠与配置窗口退出。

### 宠物

**`GET /v1/pets`** → `{"pets": [宠物, …]}`。一只宠物:

```json
{
  "uid": "79d04be37cd4",
  "slot": 0,
  "pack": "002-喵喵",
  "species": "喵喵",
  "form": { "asset": "Gra_MiaoMiao1_001", "name": "喵喵" },
  "pet_ids": [3001],
  "random": false,
  "mutation": "异色+炫彩:3/33",
  "persona": { "id": "peaceful", "name": "平和" },
  "tone": { "id": "cute", "name": "可爱" },
  "scale": 1.0,
  "voice": true,
  "voice_value": 0.0,
  "state": {
    "activity": "待机",
    "asleep": false,
    "dizzy": false,
    "dragging": false,
    "hatching": false,
    "facing": "right",
    "screen": "DP-1",
    "x": 0.5,
    "foot": { "x": 1280.0, "y": 1364.0 }
  }
}
```

- `pack` 是包名(包目录里的目录名去掉扩展名);`pet_ids` 是当前形态对应的游戏宠物 id(`base_conf_id`),清单外的形态为空。
- `random` = 还没孵化的「随机宠物」蛋。`mutation` 原样时是 `null`。
- `state.activity`:待机 / 行走 / 奔跑 / 下落 / 被拎着 / 被甩出去 / 飘下来 / 落定 / 反应 / 入睡 / 睡着 / 醒来。
  「反应」= 正在播动作或表情。

**`GET /v1/pets/{uid}`**:同上,另加

- `actions`:`[{"name": "Happy", "label": "开心"}, …]`,这一形态能播的动作(`label` 按界面语言);
- `forms`:`[{"asset", "name", "current"}, …]`,这条进化链上能切的形态。

**`POST /v1/pets/{uid}/say`** `{"text": "你好"}`:头顶冒对话框说一句。**总是显示**,不受「说话」开关影响;
最多 60 个字,换行等控制字符换成空格;停留 2.5~7 秒(按字数)。新的一句会顶掉正在说的那句。
→ `{"pet", "text", "seconds"}`。

**`POST /v1/pets/{uid}/play`** `{"action": "开心"}`:播一段动作。`action` 认动作名(`Happy`)或标签(`开心`),
见 `get_pet` 的 `actions`。播完自己回待机;睡着的会醒过来做;正在演的那段会被打断。
没有这段动作回 `404`。→ `{"pet", "action", "label"}`。

**`POST /v1/pets/{uid}/emote`** `{"emote": "heart"}`:头顶冒一个表情。可选值:
`heart`(爱心)、`pat`(摸头爱心)、`notes`(音符)、`exclaim`(感叹号)、`question`(问号)、`anger`(青筋)、
`sweat`(汗)、`sad`(三竖线)、`bulb`(灯泡)、`puzzled`(迷惑的圈)、`shout`(呐喊)、`sparkle`(星光)、
`heartbreak`(心碎)、`think`(思考)、`notice`(注意)。特效开关关着时不显示,返回的 `shown` 为 `false`。

**`POST /v1/pets/{uid}/move`** `{"x": 0.3, "screen": "DP-1", "run": false}`:走(`run` = 跑)到那块屏的那个横向位置,
到了回待机。`screen` 不填 = 它现在站的那块。只沿脚下连着的地面走,走不到的停在尽头。
睡着时回 `409 asleep`(先 `play` 一个动作叫醒),被拎着、在半空、孵化中回 `409 busy`,形态没有走路动作回 `409 cannot_walk`。

**`POST /v1/pets`**:加一只上桌,返回它的详情(含新生成的 `uid`)。

| 参数 | 类型 | 说明 |
|---|---|---|
| `pet_id` | 整数 | 游戏宠物 id(`search_catalog` 里形态的 `ids`),和 `pack` 二选一;直接定到那个形态 |
| `pack` | 字符串 | 包名(`002-喵喵`),和 `pet_id` 二选一 |
| `form` | 字符串 | 形态:资产名或中文名;不填 = 链首(或 `pet_id` 指的那个) |
| `mutation` | 字符串 | 外观:`异色`、`炫彩:<粒子id>/<配色id>`、`炫彩:<隐藏款名>`,可用 `+` 连写(`异色+炫彩:黑白`) |
| `persona` | 字符串 | 性格 id 或中文名 |
| `tone` | 字符串 | 说话口吻 id |
| `scale` | 0.5~2 | 相对大小 |
| `voice` | 布尔 | 参与叫声 |
| `screen` | 字符串 | 站到哪块屏,不填 = 主屏 |
| `x` | 0~1 | 脚底的横向位置,不填 = 居中 |

包要先在配置窗口的宠物包页导出过:图鉴里有、但还没导出的回 `409 not_exported`;包目录和图鉴里都没有的回 `404`。

**`PATCH /v1/pets/{uid}`**:改 `form`、`mutation`、`persona`、`tone`、`scale`、`voice`、`voice_value`(−100~100,0 = 原调),
只改给出的那几项;字符串给空串 = 回到默认(`mutation: ""` = 原样)。返回改完后的详情,`uid` 不变。
只改口吻时宠物原地换,不重建;其余改动原地重建(位置、朝向保留)。

**`DELETE /v1/pets/{uid}`** → `{"removed": uid, "pets": 剩下几只}`。

增、改、撤都写回 `roster.toml`;配置窗口开着的话会自动同步过去。

### 图鉴与可选值

**`GET /v1/catalog`**:查宠物链。

| 参数 | 说明 |
|---|---|
| `q` | 关键字:中文名、包名、资产名,或纯数字的宠物 id。不填 = 全部 |
| `scope` | `book`(默认,只查有图鉴号的)或 `all`(连扩展图鉴、NPC、路人) |
| `exported` | `true` = 只要已导出的 |
| `limit` | 最多返回几条,1~500,默认 50 |

→ `{"total": 命中条数, "results": [{"pack", "name", "book", "category", "exported", "forms": [{"asset", "name", "ids"}]}]}`。
`category` 是 `book` / `extra` / `npc` / `peo`;`exported` 为 `true` 的才能 `add_pet`。

**`GET /v1/packs`** → `{"packs": [{"pack", "name", "forms", "category", "stale"}]}`:包目录里的全部包。
`stale` = 旧版导出器导的,能用但可能显示不全,建议重新导出。

**`GET /v1/vocab`** → 性格(`personas`)、口吻(`tones`)、表情名(`emotes`)、隐藏 / 赛季炫彩(`hidden_glassy`,
其 `name` 可写进 `炫彩:<名字>`)、外观写法说明(`mutation`)以及各数值参数的范围。

### 窗口

**`POST /v1/preview`**:开一个宠物预览窗口(看动作、换炫彩、导表情包)。参数:`pet_id` 或 `pack`,可选 `form`;
外观用 `mutation`,或游戏协议里的原样字段 `shiny` / `glass_type`(1 = 常规、2 = 隐藏) / `glass_value`。
→ `{"pack", "form", "mutation"}`。同一种预览窗口只开一个,再调一次是让它换一只。

**`POST /v1/windows/settings`** `{"page": "common"}`:打开配置窗口,`page` 为 `common` / `packs` / `pets`。

## 错误码

| 状态 | `code` | 含义 |
|---|---|---|
| 400 | `bad_request` | 参数缺了、类型不对、取值超范围、请求体不是 JSON 对象 |
| 401 | `unauthorized` | 没带令牌或令牌不对 |
| 403 | `origin_not_allowed` | `Origin` 不在允许跨域的单子上 |
| 404 | `no_route` | 没有这个路径 |
| 404 | `not_found` | 宠物 / 动作 / 屏 / 包不存在 |
| 405 | `method_not_allowed` | 路径对、方法不对 |
| 409 | `not_exported` | 包还没导出 |
| 409 | `no_audio` | 没有音频输出,切不了静音 |
| 409 | `asleep` / `busy` / `cannot_walk` | `move` 走不了的三种情况 |
| 415 | `unsupported_media_type` | 带了请求体却不是 `application/json` |
| 500 | `internal` | 桌宠内部出错(详见桌宠日志),如加的宠物上不了台 |
| 503 | `overloaded` | 同时处理的请求太多 |
| 503 | `unavailable` | 桌宠正在退出或重启 |
| 503 | `no_roster_file` | 定不出阵容存档的位置,改不了阵容 |
| 504 | `timeout` | 桌宠 15 秒内没处理完 |

## MCP

端点 `http://<监听地址>/mcp`,Streamable HTTP 传输,同样要带 `Authorization: Bearer <令牌>`。

- 只收 `POST`,一律回 `application/json`(不开 SSE 流);`GET /mcp` 回 `405`。无会话,不发 `Mcp-Session-Id`。
- 支持 `initialize`(协议版本 `2025-11-25` / `2025-06-18` / `2025-03-26` / `2024-11-05`,
  客户端要的不在其中时回最新的那个)、`ping`、`tools/list`、`tools/call`;只回 JSON-RPC 请求,通知回 `202`。
- 工具即上面表里 MCP 一列的那些(没有 `quit`),`inputSchema` 与 REST 参数相同。
  结果同时给 `structuredContent`(对象)和一段 JSON 文本;工具执行失败(宠物不存在、包没导出……)按规范放进结果里的
  `isError: true`,文本是 `错误码: 说明`。工具名写错或参数不是对象才回 JSON-RPC 错误。
- 工具带 `readOnlyHint` / `destructiveHint` 标注;`remove_pet` 标为有破坏性。

接入 Claude Code:

```sh
claude mcp add --transport http rocom-pets http://127.0.0.1:47831/mcp \
  --header "Authorization: Bearer rkp_…"
```

支持 HTTP 传输的其他客户端,一般这样配置:

```json
{
  "mcpServers": {
    "rocom-pets": {
      "type": "http",
      "url": "http://127.0.0.1:47831/mcp",
      "headers": { "Authorization": "Bearer rkp_…" }
    }
  }
}
```

## 示例

```sh
T=rkp_…   # 常用配置 › 本地接口 › 鉴权令牌
B=http://127.0.0.1:47831

# 在场的宠物与它们的 uid
curl -s -H "Authorization: Bearer $T" $B/v1/pets

# 让它说话、冒爱心、播「开心」
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"text":"主人,该休息啦"}' $B/v1/pets/79d04be37cd4/say
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"emote":"heart"}' $B/v1/pets/79d04be37cd4/emote
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"action":"开心"}' $B/v1/pets/79d04be37cd4/play

# 不查 uid,直接按插槽:第一只说话、最后一只冒问号
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"text":"早上好"}' $B/v1/pets/@0/say
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"emote":"question"}' $B/v1/pets/@-1/emote

# 加一只异色的(宠物 id 来自 search_catalog),再撤掉
curl -s -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"pet_id":3001,"mutation":"异色"}' $B/v1/pets
curl -s -X DELETE -H "Authorization: Bearer $T" $B/v1/pets/<新的 uid>

# 调大一点、静音
curl -s -X PATCH -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"size_percent":150,"muted":true}' $B/v1/settings
```

浏览器里调用(页面的 Origin 要先填进「允许跨域」):

```js
const res = await fetch("http://127.0.0.1:47831/v1/pets", {
  headers: { Authorization: `Bearer ${token}` },
});
const { pets } = await res.json();
```
