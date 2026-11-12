//! 子命令照协议 §3.2：`canonicalize`、`fetch`、`list`、`health`，读参数用 plugin-kit。错误只在这里变成信封。
//!
//! 从环境里只读三个变量：`USUYO_LOCALE`（`hint` 的语言）、`USUYO_CACHE_DIR`（访客令牌的缓存）、
//! `USUYO_INPUT`（导入的归档副本）。账号凭据（`USUYO_CREDENTIALS`、`USUYO_COOKIES_TXT`）一概不读。

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;
use usuyo_plugin_kit::fetch::{Call, USAGE};
use usuyo_plugin_kit::protocol::{CallFailure, Envelope, ErrorCode, FetchHealth, OutputKind};

use crate::error::{Error, Lang};
use crate::fetch::{self, Api, Request};
use crate::site::Site;
use crate::{archive, link};

/// 跑一个子命令，stdout 上写一个信封，返回退出码。按方法分派（§2.3.1）：`guest`、`embed` 取单条，
/// `archive` 读官方归档；浏览器脚本在你的浏览器里交链接，别的方法还没做。
pub fn run(args: &[String], site: &Site) -> u8 {
    let lang = Lang::from_locale(std::env::var("USUYO_LOCALE").ok().as_deref());
    let Some(call) = Call::parse(args) else {
        return usage();
    };
    let method = call.method().map(str::to_owned);
    match (method.as_deref(), call) {
        (_, Call::Canonicalize { url }) => {
            print(link::canonicalize(site, &url).map_err(Into::into), lang)
        }
        (
            Some(m @ ("guest" | "embed")),
            Call::Fetch {
                input, out, want, ..
            },
        ) => {
            let api = if m == "guest" { Api::Guest } else { Api::Embed };
            print(
                run_fetch(
                    site,
                    api,
                    &input,
                    out,
                    want.map(|w| w.into_iter().collect()),
                ),
                lang,
            )
        }
        (Some("archive"), Call::List { source, cursor, .. }) => print(
            archive::list(
                &source,
                std::env::var_os("USUYO_INPUT")
                    .map(PathBuf::from)
                    .as_deref(),
                cursor.as_deref(),
            )
            .map_err(Into::into),
            lang,
        ),
        (_, Call::Health { probe }) => print(health(probe), lang),
        (method, _) => print::<()>(
            Err(
                Error::InvalidArgs(format!("this call does not apply to method {method:?}")).into(),
            ),
            lang,
        ),
    }
}

fn usage() -> u8 {
    eprintln!("{USAGE}");
    2
}

fn print<T: Serialize>(result: anyhow::Result<T>, lang: Lang) -> u8 {
    let envelope = match result {
        Ok(body) => Envelope::Ok(body),
        Err(e) => match e.downcast::<Error>() {
            Ok(error) => Envelope::Err(error.to_failure(lang)),
            Err(other) => {
                Envelope::Err(CallFailure::new(ErrorCode::Internal, format!("{other:#}")))
            }
        },
    };
    match serde_json::to_string(&envelope) {
        Ok(json) => {
            println!("{json}");
            if envelope.exit_code() == 0 { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("cannot write the envelope: {e}");
            1
        }
    }
}

fn run_fetch(
    site: &Site,
    api: Api,
    input: &str,
    out: PathBuf,
    want: Option<BTreeSet<OutputKind>>,
) -> anyhow::Result<usuyo_plugin_kit::protocol::FetchOutput> {
    if !out.is_absolute() || !out.is_dir() {
        return Err(Error::InvalidArgs(format!(
            "--out `{}` is not an existing absolute directory",
            out.display()
        ))
        .into());
    }
    let cache_dir = std::env::var_os("USUYO_CACHE_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir());
    Ok(fetch::fetch(
        site,
        Request {
            api,
            input,
            out: &out,
            want,
            cache_dir,
        },
    )?)
}

/// 不发请求（协议 §3.10）；没有登录站点，也没有本机设置要看。
fn health(probe: bool) -> anyhow::Result<FetchHealth> {
    let mut h = FetchHealth::new(env!("CARGO_PKG_VERSION"));
    if probe {
        h = h.check(
            "probe",
            true,
            Some("没有发请求：这个插件不用账号，没有登录可查；访客令牌在取帖子时才换".into()),
            &[],
        );
    }
    Ok(h)
}
