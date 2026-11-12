//! 读你从 X 申请下载的官方归档（协议 §3.9，设计 x.md 第 2 节）：只读 zip 里的喜欢和书签，
//! 每条只交链接，内容照样由 `fetch` 不带账号去取。不联网。
//!
//! 文件名和形状照 steipete/birdclaw（main `9895445`，2026-11-06）：`src/lib/archive/slices.ts`
//! 认 `data/like.js`、`data/like-partN.js`，也认 `data/bookmark(s).js`；`reader.ts` 取 `=` 后面的 JSON 数组；
//! `parsing.ts` 的 `extractCollectionTweet` 从 `{"like": {...}}`、`{"bookmark": {...}}` 里读 `tweetId`、`fullText`。
//! 今天的归档里有没有书签、每条有哪些字段，没核过，都在 [`record`] 一处。

use std::io::Read;
use std::path::Path;

use serde_json::Value;
use usuyo_plugin_kit::protocol::{Access, Evidence, ListAction, ListItem, ListPage, Rfc3339};

use crate::error::Error;
use crate::link;

/// 一页最多多少条。
pub const PAGE: usize = 500;
/// 一个 `data/*.js` 解压后最多读这么多字节。
const ENTRY_BYTES: u64 = 512 << 20;

/// 归档里的一个文件是哪一类。
fn kind_of(name: &str) -> Option<(ListAction, &'static str)> {
    let name = name.replace('\\', "/");
    let file = name.strip_prefix("data/").or_else(|| {
        let at = name.rfind("/data/")?;
        Some(&name[at + "/data/".len()..])
    })?;
    let stem = file.strip_suffix(".js")?;
    let (base, part) = match stem.split_once("-part") {
        Some((b, p)) => (b, Some(p)),
        None => (stem, None),
    };
    if part.is_some_and(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    match base.to_ascii_lowercase().as_str() {
        "like" | "likes" => Some((ListAction::Like, "like")),
        "bookmark" | "bookmarks" => Some((ListAction::Favorite, "bookmark")),
        _ => None,
    }
}

/// 一条喜欢或书签：`(帖子 id, 正文)`。认不出的回 `None`。
fn record(wrapper: &Value, key: &str) -> Option<(String, Option<String>)> {
    let entry = wrapper.get(key).or_else(|| wrapper.get("tweet"))?;
    let id = ["tweetId", "tweet_id", "id_str", "id"]
        .iter()
        .find_map(|k| match entry.get(*k)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })?;
    let id = link::post_id(&id)?;
    let text = ["fullText", "full_text", "text"]
        .iter()
        .find_map(|k| entry.get(*k)?.as_str())
        .map(str::to_owned);
    Some((id, text))
}

/// `window.YTD.like.part0 = [ … ]`：取 `=` 后面的 JSON。
fn payload(text: &str) -> Result<Vec<Value>, String> {
    let at = text.find('=').ok_or("no `=` before the data")?;
    let value: Value = serde_json::from_str(text[at + 1..].trim().trim_end_matches(';'))
        .map_err(|e| e.to_string())?;
    match value {
        Value::Array(a) => Ok(a),
        _ => Err("the data is not an array".into()),
    }
}

fn title(text: Option<&str>) -> Option<String> {
    let line = text?.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(100).collect())
}

/// 列一页。`cursor` 是从第几条开始（插件自己的不透明字符串）。
pub fn list(source: &str, input: Option<&Path>, cursor: Option<&str>) -> Result<ListPage, Error> {
    if !source.starts_with("file:") {
        return Err(Error::InvalidArgs(format!(
            "source `{source}` is not a file import"
        )));
    }
    let input = input
        .ok_or_else(|| Error::InvalidArgs("USUYO_INPUT is not set for a file import".into()))?;
    let start = match cursor {
        None => 0,
        Some(c) => c
            .parse::<usize>()
            .map_err(|_| Error::InvalidArgs(format!("cursor `{c}` is not ours")))?,
    };
    let fetched_at = Rfc3339::from_timestamp(jiff::Timestamp::now());
    let all = read_all(input)?;
    let end = (start + PAGE).min(all.len());
    let items = all.get(start..end).unwrap_or_default().to_vec();
    Ok(ListPage {
        items,
        next_cursor: (end < all.len()).then(|| end.to_string()),
        evidence: Evidence {
            fetched_at,
            access: Access::OfficialExport,
            site: None,
            upstream: None,
            status: None,
        },
    })
}

