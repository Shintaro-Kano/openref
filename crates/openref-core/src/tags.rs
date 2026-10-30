//! タグ（Paperpile LABELS 相当）の CRUD。
//! フォルダが 1 論文 1 所属の分類軸なのに対し、タグは多対多の横断軸。

use crate::{CoreError, Db, Result};
use rusqlite::{Connection, OptionalExtension};

/// ラベルの色 = Paperpile の 25 スタイル（背景 + 文字色の組。5×5 グリッドの並び順）。
/// DB の tags.color にはこの id を入れる。見た目（背景・文字・枠の色とダークモード）は UI 側
/// `app/src/lib/labels.ts` の PALETTE が持つ（id の並びは本表と一致させる = テスト `palette_ids_match_ui`）。
/// 旧来の `#rrggbb`（以前の 8 色パレット・外部から入った値）も表示できるよう受け付ける
pub const LABEL_COLORS: [&str; 25] = [
    "gray", "sky", "periwinkle", "lilac", "blush", //
    "rose", "slate", "blue", "navy", "violet", //
    "plum", "red", "peach", "apricot", "sand", //
    "cream", "lime", "mint", "orange", "brown", //
    "mustard", "olive", "green", "forest", "steel",
];

/// 受け付ける色: パレット id か `#rrggbb`
pub fn valid_color(c: &str) -> bool {
    LABEL_COLORS.contains(&c)
        || (c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn check_color(color: Option<&str>) -> Result<()> {
    match color {
        Some(c) if !valid_color(c) => Err(CoreError::BadInput(format!("ラベルの色が不正です: {c}"))),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRow {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub n_items: i64,
}

#[derive(Debug, Clone, Copy)]
pub enum TagSort {
    Alphabetical,
    Count,
}

pub fn list_tags(db: &Db, sort: TagSort) -> Result<Vec<TagRow>> {
    // 名前順は名前だけで決める（sort_order を先に効かせると「Alphabetically」が作成順になってしまう）
    let order = match sort {
        TagSort::Alphabetical => "t.name COLLATE NOCASE, t.id",
        TagSort::Count => "n_items DESC, t.name COLLATE NOCASE, t.id",
    };
    let sql = format!(
        "SELECT t.id, t.name, t.color,
                (SELECT COUNT(*) FROM item_tags it
                 JOIN items i ON i.id = it.item_id
                 WHERE it.tag_id = t.id AND i.deleted_at IS NULL) AS n_items
         FROM tags t ORDER BY {order}"
    );
    let mut stmt = db.conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(TagRow {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
            n_items: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// ラベル名の検査: 前後の空白を落とし、空は拒否する
fn tag_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CoreError::BadInput("タグ名が空です".into()));
    }
    Ok(name)
}

/// 名前（大小無視 = 列の COLLATE NOCASE）でラベルを引く → (id, DB 上の名前)。名前で引く問い合わせはここだけ
pub fn tag_by_name(conn: &Connection, name: &str) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row("SELECT id, name FROM tags WHERE name = ?1 COLLATE NOCASE", [name], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?)
}

/// ラベルを作る唯一の経路: 名前のラベルが無ければ color で作り、あれば既存の id を返す（色は変えない）。
/// 検査はしない（name は trim 済みで空でないもの・color は検査済みを渡す）。Connection で受けるのは移行（import::apply の
/// トランザクション）から呼ぶため。tags.sort_order（migration v3）は書かない = 既定の 0（並べ替えの UI は無く、どこからも読まない）
pub(crate) fn ensure_tag_conn(conn: &Connection, name: &str, color_if_new: Option<&str>) -> Result<i64> {
    // 別の接続（app・connectd・他のセッションの MCP）が同じ名前を同時に作っても UNIQUE で落ちないよう、衝突は黙って既存へ合流する
    // （旧 edit::add_tag の INSERT OR IGNORE と同じ。既存の色は変えない）
    conn.execute(
        "INSERT INTO tags (name, color) VALUES (?1, ?2) ON CONFLICT(name) DO NOTHING",
        rusqlite::params![name, color_if_new],
    )?;
    tag_by_name(conn, name)?
        .map(|(id, _)| id)
        .ok_or_else(|| CoreError::NotFound(format!("ラベル '{name}'（作成直後に見つからない）")))
}

/// 名前のラベルを作る。既存なら色だけ更新して返す（冪等）
pub fn create_tag(db: &Db, name: &str, color: Option<&str>) -> Result<i64> {
    let name = tag_name(name)?;
    check_color(color)?;
    let id = ensure_tag_conn(&db.conn, name, color)?;
    if let Some(c) = color {
        db.conn.execute("UPDATE tags SET color = ?1 WHERE id = ?2", rusqlite::params![c, id])?;
    }
    Ok(id)
}

/// 改名だけ（検査は update_tag と同じ）
pub fn rename_tag(db: &Db, id: i64, new_name: &str) -> Result<()> {
    update_tag(db, id, Some(new_name), None)
}

pub fn set_tag_color(db: &Db, id: i64, color: Option<&str>) -> Result<()> {
    check_color(color)?;
    let n = db
        .conn
        .execute("UPDATE tags SET color = ?1 WHERE id = ?2", rusqlite::params![color, id])?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("tag {id}")));
    }
    Ok(())
}

