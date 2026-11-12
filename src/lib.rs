//! usuyo 的 X（推特）获取插件：按帖子链接取公开帖子，存成个人备份；不用账号。
//!
//! - [`cli`]：子命令和信封，入口只调它。
//! - [`link`]：认链接、展开 `t.co`（`canonicalize`）。
//! - [`fetch`]：取一条帖子，写进输出目录。
//! - [`archive`]：读官方归档里的喜欢和书签（`list`）。
//! - `upstream`：照上游抄来的接口参数，X 改了只改这里；`post`：两种回答的形状。
//! - [`Site`]：能连哪些主机；正式运行只用 [`Site::x`]。

mod archive;
pub mod cli;
mod error;
pub mod fetch;
mod http;
pub mod link;
mod post;
mod site;
mod upstream;

pub use error::{Error, Lang};
pub use site::Site;
