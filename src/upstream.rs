//! 照上游项目抄来的、X 随时会改的东西，都在这一个文件里（设计 x.md 第 3、10 节）：
//! 网页的公开 bearer、`TweetResultByRestId` 的 queryId 和参数、syndication 的 token 算法。
//! X 改了这些，帖子接口回参数错误，插件报 `upstream-changed`，只改这里。
//!
//! 依据（2026-11-11 用 `gh api` 读的源码，没有运行）：
//! - yt-dlp master `412e7f0`（2026-11-04）`yt_dlp/extractor/twitter.py`
//! - gallery-dl master `811d917`（2026-10-31）`gallery_dl/extractor/twitter.py`（GPL-2.0，只对照、不抄）
//! - vercel/react-tweet main `cb5070e`（2026-09-24）`packages/react-tweet/src/api/fetch-tweet.ts`

use serde_json::{Value, json};

/// 网页自己用的公开 bearer（不是账号凭据，任何访客的页面里都有）。
/// yt-dlp `TwitterBaseIE._AUTH`（第 39 行）；gallery-dl `TwitterAPI.__init__` 的 `authorization` 是同一个值。
pub const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

/// 换访客令牌：`POST` 空体（yt-dlp `_API_BASE` + `guest/activate.json`，`_fetch_guest_token` 第 104 行）。
pub const GUEST_ACTIVATE_PATH: &str = "/1.1/guest/activate.json";

/// 帖子接口：`GET <x.com>/i/api/graphql/<queryId>/TweetResultByRestId`。
/// queryId 取 yt-dlp 的 `TwitterIE._GRAPHQL_ENDPOINT`（第 1075 行）；gallery-dl 用 `qxWQxcMLiTPcavz9Qy5hwQ`，
/// fieldtheory-cli 用 `fHLDP3qFEjnTqhWBVvsREg`。过期了去网页的 `main.<hash>.js` 里找
/// `operationName:"TweetResultByRestId"`（fieldtheory-cli 的注释）。
pub const TWEET_RESULT_PATH: &str = "/i/api/graphql/2ICDjqPd81tulZcYrtpTuQ/TweetResultByRestId";

