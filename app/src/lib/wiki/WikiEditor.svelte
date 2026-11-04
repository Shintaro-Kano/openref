<script lang="ts">
  /* OpenNote のエディタ。CodeMirror 6 は編集を開いたときに動的 import（editor/cm.ts）。
     本文と保存の規則は edit.svelte.ts（このエディタは attach / detach で本文の出入口を渡すだけ）。
     ⌘B / ⌘I / ⌘K / ⌘⇧8 ⌘⇧7（リスト）は md だけ・⌘F は CodeMirror の検索・画像の貼付は wiki_save_blob・「/」の機能メニューは editor/slash.ts */
  import { onDestroy, onMount } from "svelte";
  import { t } from "../i18n.svelte";
  import type { EditorHandle } from "./editor/cm";
  import { attach, changed, detach, editText, pasteImages, pickFiles } from "./edit.svelte";

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
      all: t("すべて"),
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
    };
  }

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
        phrases: phrases(),
        placeholder: mode === "md" ? t("ここに書く（Markdown）") : "",
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
