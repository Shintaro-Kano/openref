/* フォルダ・ラベルの名前の規則（作成・改名の前段検査）。core の collections.rs（validate_name・sibling_taken）・
   tags.rs（trim・UNIQUE COLLATE NOCASE）と同じ規則を写す。
   依存は sort.ts だけ = node で直接テストする（app/src/lib/folders.test.mjs）。型を消すだけで動く TS（enum 等を使わない）であること */
import { nocase } from "./sort.ts";

interface Folder {
  id: number;
  name: string;
  parentId: number | null;
}

/** 名前が同じか: 前後の空白を落として ASCII だけ大小無視（core の trim + COLLATE NOCASE と同じ） */
export const sameName = (a: string, b: string) => nocase(a.trim()) === nocase(b.trim());

/** フォルダ名を確定できない理由（確定できるなら null）。全作成経路（サイドバーの New folder / Create subfolder・
    Manage folders・ツールバーの「作成して追加」）と改名が使う。core collections::create_collection と同じ規則:
    前後の空白を落とす・空（"empty"）と `/` を含む名前（"slash" = MCP / CLI のパス区切り）は不可・
    同じ親の下で大小無視一意（"taken"。別の親の下の同名は可）。except = 改名時の自分 */
export function folderNameProblem(
  collections: readonly Folder[],
  name: string,
  parentId: number | null,
  except?: number
): "empty" | "slash" | "taken" | null {
  const n = name.trim();
  if (!n) return "empty";
  if (n.includes("/")) return "slash";
  if (collections.some((c) => c.id !== except && c.parentId === parentId && sameName(c.name, n))) return "taken";
  return null;
}

/** ラベル名の重複（全ラベルの中で一意 = DB の tags.name UNIQUE COLLATE NOCASE と同じ判定 = ASCII だけ大小無視。
    「Äab」と「äab」は別名）。空は重複にしない。except = 改名時の自分 */
export function labelTaken(tags: readonly { id: number; name: string }[], name: string, except?: number): boolean {
  return !!name.trim() && tags.some((t) => t.id !== except && sameName(t.name, name));
}
