//! 油猴脚本头里写的几条保证，照脚本的文字核一遍：只连本机、不读 cookie、不写页面的存储。
//! 交给 usuyo 的行为（握手、令牌什么时候发）在 `userscript/test/` 里对着一次性的守护进程测。

const SCRIPT: &str = include_str!("../userscript/usuyo-x-saved.user.js");

fn header(key: &str) -> Vec<&'static str> {
    SCRIPT
        .lines()
        .filter_map(|l| l.strip_prefix("// @"))
        .filter_map(|l| l.strip_prefix(key))
        .map(str::trim)
        .collect()
}

#[test]
fn userscript_connects_only_to_this_machine() {
    assert_eq!(header("connect"), ["127.0.0.1"]);
    assert_eq!(header("match"), ["https://x.com/*"]);
    let urls: Vec<&str> = SCRIPT
        .match_indices("http")
        .map(|(i, _)| &SCRIPT[i..])
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        .map(|s| {
            s.split(['\'', '"', '`', ' ', '\n', ')'])
                .next()
                .unwrap_or(s)
        })
        .collect();
    assert!(urls.iter().any(|u| u.starts_with("http://127.0.0.1")));
    for url in urls {
        assert!(
            url.starts_with("http://127.0.0.1") || url.starts_with("https://x.com/"),
            "the userscript names another address: {url}"
        );
    }
}

#[test]
fn userscript_reads_no_cookies_and_writes_no_page_storage() {
    for needle in [
        "document.cookie",
        "localStorage.",
        "sessionStorage.",
        "indexedDB",
        "scrollTo",
        "scrollBy",
        "scrollIntoView",
    ] {
        assert!(
            !SCRIPT.contains(needle),
            "the userscript still has `{needle}`"
        );
    }
}

/// 交给 usuyo 的那一段和小红书插件 0.3.0 的脚本逐字相同：令牌和握手的写法不在这里另改。
#[test]
fn delivery_code_is_the_xiaohongshu_one() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../xiaohongshu/userscript/usuyo-xhs-saved.user.js"
    );
    let Ok(xhs) = std::fs::read_to_string(path) else {
        panic!("the Xiaohongshu plugin is expected next to this one: {path}");
    };
    let block = |s: &str| {
        let start = s
            .find("  const UNREACHABLE = new Error('unreachable');")
            .unwrap();
        let end = s.find("  function today() {").unwrap();
        s[start..end].to_owned()
    };
    assert!(xhs.contains("@version      0.3.0"));
    assert_eq!(block(SCRIPT), block(&xhs));
}
