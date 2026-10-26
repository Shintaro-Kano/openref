//! PDF 本文テキストの抽出・キャッシュ・全文索引（`fulltext` + `fulltext_fts`。migration v7）。
//!
//! - 添付直後に [`index_bytes`] で本文を抽出し `fulltext`（text / cjk / md5 / status / detail）へ入れる
//!   （今書いたバイト列から抽出 = ファイルを読み直さず md5 も取り直さない）。既存添付の一括索引・Paperpile import・
//!   MCP get_fulltext は [`index_attachment`]（実体を 1 度読んで同じ経路へ）。
//!   `fulltext_fts`（external content FTS5）はトリガーで自動同期される（db.rs TRIGGERS_V7）
//! - 抽出失敗・テキスト層なし（スキャン PDF）も **空で記録**して理由を残す（二重抽出を避ける）。
//!   実体が無い / CloudStorage で未ダウンロード（dataless）/ 読めない場合は**記録しない**（後で再試行できる）
//! - 内蔵抽出が文字化け（U+FFFD）し pdftotext も無いときは化けたまま status='partial' で記録し、
//!   poppler が入った後の `backfill` で自動的に取り直す（status='ok' でも化けていれば取り直す = 旧索引の自己修復）
//! - 和文は `model::cjk_spaced` で cjk 列へ（items_fts と同じ per-char phrase 方式。検索側 search.rs と対）
//! - MCP `get_fulltext` は同じキャッシュを読む（無ければここで抽出）。頁区切りは \f

use crate::{CoreError, Db, Result};
use rusqlite::OptionalExtension;
use std::path::Path;

/// 抜粋（snippet）の一致箇所を囲むマーカー（私用領域。UI はこれを <mark> に、CLI は [ ] に変える）
pub const HL_START: char = '\u{e000}';
pub const HL_END: char = '\u{e001}';

/// SF_DATALESS（macOS）: File Provider（Google Drive 等）の未ダウンロード実体。open すると取得が走る
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

pub struct FulltextResult {
    pub attachment_id: i64,
    pub page_count: usize,
    pub text: String,
}

/// [`index_attachment`] の結果
#[derive(Debug)]
pub enum IndexOutcome {
    /// 本文を索引した
    Indexed { pages: usize, chars: usize, backend: &'static str },
    /// テキスト層なし（スキャン PDF の可能性）。fulltext に空で記録済み（status='empty'）
    Empty { pages: usize },
    /// 抽出失敗。fulltext に空で記録済み（status='error'・detail=理由）
    Failed(String),
    /// 実体なし / 未ダウンロード / 読めない。**記録していない**（再実行で再試行される）
    Skipped(String),
}

/// 添付 1 本の実体を読んで本文を抽出・索引する（既存添付の一括索引・Paperpile import・MCP get_fulltext 用。
/// 添付直後は [`index_bytes`] = 手元のバイト列から）。
/// 既存の fulltext 行は貼り替える。attachments.md5 が空か実体と違えば実体の値で更新する（md5 列は実体のキャッシュ）。
/// CloudStorage の未ダウンロード実体は開かない（大量ダウンロードを誘発しない）→ [`index_attachment_opts`]
pub fn index_attachment(conn: &rusqlite::Connection, attachment_id: i64) -> Result<IndexOutcome> {
    index_attachment_opts(conn, attachment_id, false)
}

/// `fetch_dataless` = 未ダウンロード（dataless）でも読む = File Provider に取得させる（MCP get_fulltext の単発用）
pub fn index_attachment_opts(
    conn: &rusqlite::Connection,
    attachment_id: i64,
    fetch_dataless: bool,
) -> Result<IndexOutcome> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT path, md5 FROM attachments WHERE id = ?1",
            [attachment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((path, att_md5)) = row else {
        return Err(CoreError::NotFound(format!("attachment {attachment_id}")));
    };
    let p = Path::new(&path);
    let meta = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(_) => return Ok(IndexOutcome::Skipped(format!("実体なし: {path}"))),
    };
    if !fetch_dataless && is_dataless(&meta) {
        return Ok(IndexOutcome::Skipped(format!("未ダウンロード（CloudStorage の dataless）: {path}")));
    }
    let bytes = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) => return Ok(IndexOutcome::Skipped(format!("読めない（{e}）: {path}"))),
    };
    let md5 = crate::pdffetch::md5_hex(&bytes);
    if att_md5.as_deref() != Some(md5.as_str()) {
        conn.execute("UPDATE attachments SET md5 = ?1 WHERE id = ?2", rusqlite::params![md5, attachment_id])?;
    }
    index_bytes(conn, attachment_id, &bytes, p, &md5)
}

