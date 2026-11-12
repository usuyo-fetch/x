//! 本机的假 X：换访客令牌、帖子接口、嵌入接口、`t.co`、图片视频各一个服务，
//! 回答是 `tests/fixtures/` 里照上游源码手写的样例；记下每个请求（路径、几个请求头），
//! 测试据此核对请求次数和带了什么。不碰真网络。只有 `tests/fetch.rs` 用它。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use url::Url;
use usuyo_fetch_x::Site;
use usuyo_plugin_kit::http::Policy;

pub const STALE_TOKEN: &str = "1111111111";
pub const FRESH_TOKEN: &str = "1900000000000000001";
pub const JPEG: &[u8] = b"\xff\xd8\xff\xe0fake-jpeg";
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png";
pub const MP4: &[u8] = b"\x00\x00\x00\x18ftypmp42fake-video-bytes";
pub const QUERY_PATH: &str = "/i/api/graphql/2ICDjqPd81tulZcYrtpTuQ/TweetResultByRestId";

#[derive(Debug, Clone)]
pub struct Seen {
    pub server: &'static str,
    pub method: String,
    pub target: String,
    pub headers: HashMap<String, String>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn query(&self, key: &str) -> Option<String> {
        let url = Url::parse(&format!("http://h{}", self.target)).ok()?;
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }
}

/// 换访客令牌的服务怎么答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activate {
    Ok,
    Refused,
    Limited,
}

/// 测试用的地址策略：只有 127.0.0.1 算公网，不查 DNS。
pub fn local_policy() -> Policy {
    Policy {
        lookup: |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no DNS in tests",
            ))
        },
        public: |ip| ip == IpAddr::from([127, 0, 0, 1]),
        cookies_over_http: false,
        ..Policy::strict()
    }
}

pub struct Fake {
    pub api: String,
    pub graphql: String,
    pub syndication: String,
    pub short: String,
    pub media: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

struct Ctx {
    activate: Activate,
    media: String,
    short: String,
}

impl Fake {
    pub fn start(activate: Activate) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let bind = || TcpListener::bind("127.0.0.1:0").unwrap();
        let (api, graphql, syndication, short, media) = (bind(), bind(), bind(), bind(), bind());
        let base = |l: &TcpListener| format!("http://{}", l.local_addr().unwrap());
        let fake = Self {
            api: base(&api),
            graphql: base(&graphql),
            syndication: base(&syndication),
            short: base(&short),
            media: base(&media),
            seen: seen.clone(),
        };
        let ctx = Arc::new(Ctx {
            activate,
            media: fake.media.clone(),
            short: fake.short.clone(),
        });
        spawn(api, "api", ctx.clone(), seen.clone(), api_route);
        spawn(graphql, "graphql", ctx.clone(), seen.clone(), graphql_route);
        spawn(
            syndication,
            "syndication",
            ctx.clone(),
            seen.clone(),
            syndication_route,
        );
        spawn(short, "short", ctx.clone(), seen.clone(), short_route);
        spawn(media, "media", ctx, seen, media_route);
        fake
    }

    pub fn site(&self) -> Site {
        Site::loopback(
            &self.api,
            &self.graphql,
            &self.syndication,
            &self.short,
            local_policy(),
        )
        .unwrap()
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn on(&self, server: &str) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.server == server)
            .collect()
    }
}

pub struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    headers: Vec<String>,
}

