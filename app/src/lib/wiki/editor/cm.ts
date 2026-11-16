// OpenNote のエディタ = CodeMirror 6 の組立て。WikiEditor が編集を開いたときに
// 動的 import する（読み専用の利用者・LIBRARY の起動には読まない = 初期 bundle に入れない）。
// md / html モード・行の折返し・ライト / ダーク（app.css の変数をそのまま使う = 切替に追随）・⌘B / ⌘I / ⌘K / リスト・
// CodeMirror の検索（⌘F・置換）・画像のペースト（onPasteImages へ渡すだけ = 保存は呼び出し側）・「/」の機能メニュー（slash.ts・md だけ）・
// 表の中の Tab / Shift+Tab でセルを移る（table.ts・md だけ = 字下げの indentWithTab より前）・見出しで畳む（foldGutter + foldKeymap = ⌘⌥[ / ⌘⌥]。md の見出しの範囲は lang-markdown の headerIndent）・
// 文献の引用（cite.ts・md だけ = [文献] / ⌘⌥K / 「/」から呼び出し側の窓・DOI / PubMed の URL の貼り付けのメニュー）
// 貼り付けの判定の順: 選択の上の URL（lang-markdown の pasteURLAsLink = md の拡張が先に受ける）→ 画像 → 選択なしの DOI / PubMed の URL → 既定

