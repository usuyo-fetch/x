//! 取一条帖子（协议 §3.6、§3.7，设计 x.md 第 3、6、7 节），两种方法（§2.3.1）：`guest` 用访客令牌调帖子接口，
//! `embed` 调嵌入用的 syndication 接口。哪个先试、哪个接着由宿主照清单排：帖子接口被拒（令牌、出口）回
//! `blocked`，宿主换嵌入接口；限流回 `rate-limited`，宿主照节奏等。输出里写明走的是哪一个。
//! 不带任何 cookie，不读账号凭据；按 `--want` 下载图片和视频。

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use url::Url;
use usuyo_plugin_kit::protocol::{
    Access, Author, Completeness, Evidence, FetchOutput, ItemKind, Media, MediaKind, OutputKind,
    PluginEvent, RelPath, Rfc3339, TextFormat,
};

use crate::error::Error;
use crate::http::{self, Http, Purpose};
use crate::link::{self, Link};
use crate::post::{self, Answer, MediaItem, MediaType, Post};
use crate::site::{CANONICAL_ORIGIN, Site};
use crate::upstream;

/// 本插件能给的输出，和清单里的 `outputs` 一致。
pub const OUTPUTS: [OutputKind; 4] = [
    OutputKind::Text,
    OutputKind::Images,
    OutputKind::Video,
    OutputKind::Meta,
];

/// 整次调用留给网络的时间，比清单里 `fetch.timeout: 30m` 短，留出收尾。
const BUDGET: Duration = Duration::from_secs(29 * 60);
const API_TIMEOUT: Duration = Duration::from_secs(20);
const API_BYTES: u64 = 16 << 20;
const IMAGE_TIMEOUT: Duration = Duration::from_secs(60);
const IMAGE_BYTES: u64 = 40 << 20;
/// 一条帖子最多 4 张图，引用的再 4 张。
const MAX_IMAGES: usize = 8;
const VIDEO_TIMEOUT: Duration = Duration::from_secs(25 * 60);
const VIDEO_BYTES: u64 = 2 << 30;
const PROGRESS_STEP: u64 = 8 << 20;
/// 往上取同一作者的串：最多这么多条，每条之间隔这么久（设计第 3 节，推断值）。
const THREAD_MAX: usize = 20;
const THREAD_GAP: Duration = Duration::from_secs(1);
const GUEST_CACHE_FILE: &str = "guest-token.json";

/// 走哪个接口：清单里的方法 `guest`、`embed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Guest,
    Embed,
}

pub struct Request<'a> {
    pub api: Api,
    /// 原链接或 `canonicalize` 给的 `resolvedUrl`。
    pub input: &'a str,
    pub out: &'a Path,
    /// `None` 是全要。
    pub want: Option<BTreeSet<OutputKind>>,
    /// 宿主给的插件缓存目录（`USUYO_CACHE_DIR`），访客令牌存在这里；没有就每次换一个。
    pub cache_dir: Option<PathBuf>,
}

/// 这次是哪个接口给的回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Graphql,
    Syndication,
}

impl Route {
    fn as_str(self) -> &'static str {
        match self {
            Route::Graphql => "TweetResultByRestId",
            Route::Syndication => "syndication",
        }
    }
}

/// 接口不肯答的几种。
#[derive(Debug)]
enum Trouble {
    Limited {
        message: String,
        retry_after: Option<u64>,
    },
    Refused(String),
    /// 令牌本身被说不对（`Bad guest token`），换一个再试一次。
    BadToken(String),
    Other(Error),
}

impl From<Error> for Trouble {
    fn from(e: Error) -> Self {
        Trouble::Other(e)
    }
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Trouble::Limited { message, .. } => write!(f, "rate limited: {message}"),
            Trouble::Refused(m) => write!(f, "refused: {m}"),
            Trouble::BadToken(m) => write!(f, "guest token rejected: {m}"),
            Trouble::Other(e) => write!(f, "{e}"),
        }
    }
}

