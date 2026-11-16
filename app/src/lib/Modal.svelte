<script lang="ts">
  /* モーダルの骨格 = 覆い（.overlay）・枠（.modal + 窓ごとの class）・Esc・覆いのクリック。中身（head / body / foot）と
     開いたときの focus は窓の側（WKWebView はボタンのクリックで focus を移さず autofocus も効かない場面がある = 窓が明示の focus()）。
     見た目は app.css の .overlay / .modal の規則がそのまま当たる（骨格は class を足すだけ・自前の CSS を持たない）。
     窓ごとの差は引数で表す（OpenNote / チャットの窓も同じ骨格に載せられるように）:
     - open: 出すか。骨格は窓の部品と一緒に常に mount しておく = Esc の受け手を window に付ける順は、窓の部品を置いた順のまま
       （出たときに付けると、後から mount された受け手 = OpenNote のページ等との順序が入れ替わる）
     - onescape: Esc（null = 骨格では扱わない = 参照エディタ・PDF 取込は App の Esc の連鎖）。IME の変換中の Esc では呼ばない
       （変換の取消の Esc で窓を閉じない = 全部の窓で同じ）
     - onbackdrop: 覆い自身を押したとき（null = 閉じない = 参照エディタ・移行 = 入力を失わない）。押し始め（pointerdown・主ボタン）と
       離し（pointerup）が両方とも覆い自身のときだけ呼ぶ = 窓の中で押して外へドラッグ（欄の文字の選択）・覆いで押して窓の中で離す、
       は閉じない。click は押しと離しの共通の祖先に飛ぶ（中で押して外で離すと target が覆い）ので click の target だけでは分からない。
       記憶は押すたびに上書き・pointercancel と閉じたときに消す = 画面の外で離して click が来なくても、次の押しで正される。
       開く操作の押し（覆いの出る前）は数えない = 開いた直後の最初の離しでは閉じない。右クリック・中クリックは click を出さない = 閉じない
     - stopclicks: 枠の中のクリックを外へ伝えない（window の bubble の受け手 = Sidebar のメニューの閉じ方に届かない。
       引用スタイル・完全に削除の今の作法）
     - onframeclick: 枠の中のクリック（ラベルの窓 = 色の見本を閉じる）
     - cls / role / label / labelledby / ariaModal / tabindex: 枠の属性
     - frame（bindable）: 枠の要素（開いたときに枠へ focus する窓 = フォルダの削除）
     覆いとして数えるか（state の modalOpen / covered）は窓を開く側の状態（app.modal・editorOpen・importJob・OpenNote の guard）で決まる = 骨格は関わらない */
  import type { Snippet } from "svelte";
  import { composing } from "./input";
  import { holdTab } from "./focusTrap";

  let {
    open,
    onescape = null,
    onbackdrop = null,
    stopclicks = false,
    onframeclick,
    cls = "",
    role = "dialog",
    label,
    labelledby,
    ariaModal = false,
    tabindex = null,
    frame = $bindable(null),
    children,
  }: {
    open: boolean;
    onescape?: ((e: KeyboardEvent) => void) | null;
    onbackdrop?: (() => void) | null;
    stopclicks?: boolean;
    onframeclick?: (e: MouseEvent) => void;
    cls?: string;
    role?: "dialog" | "alertdialog";
    label?: string;
    labelledby?: string;
    ariaModal?: boolean;
    tabindex?: number | null;
    frame?: HTMLDivElement | null;
    children: Snippet;
  } = $props();

  // 開いている間、Tab の焦点を枠の中に留める（Tab で窓の外の裏のボタン・本文へ出さない。骨格に載っていない窓も同じ holdTab）
  $effect(() => {
    const f = frame;
    if (open && f) return holdTab(f);
  });

  function onKey(e: KeyboardEvent) {
    if (open && onescape && e.key === "Escape" && !composing(e)) onescape(e);
  }

  // 覆いの押し始めと離しが覆い自身だったか
  let downOnBackdrop = false;
  let upOnBackdrop = false;
  function onBackdropDown(e: PointerEvent) {
    downOnBackdrop = e.button === 0 && e.target === e.currentTarget;
    upOnBackdrop = false;
  }
  function onBackdropUp(e: PointerEvent) {
    upOnBackdrop = downOnBackdrop && e.target === e.currentTarget;
  }
  function onBackdropClick(e: MouseEvent) {
    if (upOnBackdrop && e.target === e.currentTarget) onbackdrop?.();
  }
  function forgetBackdrop() {
    downOnBackdrop = upOnBackdrop = false;
  }
  // 骨格は常に mount = 閉じたら記憶を消す（覆いで押したまま Esc で閉じ、次に開いた直後の最初の離しで閉じない）
  $effect(() => {
    if (!open) forgetBackdrop();
  });

  function onFrameClick(e: MouseEvent) {
    if (stopclicks) e.stopPropagation();
    onframeclick?.(e);
  }
</script>

<svelte:window onkeydown={onKey} />

{#if open}
  <div
    class="overlay"
    role="presentation"
    onpointerdown={onbackdrop ? onBackdropDown : undefined}
    onpointerup={onbackdrop ? onBackdropUp : undefined}
    onpointercancel={onbackdrop ? forgetBackdrop : undefined}
    onclick={onbackdrop ? onBackdropClick : undefined}>
    <!-- 枠のクリックは外へ伝えない / 色の見本を閉じるだけ（キーの操作は中の部品）。tabindex は -1（枠へ focus する窓）だけ = 中身の Tab の巡回に入らない -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
    <div
      bind:this={frame}
      class={["modal", cls]}
      {role}
      aria-modal={ariaModal ? "true" : undefined}
      aria-label={label}
      aria-labelledby={labelledby}
      tabindex={tabindex ?? undefined}
      onclick={stopclicks || onframeclick ? onFrameClick : undefined}>
      {@render children()}
    </div>
  </div>
{/if}
