/* OpenRef Capture — service worker。
   ①ページ内 fetch（content script）が CORS 等で失敗した時の fallback fetch。
     activeTab 権限（または背景タブ経路で付与された全サイト権限）により、当該オリジンに fetch できる。
     service worker の fetch はページオリジンの CORS を受けない（content script の fetch は受ける）。
   ②popup が DOI ページ用に開いた背景タブの後始末。
     Chrome の拡張 service worker は 30 秒無活動で停止する（Chrome 114+ は port を開いているだけでは延びない。
     長寿命メッセージの送受信・拡張 API 呼び出しで延びる。Chrome 公式の extension service worker lifecycle 文書の記述）。
     よってメモリ上の Set だけに頼らず、タブ id を chrome.storage.session に持ち、
     (a) popup の port 切断 (b) この SW の起動時に popup が居なければ (c) alarms による期限（90 秒）
     で閉じる。閉じるのは **登録済みの id だけ**（ブラウザ再起動で tab id が振り直されても user のタブを閉じない —
     storage.session はブラウザ再起動で消えるので登録も消える）。popup は処理中 20 秒ごとに ping を送り、
     通常は SW が停止しないようにする。閉じ損ねは最長で alarm の 90 秒。 */
"use strict";

const TAB_KEY = "openref_hidden_tabs"; // {tabId: openedAt(ms)}
const TAB_TTL_MS = 90 * 1000;
const ALARM_PREFIX = "openref-hidden-tab-"; // + tabId

/* storage.session の読み書きは 1 本の直列キューで行う（onRemoved / onAlarm / onMessage が同時に走っても
   read-modify-write が交差しない = 単一の書き手） */
let queue = Promise.resolve();
function serialized(fn) {
  const run = queue.then(fn);
  queue = run.catch(() => {});
  return run;
}

async function loadTabs() {
  try {
    const v = await chrome.storage.session.get(TAB_KEY);
    return v[TAB_KEY] || {};
  } catch (e) {
    return {};
  }
}
async function saveTabs(tabs) {
  try {
    await chrome.storage.session.set({ [TAB_KEY]: tabs });
  } catch (e) { /* session storage 不可でも alarms で閉じる */ }
}
function registerTab(tabId) {
  return serialized(async () => {
    const tabs = await loadTabs();
    tabs[tabId] = Date.now();
    await saveTabs(tabs);
    try {
      chrome.alarms.create(ALARM_PREFIX + tabId, { when: Date.now() + TAB_TTL_MS });
    } catch (e) { /* alarms 不可 */ }
  });
}
/* 登録を外す。close=true のときは **登録されていた場合だけ** タブを閉じる */
function unregisterTab(tabId, close) {
  return serialized(async () => {
    const tabs = await loadTabs();
    const wasRegistered = Object.prototype.hasOwnProperty.call(tabs, String(tabId));
    if (wasRegistered) {
      delete tabs[tabId];
      await saveTabs(tabs);
    }
    try {
      chrome.alarms.clear(ALARM_PREFIX + tabId);
    } catch (e) { /* noop */ }
    if (close && wasRegistered) chrome.tabs.remove(Number(tabId)).catch(() => {});
  });
}
function closeAllRegistered() {
  return serialized(async () => {
    const tabs = await loadTabs();
    await saveTabs({});
    for (const id of Object.keys(tabs)) {
      try {
        chrome.alarms.clear(ALARM_PREFIX + id);
      } catch (e) { /* noop */ }
      chrome.tabs.remove(Number(id)).catch(() => {});
    }
  });
}
async function popupOpen() {
  try {
    const ctx = await chrome.runtime.getContexts({ contextTypes: ["POPUP"] });
    return ctx.length > 0;
  } catch (e) {
    return false; // getContexts 非対応（Chrome 116 未満）は「居ない」とみなして閉じる側に倒す
  }
}

// (b) SW 起動時: popup が居なければ前回の残りを閉じる
popupOpen().then((open) => {
  if (!open) closeAllRegistered();
});

// (a) popup の port 切断
chrome.runtime.onConnect.addListener((port) => {
  if (port.name !== "openref-popup") return;
  port.onDisconnect.addListener(() => {
    closeAllRegistered();
  });
});