pub fn fetch(site: &Site, request: Request<'_>) -> Result<FetchOutput, Error> {
    let wanted = request.want.clone().unwrap_or_else(|| OUTPUTS.into());
    if let Some(other) = wanted.iter().find(|w| !OUTPUTS.contains(w)) {
        return Err(Error::InvalidArgs(format!("cannot give `{other}`")));
    }
    let http = Http::new(site, BUDGET);
    let id = match link::classify(site, request.input) {
        Some(Link::Post(id)) => id,
        Some(Link::Short(short)) => {
            link::expand(&http, &short)?
                .map(|(id, _)| id)
                .ok_or_else(|| {
                    Error::Unsupported(format!("`{}` does not lead to an X post", request.input))
                })?
        }
        Some(Link::Unsupported(what)) => {
            return Err(Error::Unsupported(format!("{what} is not fetched")));
        }
        None => {
            return Err(Error::Unsupported(format!(
                "`{}` is not an X post link",
                request.input
            )));
        }
    };

    let fetched_at = Rfc3339::from_timestamp(jiff::Timestamp::now());
    let mut guest = Guest::new(request.cache_dir.as_deref());
    let (answer, route) = match request.api {
        Api::Guest => (
            tweet_result(&http, &mut guest, &id).map_err(|t| refusal(Route::Graphql, t))?,
            Route::Graphql,
        ),
        Api::Embed => (
            syndication(&http, &id).map_err(|t| refusal(Route::Syndication, t))?,
            Route::Syndication,
        ),
    };
    let post = into_post(answer)?;
    let thread = match route {
        Route::Graphql => walk_up(&http, &mut guest, &post),
        Route::Syndication => parent_only(&post),
    };
    write_post(
        &http,
        Fetched {
            post,
            thread,
            route,
        },
        &wanted,
        fetched_at,
        request.out,
    )
}

/// 接口不肯答时报什么（设计第 7 节）：限流是限流，被拒（令牌、出口）是被挡，宿主换另一种方法。
fn refusal(route: Route, trouble: Trouble) -> Error {
    let message = format!("{}: {trouble}", route.as_str());
    match trouble {
        Trouble::Other(e) => e,
        Trouble::Limited { retry_after, .. } => Error::RateLimited {
            message,
            retry_after,
        },
        Trouble::Refused(_) | Trouble::BadToken(_) => Error::Blocked(message),
    }
}

fn into_post(answer: Answer) -> Result<Post, Error> {
    match answer {
        Answer::Post(p) => Ok(*p),
        Answer::Unavailable(Some(reason))
            if matches!(
                reason.as_str(),
                "NsfwLoggedOut" | "NsfwViewerHasNoStatedAge" | "Protected"
            ) =>
        {
            Err(Error::LoginOnly { reason })
        }
        Answer::Unavailable(reason) => Err(Error::NotFound {
            message: format!(
                "TweetUnavailable ({})",
                reason.as_deref().unwrap_or("no reason")
            ),
            deleted: false,
        }),
        // 账号被封的帖子也是墓碑；只有文字明说删了才记 deleted
        Answer::Tombstone(text) => Err(Error::NotFound {
            deleted: text
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().contains("deleted")),
            message: format!("tombstone: {}", text.as_deref().unwrap_or("no text")),
        }),
        Answer::Absent => Err(Error::NotFound {
            message: "the API has no such post".into(),
            deleted: false,
        }),
    }
}

// ---------- 访客令牌 ----------

/// 访客令牌：插件自己一次 POST 换来的，不是账号凭据；照 gallery-dl 在缓存目录里留一小时。
struct Guest<'c> {
    cache: Option<&'c Path>,
    token: Option<String>,
    from_cache: bool,
}

impl<'c> Guest<'c> {
    fn new(cache: Option<&'c Path>) -> Self {
        Self {
            cache,
            token: None,
            from_cache: false,
        }
    }

    fn cache_file(&self) -> Option<PathBuf> {
        self.cache.map(|d| d.join(GUEST_CACHE_FILE))
    }

    fn token(&mut self, http: &Http<'_>) -> Result<String, Trouble> {
        if let Some(t) = &self.token {
            return Ok(t.clone());
        }
        if let Some(t) = self.cached() {
            self.from_cache = true;
            self.token = Some(t.clone());
            return Ok(t);
        }
        let t = activate(http)?;
        self.store(&t);
        self.from_cache = false;
        self.token = Some(t.clone());
        Ok(t)
    }

