// ==UserScript==
// @name         usuyo：交出 X 的书签和喜欢
// @namespace    usuyo-fetch/x
// @version      0.1.0
// @description  在你自己的「书签」「喜欢」页上，只看页面自己请求回来的时间线，记下帖子 id、作者和正文第一行；点「交给 usuyo」时先确认本机 127.0.0.1:47615 上的是 usuyo，再把链接交过去，你在 usuyo 的导入页确认。不对 X 发额外请求、不自动滚动、不读 cookie，除了本机的 usuyo 不往别处发。
// @match        https://x.com/*
// @run-at       document-start
// @noframes
// @grant        unsafeWindow
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_xmlhttpRequest
// @grant        GM_registerMenuCommand
// @connect      127.0.0.1
// @license      MIT
// ==/UserScript==

/*
 * 截获的做法照 prinsss/twitter-web-exporter（MIT，v1.4.3）：在页面环境里改写 XMLHttpRequest，
 * 请求完成后读页面自己拿回的回答，不发任何额外请求（src/core/extensions/manager.ts）；
 * 书签认 /graphql/<id>/Bookmarks，从 data.bookmark_timeline_v2.timeline.instructions 取
 * （src/modules/bookmarks/api.ts）；喜欢认 /graphql/<id>/Likes，从
 * data.user.result.timeline.timeline.instructions 取（src/modules/likes/api.ts）；帖子在
 * TimelineAddEntries 的 entries[].content.itemContent.tweet_results.result，外面可能多包一层
 * TweetWithVisibilityResults（src/utils/api.ts）。没有抄它的代码，也不带它的自动导出和别的模块。
 *
 * 交给 usuyo 的部分（握手、令牌、只连 127.0.0.1:47615）原样照 usuyo-fetch/xiaohongshu 的
 * userscript/usuyo-xhs-saved.user.js 0.2.1：经脚本管理器的 GM_xmlhttpRequest；先握手确认对面是 usuyo，
 * 核对不过就什么都不发，投递令牌只在核对过以后才放进请求头。
 *
 * 令牌从浏览器自己的输入对话框（prompt）或脚本管理器的菜单填，不进页面里的输入框。
 * 脚本和页面脚本可能跑在同一个环境里（例如 Violentmonkey 的页面模式），页面可以改掉
 * String、RegExp、Function 的原型方法。所以碰令牌的每一步（核对形状、取出 id、去掉首尾空白、
 * 算 HMAC 的密钥字节、显示前几位）都只用下标 t[i]、字符串拼接和脚本一开始就拿住的函数，
 * 不再按名字去原型上找方法。这只在脚本先于页面脚本跑起来时有用（@run-at document-start）；
 * 脚本管理器注入得晚、页面已经先改过这些函数时，它挡不住。令牌交给 GM_xmlhttpRequest 以后
 * 怎么走，由脚本管理器负责。
 */

