# usuyo-fetch/x

X（Twitter）的获取插件，供 [usuyo](https://github.com/usuyo/usuyo) 调用：按帖子链接取回内容，列出账号存下的帖子。

| 方法 | 清单里的 id | 取得 | 账号 | 层级 | 现状 |
|---|---|---|---|---|---|
| 访客接口 | `guest` | 单条帖子：正文、图片、视频、被引用的帖子、作者自回的上文 | 不使用 | `public-web` | 已实现；2026-11-12 线上取回过文字和带图的帖子各一条 |
| 嵌入接口 | `embed` | 单条帖子，访客接口不可用时 | 不使用 | `public-web` | 已实现，离线测试 |
| 浏览器脚本 | `userscript` | 书签、喜欢的链接 | 只在使用者的浏览器内 | `browser-session` | 已实现，离线测试 |
| 官方归档 | `archive` | 喜欢、书签的链接 | 不使用 | `official-export` | 已实现，离线测试 |
| 内容投递 | `deliver-content` | 登录后才可见的帖子、下文、长文章 | 只在使用者的浏览器内 | `browser-session` | 未实现 |
| 会话接口 | `session` | 书签、喜欢，可定时取新的 | 登录进 usuyo | `session-api` | 未实现 |
| 签名接口 | `signed` | 搜索、用户时间线 | 登录进 usuyo | `private-api` | 未实现 |
| 官方 API | `xurl` | 书签、喜欢 | 开发者凭据 | `official-export` | 未实现 |
| 手机镜像 | `iphone` | 书签、喜欢的链接 | 只在手机的 App 内 | `vision-ui` | 未实现 |

测试：`cargo test`；样例照上游项目源码手写，位于 `tests/fixtures/`。依赖同级目录下 usuyo 仓库的 `usuyo-plugin-kit`。X 与 Twitter 是 X Corp. 的商标，本项目与其无关联。许可证：MIT。
