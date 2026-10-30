//! フォルダ（Paperpile の folders 相当）の作成・改名・削除・検索置換。
//! `collections.parent_id` による任意段の木。名前は**同じ親の下で大小無視一意**（DB の COLLATE NOCASE と同じ =
//! ASCII だけ大小無視）、`/` は不可（MCP / CLI / `edit::ensure_collection_path` のパス区切り）。
//! 削除は即時でサブツリーごと（論文は消えず所属が外れるだけ。`collections.deleted_at` は読むだけで書かない）。
//! ツールバーの「作成して追加」（パス可・冪等）は従来どおり `edit::ensure_collection_path`。

use crate::tags::RenameOutcome;
use crate::{keys, CoreError, Db, Result};
use rusqlite::OptionalExtension;

/// フォルダ名の検査: 前後の空白を落とし、空と `/` を含むものは拒否する
pub fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CoreError::BadInput("フォルダ名が空です".into()));
    }
    if name.contains('/') {
        return Err(CoreError::BadInput(format!(
            "フォルダ名に「/」は使えません（フォルダの区切りに使うため）: {name}"
        )));
    }
    Ok(name)
}

/// 見えているフォルダ（soft-delete されていない）の親。無ければ NotFound
fn parent_of(db: &Db, id: i64) -> Result<Option<i64>> {
    db.conn
        .query_row(
            "SELECT parent_id FROM collections WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("collection {id}")))
}

/// 同じ親の下に大小無視の同名があるか（except = 改名時の自分）。soft-delete 済みは数えない（見えないので）
fn sibling_taken(db: &Db, name: &str, parent: Option<i64>, except: Option<i64>) -> Result<bool> {
    Ok(db.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM collections
                       WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2 AND deleted_at IS NULL AND id IS NOT ?3)",
        rusqlite::params![name, parent, except],
        |r| r.get(0),
    )?)
}

/// 同じ親の下で大小無視の同名のフォルダ（soft-delete 済みは見ない・複数あれば最初の 1 行）。
/// パスで辿る経路（`edit::ensure_collection_path`・`commands.rs` のフォルダ解決）の照合の基準
pub(crate) fn child_id(conn: &rusqlite::Connection, parent: Option<i64>, name: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2 AND deleted_at IS NULL",
            rusqlite::params![name, parent],
            |r| r.get(0),
        )
        .optional()?)
}

/// フォルダの行を 1 つ作る（新しい key）。戻り値 = 行 id
pub(crate) fn insert_child(conn: &rusqlite::Connection, parent: Option<i64>, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO collections (key, name, parent_id) VALUES (?1, ?2, ?3)",
        rusqlite::params![keys::new_key(), name, parent],
    )?;
    Ok(conn.last_insert_rowid())
}

/// New folder / Create subfolder。parent = None なら root。同じ親の下に大小無視の同名があれば拒否
/// （ensure_collection_path と違って既存を返さない = モーダルで「同名あり」を出すため）
pub fn create_collection(db: &Db, name: &str, parent: Option<i64>) -> Result<i64> {
    let name = validate_name(name)?;
    if let Some(p) = parent {
        parent_of(db, p)?; // 親が無い・見えないなら NotFound
    }
    if sibling_taken(db, name, parent, None)? {
        return Err(CoreError::BadInput(format!("同じ階層にフォルダ「{name}」が既にあります")));
    }
    insert_child(&db.conn, parent, name)
}

/// Rename folder。自分と同名（大小だけ違う改名を含む）は可、兄弟と同名は拒否
pub fn rename_collection(db: &Db, id: i64, name: &str) -> Result<()> {
    let name = validate_name(name)?;
    let parent = parent_of(db, id)?;
    if sibling_taken(db, name, parent, Some(id))? {
        return Err(CoreError::BadInput(format!("同じ階層にフォルダ「{name}」が既にあります")));
    }
    let n = db
        .conn
        .execute("UPDATE collections SET name = ?1 WHERE id = ?2", rusqlite::params![name, id])?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("collection {id}")));
    }
    Ok(())
}

