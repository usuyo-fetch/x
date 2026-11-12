//! 认链接（协议 §3.5，设计 x.md 第 5 节）：帖子链接和转发域名离线认出 id；
//! `t.co` 只请求 `t.co`、只看它跳去哪里，不带 cookie，一看到帖子链接就停。

use std::time::Duration;

use url::Url;
use usuyo_plugin_kit::protocol::{Canonicalized, Identity, PlatformId};

use crate::error::Error;
use crate::http::{self, Http, Purpose};
use crate::site::{CANONICAL_ORIGIN, Site};

pub const PLATFORM: &str = "x";
const SHORT_HOPS: usize = 3;
const SHORT_TIMEOUT: Duration = Duration::from_secs(8);
const SHORT_PAGE_BYTES: u64 = 256 << 10;

/// 帖子 id：十进制，`^[0-9]{1,20}$`。
pub fn post_id(text: &str) -> Option<String> {
    (!text.is_empty() && text.len() <= 20 && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.to_owned())
}

pub fn canonical_url(id: &str) -> String {
    format!("{CANONICAL_ORIGIN}/i/web/status/{id}")
}

pub fn post_url(screen_name: &str, id: &str) -> String {
    format!("{CANONICAL_ORIGIN}/{screen_name}/status/{id}")
}

pub fn identity(id: &str) -> Identity {
    Identity::Platform {
        platform: PlatformId::try_from(PLATFORM).expect("constant platform id"),
        content_id: id.into(),
        url: Some(canonical_url(id)),
    }
}

/// 一条链接是什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Link {
    Post(String),
    Short(Url),
    /// X 上的 Spaces、直播：认得出，但不取。
    Unsupported(&'static str),
}

/// 照 yt-dlp `TwitterBaseIE._BASE_REGEX`：`x.com`、`twitter.com`，可带 `www.`、`mobile.`、`m.`。
fn is_x_host(host: &str) -> bool {
    let bare = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("mobile."))
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(host);
    matches!(bare, "x.com" | "twitter.com")
}

/// FxEmbed README 里写的两个转发域名：只离线认 id，从不请求它们。
fn is_mirror_host(host: &str) -> bool {
    matches!(host, "fxtwitter.com" | "fixupx.com")
}

/// 路径照 yt-dlp `TwitterIE._VALID_URL`：`<screen_name>/status/<id>`、`i/web/status/<id>`、
/// `statuses/<id>`，后面可以跟 `/photo/<n>`、`/video/<n>`（不进身份）。
fn id_in_path(url: &Url) -> Option<String> {
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let (id, rest) = match segments.as_slice() {
        ["i", "web", "status", id, rest @ ..] => (id, rest),
        ["statuses", id, rest @ ..] => (id, rest),
        [_, "status", id, rest @ ..] => (id, rest),
        _ => return None,
    };
    let rest_ok = match rest {
        [] => true,
        ["photo" | "video", n] => n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty(),
        _ => false,
    };
    rest_ok.then(|| post_id(id)).flatten()
}

fn is_spaces_or_live(url: &Url) -> Option<&'static str> {
    let first: Vec<&str> = url.path_segments()?.take(2).collect();
    match first.as_slice() {
        ["i", "spaces"] => Some("X Spaces"),
        ["i", "broadcasts" | "events"] => Some("X live broadcast"),
        _ => None,
    }
}

