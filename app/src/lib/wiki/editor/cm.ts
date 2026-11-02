// OpenNote のエディタ = CodeMirror 6 の組立て。WikiEditor が編集を開いたときに
// 動的 import する（読み専用の利用者・MY LIBRARY の起動には読まない = 初期 bundle に入れない）。
// md / html モード・行の折返し・ライト / ダーク（app.css の変数をそのまま使う = 切替に追随）・⌘B / ⌘I / ⌘K / リスト・
// CodeMirror の検索（⌘F・置換）・画像のペースト（onPasteImages へ渡すだけ = 保存は呼び出し側）・「/」の機能メニュー（slash.ts・md だけ）

import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, drawSelection, highlightSpecialChars, keymap, placeholder as cmPlaceholder } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { markdown, markdownKeymap, markdownLanguage } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { tags } from "@lezer/highlight";
import type { MarkdownConfig } from "@lezer/markdown";
import { insertAtCaret, insertLink, toggleList, toggleWrap } from "./format";
import { eolOf } from "./eol";
import { slashMenu } from "./slash";

export interface EditorOptions {
  doc: string;
  mode: "md" | "html";
  /** 本文が変わった（打鍵・貼付・取消し）。本文は handle.getText() で読む（毎打鍵で全文を作らない） */
  onChange: () => void;
  /** クリップボードの画像（スクリーンショット等）。挿入は呼び出し側が保存の後に handle.insert で */
  onPasteImages?: (files: File[]) => void;
  /** 「/」メニューの「画像・PDF を埋め込む…」= ファイル選択 → 取込 → caret に `![[…]]`（呼び出し側。無ければ候補に出ない） */
  onPickFiles?: () => void;
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
  focus(): void;
  openSearch(): void;
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
  const ext: Extension[] = [
    highlightSpecialChars(),
    history(),
    drawSelection(),
    indentOnInput(),
    EditorView.lineWrapping,
    theme,
    syntaxHighlighting(highlight),
    search({ top: true }),
    o.mode === "md" ? [mdKeys(), slashMenu({ onPickFiles: o.onPickFiles }), markdown({ base: markdownLanguage, extensions: [frontmatter] })] : html(),
    keymap.of([...searchKeymap, ...historyKeymap, ...defaultKeymap, indentWithTab]),
    EditorView.updateListener.of((u) => {
      if (u.docChanged) o.onChange();
    }),
    EditorView.domEventHandlers({
      paste(e) {
        const files = clipboardImages(e.clipboardData);
        if (!files.length || !o.onPasteImages) return false;
        e.preventDefault();
        o.onPasteImages(files);
        return true;
      },
    }),
    EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off", autocapitalize: "off" }),
  ];
  if (o.phrases) ext.push(EditorState.phrases.of(o.phrases));
  if (o.placeholder) ext.push(cmPlaceholder(o.placeholder));
  const view = new EditorView({ parent, state: EditorState.create({ doc: o.doc, extensions: ext }) });
  return {
    view,
    getText: () => view.state.doc.sliceString(0, view.state.doc.length, eol),
    setText(s: string) {
      eol = eolOf(s);
      // 外部の内容で置き換える = 新しい state（取消しで外部の書換えの前へ戻れない）。拡張は同じ
      view.setState(EditorState.create({ doc: s, extensions: ext }));
    },
    insert(s: string) {
      insertAtCaret(view, s);
      view.focus();
    },
    focus: () => view.focus(),
    openSearch() {
      view.focus();
      openSearchPanel(view);
    },
    destroy: () => view.destroy(),
  };
}