import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, drawSelection, highlightSpecialChars, keymap, placeholder as cmPlaceholder } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, foldGutter, foldKeymap, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { markdown, markdownKeymap, markdownLanguage } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { gotoLine as cmGotoLine, openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { tags } from "@lezer/highlight";
import type { MarkdownConfig } from "@lezer/markdown";
import { insertAtCaret, insertLink, toggleList, toggleWrap } from "./format";
import { eolOf } from "./eol";
import { slashMenu } from "./slash";
import { citeBlocked, citePaste, citeUrlOf, insertCite, type CitePasteOptions } from "./cite";
import { tableKeymap } from "./table";
import { editorFocus } from "../../focusTargets";

export interface EditorOptions {
  doc: string;
  mode: "md" | "html";
  /** 本文が変わった（打鍵・貼付・取消し）。本文は handle.getText() で読む（毎打鍵で全文を作らない） */
  onChange: () => void;
  /** クリップボードの画像（スクリーンショット等）。挿入は呼び出し側が保存の後に handle.insert で */
  onPasteImages?: (files: File[]) => void;
  /** 「/」メニューの「画像・PDF を埋め込む…」= ファイル選択 → 取込 → caret に `![[…]]`（呼び出し側。無ければ候補に出ない） */
  onPickFiles?: () => void;
  /** 「/」メニューの「文献を引用…」= 呼び出し側の検索の窓（無ければ候補に出ない） */
  onCite?: () => void;
  /** DOI / PubMed の URL を選択なしで貼ったときの照合と登録（無ければ今どおり貼るだけ・md だけ） */
  cite?: CitePasteOptions;
  /** CodeMirror の UI の文言（検索パネル等）。鍵 = CodeMirror の英語の原文 */
  phrases?: Record<string, string>;
  placeholder?: string;
}

export interface EditorHandle {
  getText(): string;
  /** 本文を丸ごと置き換える（外部の内容で読み直す）。取消しの履歴は残さない */
  setText(s: string): void;
  /** caret に挿入（貼付・ドロップの `![[…]]`） */
  insert(s: string): void;
  /** 主の選択（無ければ caret）を文献リンクに。置けない場所なら入れずに理由を返す */
  cite(key: string): string | null;
  /** 今の選択に文献リンクを置けるか（置けなければ理由） */
  citeCheck(): string | null;
  /** 主の選択の文字 */
  selection(): string;
  focus(): void;
  /** n 行目（1 始まり・範囲の外は端へ）の頭へ caret を置き、その行を上端へスクロールする（右ペインの目次の押下） */
  gotoLine(n: number): void;
  openSearch(): void;
  /** 表示の言語が替わった: CodeMirror の UI の文言と placeholder を差し替える（作り直さない = 本文・選択・スクロール・取消しの履歴・焦点を保つ） */
  setLocale(phrases: Record<string, string>, placeholder: string): void;
  destroy(): void;
  readonly view: EditorView;
}

// 体裁 = app.css の変数（--text / --panel / --accent …）。ライト / ダークは変数が切り替わるので theme は 1 つ
const theme = EditorView.theme({
  "&": { height: "100%", color: "var(--text)", backgroundColor: "var(--panel)", fontSize: "13.5px" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: 'ui-monospace, "SF Mono", Menlo, "Hiragino Sans", monospace',
    lineHeight: "1.65",
    overflow: "auto",
  },
  ".cm-content": { padding: "14px 0 40vh", caretColor: "var(--text)" },
  ".cm-line": { padding: "0 clamp(12px, 4cqi, 32px)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--text)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "var(--accent-soft) !important",
  },
  ".cm-placeholder": { color: "var(--dim)" },
  ".cm-specialChar": { color: "var(--danger)" },
  ".cm-panels": { backgroundColor: "var(--bg)", color: "var(--text)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--border)" },
  ".cm-search": { fontSize: "12px", padding: "6px 10px" },
  ".cm-search input, .cm-search button, .cm-search label": { fontSize: "12px" },
  ".cm-textfield": {
    backgroundColor: "var(--panel)", color: "var(--text)", border: "1px solid var(--border)", borderRadius: "5px", padding: "2px 6px",
  },
  ".cm-button": {
    backgroundImage: "none", backgroundColor: "var(--panel)", color: "var(--text)", border: "1px solid var(--border)", borderRadius: "5px",
  },
  ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--star) 35%, transparent)", outline: "1px solid color-mix(in srgb, var(--star) 70%, transparent)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "color-mix(in srgb, var(--accent) 35%, transparent)" },
  ".cm-panel.cm-search [name=close]": { color: "var(--dim)" },
  // 「/」メニュー（autocompletion の tooltip）
  ".cm-tooltip": { border: "1px solid var(--border)", backgroundColor: "var(--panel)", color: "var(--text)", borderRadius: "7px", boxShadow: "0 8px 24px rgba(0, 0, 0, 0.18)" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": { fontFamily: "inherit", fontSize: "12.5px", maxHeight: "18em", minWidth: "14em", padding: "4px 0" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li": { padding: "4px 12px", display: "flex", alignItems: "baseline", gap: "10px" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--accent-soft)", color: "var(--text)" },
  ".cm-completionLabel": { flex: "1" },
  ".cm-completionDetail": { color: "var(--dim)", fontStyle: "normal", marginLeft: "0", fontFamily: 'ui-monospace, "SF Mono", Menlo, monospace', fontSize: "11.5px" },
  ".cm-completionMatchedText": { textDecoration: "none", fontWeight: "600" },
  // 雛形の欄（表の列名など。CodeMirror の既定は #00000022 = ダークで見えない）
  ".cm-snippetField": { backgroundColor: "var(--accent-soft)", borderRadius: "3px" },
  ".cm-snippetFieldPosition": { borderLeft: "1.4px dotted var(--accent)" },
  // 畳む印（左の溝）。開いている印は溝に乗せたときだけ出す（見出しごとに印が並ぶと本文の邪魔）・畳んだ印は常に出す
  ".cm-gutters": { backgroundColor: "var(--panel)", color: "var(--dim)", border: "none" },
  ".cm-foldGutter .cm-gutterElement": { cursor: "pointer", padding: "0 2px 0 6px", fontSize: "12px" },
  ".wk-fold-open": { opacity: "0", transition: "opacity 0.12s" },
  ".cm-gutters:hover .wk-fold-open": { opacity: "0.8" },
  ".wk-fold-closed": { color: "var(--accent)" },
  ".cm-foldPlaceholder": { backgroundColor: "var(--accent-soft)", border: "none", color: "var(--dim)", padding: "0 6px", borderRadius: "4px" },
  // 貼った DOI / PubMed の URL のメニュー（cite.ts）
  ".wk-cite-pop": { display: "flex", alignItems: "center", gap: "6px", padding: "5px 8px", fontSize: "12px", fontFamily: "-apple-system, system-ui, sans-serif" },
  ".wk-cite-pop button": {
    border: "1px solid var(--border)", borderRadius: "5px", padding: "2px 8px", background: "var(--bg)", color: "var(--text)", cursor: "pointer", fontSize: "12px",
  },
  ".wk-cite-pop button:hover": { borderColor: "var(--accent)" },
  ".wk-cite-pop button.wk-cite-pop-main": { background: "var(--accent-soft)" },
  ".wk-cite-pop-note": { color: "var(--dim)", overflowWrap: "anywhere", maxWidth: "28em" },
});

