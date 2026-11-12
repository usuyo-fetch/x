//! 对着本机的假 X 取帖子：两条路、退路、串、引用、各种错误码，和每个请求带了什么。

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{Activate, FRESH_TOKEN, Fake, JPEG, MP4, PNG, STALE_TOKEN};
use serde_json::Value;
use usuyo_fetch_x::fetch::{Api, Request, fetch};
use usuyo_fetch_x::{Error, Lang, cli, link};
use usuyo_plugin_kit::protocol::{
    Canonicalized, Completeness, ErrorCode, FetchOutput, NotFoundReason, OutputKind,
};

const BEARER_PREFIX: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAA";

fn id(n: u32) -> String {
    format!("18490000000000000{n:02}")
}

fn link_to(n: u32) -> String {
    format!("https://x.com/sample_person/status/{}", id(n))
}

fn run(
    fake: &Fake,
    input: &str,
    want: Option<&[OutputKind]>,
    cache: Option<&Path>,
) -> (Result<FetchOutput, Error>, tempfile::TempDir) {
    run_on(Api::Guest, fake, input, want, cache)
}

/// 方法 `guest` 或 `embed`。
fn run_on(
    api: Api,
    fake: &Fake,
    input: &str,
    want: Option<&[OutputKind]>,
    cache: Option<&Path>,
) -> (Result<FetchOutput, Error>, tempfile::TempDir) {
    let out = tempfile::tempdir().unwrap();
    let result = fetch(
        &fake.site(),
        Request {
            api,
            input,
            out: out.path(),
            want: want.map(|w| w.iter().copied().collect::<BTreeSet<_>>()),
            cache_dir: cache.map(Path::to_path_buf),
        },
    );
    (result, out)
}

fn ok(fake: &Fake, n: u32) -> (FetchOutput, tempfile::TempDir) {
    let (result, out) = run(fake, &link_to(n), None, None);
    (result.unwrap_or_else(|e| panic!("{n}: {e}")), out)
}

fn err(fake: &Fake, n: u32) -> Error {
    run(fake, &link_to(n), None, None).0.expect_err("an error")
}

fn embed_ok(fake: &Fake, n: u32) -> (FetchOutput, tempfile::TempDir) {
    let (result, out) = run_on(Api::Embed, fake, &link_to(n), None, None);
    (result.unwrap_or_else(|e| panic!("{n}: {e}")), out)
}

fn embed_err(fake: &Fake, n: u32) -> Error {
    run_on(Api::Embed, fake, &link_to(n), None, None)
        .0
        .expect_err("an error")
}

fn read_json(dir: &Path, name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap()
}

fn files(dir: &Path) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                all.insert(p.strip_prefix(dir).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    all
}

/// 每个请求都不带 cookie；bearer 和访客令牌只发给换令牌和帖子接口。
fn no_credentials_leak(fake: &Fake) {
    for s in fake.seen() {
        assert_eq!(s.header("cookie"), None, "{s:?}");
        match s.server {
            "api" => assert!(
                s.header("authorization")
                    .unwrap()
                    .starts_with(BEARER_PREFIX)
            ),
            "graphql" => {
                assert!(
                    s.header("authorization")
                        .unwrap()
                        .starts_with(BEARER_PREFIX)
                );
                assert!(s.header("x-guest-token").is_some());
            }
            _ => {
                assert_eq!(s.header("authorization"), None, "{s:?}");
                assert_eq!(s.header("x-guest-token"), None, "{s:?}");
            }
        }
    }
}

