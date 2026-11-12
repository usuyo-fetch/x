//! 子命令照协议 §3.2：`canonicalize`、`fetch`、`list`、`health`。错误只在这里变成信封。
//!
//! 从环境里只读三个变量：`USUYO_LOCALE`（`hint` 的语言）、`USUYO_CACHE_DIR`（访客令牌的缓存）、
//! `USUYO_INPUT`（导入的归档副本）。账号凭据（`USUYO_CREDENTIALS`、`USUYO_COOKIES_TXT`）一概不读。

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;
use usuyo_plugin_kit::protocol::{CallFailure, Envelope, ErrorCode, OutputKind};

use crate::error::{Error, Lang};
use crate::fetch::{self, Request};
use crate::site::Site;
use crate::{archive, link};

const USAGE: &str = "usage: usuyo-fetch-x canonicalize <url> | fetch <url> --out <dir> [--want a,b] | list <source> [--cursor c] [--since t] | health [--probe]";

/// 跑一个子命令，stdout 上写一个信封，返回退出码。
pub fn run(args: &[String], site: &Site) -> u8 {
    let lang = Lang::from_locale(std::env::var("USUYO_LOCALE").ok().as_deref());
    match args.first().map(String::as_str) {
        Some("canonicalize") if args.len() == 2 => {
            print(link::canonicalize(site, &args[1]).map_err(Into::into), lang)
        }
        Some("fetch") => match parse_fetch(&args[1..]) {
            Some((input, out, want)) => print(run_fetch(site, &input, out, want), lang),
            None => usage(),
        },
        Some("list") => match parse_list(&args[1..]) {
            Some((source, cursor)) => print(
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
            None => usage(),
        },
        Some("health") if args.len() <= 2 && args.get(1).is_none_or(|a| a == "--probe") => {
            print(health(args.len() == 2), lang)
        }
        _ => usage(),
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

type FetchArgs = (String, PathBuf, Option<BTreeSet<OutputKind>>);

fn parse_fetch(args: &[String]) -> Option<FetchArgs> {
    let (input, rest) = args.split_first()?;
    let mut out = None;
    let mut want = None;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(it.next()?)),
            "--want" => {
                want = Some(
                    it.next()?
                        .split(',')
                        .map(OutputKind::parse)
                        .collect::<Option<BTreeSet<_>>>()?,
                );
            }
            _ => return None,
        }
    }
    Some((input.clone(), out?, want))
}

/// `list <source> [--cursor c] [--since t]`；归档没有加入时间，`--since` 收下但不用（`order: unordered`）。
fn parse_list(args: &[String]) -> Option<(String, Option<String>)> {
    let (source, rest) = args.split_first()?;
    let mut cursor = None;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--cursor" => cursor = Some(it.next()?.clone()),
            "--since" => {
                it.next()?;
            }
            _ => return None,
        }
    }
    Some((source.clone(), cursor))
}

fn run_fetch(
    site: &Site,
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
            input,
            out: &out,
            want,
            cache_dir,
        },
    )?)
}

#[derive(Serialize)]
struct Health {
    ready: bool,
    protocol: u32,
    version: &'static str,
    upstream: Vec<serde_json::Value>,
    login: Vec<serde_json::Value>,
    checks: Vec<Check>,
}

#[derive(Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    message: String,
}

/// 不发请求（协议 §3.10）；没有登录站点，也没有本机设置要看。
fn health(probe: bool) -> anyhow::Result<Health> {
    let mut checks = Vec::new();
    if probe {
        checks.push(Check {
            name: "probe",
            ok: true,
            message: "没有发请求：这个插件不用账号，没有登录可查；访客令牌在取帖子时才换".into(),
        });
    }
    Ok(Health {
        ready: checks.iter().all(|c| c.ok),
        protocol: 1,
        version: env!("CARGO_PKG_VERSION"),
        upstream: Vec::new(),
        login: Vec::new(),
        checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fetch_args() {
        let (_, out, want) = parse_fetch(&strings(&[
            "https://a.b/",
            "--out",
            "/tmp/o",
            "--want",
            "text,media:video",
        ]))
        .unwrap();
        assert_eq!(out, PathBuf::from("/tmp/o"));
        assert_eq!(want, Some([OutputKind::Text, OutputKind::Video].into()));
        for bad in [
            &["https://a.b/", "--want", "video"][..],
            &["https://a.b/"][..],
            &["https://a.b/", "--out"][..],
            &["https://a.b/", "--out", "/o", "--extra"][..],
        ] {
            assert!(parse_fetch(&strings(bad)).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn list_args() {
        assert_eq!(
            parse_list(&strings(&[
                "file:a.zip",
                "--cursor",
                "500",
                "--since",
                "2026-01-01T00:00:00Z"
            ])),
            Some(("file:a.zip".into(), Some("500".into())))
        );
        assert_eq!(
            parse_list(&strings(&["file:a.zip"])),
            Some(("file:a.zip".into(), None))
        );
        assert!(parse_list(&strings(&["file:a.zip", "--cursor"])).is_none());
        assert!(parse_list(&strings(&["file:a.zip", "--page", "2"])).is_none());
        assert!(parse_list(&[]).is_none());
    }
}
