// OpenNote のエディタ: 「/」で機能を挿すメニュー（CodeMirror の autocompletion を候補の出し方だけ借りる）。
// 行の頭か空白の後の `/` で開き、続けて打った文字で候補を絞る（日本語のラベル・英語の別名の両方に当たる）。
// 候補は Markdown の記号を入れるだけ（WYSIWYG は作らない = format.ts と同じ方針）。文字色は入れない（描画側で style を
// 許す必要がある）。「画像・PDF を埋め込む…」だけはファイル選択（呼び出し側 = edit.svelte の pickFiles）。
// データベースのビュー（表 / カレンダーの切替）は Markdown に無いので対象外（普通の表まで）。
// コード（fence・インライン）と frontmatter の中では開かない。

import { autocompletion, snippet, type Completion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { t } from "../../i18n.svelte";

export interface SlashItem {
  /** 画面に出す名前（翻訳済み） */
  label: string;
  /** 右に薄く出す記号（Markdown の形） */
  detail?: string;
  /** 英語の別名（`/table` `/h1` 等。小文字） */
  keys: string[];
  /** 入れる雛形（autocompletion の snippet の形 = `${}` が caret・`${名}` が差し替える欄）。block = 行の頭から */
  template?: string;
  block?: boolean;
  /** 前が段落なら空行を挟む（`---` は段落の直後だと setext 見出しに・`[toc]` は段落の続きになる） */
  gap?: boolean;
  /** 雛形ではなく動作（ファイル選択など）。`/…` を消した後に呼ぶ */
  action?: (view: EditorView) => void;
}

export interface SlashOptions {
  /** 「画像・PDF を埋め込む…」= ファイル選択 → 取込 → caret に `![[…]]`（無ければ候補に出さない） */
  onPickFiles?: () => void;
}

/** 候補の一覧（t() は呼ぶたびに引く = 言語の切替に追随するよう、開くときに作る） */
export function slashItems(o: SlashOptions): SlashItem[] {
  const items: SlashItem[] = [
    { label: t("見出し 1"), detail: "#", keys: ["h1", "heading1", "title"], template: "# ${}", block: true },
    { label: t("見出し 2"), detail: "##", keys: ["h2", "heading2"], template: "## ${}", block: true },
    { label: t("見出し 3"), detail: "###", keys: ["h3", "heading3"], template: "### ${}", block: true },
    { label: t("箇条書き"), detail: "-", keys: ["bullet", "list", "ul"], template: "- ${}", block: true },
    { label: t("番号付きリスト"), detail: "1.", keys: ["number", "numbered", "ol"], template: "1. ${}", block: true },
    { label: t("ToDo"), detail: "- [ ]", keys: ["todo", "task", "checkbox"], template: "- [ ] ${}", block: true },
    { label: t("引用::slash"), detail: ">", keys: ["quote", "blockquote"], template: "> ${}", block: true },
    { label: t("区切り線"), detail: "---", keys: ["divider", "hr", "rule"], template: "---\n${}", block: true, gap: true },
    // caret は本文（1）・Tab で開始行の言語（2）
    { label: t("コードブロック"), detail: "```", keys: ["code", "pre"], template: "```${2}\n${1}\n```\n", block: true },
    {
      label: t("表"),
      detail: "| |",
      keys: ["table"],
      template: `| \${${t("列 1")}} | \${${t("列 2")}} |\n| --- | --- |\n| \${} |  |\n`,
      block: true,
      gap: true, // 段落・引用の行の続き（lazy continuation）に読まれない
    },
    {
      label: t("コールアウト"),
      detail: "> [!note]",
      keys: ["callout", "note", "info", "tip", "warning"],
      template: "> [!${note}] ${}\n> ${}",
      block: true,
    },
    {
      label: t("折り畳み"),
      detail: "> [!note]-",
      keys: ["toggle", "fold", "details", "collapse"],
      template: "> [!${note}]- ${}\n> ${}",
      block: true,
    },
    { label: t("目次::slash"), detail: "[toc]", keys: ["toc", "contents", "outline"], template: "[toc]\n${}", block: true, gap: true },
    { label: t("ページへのリンク"), detail: "[[ ]]", keys: ["link", "page", "wikilink"], template: "[[${}]]" },
  ];
  if (o.onPickFiles) {
    const pick = o.onPickFiles;
    items.push({ label: t("画像・PDF を埋め込む…"), detail: "![[ ]]", keys: ["image", "pdf", "embed", "file", "attach", "video"], action: () => pick() });
  }
  return items;
}

/** 行の頭・空白・開き括弧の後の `/`（全角の `／` も = 日本語入力のまま）と、その後に続けて打った文字（空白と `/` を含まない）。
    URL の `//` や `a/b` では開かない */
const TRIGGER = /(?:^|[\s(（「[])[/／]([^\s/／]*)$/u;
/** リスト・ToDo の記号だけの行（Enter で続いた直後・引用の中も）= 記号を置き換えて block を入れる。1 = 字下げ・2 = 残す `> ` */
const LIST_ONLY = /^(\s*)(>\s*)?(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s*)?$/;
/** 引用の記号だけの行 = 記号を置き換える（字下げは残す） */
const QUOTE_ONLY = /^(\s*)>\s*$/;
/** この中では開かない（コード・frontmatter） */
const NO_MENU = /^(?:FencedCode|CodeBlock|InlineCode|CodeText|CodeMark|CodeInfo|Frontmatter|HTMLBlock)$/;

/** caret がコードや frontmatter の中か */
export function inCode(state: EditorState, pos: number): boolean {
  for (let n: { name: string; parent: unknown } | null = syntaxTree(state).resolveInner(pos, -1); n; n = n.parent as typeof n) {
    if (NO_MENU.test(n.name)) return true;
  }
  return false;
}

/** 候補の絞り込み: ラベルの部分一致（大小無視）か英語の別名の前方一致 */
export function slashMatches(it: SlashItem, q: string): boolean {
  if (!q) return true;
  const s = q.toLowerCase();
  return it.label.toLowerCase().includes(s) || it.keys.some((k) => k.startsWith(s));
}

/** 雛形を入れる。`/…` は消す。block の雛形は行の頭に置く:
    - `/` の前に文字があれば改行してから。記号だけの行（`- ` `1. ` `- [ ] ` `> `）なら記号を置き換える
    - gap の雛形（`---` `[toc]`）は前が段落の行なら空行を挟む（段落の続きに読まれない） */
function applyItem(view: EditorView, it: SlashItem, from: number, to: number) {
  if (it.action) {
    view.dispatch({ changes: { from, to, insert: "" }, userEvent: "input.complete" });
    it.action(view);
    return;
  }
  let lead = "";
  if (it.block) {
    const doc = view.state.doc;
    const line = doc.lineAt(from);
    const before = doc.sliceString(line.from, from);
    const marker = LIST_ONLY.exec(before) ?? QUOTE_ONLY.exec(before);
    if (marker) from = line.from + marker[1].length + (marker[2]?.length ?? 0);
    else if (/\S/.test(before)) lead = it.gap ? "\n\n" : "\n";
    if (!lead && it.gap && line.number > 1 && /\S/.test(doc.line(line.number - 1).text)) lead = "\n";
  }
  snippet(lead + (it.template ?? ""))(view, null, from, to);
}

/** autocompletion の候補源。`/` の位置から caret までを候補で置き換える */
export function slashSource(o: SlashOptions) {
  return (cx: CompletionContext): CompletionResult | null => {
    const line = cx.state.doc.lineAt(cx.pos);
    const m = TRIGGER.exec(cx.state.sliceDoc(line.from, cx.pos));
    if (!m || inCode(cx.state, cx.pos)) return null;
    const q = m[1];
    const from = cx.pos - q.length - 1;
    const options: Completion[] = slashItems(o)
      .filter((it) => slashMatches(it, q))
      .map((it) => ({
        label: it.label,
        detail: it.detail,
        apply: (view: EditorView, _c: Completion, f: number, to: number) => applyItem(view, it, f, to),
      }));
    if (!options.length) return null;
    // filter: false = 並びはこちらの順のまま（CodeMirror の曖昧一致で並べ替えない）。validFor を付けない = 1 字ごとに絞り直す
    return { from, to: cx.pos, options, filter: false };
  };
}

/** エディタに足す拡張（md だけ） */
export function slashMenu(o: SlashOptions): Extension {
  return autocompletion({
    override: [slashSource(o)],
    icons: false,
    activateOnTyping: true,
    // 1 字ごとに問い直す（validFor なし）ので、問い直しを待たせない = `/h2⏎` と速く打っても Enter が改行にならない
    activateOnTypingDelay: 0,
    closeOnBlur: true,
    tooltipClass: () => "wk-slash",
  });
}
