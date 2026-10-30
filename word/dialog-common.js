/* OpenRef for Word — ダイアログ共通（cite-dialog / settings-dialog / notice）。
   ?host=dotm&sid=…（OpenRef Dialog.app の WKWebView で開く）: 結果のメッセージ（JSON）を connectd の
   /word/v2/dialog/<sid>/result へ POST し、届いたら document.title = "openref:done"（窓を閉じる合図）。
   host=dotm なし（普通のブラウザで開いたプレビュー・検証用）: 送らずに console.log + 画面下の debug 行に出す */
(function (root) {
  "use strict";

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

  /** 結果を返す。戻り値 false = 送っていない（プレビュー。呼び手は操作を戻す） */
  function send(msg) {
    if (DOTM) return sendDotm(msg);
    var s = JSON.stringify(msg);
    console.log("[OpenRef dialog → result]", s);
    debug("result: " + s);
    return false;
  }

  /** ページの初期化。待つものは無い（呼び手の形を保つための入口） */
  function ready(fn) {
    fn({});
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
    esc: esc,
    fetchJson: fetchJson,
    copyText: copyText,
    debug: debug,
  };
})(window);