/// 添付直後の索引（全添付経路の統一入口 = `edit` の添付関数が呼ぶ）: 今書いたバイト列から本文を抽出する =
/// 書いたファイルを読み直さず、md5 も取り直さない（`md5` は添付行に入れた値。そのまま fulltext.md5 に入れる）。
/// `path` は pdftotext fallback に渡す実体（= 書込先）
pub fn index_bytes(
    conn: &rusqlite::Connection,
    attachment_id: i64,
    bytes: &[u8],
    path: &Path,
    md5: &str,
) -> Result<IndexOutcome> {
    match crate::pdftools::extract_all_bytes(bytes, Some(path)) {
        Ok(ex) => {
            let pages = ex.pages.len();
            conn.execute(
                "UPDATE attachments SET page_count = ?1 WHERE id = ?2",
                rusqlite::params![pages as i64, attachment_id],
            )?;
            if crate::pdftools::has_text(&ex.pages) {
                let text = ex.pages.join("\u{c}");
                let chars = text.chars().count();
                if ex.backend == "builtin-garbled" {
                    // 化けたまま（poppler 無し）。検索には部分的に使えるので入れるが、後で取り直す印を残す
                    let detail = format!(
                        "内蔵抽出が文字化け（U+FFFD {:.0}%）・pdftotext 無し。poppler 導入後の openref fulltext index で取り直し",
                        crate::pdftools::garbled_ratio(&ex.pages) * 100.0
                    );
                    store_text(conn, attachment_id, &text, Some(md5), "partial", Some(&detail))?;
                } else {
                    store_text(conn, attachment_id, &text, Some(md5), "ok", Some(ex.backend))?;
                }
                Ok(IndexOutcome::Indexed { pages, chars, backend: ex.backend })
            } else {
                store_text(conn, attachment_id, "", Some(md5), "empty", Some("テキスト層なし（スキャン PDF の可能性）"))?;
                Ok(IndexOutcome::Empty { pages })
            }
        }
        Err(e) => {
            let reason = e.to_string();
            store_text(conn, attachment_id, "", Some(md5), "error", Some(&reason))?;
            Ok(IndexOutcome::Failed(reason))
        }
    }
}

/// 添付経路用: 索引できなくても添付自体は失敗させない（理由は stderr = connectd ログ / GUI ログへ）
pub fn index_bytes_quietly(conn: &rusqlite::Connection, attachment_id: i64, bytes: &[u8], path: &Path, md5: &str) {
    match index_bytes(conn, attachment_id, bytes, path, md5) {
        Ok(IndexOutcome::Indexed { .. }) | Ok(IndexOutcome::Empty { .. }) => {}
        Ok(IndexOutcome::Failed(r)) | Ok(IndexOutcome::Skipped(r)) => {
            eprintln!("fulltext: 添付 {attachment_id} の本文索引を見送り: {r}")
        }
        Err(e) => eprintln!("fulltext: 添付 {attachment_id} の本文索引でエラー: {e}"),
    }
}

