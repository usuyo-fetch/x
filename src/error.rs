//! 这个插件会失败的几种方式，和它们对应的协议错误码（协议 §3.3，设计 x.md 第 7 节）。
//! 不回 `login-required`：插件不用账号，回了会让界面去找一个不存在的登录。

use usuyo_plugin_kit::protocol::{CallFailure, ErrorCode, NotFoundReason};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 链接认不出帖子 id，或是 Spaces、直播。
    #[error("not an X post link: {0}")]
    Unsupported(String),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// 墓碑、找不到、嵌入接口回空对象；`deleted` 只在墓碑的文字明说删了时为真。
    #[error("post not found: {message}")]
    NotFound { message: String, deleted: bool },
    /// `TweetUnavailable` 的 `NsfwLoggedOut`、`NsfwViewerHasNoStatedAge`、`Protected`：要登录才看得到。
    #[error("post needs a logged-in viewer ({reason})")]
    LoginOnly { reason: String },
    #[error("rate limited: {message}")]
    RateLimited {
        message: String,
        retry_after: Option<u64>,
    },
    /// 访客令牌和嵌入接口都被拒。
    #[error("refused: {0}")]
    Blocked(String),
    /// 帖子接口说参数、queryId、features 不对，或回答的形状认不出。
    #[error("X changed: {0}")]
    UpstreamChanged(String),
    #[error("network: {0}")]
    Network(String),
    /// 交来的文件不是读得出喜欢、书签的 X 归档。
    #[error("archive: {0}")]
    Archive(String),
    #[error("{0}")]
    Internal(String),
    #[error("write output: {0}")]
    Io(#[from] std::io::Error),
}

/// `hint` 按 `USUYO_LOCALE` 写：`zh` 开头写中文，其余写英文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    pub fn from_locale(locale: Option<&str>) -> Self {
        match locale {
            Some(l) if !l.to_ascii_lowercase().starts_with("zh") => Lang::En,
            _ => Lang::Zh,
        }
    }

    fn pick(self, zh: &str, en: &str) -> String {
        match self {
            Lang::Zh => zh.into(),
            Lang::En => en.into(),
        }
    }
}

impl Error {
    pub fn to_failure(&self, lang: Lang) -> CallFailure {
        let message = self.to_string();
        let with = |code: ErrorCode, hint: Option<String>| CallFailure {
            hint,
            ..CallFailure::new(code, message.clone())
        };
        match self {
            Error::Unsupported(_) => with(ErrorCode::UnsupportedInput, None),
            Error::InvalidArgs(_) => with(ErrorCode::InvalidArgs, None),
            Error::NotFound { deleted, .. } => CallFailure {
                reason: deleted.then_some(NotFoundReason::Deleted),
                ..with(ErrorCode::NotFound, None)
            },
            Error::LoginOnly { .. } => CallFailure {
                reason: Some(NotFoundReason::Private),
                ..with(
                    ErrorCode::NotFound,
                    Some(lang.pick(
                        "这条要登录着才看得到；usuyo 不用账号取 X，链接先存着",
                        "This post is only visible when logged in; usuyo fetches X without an account, so the link is kept as it is",
                    )),
                )
            },
            Error::RateLimited { retry_after, .. } => CallFailure {
                retry_after: *retry_after,
                ..with(ErrorCode::RateLimited, None)
            },
            Error::Blocked(_) => with(
                ErrorCode::Blocked,
                Some(lang.pick(
                    "X 拒绝了这台电脑的出口：访客令牌和嵌入接口都不给；先停一阵，换回你平时上 X 的那个出口再试",
                    "X refused this network exit: neither a guest token nor the embed endpoint answered; wait a while and retry from your usual exit",
                )),
            ),
            Error::UpstreamChanged(_) => with(
                ErrorCode::UpstreamChanged,
                Some(lang.pick(
                    "X 的接口改了，这个插件要更新",
                    "X changed its endpoint; this plugin needs an update",
                )),
            ),
            Error::Network(_) => with(ErrorCode::Network, None),
            Error::Archive(_) => with(
                ErrorCode::UnsupportedInput,
                Some(lang.pick(
                    "这个文件读不出 X 归档里的喜欢和书签（要的是 X 设置里申请下载的那个 zip）",
                    "This file does not read as an X archive with likes or bookmarks (the zip you request in X's settings)",
                )),
            ),
            Error::Internal(_) | Error::Io(_) => with(ErrorCode::Internal, None),
        }
    }
}