    /// 缓存里还没过期的令牌。读不了、过期、形状不对都当没有，在 stderr 说一声。
    fn cached(&self) -> Option<String> {
        let path = self.cache_file()?;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                eprintln!("guest token cache unreadable, asking for a new one: {e}");
                return None;
            }
        };
        let parsed = serde_json::from_str::<Value>(&text).ok().and_then(|v| {
            let token = v.get("token")?.as_str()?.to_owned();
            Some((token, v.get("obtainedAt")?.as_i64()?))
        });
        let Some((token, at)) = parsed.filter(|(t, _)| guest_token_shape(t)) else {
            eprintln!("guest token cache has an unexpected shape, asking for a new one");
            return None;
        };
        let age = jiff::Timestamp::now().as_second() - at;
        (0..upstream::GUEST_TOKEN_TTL_SECS)
            .contains(&age)
            .then_some(token)
    }

    fn store(&self, token: &str) {
        let Some(path) = self.cache_file() else {
            eprintln!("no USUYO_CACHE_DIR: the guest token is not kept for the next call");
            return;
        };
        let body = json!({"token": token, "obtainedAt": jiff::Timestamp::now().as_second()});
        if let Err(e) = fs::write(&path, body.to_string()) {
            eprintln!("could not keep the guest token in the cache: {e}");
        }
    }

    /// 令牌被说不对：丢掉。缓存里的换一个再试一次；刚换的也不对，当作被拒。
    fn reject(&mut self, why: String) -> Result<(), Trouble> {
        self.token = None;
        if let Some(path) = self.cache_file()
            && let Err(e) = fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("could not drop the cached guest token: {e}");
        }
        if self.from_cache {
            self.from_cache = false;
            Ok(())
        } else {
            Err(Trouble::Refused(why))
        }
    }
}

/// 访客令牌是一串数字（gallery-dl 把它 `str()` 了，yt-dlp 只认字符串）。
fn guest_token_shape(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn activate(http: &Http<'_>) -> Result<String, Trouble> {
    let mut url = http.site().api().clone();
    url.set_path(upstream::GUEST_ACTIVATE_PATH);
    let mut response = http.send(&url, Purpose::Activate, API_TIMEOUT)?;
    let status = response.status();
    match status {
        200 => {}
        401 | 403 => {
            return Err(Trouble::Refused(format!(
                "guest/activate.json answered HTTP {status}"
            )));
        }
        429 => {
            return Err(Trouble::Limited {
                message: "guest/activate.json answered HTTP 429".into(),
                retry_after: http::retry_after(&response),
            });
        }
        500..=599 => {
            return Err(
                Error::Network(format!("guest/activate.json answered HTTP {status}")).into(),
            );
        }
        _ => {
            return Err(Error::UpstreamChanged(format!(
                "guest/activate.json answered HTTP {status}"
            ))
            .into());
        }
    }
    let body = http.read_capped(&mut response, 64 << 10)?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|e| Error::UpstreamChanged(format!("guest/activate.json: {e}")))?;
    // gallery-dl `_guest_token` 用 `str(...)`：数字和字符串都收
    let token = match value.get("guest_token") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => {
            return Err(
                Error::UpstreamChanged("guest/activate.json has no guest_token".into()).into(),
            );
        }
    };
    if !guest_token_shape(&token) {
        return Err(
            Error::UpstreamChanged("guest/activate.json gave an odd guest_token".into()).into(),
        );
    }
    Ok(token)
}

// ---------- 两个接口 ----------

fn tweet_result(http: &Http<'_>, guest: &mut Guest<'_>, id: &str) -> Result<Answer, Trouble> {
    loop {
        let token = guest.token(http)?;
        match tweet_result_once(http, &token, id) {
            Err(Trouble::BadToken(why)) => guest.reject(why)?,
            other => return other,
        }
    }
}

