<script lang="ts">
  import { t } from "../i18n.svelte";
  /* サイドバー「注釈」タブの 1 カード（表示専用。編集状態は親 PdfViewer が保持し、
     操作はコールバックで通知する — SelectionPopover / Toolbar と同じ流儀）。
     編集モード: コメント textarea・色ドット 4 種（USER_COLOR_CHOICES）。種別はハイライトのみ（下線 / 取消線は
     扱わない。DB に残る旧 underline / strikethrough もハイライトとして表示・描画する）。
     Claude 注釈（author が claude 系）は色を purple 固定のまま変更不可（予約色の意味を守る）。 */
  import type { AnnotationRow } from "../api";
  import { USER_COLOR_CHOICES, ANNOT_PALETTE, colorForAnnotation, isClaudeAuthor } from "./util";

  let {
    a,
    query = "",
    editing,
    draftColor,
    draftComment,
    onJump,
    onDelete,
    onEditStart,
    onEditCancel,
    onEditSave,
    onDraftColor,
    onDraftComment,
  }: {
    a: AnnotationRow;
    query?: string; // 注釈検索の語（quote / コメント内の一致を <mark> で強調）
    editing: boolean;
    draftColor: string;
    draftComment: string;
    onJump: (a: AnnotationRow) => void;
    onDelete: (key: string) => void;
    onEditStart: (a: AnnotationRow) => void;
    onEditCancel: () => void;
    onEditSave: () => void;
    onDraftColor: (c: string) => void;
    onDraftComment: (v: string) => void;
  } = $props();

  const isClaude = $derived(isClaudeAuthor(a.author));

  /** text を検索語との一致 / 非一致の断片に分ける（大小無視）。長さの変わる lowercase（例 'İ'）があれば強調しない */
  function parts(text: string, q: string): { t: string; m: boolean }[] {
    if (!q) return [{ t: text, m: false }];
    const lower = text
      .split("")
      .map((c) => c.toLowerCase())
      .join("");
    const ql = q.toLowerCase();
    if (lower.length !== text.length || !ql) return [{ t: text, m: false }];
    const out: { t: string; m: boolean }[] = [];
    let from = 0;
    for (;;) {
      const i = lower.indexOf(ql, from);
      if (i < 0) break;
      if (i > from) out.push({ t: text.slice(from, i), m: false });
      out.push({ t: text.slice(i, i + ql.length), m: true });
      from = i + ql.length;
    }
    if (from < text.length) out.push({ t: text.slice(from), m: false });
    return out;
  }
  // 表示色: 編集中は draft、通常時は確定値（保存前に色が変わって見えないようにする）
  const displayColor = $derived(colorForAnnotation(editing ? draftColor : a.color, a.author));

  // note（Claude のノート）以外はすべてハイライト扱い（旧 underline / strikethrough を含む）
  function kindGlyph(k?: string | null) {
    return k === "note" ? "✎" : "▉";
  }
  function kindLabel(k?: string | null) {
    return k === "note" ? t("ノート") : t("ハイライト");
  }

  /** 編集中の Esc はカード編集をキャンセルするだけ。window の Esc ハンドラ（ビューア閉）へ
      バブルさせない（過去バグ: 入力の Esc が window onKey まで伝播し closeViewer が発火した）。 */
  function onEditKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onEditCancel();
    }
  }
</script>

<div class="ann" class:editing style="--c: {displayColor}">
  <div class="ann-head">
    <span class="kind-ic" title={kindLabel(a.kind)}>{kindGlyph(a.kind)}</span>
    <span class="who">{isClaude ? "Claude" : t("自分")}</span>
    {#if a.page}<span class="dim">p.{a.page}</span>{/if}
    {#if !editing}
      <div class="actions">
        <button class="edit" title={t("編集")} aria-label={t("注釈を編集")} onclick={() => onEditStart(a)}>✎</button>
        <button class="del" title={t("削除")} aria-label={t("注釈を削除")} onclick={() => onDelete(a.key)}>×</button>
      </div>
    {/if}
  </div>
  <button class="ann-quote" onclick={() => onJump(a)}>
    {#each parts(a.quote ?? "", query) as p, i (i)}{#if p.m}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}
  </button>

  {#if editing}
    <div class="ann-edit-body">
      <div class="ann-edit-row">
        {#if isClaude}
          <span class="ann-edit-locked" title={t("Claude 注釈は紫固定・変更不可")}>
            <span class="ann-edit-dot" style="--dot:{ANNOT_PALETTE.purple}"></span>{t("色は固定")}
          </span>
        {:else}
          <div class="ann-edit-colors">
            {#each USER_COLOR_CHOICES as c (c)}
              <button
                class="ann-edit-dot"
                class:active={draftColor === c}
                style="--dot:{ANNOT_PALETTE[c]}"
                title={c}
                aria-label={c}
                onclick={() => onDraftColor(c)}
                onkeydown={onEditKeydown}
              ></button>
            {/each}
          </div>
        {/if}
      </div>
      <textarea
        class="ann-edit-textarea"
        placeholder={t("コメント（空にして保存すると削除）")}
        value={draftComment}
        oninput={(e) => onDraftComment(e.currentTarget.value)}
        onkeydown={onEditKeydown}
      ></textarea>
      <div class="ann-edit-actions">
        <button class="ann-edit-cancel" onclick={onEditCancel} onkeydown={onEditKeydown}>{t("キャンセル")}</button>
        <button class="ann-edit-save" onclick={onEditSave} onkeydown={onEditKeydown}>{t("保存")}</button>
      </div>
    </div>
  {:else if a.comment}
    <div class="ann-comment">
      {#each parts(a.comment, query) as p, i (i)}{#if p.m}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}
    </div>
  {/if}
</div>
