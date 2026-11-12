//! 插件能连哪些主机。所有请求都先过 [`Site::may_request`]，名单之外的一律不发。
//!
//! 正式运行只用 [`Site::x`]；测试用 [`Site::loopback`] 把每一类主机都指到本机，
//! 它只接受回环地址，所以测试不可能连到 X。

use std::net::IpAddr;

use url::Url;
use usuyo_plugin_kit::http::Policy;

/// 规范链接和作者链接总写真站点，测试里也一样：它们只用于去重和显示。
pub const CANONICAL_ORIGIN: &str = "https://x.com";

/// 换访客令牌、帖子接口、嵌入接口、短链、图片视频五类主机。
#[derive(Debug, Clone)]
pub struct Site {
    /// `guest/activate.json` 所在的源。
    api: Url,
    /// 帖子接口（`/i/api/graphql/…`）所在的源。
    graphql: Url,
    /// 嵌入用的 syndication 接口。
    syndication: Url,
    /// `t.co` 短链。
    short: Url,
    /// 图片、视频的主机，按整个主机名匹配。
    media: Vec<String>,
    /// 回答里的 `http://` 媒体地址改走 `https://`，带端口的不取。
    media_https: bool,
    /// 地址策略：正式运行只连公网地址。
    policy: Policy,
}

impl Site {
    /// X 本身。和清单 `permissions.network.hosts` 保持一致。
    pub fn x() -> Self {
        let parse = |u: &str| Url::parse(u).expect("constant URL");
        Self {
            api: parse("https://api.x.com"),
            graphql: parse(CANONICAL_ORIGIN),
            syndication: parse("https://cdn.syndication.twimg.com"),
            short: parse("https://t.co"),
            media: vec!["pbs.twimg.com".into(), "video.twimg.com".into()],
            media_https: true,
            policy: Policy::from_host(),
        }
    }

    /// 测试用：每一类各是一个本机地址，图片视频只能在 127.0.0.1 上。
    /// `policy` 由测试给，让本机测试服务算公网。
    pub fn loopback(
        api: &str,
        graphql: &str,
        syndication: &str,
        short: &str,
        policy: Policy,
    ) -> Result<Self, String> {
        let parse = |text: &str| -> Result<Url, String> {
            let url = Url::parse(text).map_err(|e| format!("`{text}`: {e}"))?;
            match url.host_str().and_then(|h| h.parse::<IpAddr>().ok()) {
                Some(ip) if ip.is_loopback() => Ok(url),
                _ => Err(format!("`{text}` is not a loopback address")),
            }
        };
        Ok(Self {
            api: parse(api)?,
            graphql: parse(graphql)?,
            syndication: parse(syndication)?,
            short: parse(short)?,
            media: vec!["127.0.0.1".into()],
            media_https: false,
            policy,
        })
    }

    pub(crate) fn policy(&self) -> Policy {
        self.policy
    }

    pub(crate) fn api(&self) -> &Url {
        &self.api
    }

    pub(crate) fn graphql(&self) -> &Url {
        &self.graphql
    }

    pub(crate) fn syndication(&self) -> &Url {
        &self.syndication
    }

    pub(crate) fn short(&self) -> &Url {
        &self.short
    }

    pub(crate) fn media_hosts(&self) -> Vec<String> {
        self.media.clone()
    }

    pub(crate) fn host_of(url: &Url) -> Vec<String> {
        url.host_str().map(str::to_owned).into_iter().collect()
    }

    pub(crate) fn is_short(&self, url: &Url) -> bool {
        same_host(&self.short, url)
    }

    pub(crate) fn is_media(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && url
                .host_str()
                .is_some_and(|host| self.media.iter().any(|m| m == host))
    }

    /// 回答给的图片视频地址：只认名单里的主机，正式运行时升到 https、只走默认端口。
    pub(crate) fn media_url(&self, raw: &str) -> Option<Url> {
        let mut url = Url::parse(raw.trim()).ok()?;
        if self.media_https {
            if url.port().is_some() {
                return None;
            }
            if url.scheme() == "http" {
                url.set_scheme("https").ok()?;
            }
        }
        self.is_media(&url).then_some(url)
    }

    /// 唯一的出站闸门。
    pub(crate) fn may_request(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && (same_host(&self.api, url)
                || same_host(&self.graphql, url)
                || same_host(&self.syndication, url)
                || same_host(&self.short, url)
                || self.is_media(url))
    }
}

/// 同一个主机名，端口一致（默认端口记作没写）。
pub(crate) fn same_host(origin: &Url, url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.host_str() == origin.host_str()
        && url.port() == origin.port()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_hosts() {
        let site = Site::x();
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(site.may_request(&u("https://api.x.com/1.1/guest/activate.json")));
        assert!(site.may_request(&u("https://x.com/i/api/graphql/a/TweetResultByRestId")));
        assert!(site.may_request(&u("https://cdn.syndication.twimg.com/tweet-result")));
        assert!(site.may_request(&u("https://t.co/abc")));
        assert!(site.is_media(&u("https://pbs.twimg.com/media/a.jpg")));
        assert!(site.is_media(&u("https://video.twimg.com/ext_tw_video/1/pu/vid/a.mp4")));
        assert!(!site.is_media(&u("https://abs.twimg.com/a.js")));
        assert!(!site.may_request(&u("https://twitter.com/a/status/1")));
        assert!(!site.may_request(&u("https://example.com/")));
        assert!(
            site.media_url("https://pbs.twimg.com:8443/media/a.jpg")
                .is_none()
        );
        assert_eq!(
            site.media_url("http://pbs.twimg.com/media/a.jpg")
                .unwrap()
                .scheme(),
            "https"
        );
        assert!(site.media_url("https://evil.example/media/a.jpg").is_none());
    }

    #[test]
    fn loopback_refuses_real_hosts() {
        let p = Policy::strict();
        let l = "http://127.0.0.1:1";
        assert!(Site::loopback("https://api.x.com", l, l, l, p).is_err());
        assert!(Site::loopback(l, "https://x.com", l, l, p).is_err());
        assert!(Site::loopback(l, l, "https://cdn.syndication.twimg.com", l, p).is_err());
        assert!(Site::loopback(l, l, l, "https://t.co", p).is_err());
        let site = Site::loopback(l, l, l, l, p).unwrap();
        assert!(!site.may_request(&Url::parse("https://x.com/i/api/graphql/a/b").unwrap()));
        assert!(!site.may_request(&Url::parse("https://pbs.twimg.com/media/a.jpg").unwrap()));
    }
}