fn tweet_result_once(http: &Http<'_>, token: &str, id: &str) -> Result<Answer, Trouble> {
    let mut url = http.site().graphql().clone();
    url.set_path(upstream::TWEET_RESULT_PATH);
    url.query_pairs_mut()
        .extend_pairs(upstream::tweet_result_query(id));
    let mut response = http.send(&url, Purpose::Graphql { guest_token: token }, API_TIMEOUT)?;
    let status = response.status();
    if status == 429 {
        return Err(Trouble::Limited {
            message: "TweetResultByRestId answered HTTP 429".into(),
            retry_after: http::retry_after(&response),
        });
    }
    if (500..=599).contains(&status) {
        return Err(Error::Network(format!("TweetResultByRestId answered HTTP {status}")).into());
    }
    let body = http.read_capped(&mut response, API_BYTES)?;
    // 错误的回答也可能不是 JSON；那时只按状态码判
    let parsed = serde_json::from_slice::<Value>(&body);
    if status == 200
        && let Err(e) = &parsed
    {
        return Err(
            Error::UpstreamChanged(format!("TweetResultByRestId answered non-JSON: {e}"))
                .answered(&body)
                .into(),
        );
    }
    let value = parsed.unwrap_or(Value::Null);
    let errors = api_errors(&value);
    let described = || {
        format!(
            "TweetResultByRestId answered HTTP {status}{}",
            if errors.is_empty() {
                String::new()
            } else {
                format!(" ({})", errors.join(", "))
            }
        )
    };
    // 令牌过期或不对：X 回 `Bad guest token`（code 239）
    if errors
        .iter()
        .any(|e| e.to_ascii_lowercase().contains("guest token"))
    {
        return Err(Trouble::BadToken(described()));
    }
    match status {
        200 => {}
        401 | 403 => return Err(Trouble::Refused(described())),
        _ => return Err(Error::UpstreamChanged(described()).answered(&body).into()),
    }
    match post::graphql_answer(&value) {
        Some(answer) => Ok(answer),
        // 回答里没有 `tweetResult`：多半是 queryId 或 features 不对（设计第 7 节）
        None => Err(
            Error::UpstreamChanged(format!("{} without data.tweetResult", described()))
                .answered(&body)
                .into(),
        ),
    }
}

/// `errors[].message`，去掉 yt-dlp 也忽略的那种。
fn api_errors(value: &Value) -> Vec<String> {
    value
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let msg = e.get("message")?.as_str()?;
            (!upstream::is_ignorable_api_error(msg)).then(|| msg.chars().take(200).collect())
        })
        .collect()
}

fn syndication(http: &Http<'_>, id: &str) -> Result<Answer, Trouble> {
    let mut url = http.site().syndication().clone();
    url.set_path(upstream::SYNDICATION_PATH);
    url.query_pairs_mut()
        .extend_pairs(upstream::syndication_query(id));
    let mut response = http.send(&url, Purpose::Syndication, API_TIMEOUT)?;
    let status = response.status();
    match status {
        200 => {}
        404 => return Ok(Answer::Absent),
        429 => {
            return Err(Trouble::Limited {
                message: "syndication answered HTTP 429".into(),
                retry_after: http::retry_after(&response),
            });
        }
        401 | 403 => {
            return Err(Trouble::Refused(format!(
                "syndication answered HTTP {status}"
            )));
        }
        500..=599 => {
            return Err(Error::Network(format!("syndication answered HTTP {status}")).into());
        }
        _ => {
            return Err(
                Error::UpstreamChanged(format!("syndication answered HTTP {status}")).into(),
            );
        }
    }
    let body = http.read_capped(&mut response, API_BYTES)?;
    let value: Value = serde_json::from_slice(&body).map_err(|e| {
        Error::UpstreamChanged(format!("syndication answered non-JSON: {e}")).answered(&body)
    })?;
    post::syndication_answer(&value).ok_or_else(|| {
        Error::UpstreamChanged("syndication answer has no id_str".into())
            .answered(&body)
            .into()
    })
}

// ---------- 前面的串 ----------

struct Thread {
    /// 从近到远：第一条是这条帖子回复的那一条。
    posts: Vec<Post>,
    /// 停下来的原因，给 `missingReason`。
    note: Option<String>,
}

fn self_reply_parent(p: &Post) -> Option<&str> {
    let author = p.author.as_ref()?.id.as_deref()?;
    (p.reply_to_user.as_deref() == Some(author))
        .then_some(p.reply_to_status.as_deref())
        .flatten()
}

/// 这条是作者回复自己的，就按 `in_reply_to_status_id_str` 往上取，最多 [`THREAD_MAX`] 条，
/// 每条之间隔 [`THREAD_GAP`]；不是作者自己回的就停。
fn walk_up(http: &Http<'_>, guest: &mut Guest<'_>, post: &Post) -> Thread {
    let mut posts: Vec<Post> = Vec::new();
    let author = post.author.as_ref().and_then(|a| a.id.clone());
    let mut note = None;
    loop {
        let current = posts.last().unwrap_or(post);
        let Some(parent) = self_reply_parent(current).map(str::to_owned) else {
            break;
        };
        if posts.len() == THREAD_MAX {
            note = Some(format!("往上的串只取了 {THREAD_MAX} 条"));
            break;
        }
        if http.time_left() < Duration::from_secs(60) {
            note = Some(format!("往上的串只取到 {} 条：时间不够了", posts.len()));
            break;
        }
        std::thread::sleep(THREAD_GAP);
        match tweet_result(http, guest, &parent) {
            Ok(Answer::Post(p)) if p.author.as_ref().and_then(|a| a.id.clone()) == author => {
                posts.push(*p)
            }
            Ok(Answer::Post(_)) => break,
            Ok(_) => {
                note = Some(format!(
                    "往上的串在第 {} 条停了：那一条看不到",
                    posts.len() + 1
                ));
                break;
            }
            Err(why) => {
                note = Some(format!("往上的串只取到 {} 条：{why}", posts.len()));
                break;
            }
        }
    }
    Thread { posts, note }
}

