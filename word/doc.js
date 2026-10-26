/* OpenRef Word アドイン — Office 層（DOM 非依存。関数コマンド commands.js と案内ページ taskpane.html が共用）。
   引用 = inline content control（tag: refract-cite-<8桁>・title "OpenRef citation"。改名前の文書は "Refract citation" = title は照合に使わない）
   + document settings の同名キーに {items:[{key,csl}]}。
   文献リスト = CC tag refract-bib / 見出し = CC tag refract-bib-heading。Add/Edit の仮置き = CC tag refract-pending-<8桁>。
   整形は cite-core.js（citeproc。970KB なので必要時に動的読込）。文書に CSL snapshot を持つため OpenRef 非導入環境でも文書は壊れない。
   使う Word API は WordApi 1.3 以内（manifest の Requirements）。 */
/* global Office, Word */
(function (root) {
  "use strict";

  var VERSION = "1.2.0";
  var DEFAULT_FONT = "Century";
  var DEFAULT_SIZE = 10.5;
  // 文書に保存される識別子（CC の tag・document settings のキー）は文書互換のため据え置き（改名前の原稿の引用を認識するため）
  var TAG_PREFIX = "refract-";
  var CITE = "refract-cite-";
  var PENDING = "refract-pending-";
  var BIB = "refract-bib";
  var HEAD = "refract-bib-heading";
  /** 挿入した引用 tag の台帳。Office の settings にはキー列挙 API が無いので、削除済み引用の payload（孤児）を
      Remove Citation Codes で確実に消すために持つ。Update 時の自動掃除はしない（カット＆ペースト途中の引用を壊すため） */
  var INDEX = "refract-index";
  var S_STYLE = "refract-style";
  var S_DOI_ALWAYS = "refract-doi-always";
  var S_FONT_NAME = "refract-font-name";
  var S_FONT_SIZE = "refract-font-size";
  var PLACEHOLDER = "[…]";

  var cache = { styles: null, appStyle: null, appDoiAlways: false, stylesError: null };

  /** 利用者に見せる前提のエラー（detail = 補足の 1 行） */
  function userError(message, detail) {
    var e = new Error(message);
    e.user = true;
    if (detail) e.detail = detail;
    return e;
  }

  function fetchJson(url, opts) {
    return fetch(url, opts).then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status + " " + url);
      return r.json();
    });
  }
  function fetchText(url) {
    return fetch(url).then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status + " " + url);
      return r.text();
    });
  }

  /** 引用 tag の 8 桁（英小文字 + 数字） */
  function newId() {
    var s = "";
    while (s.length < 8) s += "abcdefghijklmnopqrstuvwxyz0123456789".charAt(Math.floor(Math.random() * 36));
    return s;
  }

  /** 引用スタイル id（core::citestyle::valid_id と同じ規則） */
  function validStyleId(id) {
    return typeof id === "string" && /^[a-z0-9][a-z0-9-]{0,199}$/.test(id);
  }

  // ---------- settings ----------
  function settings() {
    return Office.context.document.settings;
  }
  function getSetting(key) {
    return settings().get(key);
  }
  function setSetting(key, value) {
    settings().set(key, value);
  }
  function removeSetting(key) {
    settings().remove(key);
  }
  function settingsCall(method, what) {
    return new Promise(function (resolve, reject) {
      settings()[method](function (r) {
        if (r.status === Office.AsyncResultStatus.Succeeded) resolve();
        else reject(new Error("文書設定の" + what + "に失敗: " + (r.error && r.error.message)));
      });
    });
  }
  function saveSettings() {
    return settingsCall("saveAsync", "保存");
  }
  /** 文書に保存済みの設定を読み直す。ダイアログを開いている間に別の runtime（taskpane・別のコマンド）が書いた値を
      取りこぼさないよう、各コマンドの頭と確定の直前に呼ぶ。
      refreshAsync の無い環境では読み直さずに続ける（メモリ上の値 = この runtime の起動時に読んだ値） */
  function refreshSettings() {
    if (typeof settings().refreshAsync !== "function") return Promise.resolve();
    return settingsCall("refreshAsync", "読み込み");
  }
  function hasPayload(tag) {
    var p = getSetting(tag);
    return !!(p && p.items && p.items.length);
  }
  function getIndex() {
    var v = getSetting(INDEX);
    return Array.isArray(v) ? v.slice() : [];
  }
  function addToIndex(tag) {
    var idx = getIndex();
    if (idx.indexOf(tag) < 0) {
      idx.push(tag);
      setSetting(INDEX, idx);
    }
  }
  function dropFromIndex(tag) {
    var idx = getIndex();
    var i = idx.indexOf(tag);
    if (i >= 0) {
      idx.splice(i, 1);
      setSetting(INDEX, idx);
    }
  }

  /** 文書ごとの設定（無い = この機能以前の文書で、従来の既定 Vancouver で整形済み）。
      新規文書には最初の引用挿入時に OpenRef app の既定スタイルを固定する（commitCitation） */
  function currentStyle() {
    return getSetting(S_STYLE) || "vancouver";
  }
  /** 文献リストに DOI（無ければ URL）を常に含めるか（Paperpile の Always include DOI and URL）。
      無い = この機能以前の文書で、付け足し無しで整形済み */
  function currentDoiAlways() {
    return getSetting(S_DOI_ALWAYS) === true;
  }
  function isPinned() {
    return !!getSetting(S_STYLE) && typeof getSetting(S_DOI_ALWAYS) === "boolean";
  }
  /** スタイル一覧と app の既定を取る（未接続なら同梱 Vancouver で続行）。fresh = 取り直す */
  async function ensureStyles(fresh) {
    if (cache.styles && !fresh) return cache;
    try {
      var r = await fetchJson("/api/styles");
      cache.styles = r.styles || [];
      cache.appStyle = r.current || null;
      cache.appDoiAlways = r.doi_always === true;
      cache.stylesError = null;
    } catch (e) {
      cache.stylesError = e.message; /* 未接続: 文書設定 or vancouver */
    }
    return cache;
  }
  function styleTitle(id) {
    var s = (cache.styles || []).filter(function (x) {
      return x.id === id;
    })[0];
    return s ? s.title : id;
  }

  /** 文書内の OpenRef の content control を tag で分類（payload は settings 側） */
  async function scan() {
    return Word.run(async function (ctx) {
      var all = ctx.document.contentControls;
      all.load("items/tag");
      await ctx.sync();
      var out = { cites: [], pending: [], bib: 0, head: 0 };
      all.items.forEach(function (cc) {
        var t = cc.tag || "";
        if (t.indexOf(CITE) === 0) out.cites.push(t);
        else if (t.indexOf(PENDING) === 0) out.pending.push(t);
        else if (t === BIB) out.bib++;
        else if (t === HEAD) out.head++;
      });
      return out;
    });
  }

  /** スタイル・DOI / URL が未保存の文書で使う値: 引用が既にあれば従来既定（Vancouver・付け足し無し）で整形済みなのでそれ、
      引用がまだ無い新規文書なら OpenRef app の既定。仮置き（refract-pending-*）は引用に数えない。fresh = app の既定を取り直す */
  async function effectivePrefs(fresh) {
    var style = getSetting(S_STYLE);
    var doi = getSetting(S_DOI_ALWAYS);
    if (style && typeof doi === "boolean") return { style: style, doiAlways: doi, pinned: true };
    await ensureStyles(fresh);
    var hadCites = (await scan()).cites.length > 0;
    return {
      style: style || (hadCites ? "vancouver" : cache.appStyle || "vancouver"),
      doiAlways: typeof doi === "boolean" ? doi : hadCites ? false : cache.appDoiAlways,
      pinned: false,
    };
  }

  /** 引用・文献の数（診断・コマンドの前提確認用）。cites = payload のある引用 */
  async function summary() {
    var s = await scan();
    var withData = 0;
    var keys = [];
    s.cites.forEach(function (t) {
      var p = getSetting(t);
      if (p && p.items && p.items.length) {
        withData++;
        p.items.forEach(function (it) {
          if (keys.indexOf(it.key) < 0) keys.push(it.key);
        });
      }
    });
    return {
      cites: withData,
      noData: s.cites.length - withData,
      items: keys.length,
      bib: s.bib,
      head: s.head,
      pending: s.pending.length,
      index: getIndex().length,
    };
  }

  // ---------- 整形の準備 ----------
  var citeprocP = null;
  /** citeproc.js（970KB）は関数ファイルで同期読込しない（ダイアログが開くまでの遅延になる）→ 必要時に <script> で読む */
  function loadCiteproc() {
    if (typeof root.CSL !== "undefined") return Promise.resolve();
    if (!citeprocP) {
      citeprocP = new Promise(function (resolve, reject) {
        var s = document.createElement("script");
        s.src = "/word/citeproc.js";
        s.onload = function () {
          if (typeof root.CSL !== "undefined") resolve();
          else reject(new Error("citeproc.js を読み込みましたが CSL が定義されていません"));
        };
        s.onerror = function () {
          reject(new Error("citeproc.js を読み込めません（/word/citeproc.js）"));
        };
        document.head.appendChild(s);
      }).catch(function (e) {
        citeprocP = null; // 次の呼び出しで読み直す
        throw e;
      });
    }
    return citeprocP;
  }

  /** スタイルが要求する locale を取り揃える（en-US 以外は connectd が取得・キャッシュ。取れなければ en-US で代用） */
  async function loadLocales(styleXml) {
    var en = await fetchText("/word/locales/en-US.xml");
    var map = { "en-US": en };
    var langs = root.OpenRefCite.requestedLocales(root.CSL, styleXml, en);
    for (var i = 0; i < langs.length; i++) {
      if (map[langs[i]]) continue;
      try {
        map[langs[i]] = await fetchText("/word/locales/" + encodeURIComponent(langs[i]) + ".xml");
      } catch (e) {
        /* 収録外の言語 */
      }
    }
    return map;
  }
  /** 引用・文献リストに適用する書式（既定 = 英文論文向け Century 10.5pt） */
  function fontConf() {
    var size = parseFloat(getSetting(S_FONT_SIZE));
    return {
      name: getSetting(S_FONT_NAME) || DEFAULT_FONT,
      size: isFinite(size) && size > 0 ? size : DEFAULT_SIZE,
    };
  }
  /** insertHtml は独自書式を持ち込むので、挿入 → sync の後に必ず上書きする */
  function applyFont(target, f, opts) {
    target.font.name = f.name;
    target.font.size = f.size;
    target.font.color = "#000000";
    if (opts && opts.bold !== undefined) target.font.bold = opts.bold;
  }

  // ---------- 引用の入れ子 ----------
  var NESTED_MSG = "引用の中に別の引用が入っています（入れ子）。中の引用を削除し、Add/Edit で付け直してから Update してください。";
  function isOpenRefCC(p) {
    return !p.isNullObject && (p.tag || "").indexOf(TAG_PREFIX) === 0;
  }
  /** 引用 CC（と extra の CC）のうち、OpenRef の CC（引用・仮置き・文献リスト・見出し）の中にある最初の 1 個を返す
      （無ければ null）。外側に insertHtml(Replace) すると中の引用が消えるので、見つけたら文書に触れる前に
      nestedError で止める。items = load 済みの CC */
  async function hasNestedCites(ctx, items, extra) {
    var pairs = [];
    items.forEach(function (cc) {
      if (cc.tag && cc.tag.indexOf(CITE) === 0) pairs.push({ cc: cc, parent: cc.parentContentControlOrNullObject });
    });
    (extra || []).forEach(function (cc) {
      pairs.push({ cc: cc, parent: cc.parentContentControlOrNullObject });
    });
    if (!pairs.length) return null;
    pairs.forEach(function (x) {
      x.parent.load("tag");
    });
    await ctx.sync();
    for (var i = 0; i < pairs.length; i++) {
      if (isOpenRefCC(pairs[i].parent)) return pairs[i].cc;
    }
    return null;
  }
  /** 入れ子のエラー。内側の CC を選択して場所を示す（Word がそこへスクロールする）。選択できなければ文言だけ */
  async function nestedError(ctx, inner) {
    try {
      inner.select();
      await ctx.sync();
      return userError(NESTED_MSG + "（入れ子の引用を選択しました）");
    } catch (e) {
      console.warn("OpenRef: 入れ子の引用を選択できませんでした", e);
      return userError(NESTED_MSG);
    }
  }

  // ---------- 引用の一括更新 ----------
  /** 文書内の全引用を初出順に番号付けし直し、文献リスト（無ければ見出しごと作る）を再生成する。
      戻り値 {citations, items, style, doiAlways, font, offline}。引用 0 件 → {citations: 0}（文書に触れない）。
      仮置き（refract-pending-*）には触れない。引用の入れ子があれば文書に触れずにエラー */
  async function updateAll() {
    var styleId = currentStyle();
    var doiAlways = currentDoiAlways();
    var f = fontConf();
    var result = { citations: 0 };

    await Word.run(async function (ctx) {
      var ccs = ctx.document.contentControls;
      ccs.load("items/tag");
      await ctx.sync();
      var inner = await hasNestedCites(ctx, ccs.items);
      if (inner) throw await nestedError(ctx, inner);

      var cites = [];
      var bib = null;
      var head = null;
      ccs.items.forEach(function (cc) {
        if (cc.tag === BIB) bib = cc;
        else if (cc.tag === HEAD) head = cc;
        else if (cc.tag && cc.tag.indexOf(CITE) === 0) {
          var payload = getSetting(cc.tag);
          if (payload && payload.items && payload.items.length) {
            cites.push({ cc: cc, payload: payload });
          }
        }
      });
      if (!cites.length) return;

      // 文書順の初出順で番号決定
      var ordered = [];
      cites.forEach(function (c) {
        c.payload.items.forEach(function (it) {
          if (ordered.indexOf(it.key) < 0) ordered.push(it.key);
        });
      });

      // 文書に触れる前に整形の材料を揃える（スタイルが取れなければここで失敗 = 文書は無傷）
      await loadCiteproc();
      var styleXml = await fetchText("/word/styles/" + encodeURIComponent(styleId) + ".csl");
      var localeXml = await loadLocales(styleXml);

      // 蔵書から最新書誌を取得（不可なら文書内 snapshot で描画 = オフライン耐性）
      var fresh = {};
      var offline = false;
      try {
        fresh = await fetchJson("/api/items", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ keys: ordered }),
        });
      } catch (e) {
        offline = true;
      }
      var itemsById = {};
      cites.forEach(function (c) {
        c.payload.items.forEach(function (it) {
          itemsById[it.key] = fresh[it.key] || it.csl;
        });
      });

      var core = new root.OpenRefCite.CiteCore(root.CSL, styleXml, localeXml);
      core.setItems(itemsById);
      core.register(ordered);

      // ---- 引用クラスタ ----
      cites.forEach(function (c) {
        var html = core.clusterHtml(
          c.payload.items.map(function (it) {
            return it.key;
          })
        );
        c.cc.insertHtml(html, "Replace");
      });
      await ctx.sync();
      cites.forEach(function (c) {
        applyFont(c.cc, f);
      });

      // ---- 見出し（既存文書に素の "References" があれば取り込む） ----
      if (!head) {
        if (bib) {
          var paras = ctx.document.body.paragraphs;
          paras.load("items/text");
          await ctx.sync();
          var legacy = null;
          paras.items.forEach(function (p) {
            if ((p.text || "").trim().toLowerCase() === "references") legacy = p;
          });
          if (legacy) {
            head = legacy.insertContentControl();
          } else {
            head = bib.paragraphs.getFirst().insertParagraph("References", "Before").insertContentControl();
          }
        } else {
          head = ctx.document.body.insertParagraph("References", "End").insertContentControl();
        }
        head.tag = HEAD;
        head.title = "References heading (OpenRef)";
        await ctx.sync();
      }

      // ---- 文献リスト本体 ----
      if (!bib) {
        var para = ctx.document.body.insertParagraph("", "End");
        bib = para.insertContentControl();
        bib.tag = BIB;
        bib.title = "References (OpenRef)";
        await ctx.sync();
      }
      bib.insertHtml(core.bibliographyHtml({ doiAlways: doiAlways }) || "<p>(no items)</p>", "Replace");
      await ctx.sync();

      applyFont(bib, f);
      applyFont(head, f, { bold: true });
      await ctx.sync();

      result = {
        citations: cites.length,
        items: ordered.length,
        style: styleId,
        doiAlways: doiAlways,
        font: f,
        offline: offline,
      };
    });
    return result;
  }

  // ---------- Add/Edit Citation ----------
  /** カーソル位置の文脈: cite（引用の中。items = 既存の [{key,csl}]・missing = payload なし）/ bib（文献リスト・見出し）/
      pending（前回の仮置きの残骸）/ body（それ以外）。id = その CC の id（コピー＆ペーストで同じ tag の CC が
      複数ありうるので、編集・削除・カーソル移動は tag でなく id で 1 個に絞る） */
  async function selectionContext() {
    var p = await Word.run(async function (ctx) {
      var cc = ctx.document.getSelection().parentContentControlOrNullObject;
      cc.load("tag,id,text");
      await ctx.sync();
      return cc.isNullObject ? { tag: "", id: null, text: "" } : { tag: cc.tag || "", id: cc.id, text: cc.text };
    });
    var tag = p.tag;
    if (tag.indexOf(CITE) === 0) {
      var payload = getSetting(tag);
      var items = payload && payload.items ? payload.items : [];
      return { kind: "cite", tag: tag, id: p.id, items: items, missing: !items.length };
    }
    if (tag === BIB || tag === HEAD) return { kind: "bib", tag: tag, id: p.id };
    // 仮置きの残骸: 中身が […] だけなら再利用できる（打った文字が入っていると確定時の insertHtml(Replace) で消える）
    if (tag.indexOf(PENDING) === 0) return { kind: "pending", tag: tag, id: p.id, reusable: p.text === PLACEHOLDER };
    return { kind: "body", tag: tag || null, id: p.id };
  }

  /** 仮置きに利用者が打った文字があるとき: 中の最初の […] を消してから（search・大文字小文字を区別）CC だけ外す
      （文字は残す）。search が使えない・見つからないときは CC を外すだけ（[…] も残る）。
      キャンセル（removePlaceholder）・Remove Citation Codes・残骸を再利用できないとき（insertPlaceholder）で使う */
  async function unwrapKeepingText(ctx, cc) {
    try {
      var hits = cc.search(PLACEHOLDER, { matchCase: true });
      hits.load("items");
      await ctx.sync();
      if (hits.items.length) hits.items[0].delete();
    } catch (e) {
      console.warn("OpenRef: 仮置きの […] を探せませんでした（CC だけ外します）", e);
    }
    cc.delete(true);
  }

  var BOUNDARY_MSG = "直前の引用の境界には挿入できません。引用の後ろに空白などを 1 文字入れてから Add/Edit を押してください。";
  function placeholderAt(range, tag) {
    var cc = range.insertContentControl();
    cc.tag = tag;
    cc.title = "OpenRef citation (pending)";
    cc.insertText(PLACEHOLDER, "Replace");
    return cc;
  }

  /** 仮置き […] を入れる。先に空の CC を作ってから中に […] を入れる（CC 化に失敗しても素の […] が本文に残らない順）。
      位置は選択範囲の末尾（選択文字列は消さない）。opts.replaceId = 文字の打たれた残骸の
      仮置き → その直後に作り、残骸は […] を消して CC を外す（打った文字は残す）。
      直前の引用の境界で押すと新しい CC がその引用の中に入ることがある（次の insertHtml(Replace) で消える）→
      親が OpenRef の CC なら作り直して親の直後へ。作り直してもまだ中なら仮置きを消してエラー（文書は元のまま）。
      最後にカーソルを仮置きの直後へ移す（ダイアログは非モーダル = 開いている間に打った文字が仮置きに入らないように）。
      戻り値 {tag, id} */
  async function insertPlaceholder(opts) {
    var tag = PENDING + newId();
    var replaceId = opts && opts.replaceId != null ? opts.replaceId : null;
    var id = await Word.run(async function (ctx) {
      var old = replaceId != null ? ctx.document.contentControls.getById(replaceId) : null;
      var cc = placeholderAt(old ? old.getRange("After") : ctx.document.getSelection().getRange("End"), tag);
      var parent = cc.parentContentControlOrNullObject;
      parent.load("tag");
      await ctx.sync();
      if (isOpenRefCC(parent)) {
        cc.delete(false);
        cc = placeholderAt(parent.getRange("After"), tag);
        var again = cc.parentContentControlOrNullObject;
        again.load("tag");
        await ctx.sync();
        if (isOpenRefCC(again)) {
          cc.delete(false);
          await ctx.sync();
          throw userError(BOUNDARY_MSG);
        }
      }
      if (old) await unwrapKeepingText(ctx, old);
      cc.load("id");
      cc.getRange("After").select();
      await ctx.sync();
      return cc.id;
    });
    return { tag: tag, id: id };
  }

  /** 仮置きを消す（Cancel・× で閉じた・時間切れ・確定の失敗）。中身が […] だけなら CC ごと、
      利用者が打った文字が入っていれば […] だけ消して CC を外す（文字は残す） */
  async function removePlaceholder(tag) {
    return Word.run(async function (ctx) {
      var list = ctx.document.contentControls.getByTag(tag);
      list.load("items/text");
      await ctx.sync();
      for (var i = 0; i < list.items.length; i++) {
        var cc = list.items[i];
        if (cc.text === PLACEHOLDER) cc.delete(false);
        else await unwrapKeepingText(ctx, cc);
      }
      await ctx.sync();
      return list.items.length;
    });
  }

  function notFound(isNew) {
    return isNew
      ? userError("挿入位置の仮置き […] が文書から消えています。", "もう一度 Add/Edit Citation を押してください。")
      : userError("編集中の引用が文書から見つかりません。", "ダイアログを開いている間に削除された可能性があります（文書は変更していません）。");
  }

  /** keys を引用として確定する: 書誌取得（蔵書から消えた key は existing の snapshot）→ 対象の CC（id で 1 個）が
      まだあるか確かめる → payload 保存・未固定文書なら style / DOI を固定・台帳に追記 → tag の付け替え →
      updateAll → カーソルを引用の直後へ。
      target = {tag, id}。tag = refract-pending-*（追加: 同じ 8 桁の refract-cite- へ付け替え）または refract-cite-*（編集）。
      編集で同じ tag の CC がほかにもある（コピー＆ペーストの複製）なら、編集した CC だけ新しい tag に分ける（fork）=
      ほかの複製は旧 tag・旧 payload のまま。戻り値 {tag, id, result} */
  async function commitCitation(target, keys, existing) {
    var tag = target.tag;
    var id = target.id;
    var uniq = [];
    (keys || []).forEach(function (k) {
      if (typeof k === "string" && k && uniq.indexOf(k) < 0) uniq.push(k);
    });
    if (!uniq.length) throw userError("引用する文献が選ばれていません。");

    var fresh = {};
    var fetchErr = null;
    try {
      fresh = await fetchJson("/api/items", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ keys: uniq }),
      });
    } catch (e) {
      fetchErr = e;
    }
    var snap = {};
    (existing || []).forEach(function (it) {
      if (it && it.key && it.csl) snap[it.key] = it.csl;
    });
    var items = [];
    var missing = [];
    uniq.forEach(function (k) {
      var csl = fresh[k] || snap[k];
      if (csl) items.push({ key: k, csl: csl });
      else missing.push(k);
    });
    if (missing.length) {
      if (fetchErr) throw userError("OpenRef（connectd）から書誌を取得できません。", fetchErr.message);
      throw userError("蔵書に見つからない文献があります: " + missing.join(", "), "ゴミ箱に移した・削除した文献は引用できません。");
    }

    // ダイアログを開いている間に文書が変わりうるので読み直してから書く
    await refreshSettings();
    // 未固定文書の固定値は引用を確定する前に決める（仮置きは引用に数えない → 引用の無い新規文書は app の既定）
    var pin = isPinned() ? null : await effectivePrefs(true);
    var isNew = tag.indexOf(PENDING) === 0;
    // 対象の CC（id）がまだあるか・同じ tag の複製がいくつあるか・引用の入れ子が無いか。
    // どれかに当たれば文書も設定も変えずにエラー（payload の無い引用も、引用の無い payload も残さない）
    var dupes = 0;
    await Word.run(async function (ctx) {
      var cc = ctx.document.contentControls.getByIdOrNullObject(id);
      cc.load("tag");
      var same = ctx.document.contentControls.getByTag(tag);
      same.load("items/id");
      var all = ctx.document.contentControls;
      all.load("items/tag");
      await ctx.sync();
      if (cc.isNullObject || cc.tag !== tag) throw notFound(isNew);
      dupes = same.items.filter(function (x) {
        return x.id !== id;
      }).length;
      // 確定後の updateAll が入れ子で止まる前に止める（追加なら確定する仮置き自身の位置も見る）
      var inner = await hasNestedCites(ctx, all.items, isNew ? [cc] : []);
      if (inner) throw await nestedError(ctx, inner);
    });
    var citeTag = isNew ? CITE + tag.slice(PENDING.length) : dupes ? CITE + newId() : tag;
    // payload を先に保存し、保存できてから tag を付け替える（逆順だと保存の失敗で payload の無い引用が残る。
    // この順なら失敗しても残るのは台帳付きの孤児 payload だけ = Remove Citation Codes で消える）
    setSetting(citeTag, { items: items });
    if (pin) {
      setSetting(S_STYLE, pin.style);
      setSetting(S_DOI_ALWAYS, pin.doiAlways);
    }
    addToIndex(citeTag);
    await saveSettings();
    if (citeTag !== tag) {
      await Word.run(async function (ctx) {
        var cc = ctx.document.contentControls.getById(id);
        cc.tag = citeTag;
        cc.title = "OpenRef citation";
        await ctx.sync();
      });
    }
    var result = await updateAll();
    await selectAfter(id);
    return { tag: citeTag, id: id, result: result };
  }

  /** カーソルを引用（id）の直後へ（続けて本文を打てるように） */
  async function selectAfter(id) {
    try {
      await Word.run(async function (ctx) {
        var cc = ctx.document.contentControls.getByIdOrNullObject(id);
        cc.load("tag");
        await ctx.sync();
        if (cc.isNullObject) return;
        cc.getRange("After").select();
        await ctx.sync();
      });
    } catch (e) {
      // カーソル移動の失敗は引用の確定を妨げない（ログには残す）
      console.warn("OpenRef: カーソルを引用の後ろへ移せませんでした", e);
    }
  }

  /** 引用を 1 個削除（編集モードで 0 件確定）。target = {tag, id}。消すのは id の CC だけ（コピー＆ペーストで
      同じ tag の複製があれば巻き添えにしない）。同じ tag の CC が残るなら payload と台帳は残す。
      残る「payload のある引用」が 0 なら文献リストと見出しも消す。CC が見つからなければ何も変えずにエラー。
      戻り値 = updateAll の結果 or {citations: 0} */
  async function deleteCitation(target) {
    var tag = target.tag;
    var id = target.id;
    await refreshSettings();
    var remaining = 0;
    var sameLeft = 0;
    await Word.run(async function (ctx) {
      var cc = ctx.document.contentControls.getByIdOrNullObject(id);
      cc.load("tag");
      var ccs = ctx.document.contentControls;
      ccs.load("items/tag,items/id");
      await ctx.sync();
      if (cc.isNullObject || cc.tag !== tag) throw notFound(false);
      var bibs = [];
      ccs.items.forEach(function (x) {
        var t = x.tag || "";
        if (x.id === id) return;
        if (t === tag) sameLeft++;
        if (t.indexOf(CITE) === 0 && hasPayload(t)) remaining++;
        else if (t === BIB || t === HEAD) bibs.push(x);
      });
      cc.delete(false);
      if (!remaining) {
        bibs.forEach(function (x) {
          x.delete(false);
        });
      }
      await ctx.sync();
    });
    if (!sameLeft) {
      removeSetting(tag);
      dropFromIndex(tag);
      await saveSettings();
    }
    return remaining ? updateAll() : { citations: 0 };
  }

  // ---------- Settings and Tools ----------
  /** 設定ダイアログの [適用]: スタイル・DOI / URL・フォントを文書に保存する（以後この文書はこの値で固定）。
      引用の入れ子があると保存後の updateAll が止まるので、保存する前に検査する（入れ子があれば設定も文書も変えない） */
  async function applySettings(r) {
    await refreshSettings();
    await Word.run(async function (ctx) {
      var ccs = ctx.document.contentControls;
      ccs.load("items/tag");
      await ctx.sync();
      var inner = await hasNestedCites(ctx, ccs.items);
      if (inner) throw await nestedError(ctx, inner);
    });
    if (r.style !== undefined && r.style !== null) {
      if (!validStyleId(r.style)) throw userError("スタイル id が不正です: " + r.style);
      setSetting(S_STYLE, r.style);
    }
    if (typeof r.doiAlways === "boolean") setSetting(S_DOI_ALWAYS, r.doiAlways);
    var name = String(r.fontName == null ? "" : r.fontName).trim() || DEFAULT_FONT;
    var size = parseFloat(r.fontSize);
    if (!isFinite(size) || size <= 0) size = DEFAULT_SIZE;
    setSetting(S_FONT_NAME, name);
    setSetting(S_FONT_SIZE, size);
    await saveSettings();
  }

  /** Range.compareLocationWith の結果のうち「重なっている」もの */
  var OVERLAP = ["Equal", "Contains", "ContainsStart", "ContainsEnd", "Inside", "InsideStart", "InsideEnd", "OverlapsBefore", "OverlapsAfter"];

  /** 文献リスト（と見出し）をカーソル段落の直後へ作り直す。中身・書式は続く updateAll で入る。
      新しい見出し・文献リストを先に作って sync し、旧いもの（最初に load した CC と旧文書の素の "References" 段落）は
      その後で消す = 途中で失敗しても文献リストが無くならない順。
      選択の段落が旧い文献リスト・見出しと重なる（跨ぐ・素の References 段落の中）なら何も変えずにエラー。
      引用の入れ子があっても何も変えずにエラー（作り直した後の updateAll が止まり、文献リストが空のまま残るため）。
      呼ぶ前に「引用がある」を確かめること（commands.js） */
  async function moveBibliographyHere() {
    await Word.run(async function (ctx) {
      var ccs = ctx.document.contentControls;
      ccs.load("items/tag");
      var paras = ctx.document.getSelection().paragraphs;
      var first = paras.getFirst();
      var last = paras.getLast();
      await ctx.sync();
      var inner = await hasNestedCites(ctx, ccs.items);
      if (inner) throw await nestedError(ctx, inner);
      var olds = ccs.items.filter(function (cc) {
        return cc.tag === BIB || cc.tag === HEAD;
      });
      var bib0 = olds.filter(function (cc) {
        return cc.tag === BIB;
      })[0];
      var hasHead = olds.some(function (cc) {
        return cc.tag === HEAD;
      });
      // 見出し CC が無い旧文書: 文献リスト直前の素の "References" 段落も旧い見出しとして扱う（残すと見出しが二重になる）
      var legacy = null;
      if (bib0 && !hasHead) {
        var prev = bib0.paragraphs.getFirst().getPreviousOrNullObject();
        prev.load("text");
        await ctx.sync();
        if (!prev.isNullObject && (prev.text || "").trim().toLowerCase() === "references") legacy = prev;
      }
      var span = first.getRange("Whole").expandTo(last.getRange("Whole"));
      var rels = olds.map(function (cc) {
        return span.compareLocationWith(cc.getRange("Whole"));
      });
      if (legacy) rels.push(span.compareLocationWith(legacy.getRange("Whole")));
      await ctx.sync();
      var hit = rels.some(function (r) {
        return OVERLAP.indexOf(r.value) >= 0;
      });
      if (hit) throw userError("本文の段落にカーソルを置いてください。", "引用・文献リスト・References 見出し・仮置きの中には移動できません。");

      var hp = last.insertParagraph("References", "After");
      hp.styleBuiltIn = "Normal";
      var bp = hp.insertParagraph("", "After");
      bp.styleBuiltIn = "Normal";
      var head = hp.insertContentControl();
      head.tag = HEAD;
      head.title = "References heading (OpenRef)";
      var bib = bp.insertContentControl();
      bib.tag = BIB;
      bib.title = "References (OpenRef)";
      await ctx.sync();

      olds.forEach(function (cc) {
        cc.delete(false);
      });
      if (legacy) legacy.delete();
      await ctx.sync();
    });
    return updateAll();
  }

  /** 引用と文献リストを通常のテキストに変換し（CC だけ外す）、引用データ（payload・台帳）を文書から消す。
      仮置きは […] だけなら CC ごと消し、利用者の打った文字が入っていれば […] だけ消して CC を外す。
      戻り値 {cites, bib, pending, removed} */
  async function removeCodes() {
    await refreshSettings();
    var n = { cites: 0, bib: 0, pending: 0, removed: 0 };
    var tags = [];
    await Word.run(async function (ctx) {
      var ccs = ctx.document.contentControls;
      ccs.load("items/tag,items/text");
      await ctx.sync();
      var pend = [];
      ccs.items.forEach(function (cc) {
        var t = cc.tag || "";
        if (t.indexOf(CITE) === 0) {
          if (tags.indexOf(t) < 0) tags.push(t);
          cc.delete(true);
          n.cites++;
        } else if (t === BIB || t === HEAD) {
          cc.delete(true);
          n.bib++;
        } else if (t.indexOf(PENDING) === 0) {
          pend.push(cc);
          n.pending++;
        }
      });
      await ctx.sync();
      for (var i = 0; i < pend.length; i++) {
        if (pend[i].text === PLACEHOLDER) pend[i].delete(false);
        else await unwrapKeepingText(ctx, pend[i]);
      }
      await ctx.sync();
    });
    getIndex().forEach(function (t) {
      if (tags.indexOf(t) < 0) tags.push(t);
    });
    tags.forEach(function (t) {
      if (getSetting(t) !== null && getSetting(t) !== undefined) n.removed++;
      removeSetting(t);
    });
    removeSetting(INDEX);
    await saveSettings();
    return n;
  }

  /** 診断情報（Show Diagnostics・案内ページ）。失敗は各欄に理由として入れる */
  async function diagnostics() {
    var out = { version: VERSION, office: {}, sets: [], connectd: null, doc: null };
    try {
      var dg = (Office.context && Office.context.diagnostics) || {};
      out.office = { host: dg.host, platform: dg.platform, version: dg.version };
    } catch (e) {
      out.office = { error: e.message };
    }
    var req = Office.context && Office.context.requirements;
    [
      ["WordApi", "1.3"],
      ["WordApi", "1.4"],
      ["WordApi", "1.5"],
      ["DialogApi", "1.1"],
      ["DialogApi", "1.2"],
    ].forEach(function (s) {
      var ok = false;
      try {
        ok = !!(req && req.isSetSupported(s[0], s[1]));
      } catch (e) {
        /* 判定不能 = 非対応扱い */
      }
      out.sets.push({ name: s[0], version: s[1], ok: ok });
    });
    try {
      out.connectd = await fetchJson("/api/status");
    } catch (e) {
      out.connectd = { ok: false, error: e.message };
    }
    try {
      var s = await summary();
      var eff = await effectivePrefs(false);
      var f = fontConf();
      out.doc = {
        style: eff.style,
        styleTitle: styleTitle(eff.style),
        pinned: eff.pinned,
        doiAlways: eff.doiAlways,
        font: f,
        cites: s.cites,
        noData: s.noData,
        items: s.items,
        bib: s.bib,
        head: s.head,
        pending: s.pending,
        index: s.index,
      };
    } catch (e) {
      out.doc = { error: e.message };
    }
    return out;
  }

  root.OpenRefDoc = {
    VERSION: VERSION,
    DEFAULT_FONT: DEFAULT_FONT,
    DEFAULT_SIZE: DEFAULT_SIZE,
    userError: userError,
    fetchJson: fetchJson,
    fetchText: fetchText,
    newId: newId,
    validStyleId: validStyleId,
    getSetting: getSetting,
    refreshSettings: refreshSettings,
    saveSettings: saveSettings,
    currentStyle: currentStyle,
    currentDoiAlways: currentDoiAlways,
    isPinned: isPinned,
    ensureStyles: ensureStyles,
    styleTitle: styleTitle,
    effectivePrefs: effectivePrefs,
    summary: summary,
    fontConf: fontConf,
    loadCiteproc: loadCiteproc,
    updateAll: updateAll,
    selectionContext: selectionContext,
    insertPlaceholder: insertPlaceholder,
    removePlaceholder: removePlaceholder,
    commitCitation: commitCitation,
    deleteCitation: deleteCitation,
    applySettings: applySettings,
    moveBibliographyHere: moveBibliographyHere,
    removeCodes: removeCodes,
    diagnostics: diagnostics,
  };
})(typeof self !== "undefined" ? self : globalThis);