#[test]
fn photos_through_the_post_endpoint() {
    let fake = Fake::start(Activate::Ok);
    let (out, dir) = ok(&fake, 1);
    assert_eq!(out.identity, Some(link::identity(&id(1))));
    assert_eq!(
        out.text.as_deref(),
        Some("Two photos & a link https://example.com/brew #coffee")
    );
    assert_eq!(
        out.title.as_deref(),
        Some("Two photos & a link https://example.com/brew #coffee")
    );
    let author = out.author.unwrap();
    assert_eq!(author.name, "Sample Person");
    assert_eq!(author.platform_id.as_deref(), Some("4400000001"));
    assert_eq!(author.url.as_deref(), Some("https://x.com/sample_person"));
    assert_eq!(out.published_at.unwrap().as_str(), "2024-10-30T12:00:00Z");
    assert_eq!(out.platform_tags, ["coffee"]);
    let images: Vec<&str> = out.images.iter().map(|p| p.as_str()).collect();
    assert_eq!(images, ["images/001.jpg", "images/002.png"]);
    assert_eq!(
        std::fs::read(dir.path().join("images/001.jpg")).unwrap(),
        JPEG
    );
    assert_eq!(
        std::fs::read(dir.path().join("images/002.png")).unwrap(),
        PNG
    );
    // 帖子本来就没有视频：不算缺。
    assert_eq!(out.completeness, Completeness::Full);
    assert!(out.missing.is_empty());
    assert_eq!(out.missing_reason, None);
    // 原图：`name=orig`
    let media = fake.on("media");
    assert_eq!(media.len(), 2);
    for m in &media {
        assert_eq!(m.query("name").as_deref(), Some("orig"), "{m:?}");
    }
    let meta = read_json(dir.path(), "meta.json");
    assert_eq!(meta["api"], "TweetResultByRestId");
    assert_eq!(meta["url"], link_to(1));
    assert_eq!(meta["counts"]["views"], 1234);
    assert_eq!(read_json(dir.path(), "post.json")["rest_id"], id(1));
    assert_eq!(fake.on("api").len(), 1);
    assert_eq!(fake.on("graphql").len(), 1);
    assert!(fake.on("syndication").is_empty());
    let q = &fake.on("graphql")[0];
    let vars: Value = serde_json::from_str(&q.query("variables").unwrap()).unwrap();
    assert_eq!(vars["tweetId"], id(1));
    assert!(
        q.query("features")
            .unwrap()
            .contains("longform_notetweets_consumption_enabled")
    );
    assert_eq!(q.header("x-guest-token"), Some(FRESH_TOKEN));
    no_credentials_leak(&fake);
}

#[test]
fn video_takes_the_highest_mp4_and_old_author_shape() {
    let fake = Fake::start(Activate::Ok);
    let (out, dir) = ok(&fake, 2);
    assert_eq!(out.media.len(), 1);
    assert_eq!(out.media[0].path.as_str(), "media/video-001.mp4");
    assert_eq!(out.media[0].mime.as_deref(), Some("video/mp4"));
    assert_eq!(out.media[0].duration_sec, Some(12.5));
    assert_eq!(
        std::fs::read(dir.path().join("media/video-001.mp4")).unwrap(),
        MP4
    );
    let media = fake.on("media");
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].target, "/video/clip-high.mp4");
    assert_eq!(
        out.author.unwrap().url.as_deref(),
        Some("https://x.com/old_shape")
    );
    assert!(out.missing.is_empty());
    assert_eq!(out.text.as_deref(), Some("A short clip"));
}

#[test]
fn quoted_post_goes_into_the_same_snapshot() {
    let fake = Fake::start(Activate::Ok);
    let (out, dir) = ok(&fake, 3);
    assert_eq!(
        out.text.as_deref(),
        Some(
            "Look at this https://twitter.com/other_one/status/1849000000000000030\n\n> 引用 https://x.com/other_one/status/1849000000000000030\n> The original thought\n> second line"
        )
    );
    let images: Vec<&str> = out.images.iter().map(|p| p.as_str()).collect();
    assert_eq!(images, ["images/q-001.jpg"]);
    assert_eq!(out.media[0].path.as_str(), "media/q-video-001.mp4");
    assert_eq!(out.completeness, Completeness::Full);
    let meta = read_json(dir.path(), "meta.json");
    assert_eq!(meta["quoted"]["id"], "1849000000000000030");
    assert_eq!(meta["videos"][0]["type"], "animated_gif");
    // 引用的帖子在同一次回答里，不另外请求
    assert_eq!(fake.on("graphql").len(), 1);
}

#[test]
fn long_posts_use_the_note_text() {
    let fake = Fake::start(Activate::Ok);
    let (out, _dir) = ok(&fake, 4);
    assert_eq!(
        out.text.as_deref(),
        Some(
            "A long post that goes past the usual length. https://example.com/notes\n\nIt keeps going with a second paragraph."
        )
    );
    assert_eq!(out.platform_tags, ["longform"]);
}

#[test]
fn a_repost_link_gives_the_original_post() {
    let fake = Fake::start(Activate::Ok);
    let (out, dir) = ok(&fake, 5);
    assert_eq!(out.identity, Some(link::identity("1849000000000000050")));
    assert_eq!(out.text.as_deref(), Some("The original post"));
    assert_eq!(read_json(dir.path(), "meta.json")["retweetId"], id(5));
}