/// 嵌入接口只给上一条（`parent`）。
fn parent_only(post: &Post) -> Thread {
    let author = post.author.as_ref().and_then(|a| a.id.as_deref());
    match (&post.parent, self_reply_parent(post)) {
        (Some(parent), Some(_))
            if parent.author.as_ref().and_then(|a| a.id.as_deref()) == author =>
        {
            Thread {
                note: self_reply_parent(parent).map(|_| "走嵌入接口时串只取到上一条".to_owned()),
                posts: vec![(**parent).clone()],
            }
        }
        (None, Some(_)) => Thread {
            posts: Vec::new(),
            note: Some("走嵌入接口时没有拿到上一条".into()),
        },
        _ => Thread {
            posts: Vec::new(),
            note: None,
        },
    }
}

// ---------- 写输出 ----------

struct Fetched {
    post: Post,
    thread: Thread,
    route: Route,
}

fn write_post(
    http: &Http<'_>,
    fetched: Fetched,
    wanted: &BTreeSet<OutputKind>,
    fetched_at: Rfc3339,
    out: &Path,
) -> Result<FetchOutput, Error> {
    let Fetched {
        post,
        thread,
        route,
    } = fetched;
    let mut missing = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();

    fs::write(out.join("post.json"), pretty(&post.raw)?)?;
    if !thread.posts.is_empty() {
        fs::create_dir_all(out.join("thread"))?;
        for p in &thread.posts {
            fs::write(out.join(format!("thread/{}.json", p.id)), pretty(&p.raw)?)?;
        }
    }
    if let Some(n) = &thread.note {
        reasons.push(n.clone());
    }

    let quoted = match &post.quoted {
        Some(Ok(q)) => Some(q.as_ref()),
        Some(Err(why)) => {
            reasons.push(why.clone());
            None
        }
        None => None,
    };

    // 正文：前面的串按时间在前，本条，再把引用的帖子写成引用块（设计第 6 节）
    let text = if wanted.contains(&OutputKind::Text) {
        let md = markdown(&post, &thread.posts, quoted);
        if post.truncated || quoted.is_some_and(|q| q.truncated) {
            reasons.push("长帖只拿到前一段（嵌入接口只给前一段）".into());
        }
        if post.article.is_some() {
            reasons.push("文章正文要登录着的页面，只存了标题和摘要".into());
        }
        if md.is_empty() {
            missing.push(OutputKind::Text);
            reasons.push("这条帖子没有文字".into());
            None
        } else {
            Some(md)
        }
    } else {
        None
    };

    let mut images = Vec::new();
    let mut image_records = Vec::new();
    if wanted.contains(&OutputKind::Images) {
        let photos: Vec<(&str, usize, &MediaItem)> =
            numbered(&post, quoted, |m| m.kind == MediaType::Photo);
        for (prefix, n, item) in photos.iter().take(MAX_IMAGES) {
            let Some(raw) = &item.url else {
                failed
                    .push(json!({"image": format!("{prefix}{n:03}"), "why": "回答里没有图片地址"}));
                continue;
            };
            match download_image(http, raw, prefix, *n, out) {
                Ok((path, source)) => {
                    image_records.push(json!({
                        "path": path.as_str(), "url": raw, "requested": source,
                        "width": item.width, "height": item.height, "alt": item.alt
                    }));
                    images.push(path);
                }
                Err(why) => failed.push(json!({"url": raw, "why": why})),
            }
        }
        let lost = photos.len().min(MAX_IMAGES) - images.len();
        // 帖子本来就没有图片不算缺；有而一张都没下载下来才算。
        if images.is_empty() {
            if !photos.is_empty() {
                missing.push(OutputKind::Images);
                reasons.push("图片都没有下载下来".into());
            }
        } else if lost > 0 {
            reasons.push(format!("有 {lost} 张图片没有下载下来"));
        }
    }

    let mut media = Vec::new();
    let mut video_records = Vec::new();
    if wanted.contains(&OutputKind::Video) {
        let videos = numbered(&post, quoted, |m| m.kind != MediaType::Photo);
        for (prefix, n, item) in &videos {
            let name = format!("media/{prefix}video-{n:03}.mp4");
            let Some(raw) = &item.url else {
                failed.push(json!({"video": name, "why": "回答里没有 mp4 地址"}));
                continue;
            };
            match download_video(http, raw, &name, out) {
                Ok((path, mime)) => {
                    video_records.push(json!({
                        "path": path.as_str(), "url": raw, "bitrate": item.bitrate,
                        "type": match item.kind { MediaType::AnimatedGif => "animated_gif", _ => "video" },
                        "width": item.width, "height": item.height, "durationMs": item.duration_ms
                    }));
                    media.push(Media {
                        path,
                        kind: MediaKind::Video,
                        mime: Some(mime),
                        lang: None,
                        duration_sec: item.duration_ms.map(|ms| ms as f64 / 1000.0),
                        group: None,
                    });
                }
                Err(why) => failed.push(json!({"url": raw, "why": why})),
            }
        }
        // 同上：没有视频的帖子不算缺视频。
        if media.is_empty() {
            if !videos.is_empty() {
                missing.push(OutputKind::Video);
                reasons.push("视频没有下载下来".into());
            }
        } else if media.len() < videos.len() {
            reasons.push(format!(
                "有 {} 个视频没有下载下来",
                videos.len() - media.len()
            ));
        }
    }

    let screen_name = post.author.as_ref().and_then(|a| a.screen_name.clone());
    let author = post.author.as_ref().and_then(|a| {
        let name = a.name.clone().or_else(|| a.screen_name.clone())?;
        Some(Author {
            name,
            platform_id: a.id.clone(),
            url: a
                .screen_name
                .as_ref()
                .map(|s| format!("{CANONICAL_ORIGIN}/{s}")),
        })
    });
    let meta_wanted = wanted.contains(&OutputKind::Meta);
    if meta_wanted && author.is_none() && post.created_at.is_none() {
        missing.push(OutputKind::Meta);
        reasons.push("回答里没有作者和发布时间".into());
    }
    let meta = json!({
        "id": post.id,
        "url": screen_name.as_deref().map_or_else(|| link::canonical_url(&post.id), |s| link::post_url(s, &post.id)),
        "api": route.as_str(),
        "author": post.author.as_ref().map(|a| json!({"id": a.id, "screenName": a.screen_name, "name": a.name})),
        "createdAt": post.created_at.as_ref().map(Rfc3339::as_str),
        "lang": post.lang,
        "counts": post.counts,
        "hashtags": post.hashtags,
        "retweetId": post.retweet_id,
        "replyTo": post.reply_to_status,
        "thread": thread.posts.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        "quoted": quoted.map(|q| json!({
            "id": q.id,
            "screenName": q.author.as_ref().and_then(|a| a.screen_name.clone()),
        })),
        "article": post.article.as_ref().map(|a| json!({"title": a.title, "preview": a.preview})),
        "images": image_records,
        "videos": video_records,
        "failed": failed,
        "missing": reasons,
    });
    fs::write(out.join("meta.json"), pretty(&meta)?)?;

    missing.sort();
    missing.dedup();
    let completeness = if missing.is_empty() {
        Completeness::Full
    } else {
        Completeness::Partial
    };
    let title = first_line(&post.text);
    Ok(FetchOutput {
        identity: Some(link::identity(&post.id)),
        kind: ItemKind::Post,
        title: title.filter(|_| meta_wanted),
        author: author.filter(|_| meta_wanted),
        published_at: post.created_at.clone().filter(|_| meta_wanted),
        text_format: text.as_ref().map(|_| TextFormat::Markdown),
        text,
        html: None,
        images,
        media,
        platform_tags: if meta_wanted {
            post.hashtags.clone()
        } else {
            Vec::new()
        },
        comments: None,
        completeness,
        missing,
        missing_reason: (!reasons.is_empty()).then(|| reasons.join("；")),
        evidence: Evidence {
            fetched_at,
            access: Access::PublicWeb,
            site: None,
            upstream: None,
            status: Some(200),
        },
        credentials_update: None,
    })
}