/// ラベルの改名と色の変更を 1 トランザクションで行う（`None` = 変えない）。検証（存在・名前・色）をすべて済ませてから書くので、
/// 色が不正なら改名も残らない。両方 `None` は存在確認だけ。色を消す（NULL に戻す）操作は無い（`set_tag_color` を使う）
pub fn update_tag(db: &Db, id: i64, new_name: Option<&str>, color: Option<&str>) -> Result<()> {
    let exists: bool = db.conn.query_row("SELECT EXISTS(SELECT 1 FROM tags WHERE id = ?1)", [id], |r| r.get(0))?;
    if !exists {
        return Err(CoreError::NotFound(format!("tag {id}")));
    }
    let new_name = match new_name {
        Some(n) => {
            let n = tag_name(n)?;
            if let Some((other, _)) = tag_by_name(&db.conn, n)? {
                if other != id {
                    return Err(CoreError::BadInput(format!("タグ '{n}' は既に存在します")));
                }
            }
            Some(n)
        }
        None => None,
    };
    check_color(color)?;
    let tx = db.conn.unchecked_transaction()?;
    if let Some(n) = new_name {
        tx.execute("UPDATE tags SET name = ?1 WHERE id = ?2", rusqlite::params![n, id])?;
    }
    if let Some(c) = color {
        tx.execute("UPDATE tags SET color = ?1 WHERE id = ?2", rusqlite::params![c, id])?;
    }
    tx.commit()?;
    Ok(())
}

/// タグを削除する（item_tags は FK ON DELETE CASCADE で自動的に外れる＝論文は消えない）
pub fn delete_tag(db: &Db, id: i64) -> Result<()> {
    let n = db.conn.execute("DELETE FROM tags WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(CoreError::NotFound(format!("tag {id}")));
    }
    Ok(())
}