/// 索引・保存用に本文を整える: Latin 合字（U+FB00–FB06 "ﬁ" 等。pdf-extract が出す）を NFKC で素の綴りに畳み、
/// 抜粋マーカー（私用領域 U+E000/E001）が本文に紛れていれば除く。それ以外（全角約物・和文）は触らない
pub fn normalize_text(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            HL_START | HL_END => {}
            '\u{FB00}'..='\u{FB06}' => out.extend(c.nfkc()),
            _ => out.push(c),
        }
    }
    out
}

/// fulltext 行を貼り替える（text が和文を含めば cjk 列に per-char 分かち書きを入れる）。
/// DELETE → INSERT を SAVEPOINT で括る（外側の transaction の有無を問わない）。fulltext_fts はトリガーが同期する
pub fn store_text(
    conn: &rusqlite::Connection,
    attachment_id: i64,
    text: &str,
    md5: Option<&str>,
    status: &str,
    detail: Option<&str>,
) -> Result<()> {
    let text = &normalize_text(text);
    let cjk = if crate::model::has_cjk(text) { crate::model::cjk_spaced(text) } else { String::new() };
    conn.execute_batch("SAVEPOINT ft_store")?;
    let r = (|| -> Result<()> {
        conn.execute("DELETE FROM fulltext WHERE attachment_id = ?1", [attachment_id])?;
        conn.execute(
            "INSERT INTO fulltext (attachment_id, text, cjk, md5, status, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![attachment_id, text, cjk, md5, status, detail],
        )?;
        Ok(())
    })();
    match r {
        Ok(()) => {
            conn.execute_batch("RELEASE ft_store")?;
            Ok(())
        }
        Err(e) => {
            conn.execute_batch("ROLLBACK TO ft_store; RELEASE ft_store")?;
            Err(e)
        }
    }
}