/// 本条和引用帖子里合条件的媒体，编号：本条 `001`…，引用的 `q-001`…。
fn numbered<'p>(
    post: &'p Post,
    quoted: Option<&'p Post>,
    keep: impl Fn(&MediaItem) -> bool,
) -> Vec<(&'static str, usize, &'p MediaItem)> {
    let pick = |p: &'p Post, prefix: &'static str| {
        p.media
            .iter()
            .filter(|m| keep(m))
            .enumerate()
            .map(move |(i, m)| (prefix, i + 1, m))
            .collect::<Vec<_>>()
    };
    let mut all = pick(post, "");
    if let Some(q) = quoted {
        all.extend(pick(q, "q-"));
    }
    all
}

fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut title: String = line.chars().take(100).collect();
    if line.chars().count() > 100 {
        title.push('…');
    }
    Some(title)
}

fn markdown(post: &Post, thread: &[Post], quoted: Option<&Post>) -> String {
    let mut parts: Vec<String> = thread
        .iter()
        .rev()
        .map(|p| p.text.clone())
        .filter(|t| !t.is_empty())
        .collect();
    if !post.text.is_empty() {
        parts.push(post.text.clone());
    }
    if let Some(a) = &post.article {
        let mut block = String::new();
        if let Some(t) = &a.title {
            block.push_str(&format!("**{t}**"));
        }
        if let Some(p) = &a.preview {
            if !block.is_empty() {
                block.push_str("\n\n");
            }
            block.push_str(p);
        }
        if !block.is_empty() {
            parts.push(block);
        }
    }
    if let Some(q) = quoted {
        let who = q
            .author
            .as_ref()
            .and_then(|a| a.screen_name.as_deref())
            .map_or_else(|| link::canonical_url(&q.id), |s| link::post_url(s, &q.id));
        let mut block = format!("> 引用 {who}");
        for line in q.text.lines() {
            block.push_str("\n>");
            if !line.is_empty() {
                block.push(' ');
                block.push_str(line);
            }
        }
        parts.push(block);
    }
    parts.join("\n\n")
}