#[test]
fn the_earlier_thread_by_the_same_author_comes_first() {
    let fake = Fake::start(Activate::Ok);
    let start = Instant::now();
    let (out, dir) = ok(&fake, 6);
    assert_eq!(
        out.text.as_deref(),
        Some("1/ a thread starts\n\n2/ the middle\n\n3/ and the end")
    );
    assert!(files(dir.path()).is_superset(&BTreeSet::from([
        format!("thread/{}.json", id(7)),
        format!("thread/{}.json", id(8)),
    ])));
    assert_eq!(
        read_json(dir.path(), "meta.json")["thread"],
        serde_json::json!([id(7), id(8)])
    );
    // 3 次帖子接口，一个令牌；串的请求之间隔了 1 秒
    assert_eq!(fake.on("graphql").len(), 3);
    assert_eq!(fake.on("api").len(), 1);
    assert!(start.elapsed() >= Duration::from_secs(2));
    // 第 8 条回复的是别人，不再往上
    assert!(
        out.missing_reason
            .as_deref()
            .is_none_or(|r| !r.contains("串"))
    );
}

#[test]
fn login_only_posts_are_not_found_private() {
    let fake = Fake::start(Activate::Ok);
    for (n, reason) in [
        (10, "NsfwLoggedOut"),
        (11, "NsfwViewerHasNoStatedAge"),
        (12, "Protected"),
    ] {
        let e = err(&fake, n);
        assert!(
            matches!(&e, Error::LoginOnly { reason: r } if r == reason),
            "{e}"
        );
        let f = e.to_failure(Lang::Zh);
        assert_eq!(f.code, ErrorCode::NotFound);
        assert_eq!(f.reason, Some(NotFoundReason::Private));
        assert_eq!(
            f.hint.as_deref(),
            Some("这条要登录着才看得到；usuyo 不用账号取 X，链接先存着")
        );
    }
    // 不重试、不退到嵌入接口
    assert_eq!(fake.on("graphql").len(), 3);
    assert!(fake.on("syndication").is_empty());
}

#[test]
fn tombstones_say_deleted_only_when_the_text_does() {
    let fake = Fake::start(Activate::Ok);
    let deleted = err(&fake, 13).to_failure(Lang::Zh);
    assert_eq!(deleted.code, ErrorCode::NotFound);
    assert_eq!(deleted.reason, Some(NotFoundReason::Deleted));
    let suspended = err(&fake, 14).to_failure(Lang::Zh);
    assert_eq!(suspended.code, ErrorCode::NotFound);
    assert_eq!(suspended.reason, None);
    let absent = err(&fake, 15).to_failure(Lang::Zh);
    assert_eq!(absent.code, ErrorCode::NotFound);
    assert_eq!(absent.reason, None);
}

#[test]
fn parameter_errors_are_upstream_changed_without_fallback() {
    let fake = Fake::start(Activate::Ok);
    for n in [16, 19] {
        let f = err(&fake, n).to_failure(Lang::Zh);
        assert_eq!(f.code, ErrorCode::UpstreamChanged, "{n}: {}", f.message);
        // 接口原样的回答跟着错误走（宿主记进失败日志）
        assert!(f.detail.is_some_and(|d| d.starts_with('{')), "{n}");
    }
    assert!(
        err(&fake, 16)
            .to_string()
            .contains("features cannot be null")
    );
    assert!(fake.on("syndication").is_empty());
}

