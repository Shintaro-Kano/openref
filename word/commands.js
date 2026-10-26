/* OpenRef Word アドイン — リボン「OpenRef」タブの関数コマンド（manifest の FunctionFile = commands.html）。
   Paperpile の Word プラグインと同じ動線: Add/Edit Citation はカーソル位置に仮置き […] を先に入れてから、文書の上に浮く
   検索ダイアログを開く（Cancel で仮置きは消える）。サイドの taskpane は使わない。
   各関数は event.completed() をどの経路でもちょうど 1 回呼ぶ（ダイアログが閉じるまで待ってから）。例外は notice（error）で見せてから。 */
/* global Office */
(function (root) {
  "use strict";
  var D = root.OpenRefDoc;

  // 関数コマンドの runtime は 5 分で閉じる（Microsoft docs: "The runtime closes when either the function completes or
  // 5 minutes have passed"）→ ダイアログを開いたままの関数は 4:45 で自動キャンセル（ダイアログ側は 4:15 で予告帯）
  var LIMIT_MS = (4 * 60 + 45) * 1000;
  // それでも終わらない（Word の応答待ちで止まった等）ときの最終手段。runtime が閉じる前に completed を返す
  var HARD_STOP_MS = (4 * 60 + 55) * 1000;
  // dialog.close() は非同期で待てない → 閉じた直後の displayDialogAsync は 12007 になりうるので間隔を置いて再試行
  var RETRY_12007 = 10;
  var RETRY_MS = 300;
  /** 引用ダイアログの URL 上限（超えたら items の label を落として key だけ渡す） */
  var CITE_URL_LIMIT = 2000;
  /** notice の URL 上限（超えたら detail → message の順に詰める） */
  var NOTICE_URL_LIMIT = 8000;
  var SIZE = {
    cite: { width: 42, height: 50 },
    settings: { width: 40, height: 60 },
    notice: { width: 30, height: 28 },
    confirm: { width: 34, height: 36 },
    info: { width: 40, height: 56 },
  };
  var NO_CITES = {
    message: "この文書に OpenRef の引用が見つかりません。",
    detail: "Add/Edit Citation で引用を挿入してください。",
  };
  /** 引用の CC はあるが payload が 1 つも無い（別の文書から貼り付けた引用だけ） */
  var NO_DATA = {
    message: "引用はありますが OpenRef の引用データがありません（別の文書から貼り付けた引用は Add/Edit で付け直してください）",
  };
  var REMOVE_MSG =
    "引用と文献リストを通常のテキストに変換し、文書内の引用データ（書誌のスナップショット）も削除します。" +
    "以後 OpenRef で更新・編集できません。投稿用の最終版で使い、編集用の原本は別名で保存しておくことをおすすめします。" +
    "旧版で削除した引用の控えは文書内に残ることがあります。";

  // 同一 runtime 内での二重押下ガード（ダイアログは host window あたり 1 枚しか開けない = 12007）。
  // 実 Office の関数コマンドはコマンドごとに別の runtime で動く（MS docs）ので通常は効かないが、害は無いので残す
  var running = null;

  // ---------- ダイアログ ----------
  function dialogUrl(page, params) {
    var q = Object.keys(params)
      .filter(function (k) {
        return params[k] !== undefined && params[k] !== null;
      })
      .map(function (k) {
        var v = params[k];
        return encodeURIComponent(k) + "=" + encodeURIComponent(typeof v === "string" ? v : JSON.stringify(v));
      })
      .join("&");
    return location.origin + "/word/" + page + (q ? "?" + q : "");
  }

  function openDialog(url, size) {
    return new Promise(function (resolve, reject) {
      var tries = 0;
      (function attempt() {
        Office.context.ui.displayDialogAsync(
          url,
          { width: size.width, height: size.height, displayInIframe: false },
          function (r) {
            if (r.status === Office.AsyncResultStatus.Succeeded) return resolve(r.value);
            var code = r.error && r.error.code;
            if (code === 12007 && tries < RETRY_12007) {
              tries++;
              setTimeout(attempt, RETRY_MS);
              return;
            }
            reject(D.userError("ダイアログを開けません（" + code + "）", r.error && r.error.message));
          }
        );
      })();
    });
  }

  /** ダイアログを開き、最初のメッセージ（JSON）・× で閉じた・期限切れのどれかまで待って閉じる。
      戻り値 = メッセージ {type:...} / {type:"closed"}（× = 12006）/ {type:"timeout"}。
      読込失敗（12002 / 12003 など）は閉じてから reject。onOpen = 開いた直後に呼ぶ */
  function runDialog(cmd, page, params, size, onOpen) {
    if (cmd.expired) return Promise.resolve({ type: "timeout" });
    return openDialog(dialogUrl(page, params), size).then(function (dlg) {
      if (onOpen) onOpen();
      return new Promise(function (resolve, reject) {
        var settled = false;
        function settle(close) {
          if (settled) return false;
          settled = true;
          cmd.onExpire = null;
          if (close) {
            try {
              dlg.close();
            } catch (e) {
              console.warn("OpenRef: dialog.close() に失敗", e);
            }
          }
          return true;
        }
        dlg.addEventHandler(Office.EventType.DialogMessageReceived, function (arg) {
          var msg = null;
          try {
            msg = JSON.parse(arg.message);
          } catch (e) {
            /* 下で invalid 扱い */
          }
          if (settle(true)) resolve(msg && typeof msg.type === "string" ? msg : { type: "invalid", raw: String(arg && arg.message) });
        });
        dlg.addEventHandler(Office.EventType.DialogEventReceived, function (arg) {
          var code = arg && arg.error;
          if (code === 12006) {
            // user が × で閉じた = キャンセル扱い
            if (settle(false)) resolve({ type: "closed", code: code });
          } else if (settle(true)) {
            reject(D.userError("ダイアログを表示できませんでした（" + code + "）", location.origin + "/word/" + page + " の配信（connectd）を確認してください。"));
          }
        });
        cmd.onExpire = function () {
          if (settle(true)) resolve({ type: "timeout" });
        };
        if (cmd.expired) cmd.onExpire();
      });
    });
  }

  function notice(cmd, kind, d, size) {
    var p = { kind: kind };
    ["title", "message", "detail", "ok", "danger", "copy"].forEach(function (k) {
      if (d[k] !== undefined && d[k] !== null && d[k] !== "") p[k] = d[k];
    });
    // URL が長すぎるときは detail → message の順に詰める
    while (dialogUrl("notice.html", { d: p }).length > NOTICE_URL_LIMIT) {
      if (p.detail && p.detail.length > 40) p.detail = p.detail.slice(0, Math.floor(p.detail.length * 0.7)) + "…";
      else if (p.detail) delete p.detail;
      else if (p.message && p.message.length > 40) p.message = p.message.slice(0, Math.floor(p.message.length * 0.7)) + "…";
      else break;
    }
    var sz = size || (kind === "confirm" ? SIZE.confirm : kind === "info" ? SIZE.info : SIZE.notice);
    return runDialog(cmd, "notice.html", { d: p }, sz);
  }

  function errorNotice(cmd, e) {
    console.error("OpenRef:", e);
    var d;
    if (e && e.user) {
      d = { message: e.message, detail: e.detail };
    } else {
      // OfficeExtension.Error は code / debugInfo.errorLocation が手がかり
      var info = [];
      if (e && e.code) info.push(e.code);
      if (e && e.debugInfo && e.debugInfo.errorLocation) info.push(e.debugInfo.errorLocation);
      d = { message: (e && e.message) || String(e), detail: info.join(" · ") };
    }
    return notice(cmd, "error", d);
  }

  // ---------- コマンドの枠 ----------
  /** event.completed() をどの経路でもちょうど 1 回（本体 → 失敗なら error notice → completed）。
      同じ runtime で実行中に別のボタンが届いたら、その押下は何もせず終える（ダイアログは 1 枚ずつ = 12007。
      実 Office ではコマンドごとに別 runtime なので通常は起きない） */
  function command(name, body) {
    return function (event) {
      var completed = false;
      var cmd = { name: name, expired: false, onExpire: null, timer: null, hard: null };
      function complete() {
        if (completed) return;
        completed = true;
        clearTimeout(cmd.timer);
        clearTimeout(cmd.hard);
        if (running === cmd) running = null;
        try {
          if (event && typeof event.completed === "function") event.completed();
        } catch (e) {
          console.error("OpenRef: event.completed() に失敗", e);
        }
      }
      if (running) {
        console.warn("OpenRef: " + running.name + " の実行中のため " + name + " は実行しません");
        complete();
        return;
      }
      running = cmd;
      cmd.timer = setTimeout(function () {
        cmd.expired = true;
        if (cmd.onExpire) cmd.onExpire();
      }, LIMIT_MS);
      cmd.hard = setTimeout(function () {
        console.error("OpenRef: " + name + " が時間内に終わらないため打ち切ります");
        complete();
      }, HARD_STOP_MS);
      Promise.resolve()
        .then(function () {
          return body(cmd);
        })
        .catch(function (e) {
          return errorNotice(cmd, e);
        })
        .catch(function (e) {
          console.error("OpenRef: エラーの表示に失敗", e);
        })
        .then(complete);
    };
  }

  function labelOf(csl) {
    csl = csl || {};
    var a = (csl.author && csl.author[0] && (csl.author[0].family || csl.author[0].literal)) || "?";
    var y = (csl.issued && csl.issued["date-parts"] && csl.issued["date-parts"][0] && csl.issued["date-parts"][0][0]) || "";
    return a + (y ? " " + y : "");
  }

  // ---------- Add/Edit Citation ----------
  var addEditCitation = command("addEditCitation", async function (cmd) {
    await D.refreshSettings();
    var at = await D.selectionContext();
    if (at.kind === "bib") {
      throw D.userError("文献リストの中には引用を挿入できません。本文にカーソルを置いてください。");
    }
    var mode = at.kind === "cite" ? "edit" : "add";
    var existing = mode === "edit" ? at.items : [];
    // 追加: 選択範囲の末尾に仮置き […]。sync してからダイアログを開く（仮置きが先に見える）。
    // 前回の残骸の中なら: 中身が […] だけなら再利用、文字が打たれていれば文字を残して外し、その直後に新しい仮置き
    // （再利用すると確定時の insertHtml(Replace) で打った文字が消える）。
    // 対象は {tag, id}: コピー＆ペーストで同じ tag の CC が複数ありうるので、確定・削除は id の 1 個に対して行う
    var target;
    if (at.kind === "cite" || (at.kind === "pending" && at.reusable)) target = { tag: at.tag, id: at.id };
    else if (at.kind === "pending") target = await D.insertPlaceholder({ replaceId: at.id });
    else target = await D.insertPlaceholder();
    var tag = target.tag;
    try {
      var eff = await D.effectivePrefs(false);
      await D.ensureStyles(false);
      var d = {
        mode: mode,
        items: existing.map(function (it) {
          return { key: it.key, label: labelOf(it.csl) };
        }),
        style: { id: eff.style, title: D.styleTitle(eff.style) },
      };
      if (at.missing) d.missing = true;
      if (dialogUrl("cite-dialog.html", { d: d }).length > CITE_URL_LIMIT) {
        d.items = d.items.map(function (it) {
          return { key: it.key };
        });
      }
      var r = await runDialog(cmd, "cite-dialog.html", { d: d }, SIZE.cite, function () {
        // 整形エンジン（970KB）を裏で先読み。失敗は確定時の updateAll で改めて出る
        D.loadCiteproc().catch(function (e) {
          console.warn("OpenRef: citeproc の先読みに失敗", e);
        });
      });
      if (r.type === "insert" || r.type === "replace") {
        await D.commitCitation(target, r.keys, existing);
      } else if (r.type === "delete" && mode === "edit") {
        await D.deleteCitation(target);
      } else if (mode === "add") {
        // cancel / × で閉じた / 時間切れ / 不正なメッセージ: 追加なら仮置きを消す。編集は何もしない
        await D.removePlaceholder(tag);
      }
    } catch (e) {
      // 確定前に失敗したら仮置きを残さない（確定後 = refract-cite- に付け替え済みなら見つからず何もしない）
      if (mode === "add") {
        await D.removePlaceholder(tag).catch(function (e2) {
          console.warn("OpenRef: 仮置きを消せませんでした", e2);
        });
      }
      throw e;
    }
  });

  // ---------- Update Citations and Bibliography ----------
  var updateCitations = command("updateCitations", async function (cmd) {
    await D.refreshSettings();
    var r = await D.updateAll();
    // 成功時はダイアログを出さない（Paperpile と同じ）。引用 0 件は Paperpile の "We could not find any Paperpile references" 相当。
    // 引用の CC はあるのに payload が 1 つも無い（別文書から貼り付けた引用だけ）なら、そう言う
    if (!r.citations) await notice(cmd, "error", (await D.summary()).noData ? NO_DATA : NO_CITES);
  });

  // ---------- Settings and Tools ----------
  function settingsCommand(name, tab) {
    return command(name, async function (cmd) {
      await D.refreshSettings();
      var eff = await D.effectivePrefs(true);
      var f = D.fontConf();
      var s = await D.summary();
      var r = await runDialog(
        cmd,
        "settings-dialog.html",
        {
          tab: tab,
          d: { style: eff.style, doiAlways: eff.doiAlways, fontName: f.name, fontSize: f.size, cites: s.cites, pinned: eff.pinned },
        },
        SIZE.settings
      );
      if (r.type !== "apply") return;
      await D.applySettings(r);
      await D.updateAll(); // 引用が無ければ何もしない（{citations: 0}）
    });
  }
  var citationStyle = settingsCommand("citationStyle", "style");
  var documentSettings = settingsCommand("documentSettings", "doc");

  var moveBibliography = command("moveBibliography", async function (cmd) {
    await D.refreshSettings();
    var s = await D.summary();
    if (!s.cites) {
      await notice(cmd, "error", NO_CITES);
      return;
    }
    var at = await D.selectionContext();
    if (at.kind !== "body") {
      throw D.userError("本文の段落にカーソルを置いてください。", "引用・文献リスト・References 見出し・仮置きの中には移動できません。");
    }
    // 選択が文献リストに跨る・旧文書の素の References 段落の中、は moveBibliographyHere が同じエラーにする
    await D.moveBibliographyHere();
  });

  var removeCitationCodes = command("removeCitationCodes", async function (cmd) {
    await D.refreshSettings();
    var s = await D.summary();
    if (!s.cites && !s.noData && !s.bib && !s.head && !s.pending && !s.index) {
      await notice(cmd, "error", NO_CITES);
      return;
    }
    var r = await notice(cmd, "confirm", { title: "引用コードの削除", message: REMOVE_MSG, ok: "削除", danger: true });
    if (r.type !== "ok") return;
    await D.removeCodes();
  });

  var showDiagnostics = command("showDiagnostics", async function (cmd) {
    await D.refreshSettings().catch(function (e) {
      console.warn("OpenRef: 文書設定を読み直せませんでした", e);
    });
    var g = await D.diagnostics();
    await notice(cmd, "info", { title: "OpenRef 診断情報", message: diagText(g), copy: true });
  });

  function diagText(g) {
    var L = [];
    L.push("OpenRef Word アドイン " + g.version);
    var o = g.office || {};
    L.push(
      "Office: " +
        (o.error
          ? "取得できません（" + o.error + "）"
          : [o.host, o.platform, o.version]
              .map(function (v) {
                return v || "?";
              })
              .join(" / "))
    );
    // "WordApi 1.3 ✓ 1.4 ✓ 1.5 ✗ · DialogApi 1.1 ✓ 1.2 ✓"
    var groups = [];
    (g.sets || []).forEach(function (s) {
      var last = groups[groups.length - 1];
      var mark = s.version + (s.ok ? " ✓" : " ✗");
      if (last && last.name === s.name) last.marks.push(mark);
      else groups.push({ name: s.name, marks: [mark] });
    });
    L.push(
      "API: " +
        groups
          .map(function (x) {
            return x.name + " " + x.marks.join(" ");
          })
          .join(" · ")
    );
    var c = g.connectd || {};
    L.push(
      c.ok
        ? "connectd: 接続 OK（v" + c.version + "・蔵書 " + c.items + " 件・DB v" + c.db_version + "）"
        : "connectd: 接続できません（" + (c.error || "不明") + "）"
    );
    var d = g.doc || {};
    if (d.error) {
      L.push("文書: 読み取れません（" + d.error + "）");
    } else {
      L.push("スタイル: " + d.styleTitle + "（" + d.style + "・" + (d.pinned ? "文書に保存済み" : "未保存 = 次の挿入で固定") + "）");
      L.push("DOI / URL を常に含める: " + (d.doiAlways ? "オン" : "オフ") + " · フォント: " + d.font.name + " " + d.font.size + "pt");
      L.push("引用: " + d.cites + " 箇所" + (d.noData ? "（データなし " + d.noData + "）" : "") + " · 文献: " + d.items + " 本");
      L.push(
        "文献リスト: " + (d.bib ? "あり" : "なし") + " · 見出し: " + (d.head ? "あり" : "なし") +
          " · 仮置き: " + d.pending + " · 台帳: " + d.index
      );
    }
    return L.join("\n");
  }

  // ---------- 登録 ----------
  var COMMANDS = {
    addEditCitation: addEditCitation,
    updateCitations: updateCitations,
    citationStyle: citationStyle,
    documentSettings: documentSettings,
    moveBibliography: moveBibliography,
    removeCitationCodes: removeCitationCodes,
    showDiagnostics: showDiagnostics,
  };
  function register() {
    if (root.Office && Office.actions && typeof Office.actions.associate === "function") {
      Object.keys(COMMANDS).forEach(function (name) {
        Office.actions.associate(name, COMMANDS[name]);
      });
    }
  }
  // 互換: manifest の FunctionName をグローバル関数として探す Office 向けに window にも生やす
  Object.keys(COMMANDS).forEach(function (name) {
    root[name] = COMMANDS[name];
  });
  register();
  if (root.Office && typeof Office.onReady === "function") Office.onReady(register);

  root.OpenRefCommands = {
    COMMANDS: COMMANDS,
    LIMIT_MS: LIMIT_MS,
    HARD_STOP_MS: HARD_STOP_MS,
    RETRY_MS: RETRY_MS,
    dialogUrl: dialogUrl,
    diagText: diagText,
  };
})(typeof self !== "undefined" ? self : globalThis);
