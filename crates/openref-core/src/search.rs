use crate::{annotate::NoteRow, model::ItemRow, CoreError, Db, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn list(db: &Db, limit: usize) -> Result<Vec<ItemRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, key, csl, starred, date_added FROM items
         WHERE deleted_at IS NULL ORDER BY date_added DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], row_to_item)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// FTS5 検索。ユーザー入力はトークンごとに引用符で包む（FTS 構文エラー防止・AND 意味論）。
/// CJK を含むトークンは index 側と同じ per-char 分かち書きに変換して phrase 化する
/// （cjk 列にヒット。"胆道" → "\"胆 道\"" が「胆 道 疾 患…」に部分一致）。
pub fn search(db: &Db, query: &str, limit: usize) -> Result<Vec<ItemRow>> {
    let fts_q = query
        .split_whitespace()
        .map(|t| {
            let t = t.replace('"', "");
            format!("\"{}\"", crate::model::cjk_spaced(&t))
        })
        .collect::<Vec<_>>()
        .join(" ");
    if fts_q.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = db.conn.prepare(
        "SELECT i.id, i.key, i.csl, i.starred, i.date_added
         FROM items_fts JOIN items i ON i.id = items_fts.rowid
         WHERE items_fts MATCH ?1 AND i.deleted_at IS NULL
         ORDER BY items_fts.rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![fts_q, limit as i64], row_to_item)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// PMID → DOI の順で既存 item を探す（connector 側 dedup 用）
pub fn find_by_ids(db: &Db, pmid: Option<&str>, doi: Option<&str>) -> Result<Option<(i64, String)>> {
    use rusqlite::OptionalExtension;
    for (val, sql) in [
        (pmid, "SELECT id, key FROM items WHERE deleted_at IS NULL AND pmid = ?1"),
        (doi, "SELECT id, key FROM items WHERE deleted_at IS NULL AND doi = ?1"),
    ] {
        if let Some(v) = val {
            let row = db
                .conn
                .query_row(sql, [v], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .optional()?;
            if row.is_some() {
                return Ok(row);
            }
        }
    }
    Ok(None)
}

pub fn get_by_key(db: &Db, key: &str) -> Result<Option<ItemRow>> {
    use rusqlite::OptionalExtension;
    Ok(db
        .conn
        .query_row(
            "SELECT id, key, csl, starred, date_added FROM items
             WHERE key = ?1 AND deleted_at IS NULL",
            [key],
            row_to_item,
        )
        .optional()?)
}

pub fn retraction_of(db: &Db, item_id: i64) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(db
        .conn
        .query_row(
            "SELECT COALESCE(detail, status) FROM retraction_flags WHERE item_id = ?1",
            [item_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

/// UI 一覧用フィルタ。search と他条件は併用可。GUI（Tauri）は JSON（camelCase）をそのまま渡す（無い項目は既定値）
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListFilter {
    pub search: Option<String>,
    pub collection_id: Option<i64>,
    /// ラベル名で絞る（MCP / CLI 用・大小無視）
    pub tag: Option<String>,
    /// ラベル id で絞る（GUI のラベルビュー。名前だと改名した瞬間に一覧が空になるため id で引く）
    pub tag_id: Option<i64>,
    pub starred_only: bool,
    pub trashed: bool,
    pub limit: usize,
    /// 著者チップ（Paperpile の author filter）: creators_index の family（または literal）完全一致・大小無視。
    /// 著者の位置は問わない（筆頭でなくても hit。Paperpile と同じ）
    pub author_family: Option<String>,
    /// 著者チップの given 頭文字（同姓の別人を外す。None なら姓だけで絞る）
    pub author_initial: Option<String>,
    /// 誌名チップ: 行に表示している誌名（full / short どちらでも可）。`model::journal_norm` で正規化し、
    /// journal_keys を推移的に辿って同一誌の全 item を返す（表記ゆれ・print/electronic ISSN・short の有無を吸収）
    pub journal: Option<String>,
    /// true = PDF 本文（fulltext_fts）を検索対象にしない（Word アドインの引用検索用: 著者名で「その著者を引用している
    /// だけの論文」が混ざると誤引用のもとになる）。GUI / CLI / MCP は false = 本文も検索
    pub metadata_only: bool,
}

/// 行・詳細に付くラベルの参照（id は GUI が detach_tag・ラベルビューに使う。名前だけで足りる MCP は名前を取り出す）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagRef {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
}

/// 詳細に付くフォルダの参照
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionRef {
    pub id: i64,
    pub name: String,
}

pub struct ListedItem {
    pub item: ItemRow,
    pub has_pdf: bool,
    pub retracted: bool,
    /// メモ（notes）が 1 件以上ある（一覧のインジケータ用）
    pub has_note: bool,
    /// 検索語が添付 PDF の本文に一致したときの抜粋（一致箇所は fulltext::HL_START / HL_END で囲む）。
    /// メタデータ（title 等）にも一致していれば併せて返す。search 無し・本文不一致は None
    pub fulltext_snippet: Option<String>,
    /// 付いているラベル（行のバッジ用・名前順）
    pub tags: Vec<TagRef>,
}

/// GUI の一覧の行（表示に要る項目だけ・CSL は持たない）。MCP の list_items の要約（commands.rs の item_summary = 誌名 journal・
/// ラベルは名前だけ・抜粋の印は [ ]）とは形が違う別物で、そちらはエージェント向けの応答形式（契約）なので変えない
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub key: String,
    pub title: String,
    pub first_author: String,
    /// 筆頭著者の given（著者チップ用。無ければ None）
    pub first_author_given: Option<String>,
    pub year: Option<i64>,
    /// 誌名（short があれば short）
    pub container: String,
    pub starred: bool,
    pub has_pdf: bool,
    pub retracted: bool,
    pub has_note: bool,
    /// 検索語が PDF 本文に一致したときの抜粋（一致箇所は fulltext::HL_START / HL_END で囲む。UI が <mark> に変える）
    pub fulltext_snippet: Option<String>,
    pub date_added: String,
    /// 付いているラベル（行のバッジ・名前順）
    pub tags: Vec<TagRef>,
}

impl ListedItem {
    /// GUI の一覧の行へ整形する
    pub fn summary(&self) -> ItemSummary {
        let c = &self.item.csl;
        ItemSummary {
            key: self.item.key.clone(),
            title: crate::model::s(c, "title").unwrap_or("(no title)").to_string(),
            first_author: crate::model::first_author_family(c),
            first_author_given: crate::model::first_author_given(c),
            year: crate::model::year(c),
            container: crate::model::s(c, "container-title-short")
                .or_else(|| crate::model::s(c, "container-title"))
                .unwrap_or("")
                .to_string(),
            starred: self.item.starred,
            has_pdf: self.has_pdf,
            retracted: self.retracted,
            has_note: self.has_note,
            fulltext_snippet: self.fulltext_snippet.clone(),
            date_added: self.item.date_added.clone(),
            tags: self.tags.clone(),
        }
    }
}

/// フィルタ付き一覧。search あり → items_fts の hit ∪ fulltext_fts の hit（添付 → item）、
/// メタデータ hit を先（items_fts.rank 順）・本文のみ hit を後（fulltext_fts.rank 順）に並べる。
/// なし → date_added 降順。チップ（著者/誌名）・ビュー（フォルダ/ラベル/★/ゴミ箱）とは常に AND
pub fn list_items(db: &Db, f: &ListFilter) -> Result<Vec<ListedItem>> {
    let limit = if f.limit == 0 { 200 } else { f.limit } as i64;
    let base_cols = "i.id, i.key, i.csl, i.starred, i.date_added,
        EXISTS(SELECT 1 FROM attachments a WHERE a.item_id = i.id) AS has_pdf,
        EXISTS(SELECT 1 FROM retraction_flags r WHERE r.item_id = i.id) AS retracted,
        EXISTS(SELECT 1 FROM notes n WHERE n.item_id = i.id) AS has_note";
    let common = "AND (?2 IS NULL OR i.id IN (SELECT item_id FROM collection_items WHERE collection_id = ?2))
        AND (?3 = 0 OR i.starred = 1)
        AND (?6 IS NULL OR i.id IN (SELECT it.item_id FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE t.name = ?6 COLLATE NOCASE))
        AND (?11 IS NULL OR i.id IN (SELECT item_id FROM item_tags WHERE tag_id = ?11))
        AND (?7 IS NULL OR i.id IN (SELECT ci.item_id FROM creators_index ci
             WHERE (ci.family = ?7 COLLATE NOCASE OR ci.literal = ?7 COLLATE NOCASE)
               AND (?8 IS NULL OR upper(substr(ci.given, 1, 1)) = upper(?8))))
        AND (?9 IS NULL OR i.id IN (
             WITH RECURSIVE jc(k) AS (
               SELECT 't:' || ?9
               UNION
               SELECT k2.key FROM jc JOIN journal_keys k1 ON k1.key = jc.k JOIN journal_keys k2 ON k2.item_id = k1.item_id
             )
             SELECT jk.item_id FROM journal_keys jk WHERE jk.key IN (SELECT k FROM jc)))";
    let (sql, fts_q) = match &f.search {
        Some(q) if !q.trim().is_empty() => {
            let fts_q = q
                .split_whitespace()
                .map(|t| {
                    let t = t.replace('"', "");
                    format!("\"{}\"", crate::model::cjk_spaced(&t))
                })
                .collect::<Vec<_>>()
                .join(" ");
            // meta = items_fts の hit、ft = fulltext_fts の hit を添付 → item に畳む（item ごとに rank 最良の 1 添付 = att）。
            // 抜粋（snippet）は全文の走査で高価なので、ここでは作らず LIMIT 後の結果行だけ後段で引く（?10 = 本文検索 off）
            (
                format!(
                    "WITH meta AS (SELECT items_fts.rowid AS id, items_fts.rank AS rank FROM items_fts WHERE items_fts MATCH ?1),
                          ft AS (SELECT id, rank, att FROM (
                                   SELECT id, rank, att, row_number() OVER (PARTITION BY id ORDER BY rank, att) AS rn FROM (
                                     SELECT a.item_id AS id, fulltext_fts.rank AS rank, fulltext_fts.rowid AS att
                                     FROM fulltext_fts JOIN attachments a ON a.id = fulltext_fts.rowid
                                     WHERE fulltext_fts MATCH ?1 AND ?10 = 0))
                                 WHERE rn = 1)
                     SELECT {base_cols}, ft.att AS att
                     FROM items i LEFT JOIN meta ON meta.id = i.id LEFT JOIN ft ON ft.id = i.id
                     WHERE (meta.id IS NOT NULL OR ft.id IS NOT NULL)
                       AND ((?4 = 1) = (i.deleted_at IS NOT NULL)) {common}
                     ORDER BY (meta.id IS NULL), COALESCE(meta.rank, ft.rank), i.id LIMIT ?5"
                ),
                Some(fts_q),
            )
        }
        _ => (
            format!(
                "SELECT {base_cols}, NULL AS att FROM items i
                 WHERE ?1 IS NULL AND ?10 = ?10 AND ((?4 = 1) = (i.deleted_at IS NOT NULL)) {common}
                 ORDER BY i.date_added DESC, i.id DESC LIMIT ?5"
            ),
            None,
        ),
    };
    // 誌名チップはラベルを正規化キーに落としてから引く（空になれば未指定扱い）
    let journal_key = f
        .journal
        .as_deref()
        .map(crate::model::journal_norm)
        .filter(|k| !k.is_empty());
    let mut stmt = db.conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![
            fts_q,
            f.collection_id,
            f.starred_only as i64,
            f.trashed as i64,
            limit,
            f.tag,
            // 空白だけは未指定扱い（GUI のチップ。MCP は渡す前に同じ判定をしている）
            f.author_family.as_deref().filter(|s| !s.trim().is_empty()),
            f.author_initial.as_deref().filter(|s| !s.trim().is_empty()),
            journal_key,
            f.metadata_only as i64,
            f.tag_id
        ],
        |row| {
            let item = row_to_item(row)?;
            Ok((
                ListedItem {
                    item,
                    has_pdf: row.get::<_, i64>(5)? != 0,
                    retracted: row.get::<_, i64>(6)? != 0,
                    has_note: row.get::<_, i64>(7)? != 0,
                    fulltext_snippet: None,
                    tags: Vec::new(),
                },
                row.get::<_, Option<i64>>(8)?,
            ))
        },
    )?;
    let mut out: Vec<(ListedItem, Option<i64>)> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    // 本文 hit の行だけ抜粋を引く: snippet(列 -1 = text / cjk のうち一致した方。マーカーは私用領域 U+E000/U+E001、最大 48 トークン)
    if let Some(q) = &fts_q {
        let mut snip_stmt = db.conn.prepare(
            "SELECT snippet(fulltext_fts, -1, char(57344), char(57345), '…', 48)
             FROM fulltext_fts WHERE fulltext_fts MATCH ?1 AND rowid = ?2",
        )?;
        for (li, att) in out.iter_mut() {
            if let Some(att) = *att {
                use rusqlite::OptionalExtension;
                let raw: Option<String> = snip_stmt
                    .query_row(rusqlite::params![q, att], |r| r.get(0))
                    .optional()?;
                li.fulltext_snippet = raw.map(|s| crate::fulltext::tidy_snippet(&s, 24, 120));
            }
        }
    }
    // 行のラベル（バッジ）: 結果行の item だけを 1 本の問い合わせで引いて配る。
    // id は JSON 配列 1 個で渡す（行ごとに ? を並べると SQLite の変数上限 32,766 を超え得る）
    if !out.is_empty() {
        let ids: Vec<i64> = out.iter().map(|(li, _)| li.item.id).collect();
        let ids_json = serde_json::to_string(&ids)?;
        let mut tag_stmt = db.conn.prepare(
            "SELECT it.item_id, t.id, t.name, t.color FROM item_tags it JOIN tags t ON t.id = it.tag_id
             WHERE it.item_id IN (SELECT value FROM json_each(?1)) ORDER BY t.name COLLATE NOCASE, t.id",
        )?;
        let mut by_item: std::collections::HashMap<i64, Vec<TagRef>> = std::collections::HashMap::new();
        let rows = tag_stmt.query_map([ids_json], |r| {
            Ok((r.get::<_, i64>(0)?, TagRef { id: r.get(1)?, name: r.get(2)?, color: r.get(3)? }))
        })?;
        for row in rows {
            let (item_id, tag) = row?;
            by_item.entry(item_id).or_default().push(tag);
        }
        for (li, _) in out.iter_mut() {
            if let Some(t) = by_item.remove(&li.item.id) {
                li.tags = t;
            }
        }
    }
    Ok(out.into_iter().map(|(li, _)| li).collect())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRow {
    pub id: i64,
    pub key: String,
    pub mode: String,
    pub path: String,
    pub exists: bool,
    pub page_count: Option<i64>,
    /// 自動取得 PDF の同一性照合（'match' / 'unknown' / NULL = 未照合）。db.rs v9
    pub doi_check: Option<String>,
}

/// 詳細（ゴミ箱の item も引ける）。GUI はこれをそのまま JSON にする: item の列は平坦（key / csl / starred / dateAdded …）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    #[serde(flatten)]
    pub item: ItemRow,
    /// ゴミ箱にある（items.deleted_at IS NOT NULL）
    pub deleted: bool,
    pub attachments: Vec<AttachmentRow>,
    pub collections: Vec<CollectionRef>,
    /// sort_order → 名前順
    pub tags: Vec<TagRef>,
    pub retraction: Option<String>,
    /// item 単位のメモ（user が UI で書くもの + MCP 経由で Claude が残すもの）
    pub notes: Vec<NoteRow>,
}

pub fn item_detail(db: &Db, key: &str) -> Result<Option<ItemDetail>> {
    use rusqlite::OptionalExtension;
    let row = db
        .conn
        .query_row(
            "SELECT id, key, csl, starred, date_added, deleted_at IS NOT NULL FROM items WHERE key = ?1",
            [key],
            |r| Ok((row_to_item(r)?, r.get::<_, i64>(5)? != 0)),
        )
        .optional()?;
    let Some((item, deleted)) = row else { return Ok(None) };
    let mut stmt = db
        .conn
        .prepare("SELECT id, key, mode, path, page_count, doi_check FROM attachments WHERE item_id = ?1 ORDER BY id")?;
    let attachments = stmt
        .query_map([item.id], |r| {
            let path: String = r.get(3)?;
            Ok(AttachmentRow {
                id: r.get(0)?,
                key: r.get(1)?,
                mode: r.get(2)?,
                exists: std::path::Path::new(&path).exists(),
                path,
                page_count: r.get(4)?,
                doi_check: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut stmt = db.conn.prepare(
        "SELECT c.id, c.name FROM collections c JOIN collection_items ci ON ci.collection_id = c.id
         WHERE ci.item_id = ?1 AND c.deleted_at IS NULL ORDER BY c.name",
    )?;
    let collections = stmt
        .query_map([item.id], |r| Ok(CollectionRef { id: r.get(0)?, name: r.get(1)? }))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut stmt = db.conn.prepare(
        "SELECT t.id, t.name, t.color FROM tags t JOIN item_tags it ON it.tag_id = t.id
         WHERE it.item_id = ?1 ORDER BY t.sort_order, t.name COLLATE NOCASE",
    )?;
    let tags = stmt
        .query_map([item.id], |r| Ok(TagRef { id: r.get(0)?, name: r.get(1)?, color: r.get(2)? }))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let retraction = retraction_of(db, item.id)?;
    let notes = crate::annotate::list_notes(db, key)?;
    Ok(Some(ItemDetail {
        item,
        deleted,
        attachments,
        collections,
        tags,
        retraction,
        notes,
    }))
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewCounts {
    pub all: i64,
    pub starred: i64,
    pub trash: i64,
}

pub fn view_counts(db: &Db) -> Result<ViewCounts> {
    let g = |sql: &str| -> Result<i64> { Ok(db.conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(ViewCounts {
        all: g("SELECT COUNT(*) FROM items WHERE deleted_at IS NULL")?,
        starred: g("SELECT COUNT(*) FROM items WHERE deleted_at IS NULL AND starred = 1")?,
        trash: g("SELECT COUNT(*) FROM items WHERE deleted_at IS NOT NULL")?,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionRow {
    pub id: i64,
    /// 8 桁の不変キー。id は削除後に再利用されうる（INTEGER PRIMARY KEY・AUTOINCREMENT なし）ので、
    /// UI の永続（折畳など）はこちらで持つ
    pub key: String,
    pub name: String,
    pub parent_id: Option<i64>,
    pub n_items: i64,
}

/// サイドバーのフォルダ一覧（名前順・大小無視）。件数 = 直下の論文だけ・ゴミ箱の論文は数えない（ラベルの件数と同じ）
pub fn collections(db: &Db) -> Result<Vec<CollectionRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT c.id, c.key, c.name, c.parent_id,
                (SELECT COUNT(*) FROM collection_items ci
                 JOIN items i ON i.id = ci.item_id
                 WHERE ci.collection_id = c.id AND i.deleted_at IS NULL)
         FROM collections c WHERE c.deleted_at IS NULL
         ORDER BY c.name COLLATE NOCASE, c.id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(CollectionRow {
            id: r.get(0)?,
            key: r.get(1)?,
            name: r.get(2)?,
            parent_id: r.get(3)?,
            n_items: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// 複数 item の所属集計（一括操作ツールバーの三状態表示: 全部 / 一部 / なし）。
/// 各 Vec は (collection_id または tag_id, keys のうち所属する件数)。0 件の所属先は含まない。JSON では [[id, n], …]
#[derive(Debug, Serialize)]
pub struct Membership {
    pub collections: Vec<(i64, i64)>,
    pub tags: Vec<(i64, i64)>,
}

pub fn membership(db: &Db, keys: &[String]) -> Result<Membership> {
    // プレースホルダ上限を避けるため keys は JSON 配列 1 本で渡し json_each で展開する
    let json = serde_json::to_string(keys).map_err(|e| CoreError::BadInput(e.to_string()))?;
    let q = |sql: &str| -> Result<Vec<(i64, i64)>> {
        let mut stmt = db.conn.prepare(sql)?;
        let rows = stmt.query_map([&json], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    };
    Ok(Membership {
        collections: q(
            "SELECT ci.collection_id, COUNT(*) FROM collection_items ci
             JOIN items i ON i.id = ci.item_id
             WHERE i.key IN (SELECT value FROM json_each(?1))
             GROUP BY ci.collection_id",
        )?,
        tags: q(
            "SELECT it.tag_id, COUNT(*) FROM item_tags it
             JOIN items i ON i.id = it.item_id
             WHERE i.key IN (SELECT value FROM json_each(?1))
             GROUP BY it.tag_id",
        )?,
    })
}

pub struct Counts {
    pub items: i64,
    pub attachments: i64,
    pub annotations: i64,
    pub notes: i64,
    pub collections: i64,
    pub tags: i64,
}

pub fn counts(db: &Db) -> Result<Counts> {
    let g = |sql: &str| -> Result<i64> { Ok(db.conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(Counts {
        items: g("SELECT COUNT(*) FROM items WHERE deleted_at IS NULL")?,
        attachments: g("SELECT COUNT(*) FROM attachments")?,
        annotations: g("SELECT COUNT(*) FROM annotations")?,
        notes: g("SELECT COUNT(*) FROM notes")?,
        collections: g("SELECT COUNT(*) FROM collections WHERE deleted_at IS NULL")?,
        tags: g("SELECT COUNT(*) FROM tags")?,
    })
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<ItemRow> {
    let csl_s: String = row.get(2)?;
    let csl: Value = serde_json::from_str(&csl_s).unwrap_or(Value::Null);
    Ok(ItemRow {
        id: row.get(0)?,
        key: row.get(1)?,
        csl,
        starred: row.get::<_, i64>(3)? != 0,
        date_added: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GUI（Tauri）は core の型をそのまま JSON にする契約: 鍵は camelCase・一覧の行（ItemSummary）と詳細（ItemDetail）の鍵が揃っている・
    /// ListFilter は camelCase の JSON から読める（app/src/lib/api.ts の TS 型と 1:1）
    #[test]
    fn gui_json_shape() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                "INSERT INTO items (key, csl) VALUES ('ITEM0001', '{\"title\":\"T\",\"type\":\"article-journal\",\"author\":[{\"family\":\"Kano\",\"given\":\"S\"}],\"container-title\":\"Radiology\",\"issued\":{\"date-parts\":[[2026]]}}')",
            )
            .unwrap();
        crate::edit::add_tag(&db, "ITEM0001", "liver").unwrap();
        crate::annotate::add_note(&db, "ITEM0001", "user", "memo").unwrap();
        let rows = list_items(&db, &ListFilter::default()).unwrap();
        let v = serde_json::to_value(rows[0].summary()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["container", "dateAdded", "firstAuthor", "firstAuthorGiven", "fulltextSnippet", "hasNote", "hasPdf", "key", "retracted", "starred", "tags", "title", "year"]
        );
        assert_eq!((v["firstAuthor"].as_str(), v["firstAuthorGiven"].as_str(), v["year"].as_i64()), (Some("Kano"), Some("S"), Some(2026)));
        assert_eq!((v["container"].as_str(), v["hasNote"].as_bool(), v["hasPdf"].as_bool()), (Some("Radiology"), Some(true), Some(false)));
        assert_eq!(v["tags"][0]["name"], "liver");
        assert!(v["tags"][0]["id"].is_i64());
        let d = serde_json::to_value(item_detail(&db, "ITEM0001").unwrap().unwrap()).unwrap();
        let keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["attachments", "collections", "csl", "dateAdded", "deleted", "id", "key", "notes", "retraction", "starred", "tags"],
            "item の列は平坦（flatten）"
        );
        assert_eq!((d["key"].as_str(), d["deleted"].as_bool(), d["csl"]["title"].as_str()), (Some("ITEM0001"), Some(false), Some("T")));
        assert_eq!((d["notes"][0]["contentMd"].as_str(), d["notes"][0]["author"].as_str()), (Some("memo"), Some("user")));
        assert_eq!(d["tags"][0]["name"], "liver");
        // 他の行型の鍵も camelCase: TagRow / CollectionRow / AttachmentRow / Purged（GUI がそのまま返す）
        let keys_of = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        let t = serde_json::to_value(&crate::tags::list_tags(&db, crate::tags::TagSort::Alphabetical).unwrap()[0]).unwrap();
        assert_eq!(keys_of(&t), ["color", "id", "nItems", "name", "sortOrder"]);
        crate::collections::create_collection(&db, "Liver", None).unwrap();
        let c = serde_json::to_value(&collections(&db).unwrap()[0]).unwrap();
        assert_eq!(keys_of(&c), ["id", "key", "nItems", "name", "parentId"]);
        db.conn
            .execute_batch(
                "INSERT INTO attachments (key, item_id, kind, mode, path, md5) SELECT 'ATT00001', id, 'pdf', 'managed', '/nonexistent/x.pdf', 'd41d8cd98f00b204e9800998ecf8427e' FROM items WHERE key = 'ITEM0001'",
            )
            .unwrap();
        let a = serde_json::to_value(&item_detail(&db, "ITEM0001").unwrap().unwrap().attachments[0]).unwrap();
        assert_eq!(keys_of(&a), ["doiCheck", "exists", "id", "key", "mode", "pageCount", "path"]);
        assert_eq!((a["exists"].as_bool(), a["doiCheck"].is_null(), a["pageCount"].is_null()), (Some(false), true, true));
        let p = serde_json::to_value(crate::edit::Purged { key: "K".into(), files_removed: 1, files_kept: 0, files_failed: 0 }).unwrap();
        assert_eq!(keys_of(&p), ["filesFailed", "filesKept", "filesRemoved", "key"]);
        crate::edit::trash(&db, "ITEM0001").unwrap();
        assert!(item_detail(&db, "ITEM0001").unwrap().unwrap().deleted, "ゴミ箱の item も引けて deleted が立つ");
        // ListFilter: camelCase・無い項目は既定値・空白だけの著者チップは未指定扱い
        let f: ListFilter = serde_json::from_value(serde_json::json!({"tagId": 3, "starredOnly": true, "authorFamily": "  "})).unwrap();
        assert_eq!((f.tag_id, f.starred_only, f.limit, f.metadata_only, f.trashed), (Some(3), true, 0, false, false));
        assert_eq!(list_items(&db, &ListFilter { trashed: true, author_family: Some("  ".into()), ..Default::default() }).unwrap().len(), 1);
    }
}