#[test]
fn a_rate_limit_on_the_post_endpoint_waits_and_the_embed_method_says_so() {
    let fake = Fake::start(Activate::Ok);
    // 限流不在插件里换接口：宿主照节奏等，下一轮停着的这种跳过、换嵌入接口
    let f = err(&fake, 17).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::RateLimited);
    assert!(fake.on("syndication").is_empty());
    let (out, dir) = embed_ok(&fake, 17);
    let meta = read_json(dir.path(), "meta.json");
    assert_eq!(meta["api"], "syndication");
    assert!(meta.get("fallbackBecause").is_none());
    assert_eq!(
        out.text.as_deref(),
        Some(
            "First part of the thread\n\nSecond part of a long reply that the embed endpoint cuts short…"
        )
    );
    let reason = out.missing_reason.unwrap();
    assert!(reason.contains("长帖只拿到前一段"), "{reason}");
    assert!(reason.contains("走嵌入接口时串只取到上一条"), "{reason}");
    assert!(files(dir.path()).contains("thread/1849000000000000170.json"));
    // 嵌入接口不带任何凭据，token 照 react-tweet 的公式算
    let syn = fake.on("syndication");
    assert_eq!(syn.len(), 1);
    assert_eq!(fake.on("graphql").len(), 1, "嵌入接口这种方法不碰帖子接口");
    assert_eq!(syn[0].query("id").as_deref(), Some(id(17).as_str()));
    assert!(
        syn[0]
            .query("token")
            .is_some_and(|t| !t.is_empty() && !t.contains('.'))
    );
    no_credentials_leak(&fake);
}

#[test]
fn each_endpoint_reports_its_own_rate_limit() {
    let fake = Fake::start(Activate::Ok);
    let f = err(&fake, 18).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::RateLimited);
    assert!(f.retry_after.is_some());
    let f = embed_err(&fake, 18).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::RateLimited);
    assert!(f.retry_after.is_some());
    // 令牌发不出来、嵌入接口也限流
    let fake = Fake::start(Activate::Limited);
    let f = err(&fake, 18).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::RateLimited);
    assert!(fake.on("graphql").is_empty());
}

#[test]
fn refused_everywhere_is_blocked() {
    // 帖子接口 403，嵌入接口也 403：各自回被挡，宿主换下一种、停这一种
    let fake = Fake::start(Activate::Ok);
    let f = err(&fake, 24).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::Blocked);
    assert!(f.hint.unwrap().contains("出口"));
    assert!(fake.on("syndication").is_empty());
    assert_eq!(
        embed_err(&fake, 24).to_failure(Lang::Zh).code,
        ErrorCode::Blocked
    );
    // 换不到访客令牌
    let fake = Fake::start(Activate::Refused);
    assert_eq!(err(&fake, 24).to_failure(Lang::Zh).code, ErrorCode::Blocked);
    assert!(fake.on("graphql").is_empty());
}

#[test]
fn a_refused_guest_token_is_blocked_and_the_embed_method_still_works() {
    let fake = Fake::start(Activate::Refused);
    assert_eq!(err(&fake, 1).to_failure(Lang::Zh).code, ErrorCode::Blocked);
    let (out, dir) = embed_ok(&fake, 1);
    assert_eq!(read_json(dir.path(), "meta.json")["api"], "syndication");
    assert_eq!(
        out.text.as_deref(),
        Some("Two photos & a link https://example.com/brew #coffee")
    );
    assert_eq!(out.images.len(), 2);
    assert_eq!(out.published_at.unwrap().as_str(), "2024-10-30T12:00:00Z");
    // 嵌入接口说没有、说删了
    let f = embed_err(&fake, 25).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::NotFound);
    let f = embed_err(&fake, 15).to_failure(Lang::Zh);
    assert_eq!(f.code, ErrorCode::NotFound);
    let f = embed_err(&fake, 13).to_failure(Lang::Zh);
    assert_eq!(f.reason, Some(NotFoundReason::Deleted));
    no_credentials_leak(&fake);
}

