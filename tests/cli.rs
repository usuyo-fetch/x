//! 按宿主的方式起插件进程：清空环境，代理指向一个关着的本机端口。
//! 这里的调用都不该联网：链接离线就认得出、归档只读本机文件、错误在发请求之前就答。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use usuyo_plugin::protocol::{Canonicalized, ListPage, PluginManifest};

const ID: &str = "1849000000000000001";
const DEAD_PROXY: &str = "http://127.0.0.1:9";

fn plugin(args: &[&str], input: Option<&Path>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_usuyo-fetch-x"));
    cmd.env_clear()
        .env("USUYO_PROTOCOL", "1")
        .env("USUYO_LOCALE", "zh-CN")
        .env("HTTP_PROXY", DEAD_PROXY)
        .env("HTTPS_PROXY", DEAD_PROXY)
        .env("ALL_PROXY", DEAD_PROXY)
        .args(args);
    if let Some(i) = input {
        cmd.env("USUYO_INPUT", i);
    }
    cmd.output().unwrap()
}

fn envelope(out: &Output) -> serde_json::Value {
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout is one JSON value: {stdout}"
    );
    serde_json::from_str(&stdout).unwrap()
}

fn manifest() -> PluginManifest {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("usuyo-plugin.yaml"),
    )
    .unwrap();
    let parsed = PluginManifest::from_yaml(&text).unwrap();
    assert!(
        parsed.unknown_fields.is_empty(),
        "{:?}",
        parsed.unknown_fields
    );
    parsed.value
}

#[test]
fn canonicalize_post_links_offline() {
    for input in [
        format!("https://x.com/someone/status/{ID}"),
        format!("https://twitter.com/someone/status/{ID}?s=20&t=AbCdEfGh"),
        format!("https://mobile.twitter.com/someone/status/{ID}/photo/1"),
        format!("https://x.com/i/web/status/{ID}"),
        format!("https://twitter.com/statuses/{ID}"),
        format!("https://fxtwitter.com/someone/status/{ID}"),
        format!("https://fixupx.com/someone/status/{ID}/video/1"),
    ] {
        let out = plugin(&["canonicalize", &input], None);
        assert!(out.status.success(), "{input}: {out:?}");
        let mut value = envelope(&out);
        assert_eq!(value["ok"], true);
        assert_eq!(value["identity"]["platform"], "x");
        assert_eq!(value["identity"]["contentId"], ID);
        assert_eq!(
            value["identity"]["url"],
            format!("https://x.com/i/web/status/{ID}")
        );
        value.as_object_mut().unwrap().remove("ok");
        let parsed: Canonicalized = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed,
            Canonicalized::Match {
                resolved_url: None,
                ..
            }
        ));
    }
    for input in [
        "https://example.com/a",
        "https://x.com/i/spaces/1YqKDqDXAbwKV",
        "https://x.com/someone",
    ] {
        let out = plugin(&["canonicalize", input], None);
        assert_eq!(
            envelope(&out),
            serde_json::json!({"ok": true, "match": false}),
            "{input}"
        );
    }
}

