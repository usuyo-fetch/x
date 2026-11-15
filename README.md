# usuyo-fetch/x

X（Twitter）的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按帖子链接取回内容，列出账号存下的帖子。

| 功能 | 实现方式（方法 id） | 账号 | 风险层级 | 状态 |
|---|---|---|---|---|
| 按链接取单条帖子 | 访客接口（`guest`） | 不使用 | `public-web`，默认开 | 已实现（2026-11-12 线上取回过文字和带图的帖子各一条） |
| | 嵌入接口（`embed`） | 不使用 | `public-web`，默认开 | 已实现（离线测试） |
| | 内容投递（`deliver-content`） | 只在使用者的浏览器内 | `browser-session`，默认开 | 未实现 |
| 视频文件 | 访客接口、嵌入接口（`guest`、`embed`）下载帖子里的 mp4 | 不使用 | `public-web`，默认开 | 已实现（随单条帖子一起取，离线测试） |
| 列出书签、喜欢 | 浏览器脚本（`userscript`） | 只在使用者的浏览器内 | `browser-session`，默认开 | 已实现（离线测试） |
| | 会话接口（`session`） | 登录进 usuyo | `session-api`，默认开 | 未实现 |
| | 手机镜像（`iphone`） | 只在手机的 App 内 | `vision-ui`，默认开 | 未实现 |
| 读官方导出 | 官方归档（`archive`） | 不使用 | `official-export`，默认开 | 已实现（离线测试） |
| 搜索、用户时间线 | 签名接口（`signed`） | 登录进 usuyo | `private-api`，默认关 | 未实现 |

风险层级用清单里 `access` 的原词；`private-api`、`client-hook` 默认关，其余层级默认开。

## 方法

### `guest`

按帖子链接取单条帖子：正文、图片、视频、被引用的帖子、作者自回的上文。用访客令牌调帖子接口，不带 cookie，不读账号凭据。访客令牌在有的出口会被拒。

### `embed`

取单条帖子，访客接口不可用时由宿主改用。走 syndication 嵌入接口。

### `userscript`

`userscript/` 里的脚本在使用者的浏览器里截获书签和喜欢，把链接交给 usuyo。

### `archive`

读 X 设置里「下载你的数据归档」得到的 `twitter-*.zip`，列出喜欢和书签的链接。

### 未实现的方法

`deliver-content`：登录后才可见的帖子、下文、长文章，由浏览器内的脚本交来。`session`：书签和喜欢，可定时取新的。`signed`：搜索和用户时间线。`iphone`：从手机 App 读书签和喜欢的链接。

测试：`cargo test`；样例照上游项目源码手写，位于 `tests/fixtures/`。依赖同级目录下 usuyo 仓库的 `usuyo-plugin-kit`。X 与 Twitter 是 X Corp. 的商标，本项目与其无关联。许可证：MIT。

## 开发

`Cargo.toml` 里的 `usuyo-plugin`、`usuyo-plugin-kit` 是对 `https://github.com/usuyo/usuyo` 的 git 依赖，钉在固定的 `rev`。

本地开发时用旁边 `usuyo/` 的工作区代替它们：`usuyo-fetch/.cargo/config.toml`（不进任何插件仓库）写了下面的覆盖，cargo 从当前目录往上找这个文件，在插件文件夹里跑 cargo 就生效；从别处跑要加 `--config <这个文件>`。

```toml
[patch."https://github.com/usuyo/usuyo"]
usuyo-plugin = { path = "../usuyo/crates/usuyo-plugin" }
usuyo-plugin-kit = { path = "../usuyo/crates/usuyo-plugin-kit" }
```

`Cargo.lock` 记的是覆盖后的路径形式，所以 CI 不加 `--locked` 构建，cargo 只重新锁这两个 usuyo crate。