(function () {
  'use strict';

  const page = unsafeWindow;
  // 书签页、喜欢页自己请求的时间线（twitter-web-exporter 的 BookmarksInterceptor、LikesInterceptor 认的名字）
  const ENDPOINT = /\/graphql\/[^/?#]+\/(Bookmarks|Likes)(?:[?#]|$)/;
  const LABEL = { favorite: '书签', like: '喜欢' };
  // usuyo 的本机端口。测试载入源码时把这一行换成别的端口；用户这边没有设置项。
  const PORT = 47615;
  const USUYO = 'http://127.0.0.1:' + PORT;
  const REQUEST_TIMEOUT_MS = 5000;
  const BATCH_MAX = 5000;
  // 投递令牌：`<16 位 id>.<64 位秘密>`，都是小写十六进制，在 usuyo 的导入页发
  const TOKEN_ID_LEN = 16;
  const TOKEN_LEN = TOKEN_ID_LEN + 1 + 64;

  // 碰到令牌的几样东西在页面脚本跑起来以前就拿住，页面之后改了原型也换不掉它们
  const reflectApply = Reflect.apply;
  const Bytes = Uint8Array;
  const subtle = crypto.subtle;
  const importKey = subtle.importKey.bind(subtle);
  const sign = subtle.sign.bind(subtle);
  const getRandomValues = crypto.getRandomValues.bind(crypto);
  const nativePrompt = page.prompt;
  const attachShadow = Element.prototype.attachShadow;
  // 字符 → 字节，只收令牌里会出现的字符；没有原型，查表不经过任何方法
  const TOKEN_BYTE = Object.create(null);
  for (let c = 0; c < 10; c++) TOKEN_BYTE['0123456789'[c]] = 48 + c;
  for (let c = 0; c < 6; c++) TOKEN_BYTE['abcdef'[c]] = 97 + c;
  const SPACE = Object.create(null);
  for (const c of [' ', '\t', '\n', '\r', '\v', '\f', ' ', ' ', ' ', '　', '﻿']) SPACE[c] = true;

  // ---------- 记下的条目，存在脚本管理器里，页面读不到 ----------

  // X 的帖子链接不过期，不像小红书的 xsec_token；记下的留到交出去或你点「清空」
  /** @type {{favorite: Object<string, {screen: string, title: string, seenAt: number}>, like: Object}} */
  const store = loadStore();
  const status = { favorite: { hasMore: null }, like: { hasMore: null }, warning: '', notice: '' };

  function loadStore() {
    const empty = { favorite: {}, like: {} };
    let saved;
    try {
      saved = GM_getValue('items', empty);
    } catch (e) {
      return empty;
    }
    const out = { favorite: {}, like: {} };
    for (const action of ['favorite', 'like']) {
      const items = (saved && saved[action]) || {};
      for (const id of Object.keys(items)) {
        const item = items[id];
        if (/^[0-9]{1,20}$/.test(id) && item && typeof item.screen === 'string') out[action][id] = item;
      }
    }
    return out;
  }

  function saveStore() {
    GM_setValue('items', store);
  }

  function count(action) {
    return Object.keys(store[action]).length;
  }

  // ---------- 只看页面自己请求回来的时间线 ----------

  function actionOf(url) {
    const m = ENDPOINT.exec(url);
    if (!m) return null;
    return m[1] === 'Bookmarks' ? 'favorite' : 'like';
  }

  /**
   * 登录的是谁：读页面已有的状态对象 __INITIAL_STATE__.session.user_id，不发请求、不读 cookie。
   * 这个位置没在真页面上核过（OldTweetDeck 的 src/interception.js 用同一个路径），只在这一处；
   * 读不出来就是空字符串，喜欢就不记。
   */
  function ownUserId() {
    try {
      const state = page.__INITIAL_STATE__;
      const id = state && state.session && state.session.user_id;
      return /^[0-9]{1,20}$/.test(String(id || '')) ? String(id) : '';
    } catch (e) {
      return '';
    }
  }

  /** 喜欢的请求是谁的：查询参数 variables 里的 userId（X 的 graphql 请求把变量写成 JSON）。 */
  function listOwner(rawUrl) {
    try {
      const vars = new URL(rawUrl, page.location.href).searchParams.get('variables');
      const userId = vars && JSON.parse(vars).userId;
      return typeof userId === 'string' ? userId : '';
    } catch (e) {
      return '';
    }
  }

  /** 两种时间线的 instructions 在回答里的位置（twitter-web-exporter 的 bookmarks/api.ts、likes/api.ts）。 */
  function instructionsOf(action, json) {
    const data = json.data || {};
    const timeline =
      action === 'favorite'
        ? data.bookmark_timeline_v2 && data.bookmark_timeline_v2.timeline
        : data.user && data.user.result && data.user.result.timeline && data.user.result.timeline.timeline;
    return timeline && Array.isArray(timeline.instructions) ? timeline.instructions : null;
  }

  /** 一条时间线条目里的帖子：id、作者的 screen_name、正文第一行。 */
  function postOf(entry) {
    const content = entry && entry.content;
    if (!content || content.entryType !== 'TimelineTimelineItem') return null;
    let tweet = content.itemContent && content.itemContent.tweet_results && content.itemContent.tweet_results.result;
    if (tweet && tweet.__typename === 'TweetWithVisibilityResults') tweet = tweet.tweet;
    if (!tweet || !/^[0-9]{1,20}$/.test(String(tweet.rest_id || ''))) return null;
    const user = (tweet.core && tweet.core.user_results && tweet.core.user_results.result) || {};
    const screen = String((user.core && user.core.screen_name) || (user.legacy && user.legacy.screen_name) || '');
    const note = tweet.note_tweet && tweet.note_tweet.note_tweet_results && tweet.note_tweet.note_tweet_results.result;
    const text = String((note && note.text) || (tweet.legacy && tweet.legacy.full_text) || '');
    const first = text.split('\n').map((l) => l.trim()).find((l) => l) || '';
    return {
      id: String(tweet.rest_id),
      screen: /^[A-Za-z0-9_]{1,50}$/.test(screen) ? screen : '',
      title: first.slice(0, 200),
    };
  }

  function absorb(rawUrl, json) {
    const action = actionOf(rawUrl);
    if (!action || !json || typeof json !== 'object') return;
    const instructions = instructionsOf(action, json);
    if (!instructions) {
      // 掉线或被限流时页面自己拿到的就是 errors；照实说出来，让你停手
      const first = Array.isArray(json.errors) && json.errors[0];
      status.warning = 'X 回了' + (first ? '错误 ' + String(first.code || first.message || '').slice(0, 40) : '认不出的回答') + '，先别往下翻了';
      render();
      return;
    }
    // 只收你自己的喜欢：请求里的 userId 要等于页面里登录的那个人。书签只有你自己能看，不用核
    if (action === 'like') {
      const me = ownUserId();
      if (!me || listOwner(rawUrl) !== me) {
        status.notice = me ? '这不是你自己的喜欢，没有记' : '没认出登录的账号，没有记';
        render();
        return;
      }
    }
    const now = Date.now();
    let seen = 0;
    for (const ins of instructions) {
      if (!ins || ins.type !== 'TimelineAddEntries' || !Array.isArray(ins.entries)) continue;
      for (const entry of ins.entries) {
        const post = postOf(entry);
        if (!post) continue;
        seen++;
        store[action][post.id] = { screen: post.screen, title: post.title, seenAt: now };
      }
    }
    // 一页里一条帖子都没有（只剩翻页游标），推断是到底了
    status[action].hasMore = seen > 0;
    status.notice = '';
    status.warning = '';
    saveStore();
    render();
  }

  // fetch：拿响应的副本读，原响应原样交回页面
  const originalFetch = page.fetch;
  page.fetch = function (input) {
    const result = originalFetch.apply(this, arguments);
    try {
      const url = typeof input === 'string' ? input : (input && input.url) || '';
      if (actionOf(url)) {
        result
          .then((r) => r.clone().json())
          .then((j) => absorb(url, j))
          .catch(() => {});
      }
    } catch (e) {
      // 读不了就不记，页面照常
    }
    return result;
  };

  // XHR：请求地址记在脚本自己的 WeakMap 里，不往页面的对象上挂属性
  const xhrUrls = new WeakMap();
  const xhr = page.XMLHttpRequest.prototype;
  const originalOpen = xhr.open;
  const originalSend = xhr.send;
  xhr.open = function (_method, url) {
    try {
      xhrUrls.set(this, String(url));
    } catch (e) {
      // 同上
    }
    return originalOpen.apply(this, arguments);
  };
  xhr.send = function () {
    const url = xhrUrls.get(this) || '';
    if (actionOf(url)) {
      this.addEventListener('load', () => {
        try {
          const body = this.responseType === 'json' ? this.response : JSON.parse(this.responseText);
          absorb(url, body);
        } catch (e) {
          // 同上
        }
      });
    }
    return originalSend.apply(this, arguments);
  };

  // ---------- 交给 usuyo ----------

  const UNREACHABLE = new Error('unreachable');
  let busy = false;

  // ---------- 令牌：下面几个函数只用 t[i]、length 和拼接 ----------

  function isToken(t) {
    if (typeof t !== 'string' || t.length !== TOKEN_LEN) return false;
    for (let i = 0; i < TOKEN_LEN; i++) {
      if (i === TOKEN_ID_LEN ? t[i] !== '.' : TOKEN_BYTE[t[i]] === undefined) return false;
    }
    return true;
  }

  /** 令牌 `.` 前的 id（不是秘密）的前 `n` 位。 */
  function tokenId(t, n) {
    let id = '';
    for (let i = 0; i < n; i++) id += t[i];
    return id;
  }

  /** 令牌的字节（HMAC 的密钥）；令牌只有 ASCII，一个字符一个字节。 */
  function tokenBytes(t) {
    const b = new Bytes(t.length);
    for (let i = 0; i < t.length; i++) b[i] = t[i] === '.' ? 46 : TOKEN_BYTE[t[i]];
    return b;
  }

  /** 去掉首尾空白，不用 String.prototype.trim。 */
  function trimmed(v) {
    let start = 0;
    let end = v.length;
    while (start < end && SPACE[v[start]]) start++;
    while (end > start && SPACE[v[end - 1]]) end--;
    let out = '';
    for (let i = start; i < end; i++) out += v[i];
    return out;
  }

  function storedToken() {
    const t = GM_getValue('deliverToken', '');
    return isToken(t) ? t : '';
  }

  /** 只连本机的 usuyo；连不上、超时都当作没人在听。 */
  function local(method, path, headers, data) {
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method,
        url: USUYO + path,
        headers,
        data,
        timeout: REQUEST_TIMEOUT_MS,
        anonymous: true,
        onload: (r) => resolve({ status: r.status, text: String(r.responseText || '') }),
        onerror: () => reject(UNREACHABLE),
        ontimeout: () => reject(UNREACHABLE),
        onabort: () => reject(UNREACHABLE),
      });
    });
  }

  function hex(bytes) {
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  }

  /** 和 usuyo 的 crates/usuyo-api/src/handshake.rs 签同样的字节。 */
  async function proofOf(token, nonce) {
    const enc = new TextEncoder();
    const key = await importKey('raw', tokenBytes(token), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
    const msg = 'usuyo-daemon-proof/v1\n127.0.0.1:' + PORT + '\n' + nonce;
    return hex(new Uint8Array(await sign('HMAC', key, enc.encode(msg))));
  }

  function same(a, b) {
    if (a.length !== b.length) return false;
    let diff = 0;
    for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
    return diff === 0;
  }

  /**
   * 握手：只发一个新的随机数和令牌的 id（`.` 前那段，不是秘密），对面用整个令牌签回来，
   * 这边用自己存着的令牌核对。回 'usuyo'、'refused'（usuyo 不让这个来源进）或 'stranger'。
   */
  async function prove(token) {
    const nonce = hex(getRandomValues(new Uint8Array(32)));
    const client = tokenId(token, TOKEN_ID_LEN);
    const res = await local('POST', '/handshake', { 'Content-Type': 'application/json' }, JSON.stringify({ nonce, client }));
    if (res.status === 403) return 'refused';
    if (res.status !== 200) return 'stranger';
    let proof;
    try {
      proof = JSON.parse(res.text).proof;
    } catch (e) {
      return 'stranger';
    }
    if (typeof proof !== 'string' || !/^[0-9a-f]{64}$/.test(proof)) return 'stranger';
    return same(await proofOf(token, nonce), proof) ? 'usuyo' : 'stranger';
  }

  function today() {
    const d = new Date();
    return d.getFullYear() + '-' + String(d.getMonth() + 1).padStart(2, '0') + '-' + String(d.getDate()).padStart(2, '0');
  }

  /** 交出去的链接：`https://x.com/<screen_name>/status/<id>`；没认出作者时用 `i/web/status/<id>`。 */
  function postUrl(id, screen) {
    return 'https://x.com/' + (screen ? screen + '/status/' : 'i/web/status/') + id;
  }

  /** 记下的条目，按列表分好、每批不超过 BATCH_MAX。 */
  function batches() {
    const out = [];
    for (const action of ['favorite', 'like']) {
      const ids = Object.keys(store[action]);
      for (let i = 0; i < ids.length; i += BATCH_MAX) out.push({ action, ids: ids.slice(i, i + BATCH_MAX) });
    }
    return out;
  }

  async function deliver() {
    if (busy) return;
    const todo = batches();
    if (!todo.length) {
      status.notice = '还没有记下的条目：先在书签、喜欢页往下翻';
      render();
      return;
    }
    let token = storedToken();
    if (!token) {
      if (!askToken()) return;
      token = storedToken();
    }
    busy = true;
    status.warning = '';
    status.notice = '先确认本机上的是 usuyo…';
    render();
    const handed = { favorite: 0, like: 0 };
    let stop = '';
    try {
      const who = await prove(token);
      if (who === 'refused') {
        stop = 'usuyo 没让这个脚本管理器的请求进来，什么都没交';
      } else if (who === 'stranger') {
        stop = '端口 ' + PORT + ' 上的程序没能证明自己是 usuyo（也可能这个令牌在 usuyo 里撤掉了），令牌没有发出去，什么都没交';
      } else {
        for (const b of todo) {
          const links = b.ids.map((id) => {
            const item = store[b.action][id];
            const link = { url: postUrl(id, item.screen) };
            if (item.title) link.title = item.title;
            return link;
          });
          const res = await local(
            'POST',
            '/v1/batches',
            { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' },
            JSON.stringify({ name: 'X ' + LABEL[b.action] + ' ' + today(), action: b.action, links }),
          );
          if (res.status === 401) {
            stop = 'usuyo 不认这个令牌（可能撤掉了），剩下的没有收下，还存着；点「忘掉令牌」，再贴一个新的';
            break;
          }
          if (res.status !== 202) {
            stop = 'usuyo 这一批没有收下（回了 ' + res.status + '），剩下的还存着';
            break;
          }
          for (const id of b.ids) delete store[b.action][id];
          saveStore();
          handed[b.action] += b.ids.length;
        }
      }
    } catch (e) {
      if (e !== UNREACHABLE) throw e;
      stop = 'usuyo 没有开着（本机 ' + PORT + ' 端口没有回应），' + (handed.favorite + handed.like ? '剩下的还存着' : '什么都没交');
    } finally {
      busy = false;
    }
    const total = handed.favorite + handed.like;
    const done = total
      ? '交给 usuyo 了：书签 ' + handed.favorite + ' 条、喜欢 ' + handed.like + ' 条，到 usuyo 的导入页确认了才会去取。'
      : '';
    status.notice = done + stop;
    render();
  }

  /** 用浏览器自己的对话框问令牌，存进脚本管理器；存下了回 true。 */
  function askToken() {
    let value = null;
    try {
      value = reflectApply(nativePrompt, page, ['贴上 usuyo 导入页发的令牌（交进来的清单 › 发一个令牌）。它只存在脚本管理器里。']);
    } catch (e) {
      value = null;
    }
    if (typeof value !== 'string' || !trimmed(value).length) {
      status.notice = '没拿到令牌（对话框被关掉或没弹出来）；也可以从脚本管理器的菜单里点「填 usuyo 令牌」';
      render();
      return false;
    }
    value = trimmed(value);
    if (!isToken(value)) {
      status.notice = '这不像 usuyo 的投递令牌：应是一串 id、一个点、再一串秘密';
      render();
      return false;
    }
    GM_setValue('deliverToken', value);
    status.notice = '令牌存着了';
    render();
    return true;
  }

  // 按钮的入口：意料之外的错误在这里说出来，不留在「先确认…」上
  function handOver() {
    deliver().catch((e) => {
      status.notice = '没交成：' + String((e && e.message) || e).slice(0, 80);
      render();
    });
  }

  function forgetToken() {
    GM_setValue('deliverToken', '');
    status.notice = '令牌忘掉了；下次交的时候再贴一个';
    render();
  }

  GM_registerMenuCommand('填 usuyo 令牌', askToken);
  GM_registerMenuCommand('忘掉 usuyo 令牌', forgetToken);

  function clearAll() {
    store.favorite = {};
    store.like = {};
    status.notice = '';
    status.warning = '';
    saveStore();
    render();
  }

  // ---------- 角落里的一小块计数 ----------

  let ui = null;

  /** 书签页 `/i/bookmarks`、喜欢页 `/<screen_name>/likes`。 */
  function onSavedPage() {
    const path = page.location.pathname;
    return path === '/i/bookmarks' || /^\/[A-Za-z0-9_]{1,50}\/likes\/?$/.test(path);
  }

  function mount() {
    if (ui || !document.body) return;
    const host = document.createElement('div');
    const root = attachShadow.call(host, { mode: 'closed' });
    const style = document.createElement('style');
    style.textContent =
      ':host{all:initial}' +
      '.box{position:fixed;right:12px;bottom:12px;z-index:2147483647;max-width:260px;padding:8px 10px;' +
      'border-radius:8px;background:rgba(32,33,36,.86);color:#eee;font:12px/1.5 -apple-system,system-ui,sans-serif}' +
      '.row{display:flex;gap:6px;margin-top:6px}' +
      'button{flex:1;padding:3px 6px;border:1px solid rgba(255,255,255,.25);border-radius:5px;' +
      'background:transparent;color:inherit;font:inherit;cursor:pointer}' +
      'button:hover{background:rgba(255,255,255,.12)}' +
      '.key{display:flex;gap:6px;align-items:center;margin-top:4px;opacity:.75}.key button{flex:0 0 auto}' +
      '.note{margin-top:4px;opacity:.75}.warn{margin-top:4px;color:#ffb4a8}';
    const box = document.createElement('div');
    box.className = 'box';
    const line = document.createElement('div');
    const warn = document.createElement('div');
    warn.className = 'warn';
    const note = document.createElement('div');
    note.className = 'note';
    const row = document.createElement('div');
    row.className = 'row';
    const go = document.createElement('button');
    go.textContent = '交给 usuyo';
    go.addEventListener('click', handOver);
    const clear = document.createElement('button');
    clear.textContent = '清空';
    clear.addEventListener('click', clearAll);
    row.append(go, clear);
    // 令牌存着时只显示 id 的前几位
    const key = document.createElement('div');
    key.className = 'key';
    const keyText = document.createElement('span');
    const forget = document.createElement('button');
    forget.textContent = '忘掉令牌';
    forget.addEventListener('click', forgetToken);
    key.append(keyText, forget);
    box.append(line, warn, note, key, row);
    root.append(style, box);
    document.body.appendChild(host);
    ui = { host, line, warn, note, key, keyText };
    render();
  }

  function render() {
    if (!ui) return;
    ui.host.style.display = onSavedPage() ? '' : 'none';
    const part = (action) => {
      const end = status[action].hasMore === false ? '（已到底）' : '';
      return LABEL[action] + ' ' + count(action) + end;
    };
    ui.line.textContent = 'usuyo · ' + part('favorite') + ' · ' + part('like');
    ui.warn.textContent = status.warning;
    ui.note.textContent = status.notice || '自己往下翻，翻到的会记下来';
    const token = storedToken();
    ui.key.style.display = token ? '' : 'none';
    ui.keyText.textContent = token ? '令牌 ' + tokenId(token, 6) + '… 存在脚本管理器里' : '';
  }

  // 页面是单页应用：换页时只切换显示，不做别的
  let lastPath = '';
  setInterval(() => {
    mount();
    if (page.location.pathname !== lastPath) {
      lastPath = page.location.pathname;
      render();
    }
  }, 1000);
})();