/// ラベルの統合（Paperpile の Manage labels → Merge）: sources の付与を target へ移してから sources を削除する。
/// 両方に付いていた item は target 1 つになる（重複付与しない）。戻り値 = 統合後の target の付与件数
pub fn merge_tags(db: &Db, sources: &[i64], target: i64) -> Result<i64> {
    let sources: Vec<i64> = sources.iter().copied().filter(|&s| s != target).collect();
    if sources.is_empty() {
        return Err(CoreError::BadInput("統合するラベルを 2 つ以上選んでください".into()));
    }
    let exists = |id: i64| -> Result<bool> {
        Ok(db.conn.query_row("SELECT EXISTS(SELECT 1 FROM tags WHERE id = ?1)", [id], |r| r.get(0))?)
    };
    if !exists(target)? {
        return Err(CoreError::NotFound(format!("tag {target}")));
    }
    for &s in &sources {
        if !exists(s)? {
            return Err(CoreError::NotFound(format!("tag {s}")));
        }
    }
    let tx = db.conn.unchecked_transaction()?;
    for &s in &sources {
        tx.execute(
            "INSERT OR IGNORE INTO item_tags (item_id, tag_id) SELECT item_id, ?1 FROM item_tags WHERE tag_id = ?2",
            rusqlite::params![target, s],
        )?;
        tx.execute("DELETE FROM tags WHERE id = ?1", [s])?;
    }
    tx.commit()?;
    // 件数はサイドバーと同じ数え方（ゴミ箱の item は数えない）
    Ok(db.conn.query_row(
        "SELECT COUNT(*) FROM item_tags it JOIN items i ON i.id = it.item_id WHERE it.tag_id = ?1 AND i.deleted_at IS NULL",
        [target],
        |r| r.get(0),
    )?)
}

/// 名前のラベルが無ければ color で作り、あれば既存をそのまま返す（色は変えない）。
/// 取込の「要確認」など、システムが付けるラベル用（user が Manage labels で変えた色を毎回戻さない）と、
/// 名前で付ける `edit::add_tag`（MCP / CLI。色は None = 表示は UI の既定の灰）
pub fn ensure_tag(db: &Db, name: &str, color_if_new: Option<&str>) -> Result<i64> {
    let name = tag_name(name)?;
    check_color(color_if_new)?;
    ensure_tag_conn(&db.conn, name, color_if_new)
}

/// Find and replace の 1 件分の結果
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RenameOutcome {
    pub id: i64,
    pub old: String,
    pub new: String,
    /// "renamed" / "conflict"（同名ラベルが既にある）/ "empty"（置換後が空）。dry_run では "renamed" = 予定。
    /// フォルダ（collections::find_replace_collections）は "invalid"（置換後が `/` を含む）も返す
    pub status: &'static str,
}

/// ラベル名の検索置換（Paperpile の Manage labels → Find and replace）。大文字小文字を区別せず
/// `find` を含むラベル名の該当部分をすべて `replace` に置き換える。置換後が既存ラベル（または同じ一括内の
/// 別ラベルの置換後）と同名になるものは conflict で飛ばす（勝手に統合しない。統合は Merge で明示的に）。
/// dry_run = true は何も書かずに予定だけ返す（プレビュー用）
pub fn find_replace_tags(db: &Db, find: &str, replace: &str, dry_run: bool) -> Result<Vec<RenameOutcome>> {
    find_replace(db, "tags", find, replace, dry_run, false, || {
        Ok(list_tags(db, TagSort::Alphabetical)?.into_iter().map(|t| (t.id, t.name, ())).collect())
    })
}

