//! 两种回答的形状：帖子接口 `TweetResultByRestId` 的 `data.tweetResult.result`，和嵌入接口
//! `tweet-result` 的 JSON。X 改了回答的样子，只改这里。
//!
//! 字段路径的依据（2026-11-11 读的源码）：
//! - 帖子接口：yt-dlp `TwitterIE._graphql_to_legacy`（第 1077 行起）；gallery-dl `_transform_tweet`、
//!   `_transform_user`（作者在 `core.user_results.result.core.screen_name`，旧形状在 `legacy`）、
//!   `_extract_media`（第 197 行起）；twitter-web-exporter `src/utils/api.ts` 的 `extractTweetUnion`、
//!   `extractTweetFullText`。
//! - 嵌入接口：react-tweet `src/api/types/{tweet,media,user,entities}.ts`、`fetch-tweet.ts`。

use serde_json::Value;
use usuyo_plugin_kit::protocol::Rfc3339;

/// 一个接口对一条帖子的回答。
#[derive(Debug)]
pub(crate) enum Answer {
    Post(Box<Post>),
    /// `TweetUnavailable`，带 `reason`。
    Unavailable(Option<String>),
    /// 墓碑，带它的文字。
    Tombstone(Option<String>),
    /// 接口说没有这条：帖子接口的空 `tweetResult`、嵌入接口的空对象。
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PostAuthor {
    pub id: Option<String>,
    pub screen_name: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaType {
    Photo,
    Video,
    AnimatedGif,
}

#[derive(Debug, Clone)]
pub(crate) struct MediaItem {
    pub kind: MediaType,
    /// 照片是 `media_url_https`；视频和动图是挑出来的那个 mp4。
    pub url: Option<String>,
    pub bitrate: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub duration_ms: Option<u64>,
    pub alt: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Article {
    pub title: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Post {
    pub id: String,
    pub author: Option<PostAuthor>,
    pub created_at: Option<Rfc3339>,
    /// 展开过 `t.co`、去掉了媒体短链和 HTML 转义的正文。
    pub text: String,
    /// 只拿到长帖的前一段（嵌入接口只给截断的 `text`）。
    pub truncated: bool,
    pub article: Option<Article>,
    pub media: Vec<MediaItem>,
    pub hashtags: Vec<String>,
    pub lang: Option<String>,
    pub counts: Value,
    pub reply_to_status: Option<String>,
    pub reply_to_user: Option<String>,
    /// 引用的帖子；看不到时是 `Err(原因)`。
    pub quoted: Option<Result<Box<Post>, String>>,
    /// 嵌入接口给的上一条（`parent`）。
    pub parent: Option<Box<Post>>,
    /// 这条是转帖时，转帖自己的 id；`id` 和别的字段都是被转的原帖的。
    pub retweet_id: Option<String>,
    /// 原样的回答，写进 `post.json`、`thread/<id>.json`。
    pub raw: Value,
}

fn str_at<'v>(v: &'v Value, path: &[&str]) -> Option<&'v str> {
    path.iter()
        .try_fold(v, |v, key| v.get(key))?
        .as_str()
        .filter(|s| !s.is_empty())
}

fn at<'v>(v: &'v Value, path: &[&str]) -> Option<&'v Value> {
    path.iter()
        .try_fold(v, |v, key| v.get(key))
        .filter(|v| !v.is_null())
}

fn owned(v: Option<&str>) -> Option<String> {
    v.map(str::to_owned)
}