/// 帖子接口的三个查询参数，照 yt-dlp `TwitterIE._build_graphql_query`（第 1122 行起）原样写。
/// `fieldToggles.withArticleRichContentState` 是 `false`：长文章正文不取（设计第 3 节）。
pub fn tweet_result_query(tweet_id: &str) -> [(&'static str, String); 3] {
    let variables = json!({
        "tweetId": tweet_id,
        "withCommunity": false,
        "includePromotedContent": false,
        "withVoice": false,
    });
    let features = json!({
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "tweetypie_unmention_optimization_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "view_counts_everywhere_api_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": false,
        "tweet_awards_web_tipping_enabled": false,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "standardized_nudges_misinfo": true,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "longform_notetweets_rich_text_read_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "verified_phone_label_enabled": false,
        "responsive_web_media_download_video_enabled": false,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
    });
    let field_toggles = json!({"withArticleRichContentState": false});
    // yt-dlp 用 `json.dumps(..., separators=(',', ':'))`，serde_json 的紧凑写法一样
    let compact = |v: Value| v.to_string();
    [
        ("variables", compact(variables)),
        ("features", compact(features)),
        ("fieldToggles", compact(field_toggles)),
    ]
}

/// 帖子接口回答里这几个错误不算错：yt-dlp `_call_api` 忽略 `Dependency: Unspecified`（yt-dlp #15963）。
pub fn is_ignorable_api_error(message: &str) -> bool {
    message.eq_ignore_ascii_case("dependency: unspecified")
}

/// 访客令牌缓存多久：gallery-dl `_authenticate_guest` 的 `_exp=3600`。
pub const GUEST_TOKEN_TTL_SECS: i64 = 3600;

/// 嵌入用的接口：`GET <cdn.syndication.twimg.com>/tweet-result?id=…&lang=en&features=…&token=…`，
/// 照 react-tweet `fetchTweet` 写（同一个文件里的 `features` 原样照抄）。
pub const SYNDICATION_PATH: &str = "/tweet-result";

pub fn syndication_query(tweet_id: &str) -> [(&'static str, String); 4] {
    let features = [
        "tfw_timeline_list:",
        "tfw_follower_count_sunset:true",
        "tfw_tweet_edit_backend:on",
        "tfw_refsrc_session:on",
        "tfw_fosnr_soft_interventions_enabled:on",
        "tfw_show_birdwatch_pivots_enabled:on",
        "tfw_show_business_verified_badge:on",
        "tfw_duplicate_scribes_to_settings:on",
        "tfw_use_profile_image_shape_enabled:on",
        "tfw_show_blue_verified_badge:on",
        "tfw_legacy_timeline_sunset:true",
        "tfw_show_gov_verified_badge:on",
        "tfw_show_business_affiliate_badge:on",
        "tfw_tweet_edit_frontend:on",
    ]
    .join(";");
    [
        ("id", tweet_id.to_owned()),
        ("lang", "en".to_owned()),
        ("features", features),
        ("token", syndication_token(tweet_id)),
    ]
}

/// react-tweet `getToken`：`((Number(id) / 1e15) * Math.PI).toString(36).replace(/(0+|\.)/g, '')`。
/// yt-dlp `_generate_syndication_token`（第 1156 行）同一个公式。
pub fn syndication_token(tweet_id: &str) -> String {
    let value = tweet_id.parse::<f64>().unwrap_or(f64::NAN) / 1e15 * std::f64::consts::PI;
    js_radix_string(value, 36)
        .chars()
        .filter(|c| *c != '0' && *c != '.')
        .collect()
}

/// JavaScript 的 `Number.prototype.toString(radix)`，照 V8 `DoubleToRadixCString`
/// （`src/numbers/conversions.cc`）一位一位地写：小数只写到这个 double 的精度为止，按偶数舍入。
/// 只处理有限的非负数，别的写成 JS 的样子。
fn js_radix_string(value: f64, radix: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let negative = value < 0.0;
    let value = value.abs();
    let r = f64::from(radix);
    let mut integer = value.floor();
    let mut fraction = value - integer;
    // 下一个可表示的 double 和它的距离的一半；不小于最小的正 double
    let next = f64::from_bits(value.to_bits() + 1);
    let mut delta = (0.5 * (next - value)).max(f64::from_bits(1));
    let mut frac_digits: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= r;
            delta *= r;
            let digit = fraction as u32;
            frac_digits.push(DIGITS[digit as usize]);
            fraction -= f64::from(digit);
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                // 进位：往回找能加一的那一位，加不了的去掉
                loop {
                    match frac_digits.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(c) => {
                            let d = (c as char).to_digit(36).unwrap_or(0);
                            if d + 1 < radix {
                                frac_digits.push(DIGITS[(d + 1) as usize]);
                                break;
                            }
                        }
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let mut int_digits: Vec<u8> = Vec::new();
    // 超过 2^53 的整数部分，低位已经不精确，补 0（V8 的做法）
    while integer / r >= 9_007_199_254_740_992.0 {
        integer /= r;
        int_digits.push(b'0');
    }
    loop {
        let remainder = integer % r;
        int_digits.push(DIGITS[remainder as usize]);
        integer = (integer - remainder) / r;
        if integer <= 0.0 {
            break;
        }
    }
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.extend(int_digits.iter().rev().map(|&b| b as char));
    if !frac_digits.is_empty() {
        out.push('.');
        out.extend(frac_digits.iter().map(|&b| b as char));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 期望值是在本机用 Node v22 跑 react-tweet 的公式算出来的（不联网），见 tests/fixtures/syndication-tokens.json。
    #[test]
    fn syndication_token_matches_node() {
        let text = include_str!("../tests/fixtures/syndication-tokens.json");
        let table: Vec<(String, String)> = serde_json::from_str::<Value>(text).unwrap()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["id"].as_str().unwrap().to_owned(),
                    c["token"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert!(table.len() >= 10);
        for (id, token) in table {
            assert_eq!(syndication_token(&id), token, "{id}");
        }
    }

    #[test]
    fn radix_string_matches_js_for_simple_values() {
        assert_eq!(js_radix_string(0.5, 2), "0.1");
        assert_eq!(js_radix_string(255.0, 16), "ff");
        assert_eq!(js_radix_string(35.0, 36), "z");
        assert_eq!(js_radix_string(0.0, 36), "0");
    }

    #[test]
    fn query_parameters_are_compact_json() {
        let q = tweet_result_query("20");
        assert_eq!(q[0].0, "variables");
        assert!(q[0].1.starts_with(r#"{"#) && !q[0].1.contains(' '));
        let v: Value = serde_json::from_str(&q[0].1).unwrap();
        assert_eq!(v["tweetId"], "20");
        let f: Value = serde_json::from_str(&q[1].1).unwrap();
        assert_eq!(f["longform_notetweets_consumption_enabled"], true);
        let s = syndication_query("20");
        assert_eq!(s[3], ("token", "6dq1a2xwd93".to_owned()));
    }
}