fn pretty(value: &Value) -> Result<Vec<u8>, Error> {
    serde_json::to_vec_pretty(value).map_err(|e| Error::Internal(format!("encode JSON: {e}")))
}

fn rel(path: &str) -> RelPath {
    RelPath::try_from(path).expect("plugin-made relative path")
}

/// 原图地址：`…/media/<名字>.jpg` 改成 `…/media/<名字>?format=jpg&name=orig`，
/// 已经带 `name=` 的换成 `orig`（gallery-dl `_extract_media` 和 `_init_sizes` 的 `orig`）。
pub(crate) fn original_photo(url: &Url) -> Url {
    let mut out = url.clone();
    if url.query().is_none() {
        let path = url.path();
        let last = path.rsplit('/').next().unwrap_or_default();
        if let Some((stem, ext)) = last.rsplit_once('.')
            && !stem.is_empty()
            && (3..=4).contains(&ext.len())
        {
            out.set_path(&path[..path.len() - ext.len() - 1]);
            out.query_pairs_mut()
                .append_pair("format", ext)
                .append_pair("name", "orig");
        }
        return out;
    }
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == "name" {
                "orig".into()
            } else {
                v.into_owned()
            };
            (k.into_owned(), v)
        })
        .collect();
    out.query_pairs_mut().clear().extend_pairs(pairs);
    out
}

/// 下载一张图。返回写进 `meta.json` 的原因；写盘失败另算。
fn download_image(
    http: &Http<'_>,
    raw: &str,
    prefix: &str,
    n: usize,
    out: &Path,
) -> Result<(RelPath, String), String> {
    let url = http
        .site()
        .media_url(raw)
        .ok_or_else(|| "图片地址不在 pbs.twimg.com 上，没有去取".to_owned())?;
    let url = original_photo(&url);
    let mut response = http
        .send(&url, Purpose::Image, IMAGE_TIMEOUT)
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("HTTP {}", response.status()));
    }
    let ext = response
        .header("content-type")
        .and_then(image_extension)
        .ok_or_else(|| "不是图片".to_owned())?;
    let bytes = http
        .read_capped(&mut response, IMAGE_BYTES)
        .map_err(|e| e.to_string())?;
    let name = format!("images/{prefix}{n:03}.{ext}");
    fs::create_dir_all(out.join("images")).map_err(|e| e.to_string())?;
    fs::write(out.join(&name), &bytes).map_err(|e| e.to_string())?;
    Ok((rel(&name), url.into()))
}

