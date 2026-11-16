<script lang="ts">
  /* OpenNote のエディタ。CodeMirror 6 は編集を開いたときに動的 import（editor/cm.ts）。
     本文と保存の規則は edit.svelte.ts（このエディタは attach / detach で本文の出入口を渡すだけ）。
     ⌘B / ⌘I / ⌘K / ⌘⇧8 ⌘⇧7（リスト）は md だけ・⌘F は CodeMirror の検索・画像の貼付は wiki_save_blob・「/」の機能メニューは editor/slash.ts・
     文献の引用（「/」の「文献を引用…」= WikiCite の窓・DOI / PubMed の URL の貼り付けのメニュー）は editor/cite.ts + cite.svelte.ts */
  import { onDestroy, onMount, untrack } from "svelte";
  import { i18n, t } from "../i18n.svelte";
  import type { EditorHandle } from "./editor/cm";
  import { attach, changed, citeRegister, detach, editNotice, editText, pasteImages, pickFiles } from "./edit.svelte";
  import { lookupCite, openCite } from "./cite.svelte";

  let { mode, onReady }: { mode: "md" | "html"; onReady?: (h: EditorHandle) => void } = $props();

  let el: HTMLDivElement | undefined = $state();
  let h: EditorHandle | null = null;
  let failed = $state<string | null>(null);
  let loading = $state(true);
  let alive = true;

  /** CodeMirror の UI の文言（鍵 = CodeMirror の英語の原文） */
  function phrases(): Record<string, string> {
    return {
      Find: t("検索"),
      Replace: t("置換"),
      next: t("次へ"),
      previous: t("前へ"),
      all: t("すべて::search"), // 検索と置換の窓の [all]（LIBRARY の左バーの鍵「すべて」と分ける = 共用すると英語の表示で「All Papers」）
      "match case": t("大小を区別"),
      regexp: t("正規表現"),
      "by word": t("単語単位"),
      replace: t("置換"),
      "replace all": t("すべて置換"),
      close: t("閉じる"),
      "current match": t("現在の一致"),
      "on line": t("行"),
      "replaced match on line $": t("$ 行目を置換しました"),
      "replaced $ matches": t("$ か所を置換しました"),
      "Control character": t("制御文字"),
      // 見出しで畳んだ所の印（.cm-foldPlaceholder の aria-label / title・訳さないと日本語の表示でも CodeMirror の英語のまま）。
      // 英語の値は CodeMirror の原文と同じ（英語の表示は前のまま）
      "folded code": t("畳んだ部分"),
      unfold: t("開く::fold"),
      // 読み上げ（EditorView.announce）・行へ移動の窓（⌘⌥G = searchKeymap）・補完の一覧の aria-label（CodeMirror の phrase の残り）
      "Folded lines": t("畳んだ行"),
      "Unfolded lines": t("開いた行"),
      to: t("〜::fold"),
      "Go to line": t("行へ移動"),
      go: t("移動::goto"),
      Completions: t("候補"),
      "Selection deleted": t("選択範囲を削除しました"),
    };
  }

  const placeholderText = () => (mode === "md" ? t("ここに書く（Markdown）") : "");
  // 表示の言語が替わったら、開いているエディタの文言を差し替える（作り直さない = 本文・選択・スクロール・取消しの履歴・焦点・検索の窓を保つ。
  // 前は開いた時点の言語のまま = 開き直すまで替わらなかった）
  let langSeen = untrack(() => i18n.lang);
  $effect(() => {
    const lang = i18n.lang;
    if (lang === langSeen) return;
    langSeen = lang;
    untrack(() => h?.setLocale(phrases(), placeholderText()));
  });

  onMount(async () => {
    try {
      const cm = await import("./editor/cm");
      if (!alive || !el) return;
      h = cm.createEditor(el, {
        doc: editText(),
        mode,
        onChange: changed,
        onPasteImages: (files) => void pasteImages(files),
        onPickFiles: () => void pickFiles(),
        onCite: () => void openCite(),
        cite: {
          lookup: async (url) => {
            const r = await lookupCite(url);
            if (!r) throw new Error(t("DOI / PMID として読めません"));
            return r.item ? { key: r.item.key, retracted: r.item.retracted } : null;
          },
          register: citeRegister,
          notice: editNotice,
        },
        phrases: phrases(),
        placeholder: placeholderText(),
      });
      attach(h);
      h.focus(); // WKWebView ではボタンのクリックでフォーカスが移らない = 明示
      onReady?.(h);
    } catch (e) {
      failed = String(e);
    } finally {
      loading = false;
    }
  });

  onDestroy(() => {
    alive = false;
    if (h) {
      detach(h);
      h.destroy();
      h = null;
    }
  });
</script>

<div class="wk-editor">
  {#if loading}<p class="wk-ed-note">{t("エディタを読込中…")}</p>{/if}
  {#if failed}<p class="wk-ed-note wk-ed-err">{t("エディタを開けません")}: {failed}</p>{/if}
  <div class="wk-ed-host" bind:this={el}></div>
</div>

<style>
  .wk-editor { position: relative; display: flex; flex-direction: column; min-width: 0; min-height: 0; height: 100%; container-type: inline-size; }
  .wk-ed-host { flex: 1; min-height: 0; }
  .wk-ed-note { padding: 12px 20px; font-size: 12px; color: var(--dim); }
  .wk-ed-err { color: var(--danger); white-space: pre-wrap; }
</style>
