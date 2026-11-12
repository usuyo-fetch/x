# usuyo X（推特）获取插件

给 [usuyo](https://github.com/usuyo/usuyo) 用的获取插件：把你在 X 上存过的帖子，按链接取下正文、图片、视频和引用的帖子，存到你自己的电脑上，做个人备份和离线阅读。**不用你的账号。**

插件是一个独立的命令行程序，按 usuyo 插件协议和宿主对话；你一般不会直接运行它。设计在 usuyo 仓库的 `docs/platforms/x.md`。

<a id="它做什么"></a>
## 它做什么

- **认链接**：`x.com`、`twitter.com`（可带 `www.`、`mobile.`、`m.`）上的 `<作者>/status/<id>`、`i/web/status/<id>`、`statuses/<id>`，后面可以跟 `/photo/<n>`、`/video/<n>`；`fxtwitter.com`、`fixupx.com` 上同样路径的链接只离线认出 id，从不请求这两个主机。手机分享带的 `?s=…&t=…` 不进身份，也从不发出去：插件的请求只用帖子 id 拼。`t.co` 短链只请求 `t.co`、只看它跳去哪里，不带 cookie。
- **取一条帖子**：先用访客令牌调帖子接口 `TweetResultByRestId`（插件自己 `POST guest/activate.json` 换来的，不是账号，缓存一小时）；帖子接口被限流或令牌被拒时，退到给嵌入帖子用的 syndication 接口。两条路都不带 cookie，不生成签名或 `x-client-transaction-id`。走的是哪一个，写在 `meta.json` 的 `api` 里，退的原因写在 `fallbackBecause`。
- **存下**：正文（长帖取全文，`t.co` 按回答里的 `expanded_url` 展开，不去请求 `t.co`）、原图（`name=orig`）、视频和动图（码率最高的 mp4，不取 m3u8）、引用的帖子（一层，写成引用块，它的图和视频一起存）。这条是作者回复自己的，就往上取同一作者的串，最多 20 条，每条隔 1 秒。链接指向转帖时，存被转的原帖。
- **如实说缺了什么**：没有视频、某张图没下载下来、长帖只拿到前一段、串只取到几条、文章正文要登录，都写进 `missing` 和 `missingReason`。
- **读官方归档**：`list` 读你从 X 设置里申请下载的 `twitter-*.zip`，只读里面的 `data/like*.js` 和 `data/bookmark*.js`，每条只交链接，内容照样由上面的 `fetch` 不带账号去取。不联网，加入时间归档里没有，不填。
- **输出目录**：

```
<out>/
  post.json            接口的原样回答（以后解析改进了可以重算）
  thread/<id>.json     往上取到的同一作者的前几条，原样
  meta.json            作者、发布时间、互动数、走的哪个接口、引用和串的 id、每个媒体的原地址、没拿到的和原因
  images/001.jpg       图片；引用帖子的是 images/q-001.jpg
  media/video-001.mp4  视频和动图；引用帖子的是 media/q-video-001.mp4
```

<a id="它不做什么"></a>
## 它不做什么

- 不登录，不读 cookie，不读宿主交来的任何账号凭据（测试里把 `USUYO_CREDENTIALS` 一类指向命名管道，跑一遍 `fetch`，一次也没被打开）。
- 不复现签名、不生成 transaction id，不装成登录着的客户端。
- 不回 `login-required`：要登录才看得到的帖子（成人内容 `NsfwLoggedOut`、`NsfwViewerHasNoStatedAge`，受保护账号 `Protected`）回 `not-found`、`reason: private`，链接先存着。
- 不取 Spaces、直播（`unsupported-input`），不取评论、投票结果，不取一条帖子**后面**的串（要 `TweetDetail`，访客令牌下行不行没核）。
- 不走 FxTwitter 一类第三方转发服务：它们用自己的账号去取，等于把你的链接交给别人的账号。
- 不自己拼媒体地址：只下载回答里给的、在 `pbs.twimg.com`、`video.twimg.com` 上的；别的写进原因。
- 不取你没要的：宿主只要文字时，图和视频都不下载。
- 不循环重试：缓存里的访客令牌被说不对时换一个新的再试**一次**，别的失败只请求一次。

<a id="要不要账号"></a>
## 什么要账号、什么不要

| 哪一样 | 要不要账号 | 怎么取 |
|---|---|---|
| 单条：正文、图片、视频、引用的帖子 | 不要 | 插件的 `fetch` |
| 单条：前面的串（作者回复自己的） | 不要 | 同一次 `fetch` 里往上取 |
| 单条：后面的串、长文章正文、成人内容、受保护账号的帖子 | 要登录着的页面 | usuyo 还收不了，链接先存着 |
| 列出书签、喜欢 | 账号只在你自己的浏览器里 | 下面的油猴脚本交链接；或读官方归档 |

<a id="节奏"></a>
## 节奏

清单里给宿主的建议是 `batch: true`、`interval: 10s`、`daily: 300`（推断值）：同一平台两次取之间至少隔 10 秒，一天最多 300 次批量调用（你手动收下的单条不算）。往上取串时一次 `fetch` 里最多 20 个请求，算一次调用。不带账号，风险在出口 IP 上：访客令牌被拒、被限流。试的时候固定走你平时上 X 的那一个出口，别中途换节点。

<a id="油猴脚本"></a>
## 油猴脚本：把你的书签和喜欢交给 usuyo

`userscript/usuyo-x-saved.user.js` 在你**日常用的浏览器**里跑，截获的做法照 [prinsss/twitter-web-exporter](https://github.com/prinsss/twitter-web-exporter)（MIT）：只看 X 页面自己请求回来的 `Bookmarks`、`Likes` 时间线。

- 不发任何额外请求，不自动滚动，不读 cookie，不写页面的 localStorage；
- 书签只有你自己看得到，照记；喜欢只记**你自己**的：请求里的 `userId` 要等于页面里登录的那个人（读页面的 `__INITIAL_STATE__.session.user_id`），认不出就不记；
- 记下的是帖子 id、作者的 `screen_name`、正文第一行，存在脚本管理器里。X 的链接不过期，记下的留到交出去或你点「清空」；
- 点「交给 usuyo」时只连本机的 `http://127.0.0.1:47615`（`@connect 127.0.0.1`，经脚本管理器的 `GM_xmlhttpRequest`，5 秒超时）。交的那一段和小红书插件 0.2.1 的脚本逐字相同（测试核对）：先握手，发一个新的随机数和令牌的 id，对面要用整个令牌签回 HMAC-SHA256 证明，脚本用自己存着的令牌核对；核对不过就什么都不发。令牌只存在脚本管理器里，从浏览器自己的对话框或脚本管理器的菜单填，页面上没有输入框；碰令牌的每一步只用下标、字符串拼接和一开始就拿住的函数，测试里把 `String`、`RegExp`、`Function` 的原型方法换成会记账的间谍也没看到令牌（这只在脚本先于页面脚本跑起来时有用）。核对过了才带上令牌，书签一批（`favorite`）、喜欢一批（`like`）交给 `POST /v1/batches`，每条是 `https://x.com/<作者>/status/<id>` 加标题。交出去的从脚本里去掉。

用法：

1. 装一个脚本管理器（Chrome、Edge、Firefox 上用 Violentmonkey 或 Tampermonkey；Safari 上用 Userscripts，并让它能访问所有网站），新建脚本，把 `usuyo-x-saved.user.js` 的内容贴进去保存。Tampermonkey 第一次连本机时可能问要不要放行 `127.0.0.1`，选允许。各脚本管理器带什么 `Origin`，见小红书插件 README 的「脚本管理器带什么 Origin」，这里一样。
2. 在 usuyo 的导入页，「交进来的清单」里点「发一个令牌」，复制它。这个令牌只能交链接。
3. 打开 `x.com/i/bookmarks` 或你自己主页的「喜欢」。右下角会出现 `usuyo · 书签 0 · 喜欢 0`。**自己往下翻**，翻到的会记下来；一页里没有新帖子时显示「已到底」。出现「X 回了错误 …，先别往下翻了」就停下，过一阵再继续。
4. 点「交给 usuyo」，第一次贴令牌。交完写「交给 usuyo 了：书签 N 条、喜欢 M 条」，回 usuyo 的导入页确认，确认了才按节奏去取。

交不成时角落里照实说：usuyo 没有开着；端口上的程序没能证明自己是 usuyo（令牌没有发出去）；usuyo 不认这个令牌；usuyo 没让这个脚本管理器的请求进来。没交的条目留着。

<a id="脚本的测试"></a>
### 脚本的测试

```
USUYO_BIN=<编好的 usuyo> USUYO_TEST_PORT=47704 node --test userscript/test/userscript.test.mjs
```

测试只连本机：一个假的 X 答两种时间线（形状照 twitter-web-exporter 的类型手写），脚本的 `GM_*`、`unsafeWindow`、`document` 都是假的，端口那一行（`const PORT = 47615;`）载入时换掉。给了 `USUYO_BIN` 时另起一个一次性的 usuyo（端口 47704，家目录是新的临时目录，测完停掉并送进废纸篓），没给就跳过对着 usuyo 的几条。

<a id="只有你能核对的"></a>
## 只有你能核对的

插件和脚本都只对着手写的样例测过，没有向 X 的任何主机发过请求。按这个顺序看（步骤和注意事项照 usuyo 仓库 `docs/platforms/README.md` 第 4 节）：

1. **先测前提**：在你平时上 X 的那个出口上，粘一条公开的带图帖子。插件那一行要是写「平台挡住了这个插件」，就是访客令牌和嵌入接口都被拒了，停下告诉开发这边。看 `meta.json` 的 `api` 是 `TweetResultByRestId` 还是 `syndication`。
2. 再粘一条长帖、一条带视频的引用、一条串的最后一条：长帖全文有没有、视频和动图下不下得来（`video.twimg.com`、`pbs.twimg.com` 不带 cookie）、串往上取到了几条。
3. 一条成人内容的帖子：应停在「只有链接」，原因是要登录。
4. 一个 `t.co` 短链：认不认得出（`t.co` 回的是重定向还是带跳转的页面没核，两种都认）。
5. 装脚本，到书签页、喜欢页各翻一屏，交十条：计数涨不涨、喜欢页认不认得出你的账号（认不出会写「没认出登录的账号」，说明 `__INITIAL_STATE__.session.user_id` 不对，改 `ownUserId` 一处）、导入页的条数对不对。
6. 申请一份官方归档：文件名是不是 `twitter-*.zip`，里面有没有 `data/bookmark.js`，`like.js` 每条有哪些字段（插件只读 `tweetId`、`fullText`，在 `src/archive.rs` 的 `record` 一处）。
7. 取了几十条以后看有没有被限流；`queryId` 过期时会报「X 的接口改了」，改 `src/upstream.rs` 一处。

<a id="开发"></a>
## 开发

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

测试只连本机：换令牌、帖子接口、嵌入接口、`t.co`、图片视频各一个假服务，回答是 `tests/fixtures/` 里照上游源码手写的样例，每个样例文件写明形状出自哪个项目的哪个文件。正式运行只认 `Site::x()` 的主机名单；测试用的 `Site::loopback()` 只接受回环地址。X 改了接口参数，只改 `src/upstream.rs`；回答的形状变了，只改 `src/post.rs`。

依赖 usuyo 仓库里的 `usuyo-plugin-kit` crate（协议类型和发请求的客户端：每个请求只连名单里那一类主机、只连公网地址），按路径引用，要和 usuyo 仓库放在同一个上级目录下；`tests/userscript.rs` 另读旁边的小红书插件的脚本做对照。

许可证：MIT。