#[test]
fn server_errors_are_network() {
    let fake = Fake::start(Activate::Ok);
    assert_eq!(err(&fake, 23).to_failure(Lang::Zh).code, ErrorCode::Network);
    assert!(fake.on("syndication").is_empty());
    // 连不上
    let gone = Fake::start(Activate::Ok);
    let site = usuyo_fetch_x::Site::loopback(
        "http://127.0.0.1:9",
        &gone.graphql,
        &gone.syndication,
        &gone.short,
        common::local_policy(),
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let e = fetch(
        &site,
        Request {
            api: Api::Guest,
            input: &link_to(1),
            out: out.path(),
            want: None,
            cache_dir: None,
        },
    )
    .unwrap_err();
    assert_eq!(e.to_failure(Lang::Zh).code, ErrorCode::Network);
}

#[test]
fn media_off_the_host_list_or_broken_is_missing_not_an_error() {
    let fake = Fake::start(Activate::Ok);
    let (out, dir) = ok(&fake, 20);
    assert_eq!(out.missing, [OutputKind::Images]);
    assert!(out.missing_reason.unwrap().contains("图片都没有下载下来"));
    assert!(
        read_json(dir.path(), "meta.json")["failed"][0]["why"]
            .as_str()
            .unwrap()
            .contains("pbs.twimg.com")
    );
    assert!(fake.on("media").is_empty());

    let (out, dir) = ok(&fake, 21);
    assert!(out.missing.contains(&OutputKind::Video));
    assert!(out.missing_reason.unwrap().contains("视频没有下载下来"));
    assert!(!dir.path().join("media/video-001.mp4").exists());
}

#[test]
fn articles_keep_the_title_and_say_the_body_needs_a_login() {
    let fake = Fake::start(Activate::Ok);
    let (out, _dir) = ok(&fake, 22);
    let text = out.text.unwrap();
    assert!(
        text.contains("**An article title**\n\nThe first lines of the article"),
        "{text}"
    );
    assert!(
        out.missing_reason
            .unwrap()
            .contains("文章正文要登录着的页面")
    );
}

#[test]
fn only_what_is_wanted_is_downloaded() {
    let fake = Fake::start(Activate::Ok);
    let (out, dir) = run(&fake, &link_to(3), Some(&[OutputKind::Text]), None);
    let out = out.unwrap();
    assert_eq!(out.completeness, Completeness::Full);
    assert!(out.images.is_empty() && out.media.is_empty() && out.author.is_none());
    assert!(fake.on("media").is_empty());
    assert_eq!(
        files(dir.path()),
        BTreeSet::from(["meta.json".to_owned(), "post.json".to_owned()])
    );
}

#[test]
fn the_guest_token_is_cached_and_a_stale_one_replaced_once() {
    let fake = Fake::start(Activate::Ok);
    let cache = tempfile::tempdir().unwrap();
    let file = cache.path().join("guest-token.json");
    std::fs::write(
        &file,
        serde_json::json!({"token": STALE_TOKEN, "obtainedAt": jiff::Timestamp::now().as_second()})
            .to_string(),
    )
    .unwrap();
    let (result, _o) = run(
        &fake,
        &link_to(1),
        Some(&[OutputKind::Text]),
        Some(cache.path()),
    );
    result.unwrap();
    let tokens: Vec<_> = fake
        .on("graphql")
        .iter()
        .map(|s| s.header("x-guest-token").unwrap().to_owned())
        .collect();
    assert_eq!(tokens, [STALE_TOKEN, FRESH_TOKEN]);
    assert_eq!(fake.on("api").len(), 1);
    assert_eq!(
        read_json(cache.path(), "guest-token.json")["token"],
        FRESH_TOKEN
    );
    // 下一次直接用缓存里的
    let (result, _o) = run(
        &fake,
        &link_to(4),
        Some(&[OutputKind::Text]),
        Some(cache.path()),
    );
    result.unwrap();
    assert_eq!(fake.on("api").len(), 1);
    // 过期的不用
    std::fs::write(
        &file,
        serde_json::json!({"token": FRESH_TOKEN, "obtainedAt": 1}).to_string(),
    )
    .unwrap();
    let (result, _o) = run(
        &fake,
        &link_to(4),
        Some(&[OutputKind::Text]),
        Some(cache.path()),
    );
    result.unwrap();
    assert_eq!(fake.on("api").len(), 2);
}

#[test]
fn spaces_and_foreign_links_are_unsupported_input() {
    let fake = Fake::start(Activate::Ok);
    for input in [
        "https://x.com/i/spaces/1YqKDqDXAbwKV",
        "https://example.com/a",
    ] {
        let (r, _o) = run(&fake, input, None, None);
        assert_eq!(
            r.unwrap_err().to_failure(Lang::Zh).code,
            ErrorCode::UnsupportedInput
        );
    }
    let (r, _o) = run(&fake, "https://t.co/elsewhere", None, None);
    assert_eq!(
        r.unwrap_err().to_failure(Lang::Zh).code,
        ErrorCode::UnsupportedInput
    );
    assert!(fake.on("graphql").is_empty());
}

#[test]
fn short_links_expand_by_looking_at_the_redirect_only() {
    let fake = Fake::start(Activate::Ok);
    let site = fake.site();
    let expect = Canonicalized::Match {
        identity: link::identity(&id(1)),
        resolved_url: Some(format!("{}?s=20&t=share", link_to(1))),
    };
    assert_eq!(
        link::canonicalize(&site, "https://t.co/abc").unwrap(),
        expect
    );
    assert_eq!(
        link::canonicalize(&site, "https://t.co/hop").unwrap(),
        expect
    );
    for page in ["https://t.co/page", "https://t.co/warn"] {
        assert!(matches!(
            link::canonicalize(&site, page).unwrap(),
            Canonicalized::Match { identity, .. } if identity == link::identity(&id(1))
        ));
    }
    assert_eq!(
        link::canonicalize(&site, "https://t.co/elsewhere").unwrap(),
        Canonicalized::NoMatch
    );
    assert_eq!(
        link::canonicalize(&site, "https://t.co/gone")
            .unwrap_err()
            .to_failure(Lang::Zh)
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        link::canonicalize(&site, "https://t.co/loop")
            .unwrap_err()
            .to_failure(Lang::Zh)
            .code,
        ErrorCode::UpstreamChanged
    );
    // 只问了 t.co，一个 cookie 也没带，帖子页一次也没开
    for s in fake.seen() {
        assert_eq!(s.server, "short");
        assert_eq!(s.header("cookie"), None);
        assert_eq!(s.header("user-agent"), Some("curl"));
    }
    // fetch 一个短链：先展开，再按 id 取
    let (r, _o) = run(&fake, "https://t.co/abc", Some(&[OutputKind::Text]), None);
    assert_eq!(r.unwrap().identity, Some(link::identity(&id(1))));
}

/// `fetch` 跑一遍，环境里点名了账号凭据文件，插件也一次都不打开它。
/// 凭据文件做成命名管道：谁打开它来读，就会一直卡住，外面的计时器判失败。
/// 环境变量要在子进程里设（这个仓库不写 `unsafe`），所以这里重新起一次测试程序，只跑下面那一条。
#[test]
fn a_fetch_never_opens_a_credentials_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fifos = Vec::new();
    for name in [
        "credentials.json",
        "cookies.txt",
        "visitor.json",
        "secrets.json",
    ] {
        let path = tmp.path().join(name);
        let status = within(
            Command::new("mkfifo").arg(&path).stdout(Stdio::null()),
            Duration::from_secs(10),
        );
        assert!(status.success(), "mkfifo failed");
        fifos.push(path);
    }
    let cache = tmp.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "credentials_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("USUYO_X_CREDENTIALS_CHILD", "1")
        .env("USUYO_CREDENTIALS", &fifos[0])
        .env("USUYO_COOKIES_TXT", &fifos[1])
        .env("USUYO_VISITOR", &fifos[2])
        .env("USUYO_SECRETS", &fifos[3])
        .env("USUYO_CACHE_DIR", &cache)
        .stdout(std::fs::File::create(tmp.path().join("child.txt")).unwrap())
        .stderr(Stdio::null());
    let status = within(&mut child, Duration::from_secs(60));
    assert!(
        status.success(),
        "the child fetch failed or opened a credentials file"
    );
    let report = std::fs::read_to_string(tmp.path().join("child.txt")).unwrap();
    assert!(
        report.contains("1 passed"),
        "the child test did not run: {report}"
    );
}

#[test]
fn credentials_child() {
    if std::env::var_os("USUYO_X_CREDENTIALS_CHILD").is_none() {
        return;
    }
    let fake = Fake::start(Activate::Ok);
    let out = tempfile::tempdir().unwrap();
    let args: Vec<String> = [
        "fetch",
        &link_to(3),
        "--out",
        out.path().to_str().unwrap(),
        "--method",
        "guest",
    ]
    .map(String::from)
    .to_vec();
    assert_eq!(cli::run(&args, &fake.site()), 0);
    assert!(out.path().join("images/q-001.jpg").exists());
    assert!(out.path().join("media/q-video-001.mp4").exists());
    let args: Vec<String> = ["canonicalize", "https://t.co/abc"]
        .map(String::from)
        .to_vec();
    assert_eq!(cli::run(&args, &fake.site()), 0);
    assert_eq!(cli::run(&["health".into()], &fake.site()), 0);
}

/// 起一个进程，最多等 `limit`；到时还没退出就杀掉，判失败。
fn within(cmd: &mut Command, limit: Duration) -> std::process::ExitStatus {
    let mut child = cmd.spawn().unwrap();
    let end = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > end {
            child.kill().ok();
            child.wait().ok();
            panic!("the process did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