fn reply(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Reply {
    Reply {
        status,
        content_type,
        body: body.into(),
        headers: Vec::new(),
    }
}

fn redirect(to: &str) -> Reply {
    Reply {
        headers: vec![format!("Location: {to}")],
        ..reply(301, "text/html", Vec::new())
    }
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// 样例里的一条：`{status, headers?, body}`，换掉 `{MEDIA}`、`{RESET}`、`{RESET_LATER}`。
fn canned(file: &str, id: &str, ctx: &Ctx) -> Option<Reply> {
    let all = fixture(file);
    let entry = all.get(id)?;
    let now = jiff::Timestamp::now().as_second();
    let fill = |s: &str| {
        s.replace("{MEDIA}", &ctx.media)
            .replace("{RESET}", &(now + 600).to_string())
            .replace("{RESET_LATER}", &(now + 900).to_string())
    };
    let body = fill(&entry["body"].to_string());
    let headers = entry["headers"]
        .as_array()
        .map(|h| h.iter().filter_map(|v| v.as_str()).map(fill).collect())
        .unwrap_or_default();
    Some(Reply {
        headers,
        ..reply(
            entry["status"].as_u64().unwrap() as u16,
            "application/json;charset=utf-8",
            body,
        )
    })
}

fn query(target: &str, key: &str) -> Option<String> {
    let url = Url::parse(&format!("http://h{target}")).ok()?;
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn api_route(seen: &Seen, ctx: &Ctx) -> Reply {
    if seen.method != "POST" || seen.target != "/1.1/guest/activate.json" {
        return reply(404, "application/json", "{}");
    }
    match ctx.activate {
        Activate::Ok => reply(
            200,
            "application/json",
            format!(r#"{{"guest_token":"{FRESH_TOKEN}"}}"#),
        ),
        Activate::Refused => reply(
            403,
            "application/json",
            r#"{"errors":[{"code":200,"message":"Forbidden."}]}"#,
        ),
        Activate::Limited => Reply {
            headers: vec![format!(
                "x-rate-limit-reset: {}",
                jiff::Timestamp::now().as_second() + 300
            )],
            ..reply(429, "application/json", "{}")
        },
    }
}

fn graphql_route(seen: &Seen, ctx: &Ctx) -> Reply {
    if seen.target.split('?').next() != Some(QUERY_PATH) {
        return reply(
            404,
            "application/json",
            r#"{"errors":[{"message":"Not found"}]}"#,
        );
    }
    if seen.header("x-guest-token") == Some(STALE_TOKEN) {
        return reply(
            403,
            "application/json",
            r#"{"errors":[{"code":239,"message":"Bad guest token."}]}"#,
        );
    }
    let id = query(&seen.target, "variables")
        .and_then(|v| serde_json::from_str::<Value>(&v).ok())
        .and_then(|v| v["tweetId"].as_str().map(str::to_owned))
        .unwrap_or_default();
    canned("graphql.json", &id, ctx)
        .unwrap_or_else(|| reply(200, "application/json", r#"{"data":{"tweetResult":{}}}"#))
}

fn syndication_route(seen: &Seen, ctx: &Ctx) -> Reply {
    if seen.target.split('?').next() != Some("/tweet-result") {
        return reply(404, "text/plain", "");
    }
    let id = query(&seen.target, "id").unwrap_or_default();
    canned("syndication.json", &id, ctx).unwrap_or_else(|| reply(404, "application/json", "{}"))
}

const POST_1: &str = "https://x.com/sample_person/status/1849000000000000001";

fn short_route(seen: &Seen, ctx: &Ctx) -> Reply {
    match seen.target.as_str() {
        "/abc" => redirect(&format!("{POST_1}?s=20&t=share")),
        "/hop" => redirect(&format!("{}/abc", ctx.short)),
        "/page" => reply(
            200,
            "text/html; charset=utf-8",
            format!(
                r#"<head><meta name="referrer" content="always"><noscript><META http-equiv="refresh" content="0;URL={POST_1}"></noscript><title>{POST_1}</title></head><script>window.opener = null; location.replace("{}")</script>"#,
                POST_1.replace('/', "\\/")
            ),
        ),
        "/warn" => redirect(&format!(
            "https://twitter.com/safety/unsafe_link_warning?unsafe_link={}",
            url::form_urlencoded::byte_serialize(POST_1.as_bytes()).collect::<String>()
        )),
        "/elsewhere" => redirect("https://example.com/article"),
        "/loop" => redirect(&format!("{}/loop", ctx.short)),
        _ => reply(404, "text/html", "<html>not found</html>"),
    }
}

fn media_route(seen: &Seen, _ctx: &Ctx) -> Reply {
    let path = seen.target.split('?').next().unwrap_or_default();
    let format = query(&seen.target, "format");
    match path {
        "/video/broken.mp4" => reply(500, "text/plain", "boom"),
        p if p.starts_with("/video/") && p.ends_with(".mp4") => reply(200, "video/mp4", MP4),
        p if p.starts_with("/media/") => match format.as_deref() {
            Some("png") => reply(200, "image/png", PNG),
            Some("jpg") => reply(200, "image/jpeg", JPEG),
            _ => reply(404, "text/plain", "no such size"),
        },
        _ => reply(404, "text/plain", ""),
    }
}

type Route = fn(&Seen, &Ctx) -> Reply;

fn spawn(
    listener: TcpListener,
    server: &'static str,
    ctx: Arc<Ctx>,
    seen: Arc<Mutex<Vec<Seen>>>,
    route: Route,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = seen.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let target = parts.next().unwrap_or("/").to_owned();
                let mut headers = HashMap::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                    }
                }
                let length: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).ok();
                let request = Seen {
                    server,
                    method,
                    target,
                    headers,
                };
                seen.lock().unwrap().push(request.clone());
                let r = route(&request, &ctx);
                let mut head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    r.status,
                    r.content_type,
                    r.body.len()
                );
                for h in &r.headers {
                    head.push_str(h);
                    head.push_str("\r\n");
                }
                head.push_str("\r\n");
                stream.write_all(head.as_bytes()).ok();
                stream.write_all(&r.body).ok();
            });
        }
    });
}