#[cfg(target_os = "macos")]
fn is_dataless(meta: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn is_dataless(_meta: &std::fs::Metadata) -> bool {
    false
}

// ---------- 一括索引（CLI `openref fulltext index`）----------

#[derive(Debug, Default)]
pub struct BackfillReport {
    pub total: usize,
    pub indexed: usize,
    pub empty: usize,
    pub failed: usize,
    /// 索引済み（md5 一致 or 検証不能）で飛ばした
    pub already: usize,
    /// 実体なし / 未ダウンロード / 読めない（記録せず）
    pub skipped: usize,
}

#[derive(Debug)]
pub enum BackfillStep {
    Already,
    Done(IndexOutcome),
}

/// 全 PDF 添付を順に索引する。1 本ごとに autocommit されるので Ctrl-C で止めても再実行で続きから。
/// `force` = 索引済みも抽出し直す。`on_each(i, total, path, step)` は進捗表示用。
/// 索引済み判定 = fulltext 行があり、fulltext.md5 が記録済み（旧バイナリの get_or_extract 由来の md5 無し行は
/// cjk も無いので抽出し直す）、status が 'error' でない（環境要因の失敗は再試行）、attachments.md5 と一致
/// （attachments.md5 が空なら検証不能 = 済み扱い）。さらに
/// - status='partial'（内蔵抽出が文字化け）は pdftotext が見つかるときだけ取り直す
/// - status='ok' でも本文が文字化けしていれば取り直す（文字化けゲート導入前に索引した行の自己修復）
pub fn backfill(
    db: &Db,
    force: bool,
    mut on_each: impl FnMut(usize, usize, &str, &BackfillStep),
) -> Result<BackfillReport> {
    let mut stmt = db.conn.prepare(
        "SELECT a.id, a.path, a.md5, f.md5, f.status, f.text
         FROM attachments a LEFT JOIN fulltext f ON f.attachment_id = a.id
         WHERE a.kind = 'pdf' ORDER BY a.id",
    )?;
    let rows: Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let have_pdftotext = crate::pdftools::find_pdftotext().is_some();
    let mut rep = BackfillReport { total: rows.len(), ..Default::default() };
    for (i, (id, path, a_md5, f_md5, status, text)) in rows.into_iter().enumerate() {
        let done = match (&f_md5, status.as_deref()) {
            (Some(f), Some(st)) => {
                let md5_ok = a_md5.as_deref().map(|a| a == f).unwrap_or(true);
                let garbled = text
                    .as_deref()
                    .map(|t| crate::pdftools::looks_garbled(&[t.to_string()]))
                    .unwrap_or(false);
                match st {
                    "error" => false,
                    "partial" => md5_ok && !have_pdftotext,
                    "ok" => md5_ok && !garbled,
                    _ => md5_ok,
                }
            }
            _ => false,
        };
        let step = if !force && done {
            rep.already += 1;
            BackfillStep::Already
        } else {
            let out = index_attachment(&db.conn, id)?;
            match &out {
                IndexOutcome::Indexed { .. } => rep.indexed += 1,
                IndexOutcome::Empty { .. } => rep.empty += 1,
                IndexOutcome::Failed(_) => rep.failed += 1,
                IndexOutcome::Skipped(_) => rep.skipped += 1,
            }
            BackfillStep::Done(out)
        };
        on_each(i + 1, rep.total, &path, &step);
    }
    Ok(rep)
}

/// FTS 索引を fulltext テーブルから作り直す（external content の 'rebuild'。索引がずれた疑いがあるときの修復）
pub fn rebuild(db: &Db) -> Result<()> {
    db.conn.execute_batch("INSERT INTO fulltext_fts(fulltext_fts) VALUES('rebuild');")?;
    Ok(())
}

/// FTS 索引と fulltext テーブルの整合検査（'integrity-check'）。不整合ならその理由を Some で返す
pub fn integrity_check(db: &Db) -> Result<Option<String>> {
    match db.conn.execute_batch("INSERT INTO fulltext_fts(fulltext_fts) VALUES('integrity-check');") {
        Ok(()) => Ok(None),
        Err(e) => Ok(Some(e.to_string())),
    }
}

/// 索引の現況（CLI `openref fulltext status`）
pub struct Status {
    pub total: i64,
    pub ok: i64,
    /// 内蔵抽出が文字化けし pdftotext 無しで化けたまま（poppler 導入後の index で取り直し）
    pub partial: i64,
    pub empty: i64,
    pub error: i64,
    pub missing: i64,
}

pub fn status(db: &Db) -> Result<Status> {
    let g = |sql: &str| -> Result<i64> { Ok(db.conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(Status {
        total: g("SELECT COUNT(*) FROM attachments WHERE kind = 'pdf'")?,
        ok: g("SELECT COUNT(*) FROM fulltext WHERE status = 'ok'")?,
        partial: g("SELECT COUNT(*) FROM fulltext WHERE status = 'partial'")?,
        empty: g("SELECT COUNT(*) FROM fulltext WHERE status = 'empty'")?,
        error: g("SELECT COUNT(*) FROM fulltext WHERE status = 'error'")?,
        missing: g("SELECT COUNT(*) FROM attachments a WHERE a.kind = 'pdf' AND NOT EXISTS (SELECT 1 FROM fulltext f WHERE f.attachment_id = a.id)")?,
    })
}

// ---------- MCP get_fulltext ----------

pub fn get_or_extract(db: &Db, item_key: &str) -> Result<FulltextResult> {
    let row: Option<(i64, Option<i64>)> = db
        .conn
        .query_row(
            "SELECT a.id, a.page_count FROM attachments a JOIN items i ON i.id = a.item_id
             WHERE i.key = ?1 AND a.kind = 'pdf' ORDER BY a.id LIMIT 1",
            [item_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (att_id, page_count_hint) = row.ok_or_else(|| {
        CoreError::NotFound(format!("item {item_key} に PDF 添付がありません"))
    })?;

    let cached = db
        .conn
        .query_row(
            "SELECT text, status, detail FROM fulltext WHERE attachment_id = ?1",
            [att_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
        )
        .optional()?;
    let (text, status, detail) = match cached {
        Some(c) => c,
        None => {
            // 単発なので未ダウンロードでも読む（File Provider に取得させる。旧 get_or_extract と同じ挙動）
            if let IndexOutcome::Skipped(r) = index_attachment_opts(&db.conn, att_id, true)? {
                return Err(CoreError::NotFound(r));
            }
            db.conn.query_row(
                "SELECT text, status, detail FROM fulltext WHERE attachment_id = ?1",
                [att_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
            )?
        }
    };
    if status == "error" {
        // 失敗は記録済み（再抽出しない）。理由をそのまま返す
        return Err(CoreError::Parse(format!(
            "本文抽出に失敗した記録があります（openref fulltext index --force で再試行）: {}",
            detail.unwrap_or_default()
        )));
    }
    let page_count = if text.is_empty() {
        page_count_hint.unwrap_or(1).max(1) as usize
    } else {
        count_pages(&text)
    };
    Ok(FulltextResult { attachment_id: att_id, page_count, text })
}

fn count_pages(text: &str) -> usize {
    text.trim_end_matches('\u{c}').split('\u{c}').count()
}

/// 1-based・両端 inclusive の頁範囲を切り出す。範囲は clamp。
pub fn slice_pages(text: &str, from: usize, to: usize) -> (String, usize, usize) {
    let pages: Vec<&str> = text.trim_end_matches('\u{c}').split('\u{c}').collect();
    let n = pages.len();
    let from = from.clamp(1, n);
    let to = to.clamp(from, n);
    (pages[from - 1..to].join("\u{c}"), from, to)
}

// ---------- 抜粋の整形 ----------

/// FTS5 `snippet()` の生出力を一覧表示用に整える:
/// 改行・\f・連続空白を 1 空白に畳み、CJK 文字間の空白を除去（cjk 列の per-char 分かち書きを元に戻す。
/// マーカーは透過）、最初の一致箇所の前 `before` 文字・後 `after` 文字に切り詰めて端に … を付ける
/// （一覧の行は 1 行省略表示なので前は短く、一致語が見切れないようにする）
pub fn tidy_snippet(raw: &str, before: usize, after: usize) -> String {
    let is_marker = |c: char| c == HL_START || c == HL_END;
    // 1) 空白の正規化
    let mut flat: Vec<char> = Vec::with_capacity(raw.len());
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() || c == '\u{c}' {
            pending_space = true;
        } else {
            if pending_space && !flat.is_empty() {
                flat.push(' ');
            }
            pending_space = false;
            flat.push(c);
        }
    }
    // 2) CJK 周りの空白を落とす（前後どちらかの非マーカー文字が CJK / 和文約物・全角記号 / 省略記号 … のとき。
    //    cjk 列の per-char 分かち書きは「MRI 検 査」「（ a ）」のように英数・約物との境にも空白を入れるので、
    //    和文の中では空白を全部戻す。英文 snippet（text 列）には CJK が無いので影響しない）
    let glue = |c: char| {
        crate::model::is_cjk(c)
            || c == '…'
            || matches!(c as u32, 0x3000..=0x303F | 0xFF01..=0xFF60 | 0xFFE0..=0xFFEE)
    };
    let mut out: Vec<char> = Vec::with_capacity(flat.len());
    for (i, &c) in flat.iter().enumerate() {
        if c == ' ' {
            let prev = flat[..i].iter().rev().find(|&&x| !is_marker(x)).copied();
            let next = flat[i + 1..].iter().find(|&&x| !is_marker(x)).copied();
            if let (Some(p), Some(n)) = (prev, next) {
                if glue(p) || glue(n) {
                    continue;
                }
            }
        }
        out.push(c);
    }
    // 3) 最初の一致の前 before・後 after 文字に切り詰め
    let first = out.iter().position(|&c| c == HL_START).unwrap_or(0);
    let start = first.saturating_sub(before);
    let end_hint = out[first..].iter().position(|&c| c == HL_END).map(|p| first + p + 1).unwrap_or(first);
    let end = (end_hint + after).min(out.len()).max(start);
    // 4) 切り詰めで対にならなくなったマーカーを落とす（末尾の開き / 先頭の閉じ）
    let mut seg: Vec<char> = Vec::with_capacity(end - start);
    let mut open_at: Option<usize> = None;
    for &c in &out[start..end] {
        match c {
            HL_START => {
                if open_at.is_none() {
                    open_at = Some(seg.len());
                    seg.push(c);
                }
            }
            HL_END => {
                if open_at.is_some() {
                    open_at = None;
                    seg.push(c);
                }
            }
            _ => seg.push(c),
        }
    }
    if let Some(i) = open_at {
        seg.remove(i);
    }
    let mut s = String::new();
    if start > 0 && seg.first() != Some(&'…') {
        s.push('…');
    }
    s.extend(seg.iter());
    if end < out.len() && seg.last() != Some(&'…') {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_slicing() {
        let text = "page one\u{c}page two\u{c}page three";
        let (s, f, t) = slice_pages(text, 2, 2);
        assert_eq!((s.as_str(), f, t), ("page two", 2, 2));
        let (s, f, t) = slice_pages(text, 1, 99);
        assert_eq!((f, t), (1, 3));
        assert!(s.contains("page three"));
        let (_, f, t) = slice_pages(text, 99, 99);
        assert_eq!((f, t), (3, 3));
    }

    #[test]
    fn snippet_tidy_collapses_cjk_and_trims() {
        // cjk 列由来（per-char）: マーカーを跨いでも CJK 周りの空白は落ちる（英数・約物との境界も。「MRI 検 査」→「MRI検査」）
        let raw = format!("… 小 児 の {}胆 道{} 疾 患 （ a ） ． MRI 検 査 ， 適 応 …", HL_START, HL_END);
        let s = tidy_snippet(&raw, 80, 80);
        assert_eq!(s, format!("…小児の{}胆道{}疾患（a）．MRI検査，適応…", HL_START, HL_END));
        // text 列由来（英文）: 改行・\f を畳み、前後 ctx 文字に切る
        let long = format!("{}\n{}zebrafish{} xenograft\u{c}{}", "a".repeat(200), HL_START, HL_END, "b".repeat(200));
        let s = tidy_snippet(&long, 10, 10);
        assert_eq!(s, format!("…{} {}zebrafish{} xenograft…", "a".repeat(9), HL_START, HL_END));
        // マーカー無し（想定外）でも落ちない。after=0・空文字でも panic しない
        assert_eq!(tidy_snippet("plain  text", 80, 80), "plain text");
        assert_eq!(tidy_snippet("", 24, 120), "");
        assert_eq!(tidy_snippet(&format!("{}x{} tail", HL_START, HL_END), 0, 0), format!("{}x{}…", HL_START, HL_END));
        // 切り詰めで 2 つ目の一致の開きだけが残る / 先頭に閉じだけが来る → 落とす
        let raw = format!("{}a{} {}b{}", HL_START, HL_END, HL_START, HL_END);
        assert_eq!(tidy_snippet(&raw, 0, 3), format!("{}a{} b…", HL_START, HL_END));
        assert_eq!(tidy_snippet(&format!("x{} y", HL_END), 24, 120), "x y");
    }

    #[test]
    fn normalize_folds_ligatures_and_strips_markers() {
        assert_eq!(normalize_text("liver ﬁbrosis classiﬁcation ﬂow"), "liver fibrosis classification flow");
        assert_eq!(normalize_text(&format!("a{}b{}c", HL_START, HL_END)), "abc");
        assert_eq!(normalize_text("小児の胆道疾患（a）．"), "小児の胆道疾患（a）．", "和文・全角約物は触らない");
    }
}
