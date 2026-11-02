<script lang="ts">
  /* チャットを浮かせたパネル（user: 右ペイン / PDF の右バーの中は狭い → アプリの窓の中で自由に動かせる・大きさも変えられる）。
     中身は WikiChat そのもの（同じ会話・文脈は前面のタブ）。見出しを掴んで移動・右下の角で大きさ・[戻す] で右ペインへ。
     位置と大きさは chat.svelte の setFloat が覚える（ui_wiki_chat_float）。窓の大きさが変わったら中に収める。
     マウス / トラックパッドの追跡は pointer capture（Tauri の dragDropEnabled と HTML5 DnD が競合するので pointer で自作 = 行ドラッグと同じ） */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import WikiChat from "./WikiChat.svelte";
  import { chat, setFloat } from "./chat.svelte";
  import { clampFloat, type ChatFloat } from "./chat";

  const f = $derived(chat.float ?? { x: 0, y: 0, w: 320, h: 260 });
  /** 進行中のドラッグを中断する（パネルが消えるとき = pointerup が届かない） */
  let abort: (() => void) | null = null;

  function drag(e: PointerEvent, mode: "move" | "resize") {
    if (e.button !== 0 || !chat.float) return;
    if (mode === "move" && (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    const start: ChatFloat = { ...chat.float };
    const sx = e.clientX;
    const sy = e.clientY;
    let last = start;
    const onMove = (ev: PointerEvent) => {
      if (!(ev.buttons & 1)) return finish();
      const dx = ev.clientX - sx;
      const dy = ev.clientY - sy;
      last = clampFloat(mode === "move" ? { ...start, x: start.x + dx, y: start.y + dy } : { ...start, w: start.w + dx, h: start.h + dy }, window.innerWidth, window.innerHeight);
      chat.float = last;
    };
    const finish = () => {
      abort = null;
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", finish);
      el.removeEventListener("pointercancel", finish);
      el.removeEventListener("lostpointercapture", finish);
      window.removeEventListener("blur", finish);
      try {
        el.releasePointerCapture(e.pointerId);
      } catch {
        /* 既に離れている */
      }
      // 途中で戻された（chat.float が null）なら覚えない = 戻したパネルを蘇らせない
      if (last !== start && chat.float) setFloat(last);
    };
    abort?.();
    abort = finish;
    try {
      el.setPointerCapture(e.pointerId);
    } catch {
      /* jsdom 等 */
    }
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", finish);
    el.addEventListener("pointercancel", finish);
    el.addEventListener("lostpointercapture", finish);
    window.addEventListener("blur", finish);
  }

  // 出たとき（起動時の復元・無効の間に窓が小さくなった）と窓の大きさが変わったとき = 中に収める（表示の値を直すだけで覚え直さない =
  // 次の起動では覚えた位置から収め直す）。消えるときは進行中のドラッグを中断する（pointerup が届かず blur のリスナーが残る）
  $effect(() => {
    const onResize = () => {
      // effect の中で chat.float を読んで書くと（同じ値でも新しいオブジェクト）無限に回る → untrack で読み、値が変わるときだけ書く
      const cur = untrack(() => chat.float);
      if (!cur) return;
      const c = clampFloat(cur, window.innerWidth, window.innerHeight);
      if (c.x !== cur.x || c.y !== cur.y || c.w !== cur.w || c.h !== cur.h) chat.float = c;
    };
    onResize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      abort?.();
    };
  });
</script>

<div class="wk-float" style="left: {f.x}px; top: {f.y}px; width: {f.w}px; height: {f.h}px" role="dialog" aria-label={t("チャット")}>
  <div class="wk-float-bar" role="presentation" onpointerdown={(e) => drag(e, "move")}>
    <span class="wk-float-title">{t("チャット")}{#if chat.status?.state === "busy"}<span class="wk-float-dot" aria-hidden="true"></span>{/if}</span>
    <span class="wk-float-hint">{t("掴んで移動・右下の角で大きさ")}</span>
    <button class="wk-float-btn" title={t("右ペインに戻す")} onclick={() => setFloat(null)}>{t("戻す")}</button>
  </div>
  <div class="wk-float-body"><WikiChat /></div>
  <div class="wk-float-grip" role="presentation" onpointerdown={(e) => drag(e, "resize")} aria-hidden="true"></div>
</div>

<style>
  .wk-float {
    position: fixed; z-index: 18; display: flex; flex-direction: column; min-width: 0; min-height: 0;
    background: var(--panel); border: 1px solid var(--border); border-radius: 10px; overflow: hidden;
    box-shadow: 0 10px 32px rgba(0, 0, 0, 0.28), 0 0 0 1px rgba(0, 0, 0, 0.04);
  }
  .wk-float-bar {
    display: flex; align-items: center; gap: 8px; padding: 6px 10px; background: var(--bg); border-bottom: 1px solid var(--border);
    cursor: grab; user-select: none; -webkit-user-select: none; flex-shrink: 0; touch-action: none;
  }
  .wk-float-bar:active { cursor: grabbing; }
  .wk-float-title { font-size: 12px; font-weight: 600; display: inline-flex; align-items: center; gap: 6px; }
  .wk-float-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
  .wk-float-hint { flex: 1; font-size: 11px; color: var(--dim); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-float-btn {
    border: 1px solid var(--border); border-radius: 6px; padding: 2px 8px; background: var(--panel); font-size: 11.5px; cursor: pointer; color: var(--text);
  }
  .wk-float-btn:hover { border-color: var(--accent); }
  .wk-float-body { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .wk-float-grip {
    position: absolute; right: 0; bottom: 0; width: 12px; height: 12px; cursor: nwse-resize; touch-action: none;
    background: linear-gradient(135deg, transparent 50%, var(--border) 50%, var(--border) 60%, transparent 60%, transparent 75%, var(--border) 75%, var(--border) 85%, transparent 85%);
  }
</style>