// (c) 期限（登録済みの id だけ閉じる）
chrome.alarms.onAlarm.addListener((alarm) => {
  if (!alarm.name.startsWith(ALARM_PREFIX)) return;
  unregisterTab(alarm.name.slice(ALARM_PREFIX.length), true);
});

// 背景タブが（ユーザー操作等で）閉じられたら登録を外す（閉じない）
chrome.tabs.onRemoved.addListener((tabId) => {
  unregisterTab(tabId, false);
});

function bufToB64(buf) {
  const b = new Uint8Array(buf);
  const CHUNK = 0x8000;
  const parts = [];
  for (let i = 0; i < b.length; i += CHUNK) {
    parts.push(String.fromCharCode.apply(null, b.subarray(i, i + CHUNK)));
  }
  return btoa(parts.join(""));
}
function isPdfBytes(b) {
  return b.length > 4 && b[0] === 0x25 && b[1] === 0x50 && b[2] === 0x44 && b[3] === 0x46;
}

/* SW 側の 1 回追従は **meta refresh だけ**（content.js は加えて「同一オリジンの .pdf パスがちょうど 1 つ」も見る。
   SW には DOMParser が無く同じ規則を忠実に再現できないので、規則を狭めて差を無くす方を選んだ）。
   HTML コメントと noscript は無視。 */
function nextPdfUrlFromHtml(html, base) {
  html = html.replace(/<!--[\s\S]*?-->/g, "").replace(/<noscript\b[\s\S]*?<\/noscript>/gi, "");
  const metas = html.match(/<meta\b[^>]*>/gi) || [];
  for (const m of metas) {
    if (!/http-equiv\s*=\s*["']?refresh["']?/i.test(m)) continue;
    const c = /content\s*=\s*(?:"([^"]*)"|'([^']*)')/i.exec(m);
    const content = (c ? c[1] || c[2] || "" : "").replace(/&amp;/g, "&").replace(/&#x2f;/gi, "/").replace(/&#47;/g, "/");
    const delay = parseInt(content, 10);
    if (!isNaN(delay) && delay > 10) continue; // logout タイマー等は中継ではない
    const mm = /url\s*=\s*['"]?([^'"]+)/i.exec(content);
    if (mm) {
      try {
        return new URL(mm[1].trim(), base).href;
      } catch (e) { /* 次へ */ }
    }
  }
  return null;
}

async function fetchPdf(url, follow, hopped) {
  const r = await fetch(url, { credentials: "include" });
  if (!r.ok) throw new Error("HTTP " + r.status);
  const buf = await r.arrayBuffer();
  const b = new Uint8Array(buf);
  if (!isPdfBytes(b)) {
    if (follow && !hopped) {
      const html = new TextDecoder().decode(buf.slice(0, 2 * 1024 * 1024));
      const next = nextPdfUrlFromHtml(html, r.url || url);
      if (next && next !== url) return fetchPdf(next, true, true);
    }
    const err = new Error("PDF ではない応答（" + buf.byteLength + " bytes" + (hopped ? "・中継先も HTML" : "") + "）");
    err.html = true; // popup が「PDF タブで開いて取り直す」経路に回す目印（content.js と同じ）
    throw err;
  }
  return bufToB64(buf);
}

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg && msg.type === "openref-ping") {
    sendResponse({ ok: true }); // 受信自体が idle timer を延ばす
    return false;
  }
  if (msg && msg.type === "openref-hidden-tab") {
    (msg.open ? registerTab(msg.tabId) : unregisterTab(msg.tabId, false)).then(
      () => sendResponse({ ok: true }),
      (e) => sendResponse({ ok: false, error: String(e) })
    );
    return true; // async
  }
  if (msg && msg.type === "openref-bg-fetch-pdf") {
    fetchPdf(msg.url, !!msg.follow, false)
      .then((b64) => sendResponse({ ok: true, b64 }))
      .catch((e) => sendResponse({ ok: false, error: String(e), html: !!e.html }));
    return true; // async
  }
  return false;
});