fn image_extension(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        _ => return None,
    })
}

/// 下载一路视频：边下边写，超过上限就停，失败时删掉写了一半的文件。
fn download_video(
    http: &Http<'_>,
    raw: &str,
    name: &str,
    out: &Path,
) -> Result<(RelPath, String), String> {
    let url = http
        .site()
        .media_url(raw)
        .ok_or_else(|| "视频地址不在 video.twimg.com 上，没有去取".to_owned())?;
    let response = http
        .send(&url, Purpose::Video, VIDEO_TIMEOUT)
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("HTTP {}", response.status()));
    }
    let mime = response
        .header("content-type")
        .unwrap_or("video/mp4")
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !(mime.starts_with("video/") || mime == "application/octet-stream") {
        return Err(format!("不是视频（{mime}）"));
    }
    let total = response
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok());
    let path = out.join(name);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let cap = http.byte_cap(VIDEO_BYTES);
    let mut done = 0u64;
    let result = (|| {
        let mut reader = response.into_reader(cap);
        let mut file = File::create(&path).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 << 10];
        let mut reported = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| {
                if e.kind() == std::io::ErrorKind::FileTooLarge && cap < VIDEO_BYTES {
                    http::BYTES_SPENT.to_owned()
                } else {
                    e.to_string()
                }
            })?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if done - reported >= PROGRESS_STEP {
                reported = done;
                progress(done, total);
            }
        }
        file.flush().map_err(|e| e.to_string())?;
        if done == 0 {
            return Err("视频是空的".to_owned());
        }
        Ok(())
    })();
    let over = matches!(&result, Err(why) if why == http::BYTES_SPENT);
    http.spend(if over { cap } else { done });
    match result {
        Ok(()) => Ok((
            rel(name),
            if mime == "application/octet-stream" {
                "video/mp4".into()
            } else {
                mime
            },
        )),
        Err(why) => {
            // 写了一半的文件不留在输出里；删不掉也只是多一个没被引用的文件
            if let Err(e) = fs::remove_file(&path) {
                eprintln!("could not remove partial {name}: {e}");
            }
            Err(why)
        }
    }
}

fn progress(done: u64, total: Option<u64>) {
    let mb = |b: u64| (b as f64 / (1u64 << 20) as f64 * 10.0).round() / 10.0;
    let event = PluginEvent::Progress {
        done: Some(mb(done)),
        total: total.map(mb),
        unit: Some("MB".into()),
        message: Some("下载视频".into()),
    };
    eprintln!("{}", event.stderr_line());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_photo_urls() {
        let u = |s: &str| original_photo(&Url::parse(s).unwrap()).to_string();
        assert_eq!(
            u("https://pbs.twimg.com/media/GaBcDeFg.jpg"),
            "https://pbs.twimg.com/media/GaBcDeFg?format=jpg&name=orig"
        );
        assert_eq!(
            u("https://pbs.twimg.com/media/GaBcDeFg?format=png&name=small"),
            "https://pbs.twimg.com/media/GaBcDeFg?format=png&name=orig"
        );
        assert_eq!(
            u("https://pbs.twimg.com/tweet_video_thumb/abc.jpg"),
            "https://pbs.twimg.com/tweet_video_thumb/abc?format=jpg&name=orig"
        );
    }

    #[test]
    fn refusals() {
        let limited = Trouble::Limited {
            message: "429".into(),
            retry_after: Some(900),
        };
        assert!(matches!(
            refusal(Route::Graphql, limited),
            Error::RateLimited {
                retry_after: Some(900),
                ..
            }
        ));
        assert!(matches!(
            refusal(Route::Syndication, Trouble::Refused("403".into())),
            Error::Blocked(_)
        ));
        assert!(matches!(
            refusal(Route::Graphql, Trouble::BadToken("x".into())),
            Error::Blocked(_)
        ));
        assert!(matches!(
            refusal(Route::Graphql, Trouble::Other(Error::Network("x".into()))),
            Error::Network(_)
        ));
    }

    #[test]
    fn titles() {
        assert_eq!(first_line("\n  hello \nworld").as_deref(), Some("hello"));
        assert_eq!(first_line("   ").as_deref(), None);
        let long = "字".repeat(150);
        assert_eq!(first_line(&long).unwrap().chars().count(), 101);
    }
}
