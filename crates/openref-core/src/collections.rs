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

/// 同じ親の下に大小無視の同名があるか（except = 改名時の自分）。soft-delete 済みは数えない（見えないので）。
/// 移行（import::apply の表示名の衝突検査）も使う
pub(crate) fn sibling_taken(conn: &rusqlite::Connection, name: &str, parent: Option<i64>, except: Option<i64>) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM collections
                       WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2 AND deleted_at IS NULL AND id IS NOT ?3)",
        rusqlite::params![name, parent, except],
        |r| r.get(0),
    )?)
}

/// 同じ親の下で大小無視の同名のフォルダ → (id, key, DB 上の名前)（soft-delete 済みは見ない・複数あれば id の小さい 1 行）。
/// パスで辿る経路（`walk`）と移行（import::apply の合流先）の照合の基準
pub(crate) fn child(conn: &rusqlite::Connection, parent: Option<i64>, name: &str) -> Result<Option<(i64, String, String)>> {
    Ok(conn
        .query_row(
            "SELECT id, key, name FROM collections
             WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2 AND deleted_at IS NULL ORDER BY id LIMIT 1",
            rusqlite::params![name, parent],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// `child` の id だけ
pub(crate) fn child_id(conn: &rusqlite::Connection, parent: Option<i64>, name: &str) -> Result<Option<i64>> {
    Ok(child(conn, parent, name)?.map(|(id, _, _)| id))
}

/// フォルダの行を 1 つ作る（新しい key）。戻り値 = (行 id, key)
pub(crate) fn insert_child_keyed(conn: &rusqlite::Connection, parent: Option<i64>, name: &str) -> Result<(i64, String)> {
    let key = keys::new_key();
    conn.execute(
        "INSERT INTO collections (key, name, parent_id) VALUES (?1, ?2, ?3)",
        rusqlite::params![key, name, parent],
    )?;
    Ok((conn.last_insert_rowid(), key))
}

/// `insert_child_keyed` の id だけ
pub(crate) fn insert_child(conn: &rusqlite::Connection, parent: Option<i64>, name: &str) -> Result<i64> {
    Ok(insert_child_keyed(conn, parent, name)?.0)
}

/// パス（'親/子'）の段: `/` で区切り、前後の空白を落とし、空の段は飛ばす
pub(crate) fn segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').map(str::trim).filter(|s| !s.is_empty())
}

/// `walk` の行き先
pub(crate) enum Walked<'p> {
    /// 辿り着いたフォルダ（段が無ければ `from` のまま）と、作った段の名前（create = false なら空）
    At(Option<i64>, Vec<String>),
    /// create = false で見つからなかった最初の段
    Missing(&'p str),
}

/// `from`（None = root）の下をパスの段ごとに `child` で辿る（大小無視・soft-delete 済みは見ない・同名が複数なら id の小さい 1 行）。
/// create なら無い段を作る。パスで辿る経路（`edit::ensure_collection_path`・MCP のフォルダ指定と create_collection）の唯一の照合
pub(crate) fn walk<'p>(conn: &rusqlite::Connection, from: Option<i64>, path: &'p str, create: bool) -> Result<Walked<'p>> {
    let mut cur = from;
    let mut created = Vec::new();
    for seg in segments(path) {
        cur = Some(match child_id(conn, cur, seg)? {
            Some(id) => id,
            None if create => {
                created.push(seg.to_string());
                insert_child(conn, cur, seg)?
            }
            None => return Ok(Walked::Missing(seg)),
        });
    }
    Ok(Walked::At(cur, created))
}

/// フォルダをパスで確保する（冪等）。`from` の下に、途中の段も無ければ作る。戻り値 = (最後の段の id, 作った段の名前の列)。
/// 段が無ければ `from`（root なら「フォルダ名が空です」）
pub(crate) fn ensure_path(conn: &rusqlite::Connection, from: Option<i64>, path: &str) -> Result<(i64, Vec<String>)> {
    match walk(conn, from, path, true)? {
        Walked::At(Some(id), created) => Ok((id, created)),
        Walked::At(None, _) => Err(CoreError::BadInput("フォルダ名が空です".into())),
        Walked::Missing(_) => unreachable!("create = true では段が欠けない"),
    }
}

/// id → 実表記のパス（'親/子'）。親を再帰 CTE で辿る（soft-delete 済みの祖先も名前を出す・257 段を超える鎖 = 万一の循環は 257 段で打ち切る）。
/// 無い id は QueryReturnedNoRows
pub(crate) fn path_of(conn: &rusqlite::Connection, id: i64) -> Result<String> {
    let mut stmt = conn.prepare_cached(
        "WITH RECURSIVE up(name, parent_id, depth) AS (
           SELECT name, parent_id, 0 FROM collections WHERE id = ?1
           UNION ALL
           SELECT c.name, c.parent_id, up.depth + 1 FROM collections c JOIN up ON c.id = up.parent_id WHERE up.depth < 256
         )
         SELECT name FROM up ORDER BY depth DESC",
    )?;
    let parts: Vec<String> = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    if parts.is_empty() {
        return Err(rusqlite::Error::QueryReturnedNoRows.into());
    }
    Ok(parts.join("/"))
}

