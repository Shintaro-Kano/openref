/* OpenRef — Citation Style... / Document Settings... ダイアログ（tab=style / tab=doc）。
   初期値 d = {style, doiAlways, fontName, fontSize, cites, pinned}（文書の実効のスタイル・DOI 設定・書式・引用数。VBA の ORDoc.EffectivePrefs）。
   結果: {type:"apply", style, doiAlways, fontName, fontSize} / {type:"cancel"} */
(function () {
  "use strict";
  var RD = window.OpenRefDialog;
  var p = RD.params();
  var d = p.d || {};
  var S = {
    tab: p.tab === "doc" ? "doc" : "style",
    styles: [],
    loaded: false,
    error: "",
    sel: typeof d.style === "string" && d.style ? d.style : "vancouver",
    sent: false,
  };
  var el = {};

  function $(id) {
    return document.getElementById(id);
  }

  function showTab(t) {
    S.tab = t;
    el.tabStyle.setAttribute("aria-selected", t === "style" ? "true" : "false");
    el.tabDoc.setAttribute("aria-selected", t === "doc" ? "true" : "false");
    el.paneStyle.hidden = t !== "style";
    el.paneDoc.hidden = t !== "doc";
    RD.focus(t === "style" ? el.filter : el.font);
  }

  function visible() {
    var f = el.filter.value.trim().toLowerCase();
    return S.styles.filter(function (s) {
      return !f || (s.title || "").toLowerCase().indexOf(f) >= 0 || s.id.indexOf(f) >= 0;
    });
  }

  function titleOf(id) {
    for (var i = 0; i < S.styles.length; i++) if (S.styles[i].id === id) return S.styles[i].title || id;
    return id;
  }

  function msg(text, err) {
    var m = document.createElement("div");
    m.className = "rd-msg" + (err ? " rd-err" : "");
    m.textContent = text;
    el.list.appendChild(m);
  }

  function renderStyles(scroll) {
    var box = el.list;
    box.innerHTML = "";
    if (!S.loaded) {
      msg("読み込み中…");
      return;
    }
    if (S.error) msg("スタイル一覧を取得できません（connectd: " + S.error + "）", true);
    var list = visible();
    list.forEach(function (s) {
      var on = s.id === S.sel;
      var row = document.createElement("div");
      row.className = "rd-opt" + (on ? " rd-on" : "");
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", on ? "true" : "false");
      row.innerHTML =
        '<span class="rd-mark">' + (on ? "✓" : "") + "</span>" +
        "<span>" + RD.esc(s.title || s.id) + "</span>" +
        '<span class="rd-opt-id">' + RD.esc(s.id) + "</span>";
      row.addEventListener("click", function () {
        S.sel = s.id;
        renderStyles(false);
        renderFoot();
      });
      row.addEventListener("dblclick", function () {
        S.sel = s.id;
        apply();
      });
      box.appendChild(row);
      if (on && scroll && row.scrollIntoView) row.scrollIntoView({ block: "nearest" });
    });
    if (!list.length && !S.error) msg("該当なし");
  }

  function renderFoot() {
    var n = d.cites || 0;
    el.foot.textContent =
      "この文書の設定 · " + titleOf(S.sel) + " · " +
      (n ? "適用すると引用 " + n + " 箇所と文献リストを再整形" : "引用はまだありません（挿入時にこの設定で整形）");
  }

  /** 絞り込み欄の ↑↓ / Enter: 表示中の候補の中で選択を動かす */
  function moveSel(delta) {
    var list = visible();
    if (!list.length) return;
    var i = -1;
    for (var k = 0; k < list.length; k++) if (list[k].id === S.sel) i = k;
    i = i < 0 ? 0 : Math.max(0, Math.min(list.length - 1, i + delta));
    S.sel = list[i].id;
    renderStyles(true);
    renderFoot();
  }

  function finish(m) {
    S.sent = true;
    el.ok.disabled = true;
    el.cancel.disabled = true;
    if (!RD.send(m)) {
      setTimeout(function () {
        S.sent = false;
        el.ok.disabled = false;
        el.cancel.disabled = false;
      }, 1500);
    }
  }
  function cancel() {
    if (!S.sent) finish({ type: "cancel" });
  }
  function apply() {
    if (S.sent) return;
    var size = parseFloat(el.size.value);
    if (!isFinite(size) || size < 4 || size > 72) {
      el.err.textContent = "サイズは 4〜72 pt の数値で入力してください。";
      el.err.hidden = false;
      showTab("doc");
      RD.focus(el.size);
      return;
    }
    el.err.hidden = true;
    finish({
      type: "apply",
      style: S.sel,
      doiAlways: !!el.doi.checked,
      fontName: el.font.value.trim() || "Century",
      fontSize: size,
    });
  }

  function loadStyles() {
    RD.fetchJson("/api/styles").then(
      function (r) {
        S.styles = (r && r.styles) || [];
        S.loaded = true;
        finishLoad();
      },
      function (e) {
        S.error = e.message;
        S.loaded = true;
        finishLoad();
      }
    );
  }
  function finishLoad() {
    // 文書のスタイルが一覧に無くても選べるように出す。「未導入」と言えるのは一覧を取得できたときだけ
    // （取得に失敗したときは導入済みかどうか分からない）
    var known = S.styles.some(function (s) {
      return s.id === S.sel;
    });
    if (!known) S.styles.unshift({ id: S.sel, title: S.error ? S.sel : S.sel + "（この Mac に未導入 — 適用時に取得）" });
    renderStyles(true);
    renderFoot();
  }

  RD.ready(function () {
    el.tabStyle = $("rd-tab-style");
    el.tabDoc = $("rd-tab-doc");
    el.paneStyle = $("rd-pane-style");
    el.paneDoc = $("rd-pane-doc");
    el.filter = $("rd-filter");
    el.list = $("rd-styles");
    el.doi = $("rd-doi");
    el.font = $("rd-font");
    el.size = $("rd-size");
    el.err = $("rd-err");
    el.ok = $("rd-ok");
    el.cancel = $("rd-cancel");
    el.foot = $("rd-foot");

    el.doi.checked = d.doiAlways === true;
    el.font.value = d.fontName || "Century";
    el.size.value = d.fontSize || 10.5;

    el.tabStyle.addEventListener("click", function () {
      showTab("style");
    });
    el.tabDoc.addEventListener("click", function () {
      showTab("doc");
    });
    el.filter.addEventListener("input", function () {
      renderStyles(false);
    });
    el.filter.addEventListener("keydown", function (e) {
      if (e.isComposing || e.keyCode === 229) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        moveSel(e.key === "ArrowDown" ? 1 : -1);
      } else if (e.key === "Enter" && !(e.metaKey || e.ctrlKey)) {
        // 絞り込みで現在の選択が隠れていたら、見えている先頭を選ぶ
        e.preventDefault();
        var list = visible();
        var shown = list.some(function (s) {
          return s.id === S.sel;
        });
        if (!shown && list.length) {
          S.sel = list[0].id;
          renderStyles(true);
          renderFoot();
        }
      }
    });
    document.addEventListener("keydown", function (e) {
      if (e.isComposing || e.keyCode === 229) return;
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey || (e.target !== el.filter && e.target.tagName === "INPUT"))) {
        e.preventDefault();
        apply();
      }
    });
    el.cancel.addEventListener("click", cancel);
    el.ok.addEventListener("click", apply);

    renderStyles(false);
    renderFoot();
    loadStyles();
    showTab(S.tab);
  });
})();
