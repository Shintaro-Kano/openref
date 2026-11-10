// chat.test の偽ホスト = PdfViewer の代わり（pdf.js は jsdom で動かない）。PdfViewer の初期化と同じく followItemSelection に前面と選択を渡す
import { followItemSelection, setViewingSelection } from "../../viewing.svelte";

/** setViewingSelection を effect から直に呼ぶ書き手（その effect が chat.status を追わないことを見る）。runs = effect が走った回数 */
export function startRawWriter(key: string, text: string): { stop: () => void; runs: () => number } {
  let runs = 0;
  const stop = $effect.root(() => {
    $effect(() => {
      runs++;
      setViewingSelection(key, text);
    });
  });
  return { stop, runs: () => runs };
}

export const host = $state({ front: true, text: null as string | null });

/** PdfViewer と同じ呼び方で effect を張る（戻り値 = 解除） */
export function startHost(itemKey: string): () => void {
  return $effect.root(() => followItemSelection(itemKey, () => host.front, () => host.text));
}