/// フォルダをサブフォルダごと削除する（即時・ゴミ箱なし）。論文は消えない（collection_items は FK CASCADE で外れる）。
/// `parent_id` の FK には ON DELETE が無く `foreign_keys = ON`（db.rs）なので、親だけ先に消すと FK 違反になる。
/// 子孫 id を再帰 CTE で集めて**単一の DELETE 文**で消す（FK の検査は文の終わり = 文の中の順序に依存しない）。
/// CTE に `deleted_at IS NULL` を付けない（soft-delete 済みの子が残ると FK 違反）。UNION = 万一の循環でも止まる。
/// 戻り値 = 消したフォルダ数（自分 + 子孫。toast 用）。存在しない id は NotFound（`tags::delete_tag` と同じ）
pub fn delete_collection(db: &Db, id: i64) -> Result<usize> {
    let exists: bool =
        db.conn
            .query_row("SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)", [id], |r| r.get(0))?;
    if !exists {
        return Err(CoreError::NotFound(format!("collection {id}")));
    }
    let n = db.conn.execute(
        "WITH RECURSIVE sub(id) AS (
           SELECT ?1
           UNION
           SELECT c.id FROM collections c JOIN sub ON c.parent_id = sub.id
         )
         DELETE FROM collections WHERE id IN (SELECT id FROM sub)",
        [id],
    )?;
    Ok(n)
}

