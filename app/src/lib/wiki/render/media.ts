/* OpenNote: 画像・動画・音声の部品（中央の MediaView と md の `![[…]]` の展開が同じこれを使う）。
   DOM を組むだけ（Svelte の部品は shadow root の中へ入れにくい = 同じ見た目を素の DOM で共有する）。
   「外部で開く」は data-wk-open（絶対パス）を付けたボタン = 呼び手のクリックの横取りが onNavigate({kind:"open"}) に変える。
   再生に失敗したら（error・WebKit が扱えない符号化）同じボタンの案内に差し替える */

export interface MediaLabels {
  /** 「外部で開く」 */
  openExternal: string;
  /** 「再生できません。外部のアプリで開いてください」 */
  playFailed: string;
}

export interface MediaOpts {
  kind: "image" | "video" | "audio";
  /** asset URL */
  src: string;
  /** 外部で開く用の絶対パス */
  abs: string;
  name: string;
  labels: MediaLabels;
  /** md の中の埋め込み（ファイル名のバーを出さない・画像は幅合わせ） */
  inline?: boolean;
}

/** checkPdf の結果。pdf = 先頭が `%PDF-`・other = 読めたが PDF でない（空のファイル = 416 も）・unreadable = 読めなかった（reason = `HTTP 403` / 例外の文言） */
export type PdfCheck = { kind: "pdf" } | { kind: "other" } | { kind: "unreadable"; reason: string };

/** 中身が本当に PDF か（先頭 5 バイトが `%PDF-`）。asset protocol は Content-Type を拡張子でなく中身で決める
   （tauri-utils の mime 判定）ので、中身が HTML の `x.pdf` は text/html で返り、iframe に載せると asset://localhost の origin で
   script が走りうる → iframe に載せる前に必ずこれを通す（iframe に載せてよいのは kind === "pdf" のときだけ）。
   Range で先頭だけ読む（Range が効かず 200 で全量が来ても最初の塊で止める）。
   読めなかった（応答が ok でない・fetch や読み出しが throw）ときは「PDF でない」と分ける = 呼び手が別の案内を出す */
export async function checkPdf(url: string, fetchFn: typeof fetch = fetch): Promise<PdfCheck> {
  try {
    const r = await fetchFn(url, { headers: { Range: "bytes=0-4" } });
    // 416 = 範囲外 = 0 バイトのファイル（自前の配信は空のファイルに bytes=0-4 を満たせない）→ 読めたが PDF でない
    if (r.status === 416) return { kind: "other" };
    if (!r.ok) return { kind: "unreadable", reason: `HTTP ${r.status}` };
    let head: Uint8Array;
    const reader = r.body?.getReader();
    if (reader) {
      const parts: Uint8Array[] = [];
      let n = 0;
      while (n < 5) {
        const { value, done } = await reader.read();
        if (done || !value) break;
        parts.push(value);
        n += value.length;
      }
      void reader.cancel().catch(() => {});
      head = new Uint8Array(n);
      let o = 0;
      for (const p of parts) {
        head.set(p, o);
        o += p.length;
      }
    } else head = new Uint8Array(await r.arrayBuffer());
    return head.length >= 5 && String.fromCharCode(...head.slice(0, 5)) === "%PDF-" ? { kind: "pdf" } : { kind: "other" };
  } catch (e) {
    return { kind: "unreadable", reason: (e instanceof Error ? e.message : String(e)) || "error" };
  }
}

function openButton(doc: Document, abs: string, label: string): HTMLButtonElement {
  const b = doc.createElement("button");
  b.type = "button";
  b.className = "wk-media-open";
  b.setAttribute("data-wk-open", abs);
  b.textContent = label;
  return b;
}

export function buildMedia(doc: Document, o: MediaOpts): HTMLElement {
  const box = doc.createElement(o.inline && o.kind === "image" ? "span" : "div");
  box.className = `wk-media wk-media-${o.kind}${o.inline ? " wk-media-inline" : ""}`;
  if (o.kind === "image") {
    const img = doc.createElement("img");
    img.className = "wk-media-img";
    img.alt = o.name;
    img.setAttribute("loading", "lazy");
    img.src = o.src;
    box.append(img);
    return box;
  }
  const el = doc.createElement(o.kind);
  el.className = `wk-media-el`;
  el.controls = true;
  el.preload = "metadata";
  el.src = o.src;
  const bar = doc.createElement("div");
  bar.className = "wk-media-bar";
  const name = doc.createElement("span");
  name.className = "wk-media-name";
  name.textContent = o.name;
  bar.append(name, openButton(doc, o.abs, o.labels.openExternal));
  el.addEventListener("error", () => {
    const fail = doc.createElement("div");
    fail.className = "wk-media-fail";
    fail.textContent = o.labels.playFailed + " ";
    fail.append(openButton(doc, o.abs, o.labels.openExternal));
    el.replaceWith(fail);
  });
  box.append(el, bar);
  return box;
}