fn number(v: Option<&Value>) -> Option<u64> {
    let v = v?;
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

// ---------- 帖子接口 ----------

/// `data` 下面的 `tweetResult`。没有 `data` 或 `tweetResult` 时回 `None`：回答的形状认不出。
pub(crate) fn graphql_answer(body: &Value) -> Option<Answer> {
    let tweet_result = at(body, &["data", "tweetResult"])?;
    let Some(result) = tweet_result.get("result").filter(|r| r.is_object()) else {
        return Some(Answer::Absent);
    };
    Some(graphql_union(result))
}

/// 一个 `TweetUnion`：`Tweet`、`TweetWithVisibilityResults`（外面多包一层 `tweet`）、
/// `TweetTombstone`、`TweetUnavailable`。
fn graphql_union(result: &Value) -> Answer {
    if let Some(tombstone) = result.get("tombstone") {
        return Answer::Tombstone(owned(str_at(tombstone, &["text", "text"])));
    }
    match str_at(result, &["__typename"]) {
        Some("TweetTombstone") => Answer::Tombstone(None),
        Some("TweetUnavailable") => Answer::Unavailable(owned(str_at(result, &["reason"]))),
        Some("TweetWithVisibilityResults") => match result.get("tweet") {
            Some(inner) => graphql_union(inner),
            None => Answer::Absent,
        },
        _ => match graphql_post(result) {
            Some(post) => Answer::Post(Box::new(post)),
            None => Answer::Absent,
        },
    }
}

fn graphql_user(user: &Value) -> PostAuthor {
    // 2025 年起 screen_name、name 搬进了 `core`（gallery-dl `_transform_user`），旧形状在 `legacy`
    let pick =
        |key: &str| owned(str_at(user, &["core", key]).or_else(|| str_at(user, &["legacy", key])));
    PostAuthor {
        id: owned(str_at(user, &["rest_id"]).or_else(|| str_at(user, &["legacy", "id_str"]))),
        screen_name: pick("screen_name"),
        name: pick("name"),
    }
}

fn graphql_post(result: &Value) -> Option<Post> {
    let legacy = result.get("legacy")?;
    // 转帖：取被转的原帖（yt-dlp `_extract_status` 最后一行；gallery-dl `retweeted_status_result`）
    if let Some(original) = at(legacy, &["retweeted_status_result", "result"])
        && let Answer::Post(mut post) = graphql_union(original)
    {
        post.retweet_id =
            owned(str_at(result, &["rest_id"]).or_else(|| str_at(legacy, &["id_str"])));
        return Some(*post);
    }
    let id = owned(str_at(result, &["rest_id"]).or_else(|| str_at(legacy, &["id_str"])))?;
    let note = at(result, &["note_tweet", "note_tweet_results", "result"]);
    let (raw_text, entities) = match note.and_then(|n| str_at(n, &["text"]).map(|t| (t, n))) {
        Some((text, n)) => (text, n.get("entity_set")),
        None => (
            str_at(legacy, &["full_text"]).unwrap_or_default(),
            legacy.get("entities"),
        ),
    };
    let media_list =
        at(legacy, &["extended_entities", "media"]).or_else(|| at(legacy, &["entities", "media"]));
    let media = media_list
        .and_then(Value::as_array)
        .map(|m| m.iter().filter_map(media_item).collect())
        .unwrap_or_default();
    let quoted = at(result, &["quoted_status_result", "result"]).map(|q| match graphql_union(q) {
        Answer::Post(p) => Ok(p),
        Answer::Tombstone(text) => Err(text.unwrap_or_else(|| "引用的帖子看不到了".into())),
        Answer::Unavailable(reason) => Err(format!(
            "引用的帖子要登录才看得到（{}）",
            reason.unwrap_or_default()
        )),
        Answer::Absent => Err("引用的帖子看不到了".into()),
    });
    let article = result.get("article").map(|a| {
        let r = at(a, &["article_results", "result"]).unwrap_or(a);
        Article {
            title: owned(str_at(r, &["title"])),
            preview: owned(str_at(r, &["preview_text"])),
        }
    });
    Some(Post {
        id,
        author: at(result, &["core", "user_results", "result"]).map(graphql_user),
        created_at: str_at(legacy, &["created_at"]).and_then(legacy_time),
        text: clean_text(raw_text, entities, media_list),
        truncated: false,
        article,
        media,
        hashtags: hashtags(entities),
        lang: owned(str_at(legacy, &["lang"])),
        counts: serde_json::json!({
            "likes": number(legacy.get("favorite_count")),
            "reposts": number(legacy.get("retweet_count")),
            "replies": number(legacy.get("reply_count")),
            "quotes": number(legacy.get("quote_count")),
            "bookmarks": number(legacy.get("bookmark_count")),
            "views": number(at(result, &["views", "count"])),
        }),
        reply_to_status: owned(str_at(legacy, &["in_reply_to_status_id_str"])),
        reply_to_user: owned(str_at(legacy, &["in_reply_to_user_id_str"])),
        quoted,
        parent: None,
        retweet_id: None,
        raw: result.clone(),
    })
}

/// 帖子接口的时间是 `Wed Oct 10 20:19:24 +0000 2018`（gallery-dl 用 `%a %b %d %H:%M:%S %z %Y` 解析）。
fn legacy_time(text: &str) -> Option<Rfc3339> {
    let zoned = jiff::fmt::strtime::parse("%a %b %d %H:%M:%S %z %Y", text)
        .ok()?
        .to_timestamp()
        .ok()?;
    Some(Rfc3339::from_timestamp(zoned))
}

// ---------- 嵌入接口 ----------

/// 嵌入接口的 JSON：空对象是没有这条，`TweetTombstone` 是墓碑（react-tweet `fetchTweet`）。
/// 认不出时回 `None`。
pub(crate) fn syndication_answer(body: &Value) -> Option<Answer> {
    let object = body.as_object()?;
    if object.is_empty() {
        return Some(Answer::Absent);
    }
    match str_at(body, &["__typename"]) {
        Some("TweetTombstone") => Some(Answer::Tombstone(owned(str_at(
            body,
            &["tombstone", "text", "text"],
        )))),
        _ => syndication_post(body).map(|p| Answer::Post(Box::new(p))),
    }
}

fn syndication_user(user: &Value) -> PostAuthor {
    PostAuthor {
        id: owned(str_at(user, &["id_str"])),
        screen_name: owned(str_at(user, &["screen_name"])),
        name: owned(str_at(user, &["name"])),
    }
}

fn syndication_post(body: &Value) -> Option<Post> {
    let id = owned(str_at(body, &["id_str"]))?;
    let entities = body.get("entities");
    let media_list = body.get("mediaDetails");
    let media = media_list
        .and_then(Value::as_array)
        .map(|m| m.iter().filter_map(media_item).collect())
        .unwrap_or_default();
    Some(Post {
        id,
        author: body.get("user").map(syndication_user),
        created_at: str_at(body, &["created_at"])
            .and_then(|t| t.parse::<jiff::Timestamp>().ok())
            .map(Rfc3339::from_timestamp),
        text: clean_text(
            str_at(body, &["text"]).unwrap_or_default(),
            entities,
            media_list,
        ),
        // 长帖只给 `note_tweet.id`，正文是截断的（react-tweet `TweetBase.note_tweet`）
        truncated: body.get("note_tweet").is_some(),
        article: None,
        media,
        hashtags: hashtags(entities),
        lang: owned(str_at(body, &["lang"])),
        counts: serde_json::json!({
            "likes": number(body.get("favorite_count")),
            "replies": number(body.get("conversation_count").or_else(|| body.get("reply_count"))),
            "reposts": number(body.get("retweet_count")),
        }),
        reply_to_status: owned(str_at(body, &["in_reply_to_status_id_str"])),
        reply_to_user: owned(str_at(body, &["in_reply_to_user_id_str"])),
        quoted: body
            .get("quoted_tweet")
            .and_then(syndication_post)
            .map(|q| Ok(Box::new(q))),
        parent: body.get("parent").and_then(syndication_post).map(Box::new),
        retweet_id: None,
        raw: body.clone(),
    })
}

// ---------- 两种回答共用的 ----------

/// `extended_entities.media[]`（帖子接口）和 `mediaDetails[]`（嵌入接口）是同一个形状。
fn media_item(m: &Value) -> Option<MediaItem> {
    let kind = match str_at(m, &["type"])? {
        "photo" => MediaType::Photo,
        "video" => MediaType::Video,
        "animated_gif" => MediaType::AnimatedGif,
        _ => return None,
    };
    let (url, bitrate) = match kind {
        MediaType::Photo => (owned(str_at(m, &["media_url_https"])), None),
        MediaType::Video | MediaType::AnimatedGif => best_mp4(m),
    };
    Some(MediaItem {
        kind,
        url,
        bitrate,
        width: number(at(m, &["original_info", "width"])),
        height: number(at(m, &["original_info", "height"])),
        duration_ms: number(at(m, &["video_info", "duration_millis"])),
        alt: owned(str_at(m, &["ext_alt_text"])),
    })
}

/// `video_info.variants[]` 里 `content_type: video/mp4`、`bitrate` 最高的一个；`m3u8` 不取
/// （gallery-dl `_extract_media` 取最高码率，第 236–238 行）。
fn best_mp4(m: &Value) -> (Option<String>, Option<u64>) {
    at(m, &["video_info", "variants"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| str_at(v, &["content_type"]) == Some("video/mp4"))
        .filter_map(|v| Some((str_at(v, &["url"])?.to_owned(), number(v.get("bitrate")))))
        .max_by_key(|(_, b)| b.unwrap_or(0))
        .map_or((None, None), |(u, b)| (Some(u), b))
}

fn hashtags(entities: Option<&Value>) -> Vec<String> {
    entities
        .and_then(|e| e.get("hashtags"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|h| owned(str_at(h, &["text"])))
        .collect()
}

/// 正文里的 `t.co` 按 `entities.urls[].expanded_url` 展开（不去请求 `t.co`），
/// 指向本帖媒体的那个短链去掉，HTML 转义还原（gallery-dl `_transform_tweet` 的做法）。
fn clean_text(raw: &str, entities: Option<&Value>, media: Option<&Value>) -> String {
    let mut text = raw.to_owned();
    for u in entities
        .and_then(|e| e.get("urls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(short), Some(long)) = (str_at(u, &["url"]), str_at(u, &["expanded_url"])) {
            text = text.replace(short, long);
        }
    }
    for m in media.and_then(Value::as_array).into_iter().flatten() {
        if let Some(short) = str_at(m, &["url"]) {
            text = text.replace(short, "");
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_cleanup() {
        let entities = serde_json::json!({"urls": [{"url": "https://t.co/aaa", "expanded_url": "https://example.com/long"}]});
        let media = serde_json::json!([{"url": "https://t.co/mmm", "type": "photo"}]);
        assert_eq!(
            clean_text(
                "A &amp; B &lt;3 https://t.co/aaa https://t.co/mmm",
                Some(&entities),
                Some(&media)
            ),
            "A & B <3 https://example.com/long"
        );
    }

    #[test]
    fn legacy_times() {
        assert_eq!(
            legacy_time("Wed Oct 30 12:00:00 +0000 2024")
                .unwrap()
                .as_str(),
            "2024-10-30T12:00:00Z"
        );
        assert!(legacy_time("yesterday").is_none());
    }

    #[test]
    fn best_variant_skips_hls() {
        let m = serde_json::json!({"type": "video", "video_info": {"variants": [
            {"content_type": "application/x-mpegURL", "url": "https://video.twimg.com/a.m3u8"},
            {"content_type": "video/mp4", "bitrate": 256000, "url": "https://video.twimg.com/low.mp4"},
            {"content_type": "video/mp4", "bitrate": 2176000, "url": "https://video.twimg.com/high.mp4"}
        ]}});
        let item = media_item(&m).unwrap();
        assert_eq!(
            item.url.as_deref(),
            Some("https://video.twimg.com/high.mp4")
        );
        assert_eq!(item.bitrate, Some(2_176_000));
    }

    #[test]
    fn unions() {
        let gone = serde_json::json!({"data": {"tweetResult": {}}});
        assert!(matches!(graphql_answer(&gone), Some(Answer::Absent)));
        let nsfw = serde_json::json!({"data": {"tweetResult": {"result": {"__typename": "TweetUnavailable", "reason": "NsfwLoggedOut"}}}});
        assert!(
            matches!(graphql_answer(&nsfw), Some(Answer::Unavailable(Some(r))) if r == "NsfwLoggedOut")
        );
        assert!(graphql_answer(&serde_json::json!({"data": {}})).is_none());
        assert!(matches!(
            syndication_answer(&serde_json::json!({})),
            Some(Answer::Absent)
        ));
        assert!(syndication_answer(&serde_json::json!([])).is_none());
    }
}
