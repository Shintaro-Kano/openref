<script lang="ts">
  import { t } from "../i18n.svelte";
  /* ページ上の既存ハイライトをクリックしたときに浮かぶ編集ポップオーバー（Paperpile の highlight toolbar 相当）。
     色 4 種（クリックで即適用）+ コメント入力（保存で確定）+ 削除 + 閉じる。
     見た目は SelectionPopover と同じ .sel-pop を共用し、差分は .hl-pop で上書き（app.css）。
     Claude 注釈（author が claude 系）は色を purple 固定のまま変更不可（サイドバーの編集と同じ規則）。 */
  import type { AnnotationRow } from "../api";
  import { USER_COLOR_CHOICES, ANNOT_PALETTE, colorForAnnotation, type PopoverPos } from "./util";

  let {
    pos,
    a,
    comment,
    onColor,
    onComment,
    onSave,
    onDelete,
    onClose,
  }: {
    pos: PopoverPos;
    a: AnnotationRow;
    comment: string;
    onColor: (c: string) => void;
    onComment: (v: string) => void;
    onSave: () => void;
    onDelete: () => void;
    onClose: () => void;
  } = $props();

  const isClaude = $derived(a.author.startsWith("claude"));
  const curColor = $derived(a.color && ANNOT_PALETTE[a.color] ? a.color : isClaude ? "purple" : USER_COLOR_CHOICES[0]);
</script>

<div
  class="sel-pop hl-pop {pos.placement}"
  style="left:{pos.left}px; top:{pos.top}px; --c:{colorForAnnotation(a.color, a.author)}"
  role="toolbar"
  aria-label={t("ハイライトを編集")}
>
  <div class="sp-row">
    <span class="sp-label">{t("ハイライト")}</span>
    {#if isClaude}
      <span class="hp-locked" title={t("Claude 注釈は紫固定・変更不可")}>
        <span class="sp-dot" style="--dot:{ANNOT_PALETTE.purple}"></span>{t("色は固定")}
      </span>
    {:else}
      <div class="sp-colors">
        {#each USER_COLOR_CHOICES as c (c)}
          <button
            class="sp-dot"
            class:active={curColor === c}
            style="--dot:{ANNOT_PALETTE[c]}"
            title={c}
            aria-label={c}
            onclick={() => onColor(c)}
          ></button>
        {/each}
      </div>
    {/if}
    <span class="hp-spacer"></span>
    <button class="hp-del" title={t("削除")} aria-label={t("ハイライトを削除")} onclick={onDelete}>🗑</button>
  </div>
  <div class="sp-row sp-comment">
    <input
      class="sp-input"
      placeholder={t("コメント（任意）")}
      value={comment}
      oninput={(e) => onComment(e.currentTarget.value)}
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          onSave();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    />
    <button class="sp-save" title={t("コメントを保存")} onclick={onSave}>{t("保存")}</button>
    <button class="sp-cancel" title={t("閉じる")} onclick={onClose}>×</button>
  </div>
</div>