#[test]
fn fetch_errors_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let out_dir = dir.path().to_str().unwrap();
    let out = plugin(
        &[
            "fetch",
            "https://x.com/i/spaces/1YqKDqDXAbwKV",
            "--out",
            out_dir,
            "--method",
            "guest",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(envelope(&out)["error"]["code"], "unsupported-input");
    let out = plugin(
        &[
            "fetch",
            "https://x.com/a/status/1",
            "--out",
            "relative/dir",
            "--method",
            "embed",
        ],
        None,
    );
    assert_eq!(envelope(&out)["error"]["code"], "invalid-args");
    let out = plugin(
        &[
            "fetch",
            "https://x.com/a/status/1",
            "--out",
            out_dir,
            "--method",
            "guest",
            "--want",
            "comments",
        ],
        None,
    );
    assert_eq!(envelope(&out)["error"]["code"], "invalid-args");
    // 没做的方法、不在插件里跑的方法：不发请求
    for method in ["session", "userscript", "archive"] {
        let out = plugin(
            &[
                "fetch",
                "https://x.com/a/status/1",
                "--out",
                out_dir,
                "--method",
                method,
            ],
            None,
        );
        assert_eq!(envelope(&out)["error"]["code"], "invalid-args", "{method}");
    }
    let out = plugin(&["list", "file:twitter-x.zip", "--method", "guest"], None);
    assert_eq!(envelope(&out)["error"]["code"], "invalid-args");
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

fn archive(dir: &Path, with: &[&str]) -> PathBuf {
    let path = dir.join("twitter-2026-11-01-0123abcd.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("data/account.js", options).unwrap();
    zip.write_all(b"window.YTD.account.part0 = []").unwrap();
    for name in with {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/archive")
            .join(name);
        zip.start_file(format!("data/{name}"), options).unwrap();
        zip.write_all(&std::fs::read(fixture).unwrap()).unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn list_reads_likes_and_bookmarks_from_the_archive() {
    let dir = tempfile::tempdir().unwrap();
    let zip = archive(dir.path(), &["bookmark.js", "like-part1.js", "like.js"]);
    let out = plugin(
        &[
            "list",
            "file:twitter-2026-11-01-0123abcd.zip",
            "--method",
            "archive",
        ],
        Some(&zip),
    );
    assert!(out.status.success(), "{out:?}");
    let mut value = envelope(&out);
    value.as_object_mut().unwrap().remove("ok");
    let page: ListPage = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(page.next_cursor, None);
    let rows: Vec<(String, &str)> = page
        .items
        .iter()
        .map(|i| {
            let action = serde_json::to_value(i.action.unwrap()).unwrap();
            (
                i.url.clone().unwrap(),
                if action == "like" { "like" } else { "favorite" },
            )
        })
        .collect();
    let url = |n: u32| format!("https://x.com/i/web/status/18490000000000000{n:02}");
    assert_eq!(
        rows,
        [
            (url(1), "like"),
            (url(2), "like"),
            (url(3), "like"),
            (url(4), "favorite"),
            (url(1), "favorite"),
        ]
    );
    assert_eq!(
        page.items[0].title.as_deref(),
        Some("Two photos & a link https://t.co/link1 #coffee https://t.co/media1")
    );
    assert_eq!(page.items[1].title, None);
    assert_eq!(
        page.items[2].title.as_deref(),
        Some("Look at this https://t.co/quote1")
    );
    // 归档里没有加入时间：不写 actedAt
    for item in &value["items"].as_array().unwrap()[..] {
        assert!(item.get("actedAt").is_none(), "{item}");
        assert_eq!(item["identity"]["platform"], "x");
    }
    assert_eq!(value["evidence"]["access"], "official-export");
}

#[test]
fn list_pages_and_refuses_what_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    // 一页 500 条：手写 1200 条喜欢
    let path = dir.path().join("twitter-big.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("data/like.js", zip::write::SimpleFileOptions::default())
        .unwrap();
    let records: Vec<String> = (1..=1200)
        .map(|n| {
            format!(
                r#"{{"like":{{"tweetId":"{}"}}}}"#,
                1_849_000_000_000_000_000u64 + n
            )
        })
        .collect();
    write!(zip, "window.YTD.like.part0 = [{}]", records.join(",")).unwrap();
    zip.finish().unwrap();
    let mut cursor: Option<String> = None;
    let mut seen = 0;
    let mut pages = 0;
    loop {
        let mut args = vec!["list", "file:twitter-big.zip", "--method", "archive"];
        if let Some(c) = &cursor {
            args.extend(["--cursor", c.as_str()]);
        }
        let value = envelope(&plugin(&args, Some(&path)));
        seen += value["items"].as_array().unwrap().len();
        pages += 1;
        match value["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!((seen, pages), (1200, 3));

    // 不是 zip、zip 里没有喜欢和书签、没有交文件
    let not_zip = dir.path().join("twitter-x.zip");
    std::fs::write(&not_zip, b"hello").unwrap();
    let value = envelope(&plugin(
        &["list", "file:twitter-x.zip", "--method", "archive"],
        Some(&not_zip),
    ));
    assert_eq!(value["error"]["code"], "unsupported-input");
    let empty = archive(dir.path(), &[]);
    let value = envelope(&plugin(
        &[
            "list",
            "file:twitter-2026-11-01-0123abcd.zip",
            "--method",
            "archive",
        ],
        Some(&empty),
    ));
    assert_eq!(value["error"]["code"], "unsupported-input");
    assert!(value["error"]["hint"].as_str().unwrap().contains("归档"));
    let value = envelope(&plugin(
        &["list", "file:twitter-x.zip", "--method", "archive"],
        None,
    ));
    assert_eq!(value["error"]["code"], "invalid-args");
    let value = envelope(&plugin(
        &["list", "x:bookmarks", "--method", "archive"],
        Some(&empty),
    ));
    assert_eq!(value["error"]["code"], "invalid-args");
}

#[test]
fn health_answers_without_requests() {
    let m = manifest();
    let value = envelope(&plugin(&["health"], None));
    assert_eq!(value["ok"], true);
    assert_eq!(value["ready"], true);
    assert_eq!(value["protocol"], 1);
    assert_eq!(value["version"], m.header.version.to_string());
    assert_eq!(value["checks"], serde_json::json!([]));
    let value = envelope(&plugin(&["health", "--probe"], None));
    assert_eq!(value["ready"], true);
    assert_eq!(value["checks"][0]["name"], "probe");
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        &["fetch"][..],
        &["fetch", "https://x.com/a/status/1", "--out", "/o"][..],
        &["sources"][..],
        &["health", "--x"][..],
        &["list"][..],
        &[][..],
    ] {
        let out = plugin(args, None);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty());
    }
}

/// 和别的插件的测试一样：清单交给宿主自己的加载代码，能装、能认每种链接。
#[test]
fn manifest_loads_in_the_host_and_routes_every_link_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/usuyo-fetch/x/0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("usuyo-plugin.yaml"),
        dir.join("usuyo-plugin.yaml"),
    )
    .unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_usuyo-fetch-x"),
        dir.join("usuyo-fetch-x"),
    )
    .unwrap();
    let found = usuyo_plugin::discover(&tmp.path().join("plugins")).unwrap();
    assert!(found.rejected.is_empty(), "{:?}", found.rejected);
    assert_eq!(found.fetch.len(), 1);
    // 不登录：没有账号站点、没有访客身份、没有设置
    let spec = &found.fetch[0].spec;
    assert!(spec.sites.is_empty());
    assert!(spec.visitors.is_empty());
    assert!(found.fetch[0].header.settings.is_empty());
    // README 的方法表一行一种，做了的四种：访客、嵌入、浏览器脚本、官方归档
    let ids: Vec<&str> = spec.methods.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "guest",
            "embed",
            "userscript",
            "archive",
            "deliver-content",
            "session",
            "signed",
            "xurl",
            "iphone"
        ]
    );
    let built: Vec<&str> = spec.built().map(|m| m.id.as_str()).collect();
    assert_eq!(built, ["guest", "embed", "userscript", "archive"]);
    assert!(!spec.method("signed").unwrap().on_by_default());
    let list = spec
        .method("archive")
        .unwrap()
        .capabilities
        .list
        .as_ref()
        .unwrap();
    assert_eq!(list.files, ["twitter-*.zip"]);
    let host = usuyo_plugin::PluginHost::new(
        usuyo_plugin::HostConfig {
            work_dir: tmp.path().join("work"),
            cache_dir: tmp.path().join("cache"),
            locale: "zh-CN".into(),
            limits: usuyo_plugin::CallLimits::default(),
        },
        found.fetch,
    );
    for link in [
        format!("https://x.com/someone/status/{ID}"),
        format!("https://www.x.com/someone/status/{ID}"),
        format!("https://mobile.x.com/someone/status/{ID}"),
        format!("https://twitter.com/someone/status/{ID}"),
        format!("https://mobile.twitter.com/someone/status/{ID}"),
        "https://t.co/AbC123xyz".into(),
        format!("https://fxtwitter.com/someone/status/{ID}"),
        format!("https://fixupx.com/someone/status/{ID}"),
    ] {
        let url = url::Url::parse(&link).unwrap();
        assert_eq!(host.candidates(&url).len(), 1, "{link}");
    }
    assert!(
        host.candidates(&url::Url::parse("https://example.com/").unwrap())
            .is_empty()
    );
}

/// 清单里的主机名单和插件代码里的一致；`fetch` 的输出词表和代码里的一致。
#[test]
fn manifest_matches_the_code() {
    let m = manifest();
    let spec = match &m.kind {
        usuyo_plugin::protocol::PluginKind::Fetch(spec) => spec,
        other => panic!("not a fetch plugin: {other:?}"),
    };
    for method in ["guest", "embed"] {
        let fetch = spec
            .method(method)
            .unwrap()
            .capabilities
            .fetch
            .as_ref()
            .unwrap();
        assert_eq!(
            fetch.outputs.iter().copied().collect::<Vec<_>>(),
            usuyo_fetch_x::fetch::OUTPUTS.to_vec()
        );
    }
    let hosts: Vec<String> = m.header.permissions.network.hosts.clone();
    assert_eq!(
        hosts,
        [
            "api.x.com",
            "x.com",
            "cdn.syndication.twimg.com",
            "pbs.twimg.com",
            "video.twimg.com",
            "t.co"
        ]
    );
}
