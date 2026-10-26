/* OpenRef Word アドイン — ダイアログ共通（cite-dialog / settings-dialog / notice）。
   親（関数コマンド）へは Office.context.ui.messageParent で JSON 文字列を返す。Office 外（普通のブラウザ）で開いたときは
   messageParent の代わりに console.log + 画面下の debug 行に出す（プレビュー・検証用）。
   ?host=dotm&sid=…（Startup テンプレート版 = OpenRef Dialog.app の WKWebView で開く）では
   同じメッセージを connectd の /word/v2/dialog/<sid>/result へ POST し、届いたら document.title = "openref:done"（窓を閉じる合図）。
   office.js は読まない（各 HTML が host=dotm のときは読み込まない）・Office を待たない・5 分制限の予告帯も出さない */
/* global Office */
(function (root) {
  "use strict";

  var host = null; // Office.onReady の host（Office 外 = null）
  var readyFired = false;
  var QS = new URLSearchParams(location.search);
  var DOTM = QS.get("host") === "dotm";
  var DONE = "openref:done";

  /** URL クエリ（d = JSON・他は文字列） */
  function params() {
    var out = {};
    new URLSearchParams(location.search).forEach(function (v, k) {
      if (k === "d") {
        try {
          out.d = JSON.parse(v);
        } catch (e) {
          out.d = null;
          out.dError = e.message;
        }
      } else {
        out[k] = v;
      }
    });
    return out;
  }

  function debug(text) {
    var el = document.getElementById("rd-debug");
    if (!el) {
      el = document.createElement("div");
      el.id = "rd-debug";
      el.className = "rd-debug";
      document.body.appendChild(el);
    }
    el.textContent = text;
  }

  /** Startup テンプレート版: connectd へ POST → 届いたら窓を閉じる合図。送れなければ理由を出して窓は残す
      （× で閉じればアプレットが closed を送る = キャンセル扱い）。戻り値 true = 送信を始めた */
  function sendDotm(msg) {
    var sid = QS.get("sid") || "";
    fetch("/word/v2/dialog/" + encodeURIComponent(sid) + "/result", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-OpenRef": "word" },
      body: JSON.stringify(msg),
    })
      .then(function (r) {
        if (!r.ok) throw new Error("HTTP " + r.status);
        document.title = DONE;
      })
      .catch(function (e) {
        console.error("OpenRef: 結果を送れませんでした", e);
        debug("結果を OpenRef（connectd）へ送れませんでした（" + e.message + "）。× で閉じてやり直してください。");
      });
    return true;
  }

  /** 親へ JSON を返す。Office の中なら messageParent、外なら debug 表示 */
  function send(msg) {
    if (DOTM) return sendDotm(msg);
    var s = JSON.stringify(msg);
    var sent = false;
    try {
      if (readyFired && root.Office && Office.context && Office.context.ui && typeof Office.context.ui.messageParent === "function") {
        Office.context.ui.messageParent(s);
        sent = true;
      }
    } catch (e) {
      console.warn("OpenRef: messageParent に失敗", e);
    }
    if (!sent || !host) {
      console.log("[OpenRef dialog → parent]", s);
      debug("messageParent: " + s);
    }
    return sent;
  }

  /** Office.onReady の後に fn(info)。office.js が読めない・Office 外でも動く（onReady が来ない環境は 3 秒で見切る） */
  function ready(fn) {
    var started = false;
    function go(info) {
      if (started) return;
      started = true;
      fn(info || {});
    }
    if (DOTM || typeof root.Office === "undefined" || typeof Office.onReady !== "function") {
      go({});
      return;
    }
    Office.onReady(function (info) {
      readyFired = true;
      host = (info && info.host) || null;
      go(info);
    });
    setTimeout(function () {
      go({});
    }, 3000);
  }

  /** WKWebView は autofocus を無視しうる → 明示 focus()（描画直後と少し後にもう一度） */
  function focus(el) {
    if (!el) return;
    function f() {
      try {
        if (document.activeElement !== el) el.focus();
      } catch (e) {
        /* 表示前 */
      }
    }
    f();
    setTimeout(f, 60);
    setTimeout(f, 300);
  }

  /** 関数コマンドの runtime は 5 分で閉じる（親は 4:45 で自動キャンセル）→ 4:15 で予告帯を出す。
      Startup テンプレート版には 5 分制限が無いので出さない */
  function limitBanner() {
    if (DOTM) return;
    setTimeout(function () {
      var el = document.createElement("div");
      el.className = "rd-limit";
      el.textContent = "Word の制限でまもなく閉じます（約 30 秒後）。必要な操作はお早めに。";
      // 縦いっぱいの flex 枠（.rd-page）の先頭に入れる = 下端のボタン・フッターを押し出さない
      var page = document.querySelector(".rd-page") || document.body;
      page.insertBefore(el, page.firstChild);
    }, (4 * 60 + 15) * 1000);
  }

  function esc(s) {
    return String(s == null ? "" : s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function fetchJson(url, opts) {
    return fetch(url, opts).then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status);
      return r.json();
    });
  }

  /** クリップボードへ（navigator.clipboard → textarea + execCommand('copy')） */
  function copyText(text) {
    function fallback() {
      var ta = document.createElement("textarea");
      ta.value = text;
      ta.setAttribute("readonly", "");
      ta.style.position = "fixed";
      ta.style.left = "-9999px";
      document.body.appendChild(ta);
      ta.select();
      var ok = false;
      try {
        ok = document.execCommand("copy");
      } catch (e) {
        ok = false;
      }
      document.body.removeChild(ta);
      return ok ? Promise.resolve() : Promise.reject(new Error("コピーできませんでした"));
    }
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text).catch(fallback);
    }
    return fallback();
  }

  root.OpenRefDialog = {
    params: params,
    send: send,
    ready: ready,
    focus: focus,
    limitBanner: limitBanner,
    esc: esc,
    fetchJson: fetchJson,
    copyText: copyText,
    debug: debug,
  };
})(window);
