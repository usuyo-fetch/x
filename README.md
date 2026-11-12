<a id="usuyo-fetchx"></a>
# usuyo-fetch/x

X（Twitter）的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按帖子链接取回内容，列出账号存下的帖子。

同一个平台有多种取法，层级、是否使用账号、已记录的风险各不相同，逐一列在下面。已实现的随插件安装；未实现的写明做法和所等的条件。

- 版本：0.1.0，开发中。
- 验证：测试全部离线，样例照上游项目的源码手写。插件尚未在 X 的线上环境运行。

<a id="方法"></a>
## 方法

| 方法 | 取得 | 账号 | 层级 | 现状 |
|---|---|---|---|---|
| [访客接口](#访客接口) | 单条帖子：正文、图片、视频、被引用的帖子、作者自回的上文 | 不使用 | `public-web` | 已实现 |
| [嵌入接口](#嵌入接口) | 单条帖子，访客接口不可用时 | 不使用 | `public-web` | 已实现 |
| [浏览器脚本](#浏览器脚本) | 书签、喜欢的链接 | 只在使用者的浏览器内 | `browser-session` | 已实现 |
| [官方归档](#官方归档) | 喜欢、书签的链接 | 不使用 | `official-export` | 已实现 |
| [内容投递](#内容投递) | 登录后才可见的帖子、帖子的下文、长文章 | 只在使用者的浏览器内 | `browser-session` | 未实现 |
| [会话接口](#会话接口) | 书签、喜欢，可定时取新的 | 登录进 usuyo | `session-api` | 未实现 |
| [签名接口](#签名接口) | 搜索、用户时间线 | 登录进 usuyo | `private-api` | 未实现 |
| [官方 API](#官方-api) | 书签、喜欢 | 开发者凭据 | `official-export` | 未实现 |
| [手机镜像](#手机镜像) | 书签、喜欢的链接 | 只在手机的 App 内 | `vision-ui` | 未实现 |

层级沿用 [usuyo 插件协议](https://github.com/usuyo/usuyo/blob/main/docs/protocol/manifest.md)的定义。「账号」一栏写的是账号在哪里被用到。

<a id="访客接口"></a>
### 访客接口

已实现 · `public-web` · 不使用账号

以访客令牌请求帖子接口 `TweetResultByRestId`。令牌由 `POST guest/activate.json` 取得，缓存一小时。一次调用取回正文（长帖为全文）、原图、码率最高的 mp4、被引用的帖子（一层），以及同一作者自己回复的上文，至多 20 条。`t.co` 短链按回答里的 `expanded_url` 展开。

登录后才可见的帖子（成人内容、受保护的账号）返回 `not-found`，原因为 `private`，条目只保存链接。Spaces 和直播返回 `unsupported-input`。

- 依据：[yt-dlp](https://github.com/yt-dlp/yt-dlp) `yt_dlp/extractor/twitter.py`（`412e7f0`）。请求不带 cookie，不生成 `x-client-transaction-id`。
- 已记录的风险：没有账号参与。访客令牌会被拒（[gallery-dl #9505](https://github.com/mikf/gallery-dl/issues/9505)）或被限流，此时改走嵌入接口。
- 随上游变动的部分：`queryId` 与 `features`，集中在 `src/upstream.rs`；失效时返回 `upstream-changed`。

<a id="嵌入接口"></a>
### 嵌入接口

已实现 · `public-web` · 不使用账号

访客接口被限流或令牌被拒时使用。请求供嵌入帖子用的 syndication 接口，`token` 由帖子 id 算出。取回正文、图片、视频和被引用的帖子；长帖只有前一段，结果标为不完整并写明原因。`meta.json` 的 `api` 字段记录这条帖子由哪个接口取回。

- 依据：[vercel/react-tweet](https://github.com/vercel/react-tweet) `packages/react-tweet/src/api/fetch-tweet.ts`。
- 已记录的风险：没有账号参与。

<a id="浏览器脚本"></a>
### 浏览器脚本

已实现 · `browser-session` · 账号只在使用者的浏览器内

`userscript/usuyo-x-saved.user.js` 在使用者日常的浏览器里运行，读取书签页和喜欢页自身请求的 `Bookmarks`、`Likes` 回答，记下帖子 id、作者和正文首行。脚本不向 X 发起请求，不滚动页面，不读取 cookie；喜欢页只记录登录者本人的。

记下的链接经 `127.0.0.1:47615` 交给 usuyo：先握手确认对端是 usuyo，再提交。内容随后由访客接口取回，不带账号。新存的帖子不会自动进入 usuyo，需要再次打开页面提交。

- 依据：[prinsss/twitter-web-exporter](https://github.com/prinsss/twitter-web-exporter) v1.4.3 截获页面回答的做法。
- 已记录的风险：X 的自动化规则禁止以脚本操作网站。只读取页面回答、不操作页面的脚本，截至 2026-11 未找到账号因此受限的报告。
- 安装：见[安装浏览器脚本](#安装浏览器脚本)。

<a id="官方归档"></a>
### 官方归档

已实现 · `official-export` · 不使用账号

`list` 读取使用者从 X 的设置里申请下载的 `twitter-*.zip`，取其中 `data/like*.js` 与 `data/bookmark*.js` 的帖子链接；内容由访客接口取回。读取在本机完成，不联网。

- 依据：[steipete/birdclaw](https://github.com/steipete/birdclaw) `docs/archive.md`。
- 未核实：归档是否包含书签，以及文件名和字段；以实际归档为准，解析集中在 `src/archive.rs`。

<a id="内容投递"></a>
### 内容投递

未实现 · `browser-session` · 账号只在使用者的浏览器内

登录后才可见的帖子、一条帖子的下文、长文章的正文，只出现在登录状态下页面自身取回的回答里。做法是由浏览器脚本把已取得的内容交给 usuyo，而不只是链接。

- 等：usuyo 的内容投递接口。在此之前这三类条目只保存链接。

<a id="会话接口"></a>
### 会话接口

未实现 · `session-api` · 账号登录进 usuyo

带 `auth_token`、`ct0` 请求 GraphQL 的 `Bookmarks`、`Likes`、`TweetDetail`，不生成 transaction id。书签和喜欢可以由 usuyo 定时取新的。

- 依据：[afar1/fieldtheory-cli](https://github.com/afar1/fieldtheory-cli) `src/graphql-bookmarks.ts`。
- 已记录的风险：账号交给程序以后，请求的节奏和出口都由平台看到。fieldtheory-cli 的 issue 中截至 2026-09 未找到账号受限的报告；同一平台上经签名接口使用的账号有被封的记录，见下一节。

<a id="签名接口"></a>
### 签名接口

未实现 · `private-api` · 账号登录进 usuyo

在官方客户端之外生成 `x-client-transaction-id`，可请求搜索和用户时间线。

- 依据：[vladkens/twscrape](https://github.com/vladkens/twscrape) `twscrape/xclid.py`。
- 已记录的风险：注册三年的账号一次取 800 个关注后被封（[twikit #338](https://github.com/d60/twikit/issues/338)）。

<a id="官方-api"></a>
### 官方 API

未实现 · `official-export` · 开发者凭据

X 的官方接口提供书签和喜欢，需要开发者应用，按量计费。

- 依据：[xdevplatform/xurl](https://github.com/xdevplatform/xurl)。

<a id="手机镜像"></a>
### 手机镜像

未实现 · `vision-ui` · 账号只在手机的 App 内

经 iPhone 镜像或 adb 操作手机上的 X，在书签里逐条复制链接交给 usuyo；内容由访客接口取回。

- 等：usuyo 的手机驱动。

<a id="存下的文件"></a>
## 存下的文件

```
post.json            接口的原样回答
thread/<id>.json     作者自回的上文，原样
meta.json            作者、发布时间、互动数、所用接口、媒体的原地址、未取到的部分及原因
images/001.jpg       图片；被引用帖子的为 images/q-001.jpg
media/video-001.mp4  视频和动图；被引用帖子的为 media/q-video-001.mp4
```

未取到的部分写入结果的 `missing` 与 `missingReason`。

<a id="节奏"></a>
## 节奏

清单声明同一平台两次调用间隔 10 秒，每天批量调用至多 300 次；手动收下的单条不计入每日次数。节奏由 usuyo 执行。

<a id="安装浏览器脚本"></a>
## 安装浏览器脚本

1. 在浏览器的脚本管理器（Violentmonkey、Tampermonkey，Safari 上为 Userscripts）中新建脚本，粘贴 `userscript/usuyo-x-saved.user.js`。
2. 在 usuyo 的导入页签发一个投递令牌。令牌只能提交链接，不能读取资料库。
3. 打开 `x.com/i/bookmarks` 或本人主页的喜欢，向下浏览；页面角落显示已记下的条数。
4. 点「交给 usuyo」，首次粘贴令牌。提交的批次出现在 usuyo 的导入页，确认后开始取回。

<a id="构建与测试"></a>
## 构建与测试

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
USUYO_BIN=<usuyo 可执行文件> node --test userscript/test/userscript.test.mjs
```

测试只连本机：令牌、帖子接口、嵌入接口、`t.co`、媒体各由一个本地服务应答，样例在 `tests/fixtures/`，每个文件注明其形状出自哪个上游文件。发请求经 usuyo 的 `usuyo-plugin-kit`，只连清单声明的主机和公网地址；该 crate 按路径引用，本仓库需与 usuyo 仓库位于同一上级目录。

<a id="许可与声明"></a>
## 许可与声明

- 以 MIT 许可发布。
- X 与 Twitter 是 X Corp. 的商标。本项目与 X Corp. 没有关联，未获其授权或认可。
- 插件只读取内容，不发帖、不点赞、不关注、不发私信，不处理付费内容。
- 账号受限等后果和所取内容的使用，由使用者负责。
- 权利人的移除请求，按 [usuyo-fetch](https://github.com/usuyo-fetch) 组织说明处理。