/// 名前の検索置換の本体（ラベル・フォルダ共通）。`rows` = 対象の (id, 名前, 同名を数える範囲) を名前順で返す
/// （ラベル = 全体で 1 つの範囲・フォルダ = 同じ親）。`slash_invalid` = 置換後が `/` を含むものを "invalid" にする。
/// 判定の順 = empty → invalid → conflict → renamed。renamed は同じ一括の後続の同名判定に入る。`table` = 書き込む表
pub(crate) fn find_replace<S: Copy + Eq + std::hash::Hash>(
    db: &Db,
    table: &'static str,
    find: &str,
    replace: &str,
    dry_run: bool,
    slash_invalid: bool,
    rows: impl FnOnce() -> Result<Vec<(i64, String, S)>>,
) -> Result<Vec<RenameOutcome>> {
    if find.is_empty() {
        return Err(CoreError::BadInput("検索する文字列が空です".into()));
    }
    let re = regex::RegexBuilder::new(&regex::escape(find))
        .case_insensitive(true)
        .build()
        .map_err(|e| CoreError::BadInput(e.to_string()))?;
    let all = rows()?;
    // 同名判定は DB の COLLATE NOCASE（ASCII だけ大小無視。ラベル = UNIQUE・フォルダ = 兄弟の照合）と同じ畳み方にする。
    // Unicode の小文字化だと "Äab" と "äab"（DB では別名）が 1 つに潰れ、予定と実行が食い違う
    let key = |scope: S, s: &str| (scope, s.to_ascii_lowercase());
    let mut taken: std::collections::HashMap<(S, String), i64> =
        all.iter().map(|(id, name, scope)| (key(*scope, name), *id)).collect();
    let mut out = Vec::new();
    for (id, name, scope) in all.iter().filter(|(_, name, _)| re.is_match(name)) {
        let new = re.replace_all(name, regex::NoExpand(replace)).trim().to_string();
        let status = if new.is_empty() {
            "empty"
        } else if slash_invalid && new.contains('/') {
            "invalid"
        } else if taken.get(&key(*scope, &new)).is_some_and(|other| other != id) {
            "conflict"
        } else {
            "renamed"
        };
        if status == "renamed" {
            taken.remove(&key(*scope, name));
            taken.insert(key(*scope, &new), *id);
        }
        out.push(RenameOutcome { id: *id, old: name.clone(), new, status });
    }
    if !dry_run {
        let sql = format!("UPDATE {table} SET name = ?1 WHERE id = ?2");
        let tx = db.conn.unchecked_transaction()?;
        for o in out.iter().filter(|o| o.status == "renamed" && o.old != o.new) {
            tx.execute(&sql, rusqlite::params![o.new, o.id])?;
        }
        tx.commit()?;
    }
    Ok(out)
}

/// タグ id を item に付与（key 経由。存在しない組合せのみ挿入）
pub fn attach_tag(db: &Db, item_key: &str, tag_id: i64) -> Result<()> {
    let item_id = crate::edit::item_id(db, item_key)?;
    db.conn.execute(
        "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
        rusqlite::params![item_id, tag_id],
    )?;
    Ok(())
}