// 文法の色（md の記号は控えめ・見出しは表示側の色分けと同じ変数）
const highlight = HighlightStyle.define([
  { tag: tags.heading1, fontWeight: "700", color: "var(--wk-h1)" },
  { tag: tags.heading2, fontWeight: "700", color: "var(--wk-h2)" },
  { tag: tags.heading3, fontWeight: "700", color: "var(--wk-h3)" },
  { tag: [tags.heading4, tags.heading5, tags.heading6], fontWeight: "700", color: "var(--wk-h4)" },
  { tag: tags.strong, fontWeight: "700" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: [tags.link, tags.url], color: "var(--accent)" },
  { tag: tags.quote, color: "var(--dim)" },
  { tag: tags.monospace, color: "var(--wk-h3)" },
  { tag: [tags.processingInstruction, tags.meta, tags.contentSeparator], color: "var(--dim)" },
  { tag: tags.comment, color: "var(--dim)", fontStyle: "italic" },
  { tag: [tags.tagName, tags.angleBracket], color: "var(--accent)" },
  { tag: tags.attributeName, color: "var(--wk-h3)" },
  { tag: [tags.attributeValue, tags.string], color: "var(--wk-h2)" },
  { tag: [tags.keyword, tags.propertyName], color: "var(--wk-h4)" },
  { tag: tags.invalid, color: "var(--danger)" },
]);

/** 先頭の frontmatter（`---` … `---`）を 1 つの塊に（無いと `---` の次の行が setext 見出しに見える）。閉じの無いものは触らない */
const FRONTMATTER = /^---[ \t]*\r?\n[\s\S]*?\r?\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/;
const frontmatter: MarkdownConfig = {
  defineNodes: [{ name: "Frontmatter", block: true, style: tags.meta }],
  parseBlock: [
    {
      name: "Frontmatter",
      before: "HorizontalRule",
      parse(cx, line) {
        if (cx.lineStart !== 0 || !/^---[ \t]*$/.test(line.text)) return false;
        // 閉じの有無を先に見る（BlockContext は巻き戻せない）。input は型の上では非公開 = 無ければ塊にしない（従来の見え方）
        const input = (cx as unknown as { input?: { read(from: number, to: number): string; length: number } }).input;
        if (!input || !FRONTMATTER.test(input.read(0, Math.min(input.length, 1 << 20)))) return false;
        const from = cx.lineStart;
        let end = from + line.text.length;
        while (cx.nextLine()) {
          end = cx.lineStart + line.text.length;
          if (/^(?:---|\.\.\.)[ \t]*$/.test(line.text)) {
            cx.nextLine();
            break;
          }
        }
        cx.addElement(cx.elt("Frontmatter", from, end));
        return true;
      },
    },
  ],
};

/** md の書式キー（html のソースには効かせない = 素通し） */
function mdKeys(): Extension {
  return keymap.of([
    { key: "Mod-b", run: (v) => toggleWrap(v, "**") },
    { key: "Mod-i", run: (v) => toggleWrap(v, "*") },
    { key: "Mod-k", run: insertLink },
    { key: "Mod-Shift-8", run: (v) => toggleList(v, false) },
    { key: "Mod-Shift-7", run: (v) => toggleList(v, true) },
    ...markdownKeymap, // Enter でリストの続き・Backspace で記号を外す
  ]);
}

/** 畳む印（開 = ⌄ / 閉 = ›）。class で見え方を分ける（theme の .wk-fold-*） */
function foldMarker(open: boolean): HTMLElement {
  const el = document.createElement("span");
  el.className = open ? "wk-fold-open" : "wk-fold-closed";
  el.textContent = open ? "⌄" : "›";
  return el;
}

/** 画像のファイル（クリップボードの items と files の両方を見る = WebKit のスクリーンショットは items にだけ来ることがある） */
function clipboardImages(dt: DataTransfer | null): File[] {
  if (!dt) return [];
  const out: File[] = [];
  for (const it of Array.from(dt.items ?? [])) {
    if (it.kind === "file" && it.type.startsWith("image/")) {
      const f = it.getAsFile();
      if (f) out.push(f);
    }
  }
  if (!out.length) for (const f of Array.from(dt.files ?? [])) if (f.type.startsWith("image/")) out.push(f);
  return out;
}