pub(crate) fn classify(site: &Site, input: &str) -> Option<Link> {
    let url = Url::parse(input.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if is_x_host(&host) {
        if let Some(what) = is_spaces_or_live(&url) {
            return Some(Link::Unsupported(what));
        }
        return id_in_path(&url).map(Link::Post);
    }
    if is_mirror_host(&host) {
        return id_in_path(&url).map(Link::Post);
    }
    if host == "t.co" || site.is_short(&url) {
        url.path_segments()?.find(|s| !s.is_empty())?;
        let mut target = site.short().clone();
        target.set_path(url.path());
        return Some(Link::Short(target));
    }
    None
}

/// 展开 `t.co`：只请求 `t.co`，不带 cookie；一看到 X 的帖子链接就停。
/// 返回 `Ok(Some((id, 展开后的链接)))`；指向别处（`t.co` 也给普通网页做短链）回 `Ok(None)`。
pub(crate) fn expand(http: &Http<'_>, short: &Url) -> Result<Option<(String, Url)>, Error> {
    let mut at = short.clone();
    for _ in 0..SHORT_HOPS {
        let mut response = http.send(&at, Purpose::Short, SHORT_TIMEOUT)?;
        let status = response.status();
        let next = match http::location(&response, &at)? {
            Some(next) => next,
            None => match status {
                200 => {
                    let body = http.read_capped(&mut response, SHORT_PAGE_BYTES)?;
                    refresh_target(&String::from_utf8_lossy(&body), &at)?
                }
                404 | 410 => {
                    return Err(Error::NotFound {
                        message: format!("t.co answered HTTP {status}"),
                        deleted: false,
                    });
                }
                429 => {
                    return Err(Error::RateLimited {
                        message: "t.co answered HTTP 429".into(),
                        retry_after: http::retry_after(&response),
                    });
                }
                500..=599 => return Err(Error::Network(format!("t.co answered HTTP {status}"))),
                _ => {
                    return Err(Error::UpstreamChanged(format!(
                        "t.co answered HTTP {status} without a redirect"
                    )));
                }
            },
        };
        let next = unwrap_unsafe_link(next);
        if http.site().is_short(&next) {
            at = next;
            continue;
        }
        return Ok(match classify(http.site(), next.as_str()) {
            Some(Link::Post(id)) => Some((id, next)),
            _ => None,
        });
    }
    Err(Error::UpstreamChanged(format!(
        "t.co redirected more than {SHORT_HOPS} times"
    )))
}

/// `t.co` 回 200 时，去处写在页面里。没核过它会不会这样回（设计第 5 节），
/// 照它给浏览器的写法认两种：`<meta http-equiv="refresh" content="0;URL=…">` 和 `location.replace("…")`。
fn refresh_target(page: &str, from: &Url) -> Result<Url, Error> {
    let lower = page.to_ascii_lowercase();
    let from_meta = lower.find("http-equiv=\"refresh\"").and_then(|at| {
        let tag_end = lower[at..].find('>')? + at;
        let tag = &page[at..tag_end];
        let url_at = tag.to_ascii_lowercase().find("url=")? + 4;
        let rest = &tag[url_at..];
        Some(rest.split(['"', '\'']).next()?.trim().to_owned())
    });
    let from_script = || {
        let at = page.find("location.replace(")? + "location.replace(".len();
        let rest = page[at..].trim_start();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let inner = &rest[1..];
        Some(inner[..inner.find(quote)?].replace("\\/", "/"))
    };
    let raw = from_meta.or_else(from_script).ok_or_else(|| {
        Error::UpstreamChanged("t.co answered a page without a redirect target".into())
    })?;
    from.join(&raw)
        .map_err(|e| Error::UpstreamChanged(format!("t.co target `{raw}`: {e}")))
}

/// yt-dlp `TwitterShortenerIE`：去处是 X 的「不安全链接」提示页时，取它带着的原链接。
fn unwrap_unsafe_link(url: Url) -> Url {
    let is_warning = url
        .host_str()
        .is_some_and(|h| is_x_host(&h.to_ascii_lowercase()))
        && url.path() == "/safety/unsafe_link_warning";
    if !is_warning {
        return url;
    }
    url.query_pairs()
        .find(|(k, _)| k == "unsafe_link")
        .and_then(|(_, v)| Url::parse(&v).ok())
        .unwrap_or(url)
}

pub fn canonicalize(site: &Site, input: &str) -> Result<Canonicalized, Error> {
    match classify(site, input) {
        None | Some(Link::Unsupported(_)) => Ok(Canonicalized::NoMatch),
        Some(Link::Post(id)) => Ok(Canonicalized::Match {
            identity: identity(&id),
            resolved_url: None,
        }),
        Some(Link::Short(short)) => {
            let http = Http::new(site, Duration::from_secs(9));
            Ok(match expand(&http, &short)? {
                Some((id, url)) => Canonicalized::Match {
                    identity: identity(&id),
                    resolved_url: Some(url.into()),
                },
                None => Canonicalized::NoMatch,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1849000000000000001";

    fn id_of(input: &str) -> Option<String> {
        match classify(&Site::x(), input)? {
            Link::Post(id) => Some(id),
            _ => None,
        }
    }

    #[test]
    fn post_link_shapes() {
        let yes = [
            format!("https://x.com/someone/status/{ID}"),
            format!("https://twitter.com/someone/status/{ID}"),
            format!("https://www.x.com/someone/status/{ID}"),
            format!("https://mobile.twitter.com/someone/status/{ID}"),
            format!("https://m.x.com/someone/status/{ID}/"),
            format!("http://x.com/someone/status/{ID}?s=20&t=AbCdEf"),
            format!("https://x.com/someone/status/{ID}/photo/1"),
            format!("https://x.com/someone/status/{ID}/video/2"),
            format!("https://x.com/i/web/status/{ID}"),
            format!("https://twitter.com/statuses/{ID}"),
            format!("https://X.COM/Someone/status/{ID}"),
            format!("https://fxtwitter.com/someone/status/{ID}"),
            format!("https://fixupx.com/someone/status/{ID}/photo/1"),
        ];
        for input in yes {
            assert_eq!(id_of(&input).as_deref(), Some(ID), "{input}");
        }
        let no = [
            "https://x.com/someone".to_owned(),
            "https://x.com/someone/status/".to_owned(),
            "https://x.com/someone/status/12a".to_owned(),
            format!("https://x.com/someone/status/{ID}/analytics"),
            format!("https://x.com/someone/status/{ID}/photo/x"),
            "https://x.com/someone/status/123456789012345678901".to_owned(),
            format!("https://vxtwitter.com/someone/status/{ID}"),
            format!("https://d.fxtwitter.com/someone/status/{ID}"),
            format!("https://notx.com/someone/status/{ID}"),
            format!("https://x.com.evil.example/someone/status/{ID}"),
            format!("ftp://x.com/someone/status/{ID}"),
            "not a url".to_owned(),
        ];
        for input in no {
            assert_eq!(id_of(&input), None, "{input}");
        }
    }

    #[test]
    fn spaces_and_live_are_recognised_but_not_posts() {
        let site = Site::x();
        for input in [
            "https://x.com/i/spaces/1YqKDqDXAbwKV",
            "https://twitter.com/i/broadcasts/1ypKdgVXZRBGW",
        ] {
            assert!(matches!(classify(&site, input), Some(Link::Unsupported(_))));
            assert_eq!(canonicalize(&site, input).unwrap(), Canonicalized::NoMatch);
        }
    }

    #[test]
    fn short_links_are_only_recognised_offline() {
        let site = Site::x();
        assert!(matches!(
            classify(&site, "https://t.co/AbC123xyz"),
            Some(Link::Short(_))
        ));
        assert!(matches!(
            classify(&site, "http://t.co/AbC123xyz"),
            Some(Link::Short(u)) if u.scheme() == "https"
        ));
        assert!(classify(&site, "https://t.co/").is_none());
    }

    #[test]
    fn canonical_identity_drops_share_parameters() {
        let site = Site::x();
        let got = canonicalize(
            &site,
            &format!("https://twitter.com/someone/status/{ID}?s=46&t=Zx9"),
        )
        .unwrap();
        assert_eq!(
            got,
            Canonicalized::Match {
                identity: identity(ID),
                resolved_url: None
            }
        );
        let Identity::Platform { url, .. } = identity(ID) else {
            panic!()
        };
        assert_eq!(url.unwrap(), format!("https://x.com/i/web/status/{ID}"));
    }

    #[test]
    fn refresh_page_targets() {
        let from = Url::parse("https://t.co/abc").unwrap();
        let meta = r#"<head><meta name="referrer" content="always"><noscript><META http-equiv="refresh" content="0;URL=https://x.com/a/status/1849000000000000001"></noscript><title>https://x.com/a/status/1849000000000000001</title></head>"#;
        assert_eq!(
            refresh_target(meta, &from).unwrap().as_str(),
            format!("https://x.com/a/status/{ID}")
        );
        let script = r#"<script>window.opener = null; location.replace("https:\/\/example.com\/page")</script>"#;
        assert_eq!(
            refresh_target(script, &from).unwrap().as_str(),
            "https://example.com/page"
        );
        assert!(matches!(
            refresh_target("<html>nothing</html>", &from),
            Err(Error::UpstreamChanged(_))
        ));
    }

    #[test]
    fn unsafe_link_warning_is_unwrapped() {
        let u = Url::parse(
            "https://twitter.com/safety/unsafe_link_warning?unsafe_link=https%3A%2F%2Fexample.com%2Fa",
        )
        .unwrap();
        assert_eq!(unwrap_unsafe_link(u).as_str(), "https://example.com/a");
    }
}
