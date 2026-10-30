/* OpenRef — Add/Edit Citation ダイアログ（Paperpile の検索ダイアログ相当・文書の上に浮く）。
   初期データ d = {mode:"add"|"edit", items:[{key, label?}], style:{id, title}, missing?}（URL クエリ）。
   結果のメッセージ（JSON。dialog-common.js の send が connectd へ送る）:
     {type:"insert"|"replace", keys:[...]} / {type:"delete"}（編集で 0 件を確定）/ {type:"cancel"} */
(function () {
  "use strict";
  var RD = window.OpenRefDialog;
  var p = RD.params();
  var d = p.d || {};
  var mode = d.mode === "edit" ? "edit" : "add";
  var S = {
    chips: [], // [{key, label, title}]
    results: [], // /api/search の items
    hi: -1, // 強調行
    status: "hint", // hint | loading | ok | empty | error
    error: "",
    seq: 0,
    timer: null,
    pendingEnter: false, // 検索待ちの間に Enter → 結果が来たら強調行（先頭）を追加
    sent: false,
  };
  var el = {};

  (d.items || []).forEach(function (it) {
    if (it && typeof it.key === "string" && !has(it.key)) S.chips.push({ key: it.key, label: it.label || it.key, title: "" });
  });

  function $(id) {
    return document.getElementById(id);
  }
  function has(key) {
    return S.chips.some(function (c) {
      return c.key === key;
    });
  }
  function resultLabel(it) {
    return (it.first_author || "?") + (it.year ? " " + it.year : "");
  }
  function meta(it) {
    return [it.first_author, it.journal, it.year]
      .filter(function (v) {
        return v !== null && v !== undefined && v !== "";
      })
      .join(" · ");
  }

  // ---------- 描画 ----------
  function renderChips() {
    Array.prototype.slice.call(el.search.querySelectorAll(".rd-chip")).forEach(function (n) {
      n.parentNode.removeChild(n);
    });
    S.chips.forEach(function (c, i) {
      var chip = document.createElement("span");
      chip.className = "rd-chip";
      if (c.title) chip.title = c.title;
      var lab = document.createElement("span");
      lab.className = "rd-chip-label";
      lab.textContent = c.label;
      var x = document.createElement("button");
      x.type = "button";
      x.className = "rd-chip-x";
      x.textContent = "×";
      x.title = "外す";
      x.setAttribute("aria-label", c.label + " を外す");
      x.addEventListener("mousedown", function (e) {
        e.preventDefault(); // 入力欄のフォーカスを保つ
      });
      x.addEventListener("click", function (e) {
        e.stopPropagation();
        if (S.sent) return;
        S.chips.splice(i, 1);
        changed();
        RD.focus(el.q);
      });
      chip.appendChild(lab);
      chip.appendChild(x);
      el.search.insertBefore(chip, el.q);
    });
    el.q.placeholder = S.chips.length ? "続けて検索…" : "蔵書を検索（著者・タイトル・誌名・年）";
  }

  function renderButtons() {
    var ok = el.ok;
    var n = S.chips.length;
    ok.classList.remove("rd-danger");
    ok.classList.add("rd-primary");
    if (mode === "edit") {
      if (n) {
        ok.textContent = "更新";
      } else {
        ok.textContent = "引用を削除";
        ok.classList.remove("rd-primary");
        ok.classList.add("rd-danger");
      }
      ok.disabled = false;
    } else {
      ok.textContent = "挿入";
      ok.disabled = !n;
    }
    if (S.sent) {
      ok.disabled = true;
      el.cancel.disabled = true;
    } else {
      el.cancel.disabled = false;
    }
  }

  function msg(text, err) {
    var m = document.createElement("div");
    m.className = "rd-msg" + (err ? " rd-err" : "");
    m.textContent = text;
    el.results.appendChild(m);
  }

  function renderResults() {
    var box = el.results;
    box.innerHTML = "";
    if (S.status === "hint") {
      msg("著者名・タイトル・誌名・年で検索 → Enter で追加 → 空欄で Enter で" + (mode === "edit" ? "更新" : "挿入"));
      return;
    }
    if (S.status === "error") {
      msg("検索できません: " + S.error + "（OpenRef（connectd）は起動していますか）", true);
      return;
    }
    if (S.status === "empty") {
      msg("該当なし");
      return;
    }
    if (S.status === "loading" && !S.results.length) {
      msg("検索中…");
      return;
    }
    S.results.forEach(function (it, i) {
      var sel = has(it.key);
      var row = document.createElement("div");
      row.className = "rd-row" + (i === S.hi ? " rd-hi" : "") + (sel ? " rd-sel" : "");
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", i === S.hi ? "true" : "false");
      row.innerHTML =
        '<span class="rd-mark">' + (sel ? "✓" : "") + "</span>" +
        '<div class="rd-row-body"><div class="rd-title">' + RD.esc(it.title || "（無題）") + "</div>" +
        '<div class="rd-meta">' + RD.esc(meta(it)) + "</div></div>";
      row.addEventListener("mousedown", function (e) {
        e.preventDefault(); // 入力欄のフォーカスを保つ
      });
      row.addEventListener("click", function () {
        if (S.sent) return;
        S.hi = i;
        toggle(it);
        RD.focus(el.q);
      });
      row.addEventListener("mousemove", function () {
        if (S.hi !== i) {
          S.hi = i;
          markHi(false);
        }
      });
      box.appendChild(row);
    });
  }

  function markHi(scroll) {
    var rows = el.results.querySelectorAll(".rd-row");
    for (var i = 0; i < rows.length; i++) {
      rows[i].classList.toggle("rd-hi", i === S.hi);
      rows[i].setAttribute("aria-selected", i === S.hi ? "true" : "false");
    }
    if (scroll && rows[S.hi] && rows[S.hi].scrollIntoView) rows[S.hi].scrollIntoView({ block: "nearest" });
  }

  function changed() {
    renderChips();
    renderButtons();
    renderResults();
  }

  // ---------- 操作 ----------
  function add(it) {
    if (!it || has(it.key)) return;
    S.chips.push({ key: it.key, label: resultLabel(it), title: it.title || "" });
    changed();
  }
  function toggle(it) {
    for (var i = 0; i < S.chips.length; i++) {
      if (S.chips[i].key === it.key) {
        S.chips.splice(i, 1);
        changed();
        return;
      }
    }
    add(it);
  }
  /** Enter（入力あり）: 強調行をチップに追加して入力を空に */
  function enterAdd() {
    var it = S.results[S.hi];
    if (!it) return;
    add(it);
    el.q.value = "";
    onInput();
  }
  function move(delta) {
    if (!S.results.length) return;
    S.hi = Math.max(0, Math.min(S.results.length - 1, (S.hi < 0 ? -1 : S.hi) + delta));
    markHi(true);
  }

  function onInput() {
    var q = el.q.value;
    el.clear.hidden = !q;
    S.pendingEnter = false;
    clearTimeout(S.timer);
    S.timer = null;
    if (!q.trim()) {
      S.seq++; // 走っている検索の結果は捨てる
      S.results = [];
      S.hi = -1;
      S.status = "hint";
      renderResults();
      return;
    }
    S.timer = setTimeout(function () {
      search(q.trim());
    }, 250);
  }
  function search(q) {
    var seq = ++S.seq;
    S.timer = null;
    S.status = "loading";
    if (!S.results.length) renderResults();
    RD.fetchJson("/api/search?q=" + encodeURIComponent(q)).then(
      function (r) {
        if (seq !== S.seq) return;
        S.results = (r && r.items) || [];
        S.hi = S.results.length ? 0 : -1;
        S.status = S.results.length ? "ok" : "empty";
        renderResults();
        if (S.pendingEnter) {
          S.pendingEnter = false;
          enterAdd();
        }
      },
      function (e) {
        if (seq !== S.seq) return;
        S.results = [];
        S.hi = -1;
        S.status = "error";
        S.error = e.message;
        S.pendingEnter = false;
        renderResults();
      }
    );
  }
  function searching() {
    return !!S.timer || S.status === "loading";
  }

  function onKey(e) {
    if (e.isComposing || e.keyCode === 229) return; // IME 変換中（Enter = 変換の確定）は触らない
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Enter":
        e.preventDefault();
        if (e.metaKey || e.ctrlKey) {
          confirm(false);
        } else if (el.q.value.trim()) {
          if (searching()) S.pendingEnter = true;
          else enterAdd();
        } else if (S.chips.length) {
          confirm(false);
        }
        break;
      case "Backspace":
        if (!el.q.value && S.chips.length && !S.sent) {
          e.preventDefault();
          S.chips.pop();
          changed();
        }
        break;
      case "Escape":
        e.preventDefault();
        cancel();
        break;
    }
  }

  // ---------- 親へ返す ----------
  function finish(m) {
    S.sent = true;
    el.q.disabled = true;
    renderButtons();
    if (!RD.send(m)) {
      // プレビュー（host=dotm なし）: 送っていないので操作を戻す
      setTimeout(function () {
        S.sent = false;
        el.q.disabled = false;
        renderButtons();
      }, 1500);
    }
  }
  function cancel() {
    if (!S.sent) finish({ type: "cancel" });
  }
  /** 確定。キー操作（fromButton = false）はチップがあるときだけ。0 件の削除は赤いボタンからのみ */
  function confirm(fromButton) {
    if (S.sent) return;
    var keys = S.chips.map(function (c) {
      return c.key;
    });
    if (keys.length) finish({ type: mode === "edit" ? "replace" : "insert", keys: keys });
    else if (mode === "edit" && fromButton) finish({ type: "delete" });
  }

  /** 編集: チップの title 属性に論文タイトル（label は connectd が d に入れて渡す） */
  function loadChipTitles() {
    if (!S.chips.length) return;
    RD.fetchJson("/api/items", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        keys: S.chips.map(function (c) {
          return c.key;
        }),
      }),
    }).then(
      function (m) {
        S.chips.forEach(function (c) {
          var csl = m && m[c.key];
          if (csl) {
            c.title = csl.title || "";
          } else {
            c.title = "蔵書に見つかりません（文書内のデータで整形されます）";
          }
        });
        renderChips();
      },
      function (e) {
        console.warn("OpenRef: 引用の書誌を取得できません", e);
      }
    );
  }

  RD.ready(function () {
    el.search = $("rd-search");
    el.q = $("rd-q");
    el.clear = $("rd-clear");
    el.results = $("rd-results");
    el.ok = $("rd-ok");
    el.cancel = $("rd-cancel");
    el.note = $("rd-note");

    el.search.addEventListener("mousedown", function (e) {
      if (e.target === el.search) {
        e.preventDefault();
        RD.focus(el.q);
      }
    });
    el.q.addEventListener("input", onInput);
    el.q.addEventListener("keydown", onKey);
    document.addEventListener("keydown", function (e) {
      if (e.target === el.q || e.isComposing) return;
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        confirm(false);
      }
    });
    el.clear.addEventListener("click", function () {
      el.q.value = "";
      onInput();
      RD.focus(el.q);
    });
    el.cancel.addEventListener("click", cancel);
    el.ok.addEventListener("click", function () {
      confirm(true);
    });

    var notes = [];
    if (p.dError) notes.push("初期データを読めません（" + p.dError + "）。");
    if (mode === "edit" && d.missing) {
      notes.push("この引用のデータが文書にありません（別の文書からコピーした引用など）。文献を検索して付け直すか、［引用を削除］で消してください。");
    }
    if (notes.length) {
      el.note.textContent = notes.join(" ");
      el.note.hidden = false;
    }
    var st = d.style && (d.style.title || d.style.id);
    $("rd-style").textContent = (mode === "edit" ? "引用を編集 · " : "") + (st ? "スタイル: " + st : "");

    changed();
    loadChipTitles();
    RD.focus(el.q);
  });
})();