/// フォルダ名の検索置換（Manage folders → Find and replace。判定と書込は `tags::find_replace` をラベルと共有）。
/// 大小無視で `find` を含む名前の該当部分をすべて `replace` に置き換える。同名の判定は**兄弟の中だけ**
/// （別の親の下の同名は衝突しない）。status = "renamed"（dry_run では予定）/ "conflict"（兄弟に同名）/
/// "empty"（置換後が空）/ "invalid"（置換後が `/` を含む）。conflict / empty / invalid は飛ばす。dry_run は書かない
pub fn find_replace_collections(db: &Db, find: &str, replace: &str, dry_run: bool) -> Result<Vec<RenameOutcome>> {
    // 同名を数える範囲 = 同じ親（parent_id）
    crate::tags::find_replace(db, "collections", find, replace, dry_run, true, || {
        let mut stmt = db.conn.prepare(
            "SELECT id, name, parent_id FROM collections WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<i64>>(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{edit, search};

    fn seed_items(db: &Db, keys: &[&str]) {
        for k in keys {
            db.conn
                .execute(
                    "INSERT INTO items (key, csl) VALUES (?1, '{\"title\":\"x\",\"type\":\"article-journal\"}')",
                    [k],
                )
                .unwrap();
        }
    }

    fn count(db: &Db, sql: &str) -> i64 {
        db.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn name_of(db: &Db, id: i64) -> String {
        db.conn.query_row("SELECT name FROM collections WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn create_enforces_sibling_unique_and_slash() {
        let db = Db::open_in_memory().unwrap();
        let liver = create_collection(&db, "  Liver ", None).unwrap();
        assert_eq!(name_of(&db, liver), "Liver", "前後の空白は落とす");
        // 兄弟の同名（大小違い）は拒否
        assert!(matches!(create_collection(&db, "liver", None), Err(CoreError::BadInput(_))));
        // 別の親の下なら同名可
        let sub = create_collection(&db, "liver", Some(liver)).unwrap();
        assert!(create_collection(&db, "LIVER", Some(liver)).is_err());
        let _grand = create_collection(&db, "Liver", Some(sub)).unwrap();
        // `/`・空は拒否
        assert!(matches!(create_collection(&db, "a/b", None), Err(CoreError::BadInput(_))));
        assert!(matches!(create_collection(&db, "   ", None), Err(CoreError::BadInput(_))));
        // 存在しない親 / soft-delete 済みの親は NotFound
        assert!(matches!(create_collection(&db, "x", Some(999)), Err(CoreError::NotFound(_))));
        let gone = create_collection(&db, "gone", None).unwrap();
        db.conn.execute("UPDATE collections SET deleted_at = '2026-01-01' WHERE id = ?1", [gone]).unwrap();
        assert!(matches!(create_collection(&db, "x", Some(gone)), Err(CoreError::NotFound(_))));
        // soft-delete 済みの同名は見えないので塞がない（ensure_collection_path と同じ基準）
        create_collection(&db, "GONE", None).unwrap();
        // DB は ASCII だけ大小無視: "Äab" と "äab" は別名として共存できる
        create_collection(&db, "Äab", None).unwrap();
        create_collection(&db, "äab", None).unwrap();
    }

    #[test]
    fn rename_rules() {
        let db = Db::open_in_memory().unwrap();
        let a = create_collection(&db, "Liver", None).unwrap();
        let b = create_collection(&db, "Pancreas", None).unwrap();
        let child = create_collection(&db, "Pancreas", Some(a)).unwrap();
        // 自分と同名（大小だけ違う）は可
        rename_collection(&db, a, "liver").unwrap();
        assert_eq!(name_of(&db, a), "liver");
        // 兄弟と同名は拒否・別の親の同名は可
        assert!(matches!(rename_collection(&db, a, "PANCREAS"), Err(CoreError::BadInput(_))));
        rename_collection(&db, child, "pancreas").unwrap();
        assert!(rename_collection(&db, b, "a/b").is_err());
        assert!(rename_collection(&db, b, " ").is_err());
        assert!(matches!(rename_collection(&db, 999, "x"), Err(CoreError::NotFound(_))));
        assert_eq!(name_of(&db, b), "Pancreas", "失敗した改名は書かない");
    }

    #[test]
    fn delete_subtree_keeps_items_and_other_folders() {
        let db = Db::open_in_memory().unwrap();
        seed_items(&db, &["A", "B", "C", "D"]);
        let root = create_collection(&db, "Abdomen", None).unwrap();
        let mid = create_collection(&db, "Liver", Some(root)).unwrap();
        let leaf = create_collection(&db, "MRI", Some(mid)).unwrap();
        let other = create_collection(&db, "Chest", None).unwrap();
        let other_child = create_collection(&db, "Liver", Some(other)).unwrap();
        for (k, c) in [("A", root), ("B", mid), ("C", leaf), ("A", leaf), ("D", other), ("B", other_child)] {
            edit::add_to_collection(&db, k, c).unwrap();
        }
        assert_eq!(delete_collection(&db, root).unwrap(), 3, "自分 + 子 + 孫");
        let ids: Vec<i64> = {
            let mut st = db.conn.prepare("SELECT id FROM collections ORDER BY id").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(ids, vec![other, other_child], "別の木は不変");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM items"), 4, "論文は消えない");
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM collection_items"),
            2,
            "消したフォルダの所属だけ CASCADE で外れる"
        );
        assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM collection_items WHERE collection_id IN ({other}, {other_child})")), 2);
        // 葉だけの削除・存在しない id
        assert_eq!(delete_collection(&db, other_child).unwrap(), 1);
        assert!(matches!(delete_collection(&db, root), Err(CoreError::NotFound(_))));
        assert!(matches!(delete_collection(&db, 999), Err(CoreError::NotFound(_))));
    }

    #[test]
    fn delete_with_soft_deleted_descendant() {
        let db = Db::open_in_memory().unwrap();
        seed_items(&db, &["A"]);
        let root = create_collection(&db, "root", None).unwrap();
        let mid = create_collection(&db, "mid", Some(root)).unwrap();
        let hidden = create_collection(&db, "hidden", Some(mid)).unwrap();
        let _hidden_child = create_collection(&db, "hidden child", Some(hidden)).unwrap();
        edit::add_to_collection(&db, "A", hidden).unwrap();
        // soft-delete 済みの子（読むだけの列。過去のデータ・外部ツールが作りうる）が混ざっても FK 違反にならない
        db.conn.execute("UPDATE collections SET deleted_at = '2026-01-01' WHERE id = ?1", [hidden]).unwrap();
        assert_eq!(delete_collection(&db, root).unwrap(), 4);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM collections"), 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM collection_items"), 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM items"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check"), 0);
    }

    #[test]
    fn find_replace_is_sibling_scoped() {
        let db = Db::open_in_memory().unwrap();
        let liver_mri = create_collection(&db, "Liver MRI", None).unwrap();
        let liver_ct = create_collection(&db, "liver CT", None).unwrap();
        let _hep_ct = create_collection(&db, "Hepatic CT", None).unwrap();
        let parent = create_collection(&db, "Teaching", None).unwrap();
        // 別の親の下の "liver CT" → "Hepatic CT" は root の Hepatic CT と衝突しない
        let nested = create_collection(&db, "LIVER CT", Some(parent)).unwrap();
        let slash = create_collection(&db, "liver-a", None).unwrap();
        let get = |v: &[RenameOutcome], id: i64| v.iter().find(|o| o.id == id).cloned().unwrap();

        // 予定だけ（書かない）
        let plan = find_replace_collections(&db, "LIVER", "Hepatic", true).unwrap();
        assert_eq!(plan.len(), 4);
        assert_eq!(get(&plan, liver_mri).status, "renamed");
        assert_eq!(get(&plan, liver_mri).new, "Hepatic MRI");
        assert_eq!(get(&plan, liver_ct).status, "conflict", "兄弟の Hepatic CT と同名 → 飛ばす");
        assert_eq!(get(&plan, nested).status, "renamed", "別の親の同名は衝突しない");
        assert_eq!(name_of(&db, liver_mri), "Liver MRI", "dry_run は書かない");
        // `/` を含む結果は invalid
        let p2 = find_replace_collections(&db, "-", "/", true).unwrap();
        assert_eq!(get(&p2, slash).status, "invalid");
        // 置換後が空・検索語が空
        assert_eq!(find_replace_collections(&db, "liver-a", " ", true).unwrap()[0].status, "empty");
        assert!(find_replace_collections(&db, "", "x", true).is_err());

        // 実行: renamed だけ書き、conflict / invalid は元のまま
        let done = find_replace_collections(&db, "liver", "Hepatic", false).unwrap();
        assert_eq!(done.iter().filter(|o| o.status == "renamed").count(), 3);
        assert_eq!(name_of(&db, liver_mri), "Hepatic MRI");
        assert_eq!(name_of(&db, liver_ct), "liver CT");
        assert_eq!(name_of(&db, nested), "Hepatic CT");
        assert_eq!(name_of(&db, slash), "Hepatic-a");
        // 正規表現の特殊文字は文字どおり
        let dot = create_collection(&db, "a.b", None).unwrap();
        let d = find_replace_collections(&db, ".", "-", false).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(name_of(&db, dot), "a-b");
    }

    /// サイドバーの件数（search::collections）はゴミ箱の論文を数えない（ラベルと同じ）・直下だけを数える
    #[test]
    fn counts_exclude_trash_and_are_direct_only() {
        let db = Db::open_in_memory().unwrap();
        seed_items(&db, &["A", "B", "C"]);
        let parent = create_collection(&db, "parent", None).unwrap();
        let child = create_collection(&db, "child", Some(parent)).unwrap();
        edit::add_to_collection(&db, "A", parent).unwrap();
        edit::add_to_collection(&db, "B", parent).unwrap();
        edit::add_to_collection(&db, "C", child).unwrap();
        edit::trash(&db, "B").unwrap();
        let rows = search::collections(&db).unwrap();
        let n = |id: i64| rows.iter().find(|r| r.id == id).unwrap().n_items;
        assert_eq!(n(parent), 1, "ゴミ箱の B は数えない・子の C も数えない（直下だけ）");
        assert_eq!(n(child), 1);
    }
}