export function createEditor(parent: HTMLElement, o: EditorOptions): EditorHandle {
  // 改行は元のファイルに合わせる（CRLF のファイルを LF で書き戻して全行を変えない）。エディタの中は \n
  // （貼り付けた LF の文も CRLF の文書では CRLF で書き出す）・外部の内容で置き換えたらその改行コードに追随
  let eol = eolOf(o.doc);
  const cite = o.mode === "md" && o.cite ? citePaste(o.cite) : null;
  const ext: Extension[] = [
    highlightSpecialChars(),
    history(),
    drawSelection(),
    indentOnInput(),
    foldGutter({ markerDOM: foldMarker }),
    EditorView.lineWrapping,
    theme,
    syntaxHighlighting(highlight),
    search({ top: true }),
    o.mode === "md"
      ? [keymap.of(tableKeymap), mdKeys(), slashMenu({ onPickFiles: o.onPickFiles, onCite: o.onCite }), markdown({ base: markdownLanguage, extensions: [frontmatter] }), cite?.extension ?? []]
      : html(),
    keymap.of([...searchKeymap, ...historyKeymap, ...foldKeymap, ...defaultKeymap, indentWithTab]),
    EditorView.updateListener.of((u) => {
      if (u.docChanged) o.onChange();
    }),
    EditorView.domEventHandlers({
      paste(e, view) {
        const files = clipboardImages(e.clipboardData);
        if (files.length && o.onPasteImages) {
          e.preventDefault();
          o.onPasteImages(files);
          return true;
        }
        // 選択なしで DOI / PubMed の URL だけ（選択の上の URL は先に pasteURLAsLink が普通のリンクにしている）。
        // 文献リンクを置けない所（コード・frontmatter・リンクの中・`[t](` の直後 = リンク先）は今どおり貼るだけ
        const url = cite ? citeUrlOf(e.clipboardData?.getData("text/plain")) : null;
        const sel = view.state.selection;
        if (!cite || !url || sel.ranges.length !== 1 || !sel.main.empty || citeBlocked(view.state, sel.main.from, sel.main.from, true)) return false;
        e.preventDefault();
        cite.paste(view, url);
        return true;
      },
    }),
    EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off", autocapitalize: "off" }),
  ];
  // 言語で替わる物（CodeMirror の UI の文言・placeholder）は Compartment に入れる = 言語の切替で reconfigure。
  // 本文の丸ごとの置き換え（setText = 新しい state）でも今の言語の値で入れ直す
  const phrasesC = new Compartment();
  const placeholderC = new Compartment();
  let curPhrases = o.phrases ?? null;
  let curPlaceholder = o.placeholder ?? "";
  const phrasesExt = (): Extension => (curPhrases ? EditorState.phrases.of(curPhrases) : []);
  const placeholderExt = (): Extension => (curPlaceholder ? cmPlaceholder(curPlaceholder) : []);
  const withLocale = (): Extension[] => [...ext, phrasesC.of(phrasesExt()), placeholderC.of(placeholderExt())];
  const view = new EditorView({ parent, state: EditorState.create({ doc: o.doc, extensions: withLocale() }) });
  let destroyed = false;
  // 窓を閉じた後に焦点を戻す側（focusReturn.ts）が CodeMirror を import せずに使う戻し方（カーソル・選択・スクロールを保つ view.focus）
  editorFocus.set(view.dom, () => view.focus());
  return {
    view,
    getText: () => view.state.doc.sliceString(0, view.state.doc.length, eol),
    setText(s: string) {
      eol = eolOf(s);
      // 外部の内容で置き換える = 新しい state（取消しで外部の書換えの前へ戻れない）。拡張は同じ
      view.setState(EditorState.create({ doc: s, extensions: withLocale() }));
    },
    insert(s: string) {
      insertAtCaret(view, s);
      view.focus();
    },
    cite: (key: string) => insertCite(view, key),
    citeCheck() {
      const r = view.state.selection.main;
      return citeBlocked(view.state, r.from, r.to);
    },
    selection() {
      const r = view.state.selection.main;
      return view.state.sliceDoc(r.from, r.to);
    },
    focus: () => view.focus(),
    gotoLine(n: number) {
      const doc = view.state.doc;
      const at = doc.line(Math.min(Math.max(1, Math.round(n)), doc.lines)).from;
      view.dispatch({ selection: { anchor: at }, effects: EditorView.scrollIntoView(at, { y: "start", yMargin: 12 }) });
      view.focus();
    },
    openSearch() {
      view.focus();
      openSearchPanel(view);
    },
    setLocale(phrases: Record<string, string>, placeholder: string) {
      curPhrases = phrases;
      curPlaceholder = placeholder;
      // CodeMirror は phrases が替わると view を作り直す（EditorView.update → setState = 描き直し・ViewState・プラグイン・パネルが初めから）。
      // 本文・選択・取消しの履歴・問い合わせ（state）は残る。初めに戻る物を前後で保つ:
      // - スクロールの位置: 作り直しで動く（Chrome は先へ・WKWebView は先頭へ）→ scrollSnapshot（上端の行と、そこからのずれ）を後で当てる
      // - 検索の窓の欄（検索 / 置換）の焦点と、その中の選択: 作り直された窓は検索の欄へ焦点を置く → 元の欄へ戻す
      // - 行へ移動の窓（⌘⌥G・showDialog）: 作り直しで閉じる（古いパネルの destroy が結果 null を返して close）→ 開き直して打った字と焦点を戻す
      // 窓の欄に焦点があった（打ちかけ）なら、先に外して打ちかけの字を確定させる（欄の change = CodeMirror の commit。外さないと作り直しの
      // 途中で古い欄が change を出して更新の中から dispatch する = 例外 "Calls to EditorView.update are not allowed while an update is in progress"）
      const scroll = view.scrollSnapshot();
      const top0 = view.scrollDOM.scrollTop;
      const left0 = view.scrollDOM.scrollLeft;
      const keepScroll = () => {
        view.dispatch({ effects: scroll });
        // 作り直しの後の測り直し（高さの見積もり → 実測）が終わった所で、数値でも前の位置へ（WKWebView は snapshot だけでは位置が動く）
        view.requestMeasure({ read: () => null, write: () => {
          view.scrollDOM.scrollTop = top0;
          view.scrollDOM.scrollLeft = left0;
        } });
      };
      const active = document.activeElement as HTMLElement | null;
      const input = active instanceof HTMLInputElement && view.dom.contains(active) && active.closest(".cm-panel") ? active : null;
      const searchField = input?.closest(".cm-search") ? input.name : null;
      const range = input ? ([input.selectionStart, input.selectionEnd] as const) : null;
      const gotoInput = view.dom.querySelector<HTMLInputElement>(".cm-goto-line input");
      const gotoValue = gotoInput?.value ?? null;
      const inGoto = !!input && input === gotoInput;
      if (input) input.blur();
      view.dispatch({ effects: [phrasesC.reconfigure(phrasesExt()), placeholderC.reconfigure(placeholderExt())] });
      keepScroll();
      const refocus = (el: HTMLInputElement | null) => {
        if (!el) return;
        el.focus({ preventScroll: true });
        if (range) el.setSelectionRange(range[0], range[1]);
      };
      if (searchField) refocus(view.dom.querySelector<HTMLInputElement>(`.cm-search input[name="${searchField}"]`));
      // 本文に焦点があった = 本文へ戻す（作り直された検索の窓は欄へ焦点を置く。CodeMirror 自身の戻し（hasFocus）は文書に焦点が無いと働かない）
      else if (active && view.contentDOM.contains(active) && document.activeElement !== active) view.focus();
      if (gotoValue === null) return;
      // 古い行へ移動の窓の close は microtask の後に届く → それより後で開き直す
      setTimeout(() => {
        if (destroyed) return;
        if (!view.dom.querySelector(".cm-goto-line")) cmGotoLine(view);
        const g = view.dom.querySelector<HTMLInputElement>(".cm-goto-line input");
        if (!g) return;
        g.value = gotoValue;
        if (inGoto) refocus(g);
        else if (searchField) refocus(view.dom.querySelector<HTMLInputElement>(`.cm-search input[name="${searchField}"]`));
        else if (active?.isConnected && active !== document.body) active.focus({ preventScroll: true }); // エディタの本文・ほかの欄
        else g.blur();
        keepScroll();
      }, 0);
    },
    destroy: () => {
      destroyed = true;
      editorFocus.delete(view.dom);
      view.destroy();
    },
  };
}
