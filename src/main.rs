//! X 获取插件的入口：只认 X 的真主机，子命令交给 [`usuyo_fetch_x::cli::run`]。

use std::process::ExitCode;

use usuyo_fetch_x::{Site, cli};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(cli::run(&args, &Site::x()))
}