pub fn detach_tag(db: &Db, item_key: &str, tag_id: i64) -> Result<()> {
    let item_id = crate::edit::item_id(db, item_key)?;
    db.conn.execute(
        "DELETE FROM item_tags WHERE item_id = ?1 AND tag_id = ?2",
        rusqlite::params![item_id, tag_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_lifecycle() {
        let db = Db::open_in_memory().unwrap();
        // seed 1 item
        db.conn
            .execute_batch(
                "INSERT INTO items (key, csl) VALUES ('ITEM0001', '{\"title\":\"x\",\"type\":\"article-journal\"}')",
            )
            .unwrap();

        let red = create_tag(&db, "pancreas", Some("#e53935")).unwrap(); // 旧来の hex も受け付ける
        let _blue = create_tag(&db, "MRI", Some("sky")).unwrap();
        // 冪等（既存名は同 id を返す）
        assert_eq!(create_tag(&db, "pancreas", None).unwrap(), red);

        attach_tag(&db, "ITEM0001", red).unwrap();
        attach_tag(&db, "ITEM0001", red).unwrap(); // 二重付与しない

        let tags = list_tags(&db, TagSort::Alphabetical).unwrap();
        assert_eq!(tags.len(), 2);
        // alpha = 名前だけで決まる（作成順 pancreas → MRI でも MRI が先）
        assert_eq!(tags[0].name, "MRI");
        assert_eq!(tags[1].name, "pancreas");
        assert_eq!(tags[1].n_items, 1);
        assert_eq!(tags[1].color.as_deref(), Some("#e53935"));
        assert_eq!(tags[0].n_items, 0);

        // count 順
        let by_count = list_tags(&db, TagSort::Count).unwrap();
        assert_eq!(by_count[0].name, "pancreas");

        // rename 衝突
        assert!(rename_tag(&db, red, "MRI").is_err());
        rename_tag(&db, red, "pancreatic").unwrap();

        // 色はパレット id か #rrggbb のみ
        assert!(create_tag(&db, "bad", Some("pink-ish")).is_err());
        assert!(set_tag_color(&db, red, Some("url(x)")).is_err());
        set_tag_color(&db, red, Some("forest")).unwrap();
        set_tag_color(&db, red, None).unwrap();

        // delete → item は残る、付与のみ消える
        delete_tag(&db, red).unwrap();
        assert_eq!(list_tags(&db, TagSort::Alphabetical).unwrap().len(), 1);
        let cnt: i64 = db.conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0)).unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn update_tag_is_atomic() {
        let db = Db::open_in_memory().unwrap();
        let a = create_tag(&db, "alpha", Some("sky")).unwrap();
        let b = create_tag(&db, "beta", Some("red")).unwrap();
        let get = |id: i64| -> (String, Option<String>) {
            db.conn.query_row("SELECT name, color FROM tags WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        };
        let orig = (get(a), get(b));

        // 改名 + 不正な色 → エラー、改名も残らない
        assert!(matches!(update_tag(&db, a, Some("gamma"), Some("bogus")), Err(CoreError::BadInput(_))));
        // 改名 + 同名衝突（色は有効）→ エラー、色も変わらない
        assert!(matches!(update_tag(&db, a, Some("BETA"), Some("green")), Err(CoreError::BadInput(_))));
        // 空の名前 → エラー、色も変わらない
        assert!(matches!(update_tag(&db, a, Some("  "), Some("green")), Err(CoreError::BadInput(_))));
        // 無い id
        assert!(matches!(update_tag(&db, 9999, Some("x"), None), Err(CoreError::NotFound(_))));
        assert!(matches!(update_tag(&db, 9999, None, None), Err(CoreError::NotFound(_))));
        assert_eq!((get(a), get(b)), orig, "失敗した更新は何も変えない");

        // 改名だけ（前後の空白は落とす）
        update_tag(&db, a, Some(" gamma "), None).unwrap();
        assert_eq!(get(a), ("gamma".to_string(), Some("sky".to_string())));
        // 色だけ（#rrggbb も可）
        update_tag(&db, a, None, Some("#00ff00")).unwrap();
        assert_eq!(get(a), ("gamma".to_string(), Some("#00ff00".to_string())));
        // 両方
        update_tag(&db, a, Some("delta"), Some("forest")).unwrap();
        assert_eq!(get(a), ("delta".to_string(), Some("forest".to_string())));
        // 同名（自分自身・大文字小文字違い）への改名は通る
        update_tag(&db, a, Some("DELTA"), None).unwrap();
        assert_eq!(get(a).0, "DELTA");
        // 両方 None = 存在確認だけ
        update_tag(&db, a, None, None).unwrap();
        assert_eq!(get(b), orig.1);
    }

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

    #[test]
    fn merge_moves_assignments_without_duplicates() {
        let db = Db::open_in_memory().unwrap();
        seed_items(&db, &["A", "B", "C"]);
        let liver = create_tag(&db, "liver", None).unwrap();
        let hepatic = create_tag(&db, "Hepatic", Some("red")).unwrap();
        let hcc = create_tag(&db, "HCC", None).unwrap();
        attach_tag(&db, "A", liver).unwrap();
        attach_tag(&db, "A", hepatic).unwrap(); // A は両方 → 統合後 1 つ
        attach_tag(&db, "B", hepatic).unwrap();
        attach_tag(&db, "C", hcc).unwrap();
        let n = merge_tags(&db, &[liver, hepatic, hcc], liver).unwrap();
        assert_eq!(n, 3);
        let tags = list_tags(&db, TagSort::Alphabetical).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!((tags[0].id, tags[0].n_items), (liver, 3));
        // 件数はゴミ箱を数えない（サイドバーと同じ）
        db.conn.execute("UPDATE items SET deleted_at = '2026-01-01' WHERE key = 'C'", []).unwrap();
        let other = create_tag(&db, "other", None).unwrap();
        assert_eq!(merge_tags(&db, &[other], liver).unwrap(), 2);
        // ensure_tag は既存の色を変えない
        let rv = ensure_tag(&db, "要確認", Some("peach")).unwrap();
        set_tag_color(&db, rv, Some("navy")).unwrap();
        assert_eq!(ensure_tag(&db, "要確認", Some("peach")).unwrap(), rv);
        let c: Option<String> = db.conn.query_row("SELECT color FROM tags WHERE id = ?1", [rv], |r| r.get(0)).unwrap();
        assert_eq!(c.as_deref(), Some("navy"));
        // 統合先だけ / 存在しない id は拒否
        assert!(merge_tags(&db, &[liver], liver).is_err());
        assert!(merge_tags(&db, &[999], liver).is_err());
        assert!(merge_tags(&db, &[liver], 999).is_err());
    }

    #[test]
    fn find_replace_case_insensitive_and_conflicts() {
        let db = Db::open_in_memory().unwrap();
        let a = create_tag(&db, "Liver MRI", None).unwrap();
        let b = create_tag(&db, "liver CT", None).unwrap();
        let c = create_tag(&db, "Hepatic CT", None).unwrap();
        let _d = create_tag(&db, "pancreas", None).unwrap();
        // 予定だけ（書かない）
        let plan = find_replace_tags(&db, "LIVER", "Hepatic", true).unwrap();
        let get = |v: &[RenameOutcome], id: i64| v.iter().find(|o| o.id == id).cloned().unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(get(&plan, a).new, "Hepatic MRI");
        assert_eq!(get(&plan, a).status, "renamed");
        assert_eq!(get(&plan, b).new, "Hepatic CT");
        assert_eq!(get(&plan, b).status, "conflict", "既存の Hepatic CT と同名 → 飛ばす（勝手に統合しない）");
        assert_eq!(list_tags(&db, TagSort::Alphabetical).unwrap().iter().filter(|t| t.name.contains("iver")).count(), 2);
        // 実行
        let done = find_replace_tags(&db, "liver", "Hepatic", false).unwrap();
        assert_eq!(done.iter().filter(|o| o.status == "renamed").count(), 1);
        let names: Vec<String> = list_tags(&db, TagSort::Alphabetical).unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["Hepatic CT", "Hepatic MRI", "liver CT", "pancreas"]);
        let _ = c;
        // 置換後が空 / 検索語が空
        let e = find_replace_tags(&db, "pancreas", "  ", true).unwrap();
        assert_eq!(e[0].status, "empty");
        assert!(find_replace_tags(&db, "", "x", true).is_err());
        // DB は ASCII だけ大小無視: "Äab" と "äab" は別名として共存でき、予定どおりに実行できる
        create_tag(&db, "Äab", None).unwrap();
        create_tag(&db, "äab", None).unwrap();
        create_tag(&db, "äabb", None).unwrap();
        let plan = find_replace_tags(&db, "äabb", "äab", true).unwrap();
        assert_eq!(plan[0].status, "conflict", "既存の äab と同名（DB と同じ判定）");
        assert_eq!(find_replace_tags(&db, "Äab", "Äax", false).unwrap()[0].status, "renamed");
        // 正規表現の特殊文字は文字どおり
        create_tag(&db, "a.b", None).unwrap();
        let dot = find_replace_tags(&db, ".", "-", false).unwrap();
        assert_eq!(dot.len(), 1);
        assert_eq!(dot[0].new, "a-b");
    }

    /// UI のパレット（app/src/lib/labels.ts）と id の並びが一致すること
    #[test]
    fn palette_ids_match_ui() {
        let ts = include_str!("../../../app/src/lib/labels.ts");
        let ids: Vec<&str> = ts
            .lines()
            .filter_map(|l| l.trim().strip_prefix("{ id: \""))
            .map(|l| l.split('"').next().unwrap())
            .collect();
        assert_eq!(ids, LABEL_COLORS.to_vec());
    }
}
