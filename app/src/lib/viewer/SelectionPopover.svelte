<script lang="ts">
  import { t } from "../i18n.svelte";
  /* テキスト選択直後に選択近傍へ浮かぶ小ツールバー（Paperpile 参照）。
     ハイライト色 4 種 + コメント入力 + 保存。種別はハイライトのみ（下線 / 取消線は扱わない:
     線の位置が文字とずれる + ハイライトだけで足りる）。
     ※ ボタン/色ドットは mousedown で preventDefault し、選択（::selection）を消さない。
       コメント入力にフォーカスすると DOM 選択は消えるが、親は選択テキストを確定保持済みなので問題ない。 */
  import { USER_COLOR_CHOICES, ANNOT_PALETTE, type PopoverPos } from "./util";

  let {
    pos,
    page,
    color,
    comment,
    onColor,
    onComment,
    onSave,
    onCancel,
  }: {
    pos: PopoverPos;
    page: number;
    color: string;
    comment: string;
    onColor: (c: string) => void;
    onComment: (v: string) => void;
    onSave: () => void;
    onCancel: () => void;
  } = $props();

  // 選択を保持するため、コメント入力以外の mousedown は既定動作を止める
  const keepSel = (e: MouseEvent) => e.preventDefault();
</script>

<div
  class="sel-pop {pos.placement}"
  style="left:{pos.left}px; top:{pos.top}px"
  role="toolbar"
  aria-label={t("ハイライト")}
>
  <div class="sp-row">
    <span class="sp-label">{t("ハイライト")}</span>
    <div class="sp-colors">
      {#each USER_COLOR_CHOICES as c (c)}
        <button
          class="sp-dot"
          class:active={color === c}
          style="--dot:{ANNOT_PALETTE[c]}"
          title={c}
          aria-label={c}
          onmousedown={keepSel}
          onclick={() => onColor(c)}
        ></button>
      {/each}
    </div>
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
        }
      }}
    />
    <button class="sp-save" title={t("保存")} onmousedown={keepSel} onclick={onSave}>p.{page} {t("保存")}</button>
    <button class="sp-cancel" title={t("閉じる")} onmousedown={keepSel} onclick={onCancel}>×</button>
  </div>
</div>
