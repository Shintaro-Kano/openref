/* OpenRef cite-core — citeproc-js ラッパ（純ロジック・Office 非依存・node でテスト可能）。
   引用クラスタの整形と bibliography 生成のみを担う。文書操作は呼び手（Word = VBA の ORDoc・app = cite.ts）。 */
(function (root) {
  "use strict";

  /** locales: en-US の XML 文字列（従来形）または { "en-US": xml, "ja-JP": xml, … }。
      無い言語は en-US の語彙で代用する（requestedLocales で事前に取り揃えるのが本筋） */
  function CiteCore(CSL, styleXml, locales) {
    this.CSL = CSL;
    this.styleXml = styleXml;
    this.locales = typeof locales === "string" ? { "en-US": locales } : locales || {};
    this.itemsById = {};
    this.engine = null;
  }

  /** itemsById: { key: CSL-JSON item（id フィールドは自動付与） } */
  CiteCore.prototype.setItems = function (itemsById) {
    var withIds = {};
    Object.keys(itemsById).forEach(function (k) {
      var it = itemsById[k];
      var copy = {};
      Object.keys(it).forEach(function (f) {
        copy[f] = it[f];
      });
      copy.id = k;
      withIds[k] = copy;
    });
    this.itemsById = withIds;
    this.engine = null;
  };

  CiteCore.prototype.ensure = function () {
    if (!this.engine) {
      var self = this;
      var sys = {
        retrieveLocale: function (lang) {
          return self.locales[lang] || self.locales["en-US"];
        },
        retrieveItem: function (id) {
          return self.itemsById[id];
        },
      };
      this.engine = new this.CSL.Engine(sys, this.styleXml);
    }
    return this.engine;
  };

  /** 文書内の初出順 id 配列を登録（numeric スタイルの番号はこの順で決まる） */
  CiteCore.prototype.register = function (orderedIds) {
    this.ensure().updateItems(orderedIds);
  };

  /** 1 クラスタ（[K1,K2] など）の整形済み文字列（HTML 断片） */
  CiteCore.prototype.clusterHtml = function (ids) {
    return this.ensure().makeCitationCluster(
      ids.map(function (id) {
        return { id: id };
      })
    );
  };

  /** bibliography 全体の HTML。opts.doiAlways = DOI / URL を常に含める（withIdentifiers） */
  CiteCore.prototype.bibliographyHtml = function (opts) {
    var res = this.ensure().makeBibliography();
    if (!res || !res[1]) return "";
    var entries = res[1];
    if (opts && opts.doiAlways) entries = withIdentifiers(entries, res[0].entry_ids, this.itemsById, this.styleXml);
    return entries.join("");
  };

  // ---- 「DOI / URL を常に含める」（Paperpile の Always include DOI and URL） ----

  function decodeXml(s) {
    return s
      .replace(/&#(\d+);/g, function (_, n) {
        return String.fromCharCode(+n);
      })
      .replace(/&#x([0-9a-f]+);/gi, function (_, n) {
        return String.fromCharCode(parseInt(n, 16));
      })
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&quot;/g, '"')
      .replace(/&apos;/g, "'")
      .replace(/&nbsp;/g, "\u00a0")
      .replace(/&amp;/g, "&");
  }

  function escapeHtml(s) {
    return s.replace(/&/g, "&#38;").replace(/</g, "&#60;").replace(/>/g, "&#62;");
  }

  /** スタイル自身の DOI の書き方（最初の <text variable="DOI" prefix="…"> の prefix。前の区切り ". " 等は除く）。
      無ければ https://doi.org/ */
  function doiPrefix(styleXml) {
    var el = /<text\b[^>]*\bvariable="DOI"[^>]*>/.exec(styleXml || "");
    var m = el && /\bprefix="([^"]*)"/.exec(el[0]);
    var p = m ? decodeXml(m[1]).replace(/^[\s.,;:]+/, "") : "";
    return p || "https://doi.org/";
  }

  /** citeproc の CSL.Util.encodeDoiForUrl と同じ（prefix が https://doi.org/ のとき DOI を URL 用に符号化して出す） */
  function encodeDoiForUrl(doi) {
    return doi.replace(/[\u0000- "#%<>?[\\\]^`{|}\u007F-\u009F]/g, encodeURIComponent);
  }

  /** text に v が出ているか（大小無視）。citeproc は ' を ’ に変え、doi.org 形の DOI は符号化して出すので、その形でも探す */
  function printed(text, v) {
    var norm = function (s) {
      return s.toLowerCase().replace(/[‘’]/g, "'");
    };
    var t = norm(text);
    return t.indexOf(norm(v)) >= 0 || t.indexOf(norm(encodeDoiForUrl(v))) >= 0;
  }

  /** bibliography の各 entry（citeproc の HTML）に、その item の DOI（無ければ URL）が本文に出ていなければ末尾へ足す。
      entryIds = makeBibliography()[0].entry_ids（entries と同順）。識別子は item にあるものだけ使う（作らない）。
      DOI / URL の無い item・既に出ている item は触らない */
  function withIdentifiers(entries, entryIds, itemsById, styleXml) {
    var prefix = doiPrefix(styleXml);
    return entries.map(function (html, i) {
      var id = entryIds && entryIds[i] && entryIds[i][0];
      var it = id != null ? itemsById[id] : null;
      if (!it) return html;
      // URL 形・doi: 付きで入っている DOI は素の DOI に（編集画面のプレビューは保存前の入力を整形するため）
      var doi = typeof it.DOI === "string" ? it.DOI.trim().replace(/^(https?:\/\/(dx\.)?doi\.org\/|doi:\s*)/i, "") : "";
      var url = typeof it.URL === "string" ? it.URL.trim() : "";
      var text = decodeXml(html.replace(/<[^>]*>/g, ""));
      var add;
      if (doi) {
        if (printed(text, doi)) return html;
        add = prefix + (/doi\.org\/$/i.test(prefix) ? encodeDoiForUrl(doi) : doi);
      } else if (url) {
        if (printed(text, url)) return html;
        add = url;
      } else {
        return html;
      }
      var body = text.replace(/\s+$/, "");
      var sep = /[.!?。]$/.test(body) ? " " : body ? ". " : "";
      // 番号付きスタイルは csl-entry の中に csl-left-margin + csl-right-inline。本文側（内側の最後の div）の末尾へ入れる
      var at = html.lastIndexOf("</div>");
      if (at < 0) return html + escapeHtml(sep + add);
      var inner = /<\/div>\s*$/.exec(html.slice(0, at));
      if (inner) at = inner.index;
      return html.slice(0, at) + escapeHtml(sep + add) + html.slice(at);
    });
  }

  /** このスタイルで citeproc が要求する locale 一覧（style の default-locale 由来。例: ["de-DE"]）。
      citeproc の retrieveLocale は同期なので、取得してから CiteCore を作るために先に聞いておく */
  function requestedLocales(CSL, styleXml, enUSXml) {
    var seen = [];
    new CSL.Engine(
      {
        retrieveLocale: function (lang) {
          if (seen.indexOf(lang) < 0) seen.push(lang);
          return enUSXml;
        },
        retrieveItem: function () {
          return null;
        },
      },
      styleXml
    );
    return seen;
  }

  root.OpenRefCite = {
    CiteCore: CiteCore,
    requestedLocales: requestedLocales,
    withIdentifiers: withIdentifiers,
    doiPrefix: doiPrefix,
  };
})(typeof self !== "undefined" ? self : globalThis);
