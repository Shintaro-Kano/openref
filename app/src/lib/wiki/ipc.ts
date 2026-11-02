// Wiki の Tauri command の呼び口（エラーの契約）。wiki_gui.rs の wiki_run は CoreError::Wiki を `{"code", "message"}` の
// JSON 文字列で返す（code の一覧 = core の wiki::codes）。画面は code で分岐し、core の文言（改名・英訳で変わる）を照合しない。
// invoke はここを通す: code 付きのエラーは WikiError（String(e) = message = 既存の toast・表示はそのまま）、他は受けたまま投げ直す

import { invoke as tauriInvoke, type InvokeArgs } from "@tauri-apps/api/core";

/** core の wiki::codes のうち画面が分岐に使う物 */
export const WIKI_CODES = { rebuilding: "rebuilding", badQuery: "bad_query" } as const;

export class WikiError extends Error {
  code: string;
  constructor(code: string, message: string) {
    super(message);
    this.name = "WikiError";
    this.code = code;
  }
  /** 表示は文だけ（`String(e)` で toast・欄に出す所を変えない） */
  override toString() {
    return this.message;
  }
}

/** エラーを {code, message} に。code の無い物（素の文・JS の例外）は code = null・message = String(e) */
export function parseWikiError(e: unknown): { code: string | null; message: string } {
  if (e instanceof WikiError) return { code: e.code, message: e.message };
  if (typeof e === "string" && e.startsWith("{")) {
    try {
      const v = JSON.parse(e);
      if (typeof v?.code === "string" && typeof v?.message === "string") return { code: v.code, message: v.message };
    } catch {
      /* 素の文 */
    }
  }
  return { code: null, message: String(e) };
}

export const errorCode = (e: unknown) => parseWikiError(e).code;

export async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (e) {
    const { code, message } = parseWikiError(e);
    throw code === null ? e : new WikiError(code, message);
  }
}