/// New folder / Create subfolder。parent = None なら root。同じ親の下に大小無視の同名があれば拒否
/// （ensure_collection_path と違って既存を返さない = モーダルで「同名あり」を出すため）
pub fn create_collection(db: &Db, name: &str, parent: Option<i64>) -> Result<i64> {
    let name = validate_name(name)?;
    if let Some(p) = parent {
        parent_of(db, p)?; // 親が無い・見えないなら NotFound
    }
    if sibling_taken(&db.conn, name, parent, None)? {
        return Err(CoreError::BadInput(format!("同じ階層にフォルダ「{name}」が既にあります")));
    }
    insert_child(&db.conn, parent, name)
}

/// Rename folder。自分と同名（大小だけ違う改名を含む）は可、兄弟と同名は拒否
pub fn rename_collection(db: &Db, id: i64, name: &str) -> Result<()> {
    let name = validate_name(name)?;
    let parent = parent_of(db, id)?;
    if sibling_taken(&db.conn, name, parent, Some(id))? {
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

    // ---- パスの照合（walk / ensure_path / path_of）----

    /// 書き換え前の `edit::ensure_collection_path` の写し（walk との同値の基準）
    fn old_ensure(db: &Db, path: &str) -> Result<(i64, Vec<String>)> {
        let mut parent: Option<i64> = None;
        let mut created = Vec::new();
        for seg in path.split('/').map(str::trim).filter(|s| !s.is_empty()) {
            let id = match child_id(&db.conn, parent, seg)? {
                Some(id) => id,
                None => {
                    let id = insert_child(&db.conn, parent, seg)?;
                    created.push(seg.to_string());
                    id
                }
            };
            parent = Some(id);
        }
        parent.map(|id| (id, created)).ok_or_else(|| CoreError::BadInput("フォルダ名が空です".into()))
    }

    /// 書き換え前の `commands::collection_path` の写し（path_of との同値の基準）
    fn old_path(db: &Db, id: i64) -> Result<String> {
        let mut parts: Vec<String> = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let (n, p): (String, Option<i64>) = db.conn.query_row(
                "SELECT name, parent_id FROM collections WHERE id = ?1",
                [c],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            parts.push(n);
            cur = p;
            if parts.len() > 256 {
                break;
            }
        }
        parts.reverse();
        Ok(parts.join("/"))
    }

    fn dump(db: &Db) -> Vec<(i64, String, Option<i64>)> {
        let mut st = db.conn.prepare("SELECT id, name, parent_id FROM collections ORDER BY id").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect()
    }

    fn walk_to(db: &Db, from: Option<i64>, path: &str) -> std::result::Result<Option<i64>, String> {
        match walk(&db.conn, from, path, false).unwrap() {
            Walked::At(id, created) => {
                assert!(created.is_empty(), "create = false は作らない");
                Ok(id)
            }
            Walked::Missing(seg) => Err(seg.to_string()),
        }
    }

    #[test]
    fn walk_rules() {
        let db = Db::open_in_memory().unwrap();
        // 深い入れ子・存在しない段の作成
        let (e, created) = ensure_path(&db.conn, None, "a/b/c/d/e/f/g/h").unwrap();
        assert_eq!(created, ["a", "b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(path_of(&db.conn, e).unwrap(), "a/b/c/d/e/f/g/h");
        // 冪等・大小違い・前後空白・空の段（`//`・先頭末尾の `/`・空白だけの段）
        assert_eq!(ensure_path(&db.conn, None, " / A/ b //C/d/ E /f/G/h/ ").unwrap(), (e, vec![]));
        // 途中から先だけ作る。作った段は入力の表記（前後の空白は落とす）
        let (z, created) = ensure_path(&db.conn, None, "A/B/ New One /Z").unwrap();
        assert_eq!(created, ["New One", "Z"]);
        assert_eq!(path_of(&db.conn, z).unwrap(), "a/b/New One/Z", "パスは DB 上の表記");
        // from の下から辿る
        let b = walk_to(&db, None, "a/b").unwrap().unwrap();
        assert_eq!(walk_to(&db, Some(b), "c/D/e"), walk_to(&db, None, "a/b/c/d/e"));
        let (y, created) = ensure_path(&db.conn, Some(b), "c/y").unwrap();
        assert_eq!(created, ["y"]);
        assert_eq!(path_of(&db.conn, y).unwrap(), "a/b/c/y");
        // 段が無い: from を返す（root なら空の誤り）
        assert_eq!(ensure_path(&db.conn, Some(b), " / ").unwrap(), (b, vec![]));
        assert!(matches!(ensure_path(&db.conn, None, " //  "), Err(CoreError::BadInput(_))));
        assert_eq!(walk_to(&db, None, "/"), Ok(None));
        // create = false: 最初に欠けた段（前後の空白を落とした表記）を返し、何も作らない
        let before = dump(&db);
        assert_eq!(walk_to(&db, None, "a/b/ nope /deeper"), Err("nope".into()));
        assert_eq!(walk_to(&db, Some(z), "x"), Err("x".into()));
        assert_eq!(dump(&db), before);
        // ASCII だけ大小無視（DB の NOCASE）: Ä と ä は別の段
        let (ae, _) = ensure_path(&db.conn, None, "Äx").unwrap();
        assert_eq!(walk_to(&db, None, "äx"), Err("äx".into()));
        assert_eq!(walk_to(&db, None, "ÄX"), Ok(Some(ae)));
        // soft-delete 済みは辿らない（同名を作り直す）・そのパスは名前を出し続ける
        let c = walk_to(&db, None, "a/b/c").unwrap().unwrap();
        db.conn.execute("UPDATE collections SET deleted_at = '2026-01-01' WHERE id = ?1", [c]).unwrap();
        assert_eq!(walk_to(&db, None, "a/b/c/d"), Err("c".into()));
        let (c2, created) = ensure_path(&db.conn, None, "a/b/c").unwrap();
        assert!(c2 != c && created == ["c"]);
        assert_eq!(path_of(&db.conn, e).unwrap(), "a/b/c/d/e/f/g/h", "soft-delete 済みの祖先も名前を出す");
        // 旧データの大小違いの兄弟: id の小さい方に着地する
        db.conn.execute("INSERT INTO collections (key, name) VALUES ('DUPDUP01', 'A')", []).unwrap();
        let a = walk_to(&db, None, "a").unwrap().unwrap();
        assert_eq!(walk_to(&db, None, "A"), Ok(Some(a)));
        assert!(a < db.conn.last_insert_rowid());
        // 無い id
        assert!(path_of(&db.conn, 9999).is_err());
    }

    /// 旧実装の写しとの同値: 同じ入力列を 2 つの DB に流し、戻り値・作った行・全行のパスが一致する
    #[test]
    fn walk_and_path_of_match_old_implementation() {
        let old = Db::open_in_memory().unwrap();
        let new = Db::open_in_memory().unwrap();
        let names = ["Liver", "liver", " LIVER ", "MRI", "mri", "", "  ", "Äa", "äa", "早期膵癌", "画像 所見"];
        let mut seed: u64 = 0x5eed;
        let mut next = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n
        };
        for _ in 0..400 {
            let depth = 1 + next(5);
            let path: Vec<&str> = (0..depth).map(|_| names[next(names.len())]).collect();
            let path = path.join("/");
            let o = old_ensure(&old, &path).map_err(|e| e.to_string());
            let n = ensure_path(&new.conn, None, &path).map_err(|e| e.to_string());
            assert_eq!(o, n, "{path:?}");
        }
        assert_eq!(dump(&old), dump(&new));
        // 旧データの形も混ぜる: 大小違いの兄弟・soft-delete 済みの祖先・深さ 300 の鎖・循環
        for db in [&old, &new] {
            db.conn.execute("INSERT INTO collections (key, name) VALUES ('LEGACY01', 'liver')", []).unwrap();
            db.conn.execute("UPDATE collections SET deleted_at = '2026-01-01' WHERE id IN (3, 7)", []).unwrap();
            let mut parent: Option<i64> = None;
            for i in 0..300 {
                db.conn
                    .execute(
                        "INSERT INTO collections (key, name, parent_id) VALUES (?1, ?2, ?3)",
                        rusqlite::params![format!("CHAIN{i:03}"), format!("n{i}"), parent],
                    )
                    .unwrap();
                parent = Some(db.conn.last_insert_rowid());
            }
            db.conn.execute_batch(
                "INSERT INTO collections (key, name) VALUES ('CYCLE001', 'p');
                 INSERT INTO collections (key, name, parent_id) VALUES ('CYCLE002', 'q', last_insert_rowid());
                 UPDATE collections SET parent_id = last_insert_rowid() WHERE key = 'CYCLE001';",
            ).unwrap();
        }
        let rows = dump(&new);
        assert_eq!(rows, dump(&old));
        for (id, _, _) in &rows {
            assert_eq!(old_path(&old, *id).unwrap(), path_of(&new.conn, *id).unwrap(), "id {id}");
        }
        let deepest = rows.iter().find(|r| r.1 == "n299").unwrap().0;
        assert_eq!(path_of(&new.conn, deepest).unwrap().split('/').count(), 257, "257 段で打ち切る");
        // soft-delete 済みでない行は search::collection_paths（行の一覧から作る方）とも一致（祖先が全部見えるもの）
        let listed = search::collection_paths(&search::collections(&new).unwrap());
        let parent: std::collections::HashMap<i64, Option<i64>> = rows.iter().map(|r| (r.0, r.2)).collect();
        let visible = |id: i64| {
            let mut cur = Some(id);
            for _ in 0..400 {
                match cur {
                    Some(3 | 7) => return false,
                    Some(c) => cur = parent[&c],
                    None => return true,
                }
            }
            true // 循環
        };
        let mut compared = 0;
        for (id, _, _) in rows.iter().filter(|r| visible(r.0)) {
            assert_eq!(listed[id], path_of(&new.conn, *id).unwrap(), "id {id}");
            compared += 1;
        }
        assert!(compared > 300, "鎖と循環を含めて比べた: {compared}");
    }
}
