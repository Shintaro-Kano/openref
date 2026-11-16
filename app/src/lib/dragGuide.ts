// 境界のつまみのドラッグ（左の列 = SidebarGrip・チャットの柱 = ChatDock）の骨格 1 つ。
// 動かしている間は線（呼び手の guide = position: fixed）だけ動かしてレイアウトを変えない（中央の再描画・隠れたタブの ResizeObserver・
// PDF / 本の読み手の描き直しを起こさない）・離したときに 1 回だけ確定する。終わり方:
//   確定する = pointerup・pointercancel・lostpointercapture・窓の blur・ボタンを離したまま動いた（取りこぼし）
//   確定しない = Esc（escape: true のときだけ = 左の列。柱には無い = 今までどおり）・返した関数（つまみが消えるとき）
// 動かさずに離した = commit を呼ばない。終わるのは 1 回だけ（2 回目以降は何もしない）。
// pointer は自作（Tauri の dragDropEnabled と HTML5 DnD が競合する = 行ドラッグと同じ）。押下の判定（左ボタン・preventDefault・
// ダブルクリック）と幅の式は呼び手が持つ

export type GuideDrag = {
  /** 指の x（窓の座標）→ 確定する値と線の位置（窓の x） */
  at: (clientX: number) => { value: number; guide: number };
  /** 線の位置（null = 消す） */
  guide: (x: number | null) => void;
  /** 離したときに 1 回（動かしていなければ呼ばない） */
  commit: (value: number) => void;
  /** Esc = 取り消し（確定しない・背後へ流さない） */
  escape?: boolean;
};

/** e.currentTarget（つまみ）で pointer を捕まえてドラッグを始める。返す関数 = 確定せずに終える（つまみが消えるとき） */
export function startGuideDrag(e: PointerEvent, o: GuideDrag): () => void {
  const g = e.currentTarget as HTMLElement;
  let next: number | null = null;
  let done = false;
  const onMove = (ev: PointerEvent) => {
    if (!(ev.buttons & 1)) return finish();
    const r = o.at(ev.clientX);
    next = r.value;
    o.guide(r.guide);
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key !== "Escape") return;
    ev.preventDefault();
    ev.stopPropagation();
    end(false);
  };
  const end = (commit: boolean) => {
    // 1 回だけ: 呼び手は終わった後も返した関数を持ち続ける（次の押下・つまみが消えるときに呼ぶ）= 2 回目に body の印を外すと、
    // その間に始まった別のドラッグ（もう 1 つのつまみ・行のドラッグ）の印を消してしまう（sidebarw.test ⑧）
    if (done) return;
    done = true;
    g.removeEventListener("pointermove", onMove);
    g.removeEventListener("pointerup", finish);
    g.removeEventListener("pointercancel", finish);
    g.removeEventListener("lostpointercapture", finish);
    window.removeEventListener("blur", finish);
    if (o.escape) window.removeEventListener("keydown", onKey, true);
    try {
      g.releasePointerCapture(e.pointerId);
    } catch {
      /* 既に離れている */
    }
    document.body.classList.remove("dragging");
    o.guide(null);
    if (commit && next != null) o.commit(next); // 値が変わるのはここ 1 回だけ
  };
  const finish = () => end(true);
  try {
    g.setPointerCapture(e.pointerId);
  } catch {
    /* jsdom 等 */
  }
  document.body.classList.add("dragging");
  g.addEventListener("pointermove", onMove);
  g.addEventListener("pointerup", finish);
  g.addEventListener("pointercancel", finish);
  g.addEventListener("lostpointercapture", finish);
  window.addEventListener("blur", finish);
  if (o.escape) window.addEventListener("keydown", onKey, true);
  return () => end(false);
}
