//! 发请求的唯一入口：主机先过 [`Site::may_request`]，再经 `usuyo_plugin_kit::http`
//! 按用途只连那一类主机、只连公网地址。重定向一律不自动跟，不带任何 cookie，
//! 每个请求都有超时，整次调用有总时限和总字节数。

use std::cell::Cell;
use std::time::{Duration, Instant};

use url::Url;
use usuyo_plugin_kit::http::{Client, Hosts, Response};

use crate::error::Error;
use crate::site::Site;
use crate::upstream::WEB_BEARER;

/// 普通桌面 Chrome 的 UA（设计第 3 节：嵌入接口用普通 UA，不照 yt-dlp 用 `Googlebot`）。
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
/// 展开 `t.co` 用 yt-dlp `TwitterShortenerIE` 的 UA（`headers={'User-Agent': 'curl'}`），让它回重定向。
const SHORT_USER_AGENT: &str = "curl";
const ACCEPT_JSON: &str = "application/json, */*;q=0.8";
const ACCEPT_IMAGE: &str = "image/avif,image/webp,image/apng,image/*,*/*;q=0.8";
const ACCEPT_ANY: &str = "*/*";
/// 整次调用最多读这么多字节：接口回答、图片、视频加起来。
const CALL_BYTES: u64 = 4 << 30;

pub(crate) struct Http<'s> {
    site: &'s Site,
    /// 每类主机一个客户端，各自只连那一类。
    api: Client,
    graphql: Client,
    syndication: Client,
    short: Client,
    media: Client,
    deadline: Instant,
    bytes_left: Cell<u64>,
}

/// 一个请求是做什么的。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Purpose<'t> {
    /// `POST guest/activate.json`，带公开 bearer。
    Activate,
    /// 帖子接口，带公开 bearer 和访客令牌。
    Graphql {
        guest_token: &'t str,
    },
    /// 嵌入接口，什么凭据都不带。
    Syndication,
    /// 展开 `t.co`。
    Short,
    Image,
    Video,
}

impl<'s> Http<'s> {
    /// `budget` 是整次调用留给网络的总时长，要比宿主的总超时短，留出收尾的时间。
    pub(crate) fn new(site: &'s Site, budget: Duration) -> Self {
        let client = |ua: &str, hosts: Vec<String>| {
            Client::new(ua, Hosts::only(hosts)).with_policy(site.policy())
        };
        Self {
            site,
            api: client(USER_AGENT, Site::host_of(site.api())),
            graphql: client(USER_AGENT, Site::host_of(site.graphql())),
            syndication: client(USER_AGENT, Site::host_of(site.syndication())),
            short: client(SHORT_USER_AGENT, Site::host_of(site.short())),
            media: client(USER_AGENT, site.media_hosts()),
            deadline: Instant::now() + budget,
            bytes_left: Cell::new(CALL_BYTES),
        }
    }

    pub(crate) fn site(&self) -> &Site {
        self.site
    }

    pub(crate) fn time_left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// 一次请求，不跟重定向。`timeout` 是这个请求从发出到读完的上限，再受总时限约束。
    pub(crate) fn send(
        &self,
        url: &Url,
        purpose: Purpose<'_>,
        timeout: Duration,
    ) -> Result<Response, Error> {
        let origin = match purpose {
            Purpose::Activate => Some(self.site.api()),
            Purpose::Graphql { .. } => Some(self.site.graphql()),
            Purpose::Syndication => Some(self.site.syndication()),
            Purpose::Short => Some(self.site.short()),
            Purpose::Image | Purpose::Video => None,
        };
        let allowed = match origin {
            Some(o) => crate::site::same_host(o, url),
            None => self.site.is_media(url),
        };
        if !allowed || !self.site.may_request(url) {
            return Err(Error::Internal(format!(
                "refusing to contact `{}` for {purpose:?}: not on the plugin's host list",
                url.host_str().unwrap_or_default()
            )));
        }
        let left = self.time_left();
        if left < Duration::from_secs(1) {
            return Err(Error::Network("the call's time budget is used up".into()));
        }
        if self.bytes_left.get() == 0 {
            return Err(Error::Network(BYTES_SPENT.into()));
        }
        let bearer = format!("Bearer {WEB_BEARER}");
        let request = match purpose {
            Purpose::Activate => self
                .api
                .post(url, Vec::new())
                .header("Accept", ACCEPT_JSON)
                .header("Authorization", &bearer),
            Purpose::Graphql { guest_token } => self
                .graphql
                .get(url)
                .header("Accept", ACCEPT_JSON)
                .header("Authorization", &bearer)
                .header("x-guest-token", guest_token),
            Purpose::Syndication => self.syndication.get(url).header("Accept", ACCEPT_JSON),
            Purpose::Short => self.short.get(url).header("Accept", ACCEPT_ANY),
            Purpose::Image => self.media.get(url).header("Accept", ACCEPT_IMAGE),
            Purpose::Video => self.media.get(url).header("Accept", ACCEPT_ANY),
        };
        request
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(timeout.min(left))
            .send()
            .map_err(|e| Error::Network(e.to_string()))
    }

    /// 一个响应最多读多少：它自己的上限和整次调用剩下的，取小的。
    pub(crate) fn byte_cap(&self, limit: u64) -> u64 {
        limit.min(self.bytes_left.get())
    }

    /// 从整次调用的字节总量里扣掉读过的。
    pub(crate) fn spend(&self, bytes: u64) {
        self.bytes_left
            .set(self.bytes_left.get().saturating_sub(bytes));
    }

    /// 读整个响应体，超过 `limit` 字节就算失败；读失败时不知道读了多少，按读满算。
    pub(crate) fn read_capped(
        &self,
        response: &mut Response,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        let cap = self.byte_cap(limit);
        let result = response.read_to_vec(cap);
        self.spend(result.as_ref().map_or(cap, |b| b.len() as u64));
        result.map_err(|e| match e {
            usuyo_plugin_kit::http::Error::TooLarge(_) if cap < limit => {
                Error::Network(BYTES_SPENT.into())
            }
            usuyo_plugin_kit::http::Error::TooLarge(_) => {
                Error::UpstreamChanged(format!("response is larger than {limit} bytes"))
            }
            other => Error::Network(other.to_string()),
        })
    }
}

pub(crate) const BYTES_SPENT: &str = "the call's byte budget is used up";

/// 重定向的去处，按请求的地址解析相对路径。
pub(crate) fn location(response: &Response, from: &Url) -> Result<Option<Url>, Error> {
    if !(300..400).contains(&response.status()) {
        return Ok(None);
    }
    let raw = response.header("location").ok_or_else(|| {
        Error::UpstreamChanged(format!("HTTP {} without Location", response.status()))
    })?;
    from.join(raw)
        .map(Some)
        .map_err(|e| Error::UpstreamChanged(format!("bad Location `{raw}`: {e}")))
}

/// 多少秒后再试：`x-rate-limit-reset` 是 Unix 时间（gallery-dl `_handle_ratelimit` 读它），
/// 没有就看 `Retry-After` 的秒数。
pub(crate) fn retry_after(response: &Response) -> Option<u64> {
    if let Some(reset) = response
        .header("x-rate-limit-reset")
        .and_then(|v| v.trim().parse::<i64>().ok())
    {
        let now = jiff::Timestamp::now().as_second();
        return Some(reset.saturating_sub(now).max(1) as u64);
    }
    response
        .header("retry-after")
        .and_then(|v| v.trim().parse().ok())
}