/// 读出全部条目：喜欢在前、书签在后，各自按文件名（`part0`、`part1`…）的顺序；同一类里重复的只留一条。
fn read_all(path: &Path) -> Result<Vec<ListItem>, Error> {
    let file = std::fs::File::open(path)?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| Error::Archive(format!("not a zip file: {e}")))?;
    let all_names = zip
        .file_names()
        .map(|n| n.map(|n| n.into_owned()))
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| Error::Archive(format!("unreadable file name in the zip: {e}")))?;
    let mut names: Vec<(ListAction, &'static str, String)> = all_names
        .into_iter()
        .filter_map(|n| kind_of(&n).map(|(a, k)| (a, k, n)))
        .collect();
    if names.is_empty() {
        return Err(Error::Archive(
            "no data/like.js or data/bookmark.js in the archive".into(),
        ));
    }
    names.sort_by_key(|(_, key, name)| (*key != "like", part_number(name), name.clone()));
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for (action, key, name) in names {
        let mut entry = zip
            .by_name(&name)
            .map_err(|e| Error::Archive(format!("{name}: {e}")))?;
        let mut bytes = Vec::new();
        (&mut entry)
            .take(ENTRY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::Archive(format!("{name}: {e}")))?;
        if bytes.len() as u64 > ENTRY_BYTES {
            return Err(Error::Archive(format!(
                "{name} is larger than {ENTRY_BYTES} bytes"
            )));
        }
        let text = String::from_utf8(bytes)
            .map_err(|e| Error::Archive(format!("{name} is not UTF-8: {e}")))?;
        let records = payload(&text).map_err(|e| Error::UpstreamChanged(format!("{name}: {e}")))?;
        let mut unreadable = 0usize;
        for wrapper in &records {
            let Some((id, text)) = record(wrapper, key) else {
                unreadable += 1;
                continue;
            };
            if !seen.insert((key, id.clone())) {
                continue;
            }
            items.push(ListItem {
                url: Some(link::canonical_url(&id)),
                reference: None,
                identity: Some(link::identity(&id)),
                title: title(text.as_deref()),
                author: None,
                published_at: None,
                action: Some(action),
                // 归档里没有加入时间（设计第 2 节）：留空，不用抓取时间冒充
                acted_at: None,
            });
        }
        if unreadable > 0 {
            eprintln!("{name}: {unreadable} entries without a post id were skipped");
            if unreadable == records.len() {
                return Err(Error::UpstreamChanged(format!(
                    "{name}: none of its {} entries has a post id",
                    records.len()
                )));
            }
        }
    }
    Ok(items)
}

fn part_number(name: &str) -> u64 {
    name.rsplit_once("-part")
        .and_then(|(_, p)| p.strip_suffix(".js")?.parse().ok())
        .map_or(0, |n: u64| n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kinds() {
        assert_eq!(kind_of("data/like.js").map(|k| k.1), Some("like"));
        assert_eq!(kind_of("data/like-part2.js").map(|k| k.1), Some("like"));
        assert_eq!(
            kind_of("twitter-2026/data/likes.js").map(|k| k.1),
            Some("like")
        );
        assert_eq!(
            kind_of("data/bookmark.js").map(|k| k.0),
            Some(ListAction::Favorite)
        );
        assert_eq!(
            kind_of("data/bookmarks-part1.js").map(|k| k.1),
            Some("bookmark")
        );
        assert!(kind_of("data/tweets.js").is_none());
        assert!(kind_of("data/like-partx.js").is_none());
        assert!(kind_of("data/like.json").is_none());
        assert!(kind_of("like.js").is_none());
    }

    #[test]
    fn payloads_and_records() {
        let text = "window.YTD.like.part0 = [ {\"like\": {\"tweetId\": \"20\", \"fullText\": \"just setting up my twttr\"}}, {\"like\": {\"tweetId\": 21}}, {\"like\": {}} ]";
        let records = payload(text).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(
            record(&records[0], "like"),
            Some(("20".into(), Some("just setting up my twttr".into())))
        );
        assert_eq!(record(&records[1], "like"), Some(("21".into(), None)));
        assert_eq!(record(&records[2], "like"), None);
        assert!(payload("window.YTD.like.part0 = {}").is_err());
        assert!(payload("nothing").is_err());
    }

    #[test]
    fn part_order() {
        assert!(part_number("data/like.js") < part_number("data/like-part0.js"));
        assert!(part_number("data/like-part1.js") < part_number("data/like-part10.js"));
    }
}
