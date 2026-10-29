//! Ingest: PDF ドロップ / 一括取込。
//!
//! 冒頭 2 ページのテキストから DOI/PMID を機械抽出 → resolver で書誌確定 → 登録＋添付。
//! 本文の DOI は**先頭の 1 件だけ**を候補にし、**題名ガード**は「その候補を採用するか、
//! 要確認 stub にするか」だけを決める: resolver の題名（マークアップ除去済み）が冒頭 2 頁の本文にあれば採用、
//! 無ければ不採用 → stub（import_note に理由）。本文が文字化け / 和文が主（CJK 比率）なら照合できないので
//! 従来どおり採用（lenient）。**後続候補を自動採用する経路は持たない**: 参考文献欄の DOI はその論文の題名も
//! 文献リストに載っているのでガードを通ってしまい、自分の DOI の代わりに採用される（別論文の書誌を
//! 付けるキメラレコードになる）。resolver の失敗（doi.org 404 等）も後続へ進まず Err（connectd は stub にする）。
//! 蔵書の実測（`title_guard_survey`・題名は Paperpile 由来）: 先頭 DOI = item の DOI、全文ガードの誤拒否 0
//! （lenient は照合対象外。冒頭 3000 字に絞ると Springer 系で題名が 3000〜4800 字目にあり誤拒否するので全文に広げた）。
//! この母集団では「別論文を登録する」退行は原理的に検出できないので、その系統は自作 PDF で確かめる。
//! 識別子が無ければタイトルを推定して Crossref 書誌検索 → 本文テキストとの照合で
//! 閾値以上のときだけ DOI を採用（捏造しない）。それでも決まらなければ「要確認」stub（添付は保全）。
//! 重複は PMID/DOI に加えファイル md5 で判定（同一ファイルはスキップ・別ファイルは 2 本目として添付）。
//! バイト列は 1 度だけ読み・md5 は 1 度だけ取り（添付と索引へ渡す）・解析は冒頭 2 頁（Info /Title も同じ解析から）と
//! 本文索引の 2 回（`ingest_pdf_bytes_report` = connectd はファイルに落とさず届いた本文をそのまま渡す）。

use crate::resolver::{self, Candidate, Resolved};
use crate::{edit, ingest, model, pdftools, search, tags, CoreError, Db, Result};
use serde_json::json;
use std::path::Path;

#[derive(Debug)]
pub enum PdfIngestOutcome {
    /// 新規登録して添付した
    Added { key: String },
    /// 既存 item（PDF 未添付）に添付した
    AttachedToExisting { key: String },
    /// 既存 item に別ファイル（md5 不一致・補足資料など）を 2 本目として添付した
    AttachedAdditional { key: String },
    /// 既存 item に同一ファイル（md5 一致）が添付済み → スキップ
    DuplicateFile { key: String },
    /// 識別子なし・タイトル検索も未一致 → 「要確認」ラベル付き stub として登録＋添付
    NeedsReview { key: String },
    /// 識別子なしで stub を作らない設定（connectd は自前の stub を持つ）
    NoIdentifier,
}

pub struct IngestOpts {
    /// 識別子が無いときタイトル推定 → Crossref 検索を試みる
    pub title_search: bool,
    /// それでも決まらないとき「要確認」stub を作る（false なら NoIdentifier を返す）
    pub stub_on_fail: bool,
    /// 添付名を item の CSL から付ける（"Family et al. YYYY - Title.pdf"）。false なら元ファイル名
    /// （GUI/CLI の手元ファイルは元の名前を残す。connectd は一時ファイル名を添付名にしないため true）。
    /// Drive モードでは drive_layout が CSL から命名するので、効くのは managed 添付と降格時のみ
    pub csl_filename: bool,
}

impl Default for IngestOpts {
    fn default() -> Self {
        Self { title_search: true, stub_on_fail: true, csl_filename: false }
    }
}

/// `ingest_pdf_report` の戻り値: 結果区分 + 要確認になった理由 + 呼び手がログ/UI に出すべき警告
/// （warnings = Drive → managed 降格の理由。reason = 識別子なし / 題名ガード不採用 / テキスト層なし）。
/// GUI モーダル・CLI・connectd の応答 `reason` はすべてこれを出す
#[derive(Debug)]
pub struct IngestReport {
    pub outcome: PdfIngestOutcome,
    pub warnings: Vec<String>,
    /// NeedsReview / NoIdentifier になった理由（識別子なし・題名ガード不採用・テキスト層なし）
    pub reason: Option<String>,
    /// 冒頭解析で得た PDF Info /Title（connectd が要確認 stub の題名に使う = 解析し直さない）。
    /// md5 一致で早期 return したときは None
    pub info_title: Option<String>,
    /// 入力バイト列の 16 進 md5（1 度だけ計算したもの。connectd は要確認 stub の添付にそのまま渡す）
    pub md5: String,
}

/// 「要確認」ラベル名（stub 登録時に付与・色は橙）
pub const REVIEW_TAG: &str = "要確認";
pub const REVIEW_TAG_COLOR: &str = "peach"; // 橙（tags::LABEL_COLORS）

pub fn ingest_pdf_report(db: &mut Db, pdf: &Path, opts: &IngestOpts) -> Result<IngestReport> {
    if !pdf.exists() {
        return Err(CoreError::NotFound(format!("file {}", pdf.display())));
    }
    crate::db::reject_connection_file(&db.conn, pdf)?;
    let bytes = std::fs::read(pdf)?;
    let src_name = pdf
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "file.pdf".into());
    ingest_pdf_bytes_report(db, &bytes, &src_name, Some(pdf), opts)
}

/// バイト列版（connectd の `/capture/pdf` / standalone: 届いた本文をファイルに落とさず取り込む）。
/// `src_name` = 元ファイル名（csl_filename=false の添付名・要確認 stub の題名の既定）。
/// `file` = 同じ内容が置いてあるファイル（pdftotext fallback 用。None なら必要なときだけ一時ファイルに落とす）
pub fn ingest_pdf_bytes_report(
    db: &mut Db,
    bytes: &[u8],
    src_name: &str,
    file: Option<&Path>,
    opts: &IngestOpts,
) -> Result<IngestReport> {
    let mut warnings: Vec<String> = Vec::new();
    // 同一ファイルが既にどこかの item に添付済みなら、抽出も照会もせず即スキップ
    // （要確認 stub の二重作成・同じ PDF の再ドロップを防ぐ。md5 未記録の旧添付は DOI 経路で拾う）
    let file_md5 = crate::pdffetch::md5_hex(bytes);
    if let Some(key) = find_item_by_md5(db, &file_md5)? {
        return Ok(IngestReport { outcome: PdfIngestOutcome::DuplicateFile { key }, warnings, reason: None, info_title: None, md5: file_md5 });
    }
    let ex = pdftools::extract_pages_bytes(bytes, file, 1, 2)?;
    let text = ex.pages.join("\n");
    let lenient = lenient_reason(&ex.pages, &text).is_some();
    // 先頭の識別子 1 件だけ（後続候補は見ない）。解決失敗は従来どおり Err（connectd は stub にする）
    let (resolved, reason): (Option<Resolved>, Option<String>) = match find_ids_in_text(&text) {
        Some(input) => {
            let r = resolver::resolve(&input)?;
            let title = model::s(&r.csl, "title").unwrap_or("");
            match guard_first_candidate(title, &text, lenient) {
                Guard::Accept => (Some(r), None),
                Guard::Reject => {
                    let label = match &input {
                        resolver::AddInput::Doi(d) => format!("DOI {d}"),
                        resolver::AddInput::Pmid(p) => format!("PMID {p}"),
                    };
                    let why = format!(
                        "本文の {label} の題名「{}」が本文に見当たらず不採用（別論文の識別子の可能性。DOI/PMID を確認して書誌を引き直してください）",
                        model::truncate_chars(&model::strip_markup(title), 120)
                    );
                    (None, Some(why))
                }
            }
        }
        None if opts.title_search => match title_lookup(ex.info_title.as_deref(), &text)? {
            Some(doi) => (Some(resolver::resolve(&resolver::AddInput::Doi(doi))?), None),
            None => (None, None),
        },
        None => (None, None),
    };
    let Some(resolved) = resolved else {
        let reason = reason.unwrap_or_else(|| {
            if pdftools::has_text(&ex.pages) {
                "識別子なし・タイトル検索も未一致（DOI/PMID を入力して書誌を引き直してください）".to_string()
            } else {
                "テキスト層なし（スキャン PDF の可能性。OCR は未対応）".to_string()
            }
        });
        if !opts.stub_on_fail {
            return Ok(IngestReport { outcome: PdfIngestOutcome::NoIdentifier, warnings, reason: Some(reason), info_title: ex.info_title, md5: file_md5 });
        }
        let outcome = create_review_stub(db, ex.info_title.as_deref(), bytes, &file_md5, src_name, &reason, &mut warnings)?;
        return Ok(IngestReport { outcome, warnings, reason: Some(reason), info_title: ex.info_title, md5: file_md5 });
    };
    let outcome = match ingest::add_item(db, &resolved) {
        Ok(added) => {
            let name = if opts.csl_filename { crate::pdffetch::suggest_filename(&resolved.csl) } else { src_name.to_string() };
            note_placement(edit::attach_auto_with_md5(db, &added.key, &name, bytes, &file_md5)?.placement, &mut warnings);
            PdfIngestOutcome::Added { key: added.key }
        }
        Err(CoreError::Duplicate { existing_key, .. }) => {
            let name = attach_name(db, &existing_key, src_name, opts)?;
            let (o, a) = attach_or_skip_with_md5(db, &existing_key, &name, bytes, &file_md5)?;
            if let Some(a) = a {
                note_placement(a.placement, &mut warnings);
            }
            o
        }
        Err(e) => return Err(e),
    };
    Ok(IngestReport { outcome, warnings, reason: None, info_title: ex.info_title, md5: file_md5 })
}

/// 題名ガードの判定（resolver 非依存＝単体テスト可能）。
/// - lenient（文字化け / 和文主体の本文）: 照合できないので Accept（main と同じ挙動）
/// - resolver の題名（マークアップ除去・正規化）が本文に包含、または 4 語以上で語彙 90% 以上 → Accept
/// - 20 字未満の題名は判定不能として Accept
/// - それ以外 → Reject（呼び手は要確認 stub にする。別候補へは進まない）
#[derive(Debug, PartialEq, Eq)]
pub enum Guard {
    Accept,
    Reject,
}

pub fn guard_first_candidate(title: &str, text: &str, lenient: bool) -> Guard {
    if lenient {
        return Guard::Accept;
    }
    // 本文は冒頭 2 頁すべてを正規化して照合する（頁 1 の見出し部に絞ると長い前付けの PDF で誤拒否する）
    if title_in_text(&model::strip_markup(title), &normalize(text)) {
        Guard::Accept
    } else {
        Guard::Reject
    }
}

/// 題名ガードを掛けられない本文か（掛けないときの理由）:
/// - 文字化け（U+FFFD が 1% 超・`pdftools::looks_garbled`）
/// - 和文主体（CJK 20 字以上かつ 10% 以上。実測: J-STAGE 和文は CJK 2119 / ラテン 605、英文論文は 0〜数文字）
/// - 空白主体（非空白が 25% 未満 = グリフが抽出できていない。実測: ToUnicode の無い和文 PDF は空白 95%・CJK 0、
///   通常の英文 PDF は空白 14〜15%）
/// いずれも main と同じく先頭候補をそのまま採用する（照合不能なので stub にはしない）
pub fn lenient_reason(pages: &[String], text: &str) -> Option<&'static str> {
    if pdftools::looks_garbled(pages) {
        return Some("文字化け");
    }
    if is_cjk_heavy(text) {
        return Some("和文主体");
    }
    if text_mostly_blank(text) {
        return Some("空白主体（グリフ未抽出）");
    }
    None
}

/// 非空白文字が全体の 25% 未満（200 字以上あるとき）
pub fn text_mostly_blank(text: &str) -> bool {
    let total = text.chars().count();
    if total < 200 {
        return false;
    }
    let non_ws = text.chars().filter(|c| !c.is_whitespace()).count();
    non_ws * 4 < total
}

/// 和文が主体の本文か: CJK 文字が 20 字以上かつ（CJK + ラテン文字）の 10% 以上。
/// 著者名の漢字表記が数字混じる英文論文（数文字）では発火しない。和文論文は英文抄録があっても本文が CJK 主体
pub fn is_cjk_heavy(text: &str) -> bool {
    let (mut cjk, mut latin) = (0usize, 0usize);
    for c in text.chars() {
        if model::is_cjk(c) {
            cjk += 1;
        } else if c.is_alphabetic() {
            latin += 1;
        }
    }
    cjk >= 20 && cjk * 10 >= cjk + latin
}

/// 「要確認」ラベルが付いている item か（capture / 一括取込 / Connector standalone が識別子なしで作った stub）。
/// duplicate の案内文に使う。ラベルで判定するので、user が編集で書誌を引き直してラベルを外せば案内は消える
/// （custom.source で判定すると修正後も案内が出続け、`/capture/save` の題名のみ document まで stub 扱いになる）
pub fn is_review_stub(db: &Db, key: &str) -> bool {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id JOIN items i ON i.id = it.item_id
             WHERE i.key = ?1 AND t.name = ?2",
            rusqlite::params![key, REVIEW_TAG],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// PDF 取込 1 件の結果（GUI の取込モーダルの行・`commands` の import_pdf が同じものを返す）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportSummary {
    /// added | attached | attached_additional | duplicate | needs_review | no_id | error
    pub status: &'static str,
    pub key: Option<String>,
    /// 理由（要確認の根拠・Drive → managed 降格）も添える（診断を UI へ出す文化）
    pub message: String,
}

/// 手元の PDF 1 本を取り込み（`ingest_pdf_report`・既定の opts）、結果を 1 行の要約にする。失敗も Err にせず status = error で返す
pub fn import_summary(db: &mut Db, pdf: &Path) -> ImportSummary {
    use PdfIngestOutcome as O;
    let (res, warnings, reason) = match ingest_pdf_report(db, pdf, &IngestOpts::default()) {
        Ok(r) => (Ok(r.outcome), r.warnings, r.reason),
        Err(e) => (Err(e), Vec::new(), None),
    };
    let mut r = match res {
        Ok(O::Added { key }) => ImportSummary {
            status: "added",
            key: Some(key),
            message: "書誌を確定して登録・添付しました".into(),
        },
        Ok(O::AttachedToExisting { key }) => ImportSummary {
            status: "attached",
            key: Some(key),
            message: "同じ論文が登録済み（PDF 未添付）だったので添付しました".into(),
        },
        Ok(O::AttachedAdditional { key }) => ImportSummary {
            status: "attached_additional",
            key: Some(key),
            message: "同じ論文が登録済み。別ファイル（md5 不一致）なので 2 本目として添付しました".into(),
        },
        Ok(O::DuplicateFile { key }) => {
            let hint = if is_review_stub(db, &key) {
                "（相手は要確認 stub。その item の編集から DOI/PMID を引き直せます）"
            } else {
                ""
            };
            ImportSummary {
                status: "duplicate",
                key: Some(key),
                message: format!("同一ファイル（md5 一致）が添付済みのためスキップしました{hint}"),
            }
        }
        Ok(O::NeedsReview { key }) => ImportSummary {
            status: "needs_review",
            key: Some(key),
            message: format!(
                "{}。ラベル「要確認」を付けて登録・添付しました（編集で DOI を入れると書誌を引き直せます）",
                reason.clone().unwrap_or_else(|| "DOI/PMID なし・タイトル検索も未一致".into())
            ),
        },
        Ok(O::NoIdentifier) => ImportSummary {
            status: "no_id",
            key: None,
            message: reason.clone().unwrap_or_else(|| "冒頭 2 頁に DOI / PMID が見つからず登録を見送りました".into()),
        },
        Err(CoreError::Duplicate { existing_key, field }) => ImportSummary {
            status: "duplicate",
            key: Some(existing_key),
            message: format!("登録済みの論文と重複（{field}）"),
        },
        Err(e) => ImportSummary { status: "error", key: None, message: e.to_string() },
    };
    for w in warnings {
        r.message.push_str("。⚠ ");
        r.message.push_str(&w);
    }
    r
}

/// 既存 item への添付名（csl_filename なら item の CSL から、そうでなければ元ファイル名）
fn attach_name(db: &Db, key: &str, src_name: &str, opts: &IngestOpts) -> Result<String> {
    if !opts.csl_filename {
        return Ok(src_name.to_string());
    }
    let it = search::get_by_key(db, key)?.ok_or_else(|| CoreError::NotFound(format!("item {key}")))?;
    Ok(crate::pdffetch::suggest_filename(&it.csl))
}

/// Drive → managed 降格は添付としては成功だが、理由を呼び手へ伝える（ログ / UI 診断用）
fn note_placement(p: edit::Placement, warnings: &mut Vec<String>) {
    if let edit::Placement::ManagedFallback { reason, .. } = p {
        warnings.push(reason);
    }
}

/// resolver の題名が本文にあるか（正規化して包含、空白を全部落としても包含 = マークアップ由来の空白ずれ
/// "18 F-PSMA" vs "18F-PSMA" やハイフン改行 "embed- ding" に耐える、または 4 語以上で語彙 90% 以上）。
/// 20 字未満の題名は判定できないので通す
pub fn title_in_text(title: &str, text_norm: &str) -> bool {
    let t = normalize(title);
    if t.chars().count() < 20 {
        return true;
    }
    if text_norm.contains(&t) {
        return true;
    }
    let squash = |x: &str| x.chars().filter(|c| *c != ' ').collect::<String>();
    if squash(text_norm).contains(&squash(&t)) {
        return true;
    }
    let toks: Vec<&str> = t.split(' ').filter(|w| !w.is_empty()).collect();
    if toks.len() < 4 {
        return false;
    }
    let words: std::collections::HashSet<&str> = text_norm.split(' ').collect();
    let hit = toks.iter().filter(|w| words.contains(*w)).count();
    hit as f64 / toks.len() as f64 >= 0.9
}

fn find_item_by_md5(db: &Db, md5: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(db
        .conn
        .query_row(
            "SELECT i.key FROM attachments a JOIN items i ON i.id = a.item_id WHERE a.md5 = ?1 LIMIT 1",
            [md5],
            |r| r.get(0),
        )
        .optional()?)
}

/// 既存 item への添付判断: 未添付なら添付、同一ファイル（md5）があればスキップ、別ファイルなら 2 本目。
/// md5 列が空の既存添付は実体から計算して埋める（過去データの遅延バックフィル）。
pub fn attach_or_skip(db: &Db, key: &str, pdf: &Path) -> Result<PdfIngestOutcome> {
    crate::db::reject_connection_file(&db.conn, pdf)?;
    let bytes = std::fs::read(pdf)?;
    let filename = pdf
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "file.pdf".into());
    attach_or_skip_with_md5(db, key, &filename, &bytes, &crate::pdffetch::md5_hex(&bytes)).map(|(o, _)| o)
}

/// `attach_or_skip` のバイト列版。添付したときは行 id・md5・置き場所（Drive / managed 降格）も返す。
/// md5 は計算済みのものを受ける（connectd の `/capture/attachment` は届いた PDF をファイルに落とさず、
/// 先に取った md5 で重複判定 → 同一性照合 → ここ、と同じ md5 を使い回す）。`new_md5` = `bytes` の 16 進 md5
pub fn attach_or_skip_with_md5(
    db: &Db,
    key: &str,
    filename: &str,
    bytes: &[u8],
    new_md5: &str,
) -> Result<(PdfIngestOutcome, Option<edit::Attached>)> {
    let rows: Vec<(i64, String, Option<String>)> = {
        let mut stmt = db.conn.prepare(
            "SELECT a.id, a.path, a.md5 FROM attachments a JOIN items i ON i.id = a.item_id WHERE i.key = ?1",
        )?;
        let it = stmt.query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        it.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if rows.is_empty() {
        let a = edit::attach_auto_with_md5(db, key, filename, bytes, new_md5)?;
        return Ok((PdfIngestOutcome::AttachedToExisting { key: key.to_string() }, Some(a)));
    }
    for (id, path, stored) in rows {
        let h = match stored {
            Some(h) => h,
            None => {
                let p = Path::new(&path);
                if !p.exists() {
                    continue;
                }
                crate::db::reject_connection_file(&db.conn, p)?;
                let h = crate::pdffetch::md5_hex(&std::fs::read(p)?);
                db.conn
                    .execute("UPDATE attachments SET md5 = ?1 WHERE id = ?2", rusqlite::params![h, id])?;
                h
            }
        };
        if h == new_md5 {
            return Ok((PdfIngestOutcome::DuplicateFile { key: key.to_string() }, None));
        }
    }
    let a = edit::attach_auto_with_md5(db, key, filename, bytes, new_md5)?;
    Ok((PdfIngestOutcome::AttachedAdditional { key: key.to_string() }, Some(a)))
}

// ---------- 識別子抽出 ----------

/// 冒頭テキストから DOI（優先）/ PMID を 1 つ抽出する。**取込本体もこの先頭候補だけを使う**
/// （後続候補は採用しない: 参考文献欄の DOI はガードを通るため別論文の登録になる）。
/// `find_dois_in_text` は計測（survey）用
pub fn find_ids_in_text(text: &str) -> Option<resolver::AddInput> {
    if let Some(d) = find_dois_in_text(text).into_iter().next() {
        return Some(resolver::AddInput::Doi(d));
    }
    find_pmid_in_text(text).map(resolver::AddInput::Pmid)
}

/// 本文中の DOI の候補（取込の識別子抽出と、取得物の同一性照合 `pdffetch` が同じ規則を使う）
pub(crate) fn doi_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)\b(10\.\d{4,9}/[-._;()/:A-Za-z0-9]+)").unwrap())
}

/// 本文中の DOI を出現順に（大小無視で重複を除き・最大 5 件）
pub fn find_dois_in_text(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in doi_re().captures_iter(text) {
        let doi = c[1].trim_end_matches(['.', ',', ';', ')', ']']).to_string();
        if !out.iter().any(|d| d.eq_ignore_ascii_case(&doi)) {
            out.push(doi);
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

pub fn find_pmid_in_text(text: &str) -> Option<String> {
    let pmid_re = regex::Regex::new(r"(?i)\bPMID[:\s]*([0-9]{6,9})\b").unwrap();
    pmid_re.captures(text).map(|c| c[1].to_string())
}

// ---------- タイトル推定 → Crossref 検索 → 照合 ----------

/// 検索クエリ候補: PDF Info /Title（妥当なもの）と、冒頭テキストの先頭 ~250 字。最大 2 本。
pub fn title_candidates(info_title: Option<&str>, text: &str) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(t) = info_title {
        if plausible_title(t) {
            v.push(t.trim().to_string());
        }
    }
    let mut head = String::new();
    for line in text.lines().map(str::trim).filter(|l| l.chars().count() >= 4) {
        if !head.is_empty() {
            head.push(' ');
        }
        head.push_str(line);
        if head.chars().count() >= 250 {
            break;
        }
    }
    let head: String = head.chars().take(300).collect();
    if head.chars().count() >= 20 && !v.iter().any(|x| x == &head) {
        v.push(head);
    }
    v
}

/// Info /Title が論文題名として妥当か（ファイル名・DOI・プレースホルダを除外）
pub fn plausible_title(t: &str) -> bool {
    let t = t.trim();
    let n = t.chars().count();
    if !(15..=300).contains(&n) {
        return false;
    }
    let l = t.to_lowercase();
    let bad_suffix = [".pdf", ".doc", ".docx", ".indd", ".qxd"];
    if bad_suffix.iter().any(|s| l.ends_with(s)) {
        return false;
    }
    let bad_contains = ["untitled", "microsoft word", "pii:", "no title"];
    if bad_contains.iter().any(|s| l.contains(s)) {
        return false;
    }
    if l.starts_with("doi") || l.starts_with("10.") || l.starts_with("http") {
        return false;
    }
    l.chars().any(|c| c.is_alphabetic())
}

/// 候補クエリで Crossref を引き、本文テキストと照合して閾値を満たした DOI を返す。
/// `info_title` = 冒頭解析で得た Info /Title（解析し直さない）
fn title_lookup(info_title: Option<&str>, text: &str) -> Result<Option<String>> {
    if !pdftools::has_text(&[text.to_string()]) {
        return Ok(None);
    }
    for q in title_candidates(info_title, text).iter().take(2) {
        let cands = resolver::search_bibliographic(q, 5)?;
        if let Some(m) = pick_match(&cands, text) {
            return Ok(Some(m.doi));
        }
    }
    Ok(None)
}

#[derive(Debug)]
pub struct TitleMatch {
    pub doi: String,
    pub reason: String,
}

/// NFKD で分解してアクセント（結合文字）を落とす → 小文字 → 英数字以外を空白 → 連続空白を 1 つに。
/// 本文側が ASCII 化された著者名（Brandao）でも Crossref の Brandão と照合できるようにする
pub fn normalize(s: &str) -> String {
    use unicode_normalization::char::is_combining_mark;
    use unicode_normalization::UnicodeNormalization;
    let folded: String = s
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    let mut out = String::with_capacity(folded.len());
    let mut prev_space = true;
    for c in folded.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// 捏造防止の閾値: 候補題名が本文に含まれる（or 題名語彙の 90% 以上が本文に出現・4 語以上）
/// かつ 年 または 筆頭著者姓 が本文に出現。
/// 通過が複数（同題の quiz / 正誤表 / プレプリント等）のときは journal-article を優先し、
/// ある DOI が他の DOI の接頭辞なら最短（本体）を採用、それでも割れれば誤帰属を避けて None。
pub fn pick_match(cands: &[Candidate], text: &str) -> Option<TitleMatch> {
    let passing = passing_candidates(cands, text);
    if passing.is_empty() {
        return None;
    }
    if passing.len() == 1 {
        return passing.into_iter().next().map(|(m, _)| m);
    }
    let articles: Vec<&(TitleMatch, Option<String>)> = passing
        .iter()
        .filter(|(_, k)| k.as_deref() == Some("journal-article"))
        .collect();
    let pool: Vec<&(TitleMatch, Option<String>)> =
        if articles.is_empty() { passing.iter().collect() } else { articles };
    let shortest = pool.iter().min_by_key(|(m, _)| m.doi.len()).unwrap();
    let base = shortest.0.doi.to_lowercase();
    if pool.iter().all(|(m, _)| m.doi.to_lowercase().starts_with(&base)) {
        return Some(TitleMatch {
            doi: shortest.0.doi.clone(),
            reason: format!("{}・同題 {} 件中の本体 DOI", shortest.0.reason, pool.len()),
        });
    }
    None
}

/// 閾値を満たす候補をすべて集める（順序は Crossref の関連度順のまま）
fn passing_candidates(cands: &[Candidate], text: &str) -> Vec<(TitleMatch, Option<String>)> {
    let norm_text = normalize(text);
    let words: std::collections::HashSet<&str> = norm_text.split(' ').collect();
    let mut out = Vec::new();
    for c in cands {
        let t = normalize(&c.title);
        if t.chars().count() < 20 {
            continue;
        }
        let toks: Vec<&str> = t.split(' ').filter(|w| !w.is_empty()).collect();
        let contained = norm_text.contains(&t);
        let coverage = if toks.len() >= 4 {
            toks.iter().filter(|w| words.contains(*w)).count() as f64 / toks.len() as f64
        } else {
            0.0
        };
        if !(contained || coverage >= 0.9) {
            continue;
        }
        let year_ok = c.year.map(|y| words.contains(y.to_string().as_str())).unwrap_or(false);
        let author_ok = c
            .first_author_family
            .as_deref()
            .map(normalize)
            .filter(|f| f.chars().count() >= 3)
            .map(|f| norm_text.contains(&f))
            .unwrap_or(false);
        if year_ok || author_ok {
            out.push((
                TitleMatch {
                    doi: c.doi.clone(),
                    reason: format!(
                        "題名{}・{}",
                        if contained { "一致" } else { "語彙 90%+" },
                        match (year_ok, author_ok) {
                            (true, true) => "年+筆頭著者一致",
                            (true, false) => "年一致",
                            _ => "筆頭著者一致",
                        }
                    ),
                },
                c.kind.clone(),
            ));
        }
    }
    out
}

// ---------- 要確認 stub ----------

/// `info_title` = 冒頭解析で得た Info /Title、`md5` = `bytes` の 16 進 md5（どちらも取り直さない）
fn create_review_stub(
    db: &mut Db,
    info_title: Option<&str>,
    bytes: &[u8],
    md5: &str,
    filename: &str,
    note: &str,
    warnings: &mut Vec<String>,
) -> Result<PdfIngestOutcome> {
    let stem = filename.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
    // 題名は捏造しない: Info /Title が妥当ならそれ、無ければファイル名
    let title = info_title
        .filter(|t| plausible_title(t))
        .map(str::to_string)
        .unwrap_or_else(|| if stem.trim().is_empty() { filename.to_string() } else { stem });
    let csl = json!({
        "type": "article",
        "title": title,
        "custom": { "source": "pdf-import-unresolved", "import_note": note, "import_file": filename }
    });
    let resolved = resolver::Resolved { csl, retracted: None, source: "pdf-import" };
    let added = ingest::add_item(db, &resolved)?;
    note_placement(edit::attach_auto_with_md5(db, &added.key, filename, bytes, md5)?.placement, warnings);
    let tid = tags::ensure_tag(db, REVIEW_TAG, Some(REVIEW_TAG_COLOR))?;
    tags::attach_tag(db, &added.key, tid)?;
    Ok(PdfIngestOutcome::NeedsReview { key: added.key })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::AddInput;

    #[test]
    fn dois_in_text_keeps_order_and_dedupes() {
        let text = "See also https://doi.org/10.1148/radiol.240001.\nEditorial\nhttps://doi.org/10.1148/radiol.240002\nDOI: 10.1148/RADIOL.240002";
        assert_eq!(find_dois_in_text(text), vec!["10.1148/radiol.240001".to_string(), "10.1148/radiol.240002".to_string()]);
        // 取込本体は先頭 1 件だけを使う
        assert_eq!(find_ids_in_text(text), Some(AddInput::Doi("10.1148/radiol.240001".into())));
    }

    /// 題名ガード（resolver 非依存）: 通過 / 不通過 / lenient / マークアップ / 文字化け
    #[test]
    fn first_candidate_guard() {
        let text = "Radiology 2026\nImaging of Deep Endometriosis Involving\nLateral Pelvic Structures\nBrandão AC\nReferences\n1. Smith J. A Different Paper Title About Kidneys. Radiology 2020. doi:10.1/x";
        let t = "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures";
        assert_eq!(guard_first_candidate(t, text, false), Guard::Accept);
        assert_eq!(guard_first_candidate("Imaging of Deep Endometriosis: Involving Lateral Pelvic Structures.", text, false), Guard::Accept, "句読点・末尾ピリオド");
        // 参考文献の DOI を先頭に拾った場合: その題名は本文（文献欄）にある → 通ってしまう。だから「別候補へ進む」経路を持たない
        assert_eq!(guard_first_candidate("A Different Paper Title About Kidneys", text, false), Guard::Accept);
        // 本文のどこにも無い題名（フッタの別記事 DOI・題名なし参考文献）→ Reject → 要確認 stub
        assert_eq!(guard_first_candidate("Structure of the cited protein complex at atomic resolution", text, false), Guard::Reject);
        assert_eq!(guard_first_candidate("Short title", text, false), Guard::Accept, "20 字未満は判定不能として通す");
        // lenient（和文主体 / 文字化け）は照合せず通す（main と同じ）
        assert_eq!(guard_first_candidate("Anything at all that is long enough", text, true), Guard::Accept);
        assert_eq!(guard_first_candidate("Anything at all that is long enough", text, false), Guard::Reject, "lenient でなければ照合して不採用");
        // Crossref の題名マークアップ（実例 10.1101/2021.10.26.21265520 = "Protocol for\n <sup>18</sup>\n F-PSMA …"）
        let pre = "medRxiv preprint doi: https://doi.org/10.1101/2021.10.26.21265520\nProtocol for 18F-PSMA PET imaging in staging and management of prostate cancer - a retrospective cohort study\nAuthor One";
        assert_eq!(
            guard_first_candidate("Protocol for\n <sup>18</sup>\n F-PSMA PET imaging in staging and management of prostate cancer - a retrospective cohort study", pre, false),
            Guard::Accept
        );
        let wiley = "Journal of Magnetic Resonance Imaging\nMRI of the Liver: Current Techniques in Clinical Practice\nAuthor";
        assert_eq!(guard_first_candidate("<scp>MRI</scp> of the Liver: Current Techniques in Clinical Practice", wiley, false), Guard::Accept);
        // 和文判定: CJK 20 字以上かつ 10% 以上
        assert!(is_cjk_heavy("日本小児放射線学会雑誌 症例報告 片側眼瞼下垂と弱視を契機にKallmann症候群と診断した3歳男児例 要旨 症例は3歳男児"));
        assert!(!is_cjk_heavy("Imaging of the pancreas (張三, 李四) in a large cohort of patients with chronic pancreatitis and other diseases"), "著者名の漢字数文字では発火しない");
        assert!(!is_cjk_heavy("plain english text"));
        // 空白主体（グリフ未抽出の和文 PDF。例: 空白 95%・CJK 0）→ lenient。通常の英文（空白 15%）は違う
        let blank = format!("2026 doi: 10.1007/x{}CT MRI{}1. Stoffel EM", " ".repeat(2000), " ".repeat(2000));
        assert!(text_mostly_blank(&blank));
        assert_eq!(lenient_reason(&[blank.clone()], &blank), Some("空白主体（グリフ未抽出）"));
        let normal = "Case Report Painless Spontaneous Spinal Epidural Hematoma ".repeat(10);
        assert!(!text_mostly_blank(&normal));
        assert_eq!(lenient_reason(&[normal.clone()], &normal), None);
        let garbled = "\u{FFFD}".repeat(50) + &"abc ".repeat(20);
        assert_eq!(lenient_reason(&[garbled.clone()], &garbled), Some("文字化け"));
    }

    #[test]
    fn ids_from_text() {
        assert_eq!(
            find_ids_in_text("… https://doi.org/10.1148/rg.230216. RSNA …"),
            Some(AddInput::Doi("10.1148/rg.230216".into()))
        );
        assert_eq!(
            find_ids_in_text("DOI: 10.1245/S10434-022-13033-7)"),
            Some(AddInput::Doi("10.1245/S10434-022-13033-7".into()))
        );
        assert_eq!(find_ids_in_text("… PMID: 36804602 …"), Some(AddInput::Pmid("36804602".into())));
        assert_eq!(find_ids_in_text("no identifiers here"), None);
    }

    fn cand(doi: &str, title: &str, year: Option<i64>, fam: Option<&str>) -> Candidate {
        Candidate {
            doi: doi.into(),
            title: title.into(),
            year,
            first_author_family: fam.map(String::from),
            kind: Some("journal-article".into()),
        }
    }

    #[test]
    fn tie_break_same_title_records() {
        let text = "RadioGraphics 2026\nImaging of Deep Endometriosis Involving Lateral Pelvic Structures\nBrandão AC";
        let t = "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures";
        // Crossref 実測: quiz レコード（.quiz）が先頭に来る → 接頭辞規則で本体 DOI を採用
        let quiz = cand("10.1148/rg.250127.quiz", t, Some(2026), Some("Brandão"));
        let body = cand("10.1148/rg.250127", t, Some(2026), Some("Brandão"));
        let m = pick_match(&[quiz.clone(), body.clone()], text).unwrap();
        assert_eq!(m.doi, "10.1148/rg.250127");
        assert!(m.reason.contains("同題 2 件中の本体"), "{}", m.reason);
        // プレプリント（posted-content）と本誌 → journal-article を優先
        let mut pre = cand("10.1101/2025.01.01.000001", t, Some(2026), Some("Brandão"));
        pre.kind = Some("posted-content".into());
        assert_eq!(pick_match(&[pre, body.clone()], text).unwrap().doi, "10.1148/rg.250127");
        // 同題で無関係な 2 誌（接頭辞関係なし・両方 journal-article）→ 誤帰属を避けて None
        let other = cand("10.9999/other.123", t, Some(2026), Some("Brandão"));
        assert!(pick_match(&[other, body], text).is_none());
    }

    #[test]
    fn title_match_threshold() {
        let text = "RadioGraphics 2026\nImaging of Deep Endometriosis Involving\nLateral Pelvic Structures\nBrandão AC, Silva R\nAbstract …";
        // 一致 + 年 + 著者 → 採用
        let c = cand("10.1148/rg.250127", "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures", Some(2026), Some("Brandão"));
        let m = pick_match(&[c.clone()], text).unwrap();
        assert_eq!(m.doi, "10.1148/rg.250127");
        assert!(m.reason.contains("題名一致") && m.reason.contains("年+筆頭著者"), "{}", m.reason);
        // 題名は一致するが年も著者も無い → 捏造防止で不採用
        let c2 = cand("10.1/x", "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures", Some(1999), Some("Nobody"));
        assert!(pick_match(&[c2], text).is_none());
        // 別論文（語彙が半分しか無い）→ 不採用
        let c3 = cand("10.1/y", "Imaging of Superficial Endometriosis of the Abdominal Wall", Some(2026), Some("Brandão"));
        assert!(pick_match(&[c3], text).is_none());
        // 短すぎる題名は汎用語で誤爆するので不採用
        let c4 = cand("10.1/z", "Imaging", Some(2026), Some("Brandão"));
        assert!(pick_match(&[c4], text).is_none());
        // ハイフン改行で 1 語だけ崩れていても語彙 90% + 年で採用（8 語中 7 語一致 = 87.5% は不可、9 語中 8 語 = 88.9% 不可 → 10 語中 9 語で可）
        let text2 = "2026 Multimodal large language model with knowledge retrieval using flowchart embed-\nding for forming follow-up recommendations";
        let c5 = cand("10.2/a", "Multimodal large language model with knowledge retrieval using flowchart embedding for forming follow-up recommendations", Some(2026), None);
        assert!(pick_match(&[c5], text2).is_some());
        // 先に不一致候補が並んでいても後ろの一致候補を拾う
        let m = pick_match(&[cand("10.1/n", "Something Completely Different About Livers", Some(2026), None), c], text).unwrap();
        assert_eq!(m.doi, "10.1148/rg.250127");
    }

    #[test]
    fn candidates_and_plausibility() {
        assert!(plausible_title("Imaging of Deep Endometriosis Involving Lateral Pelvic Structures"));
        assert!(!plausible_title("manuscript_final_v3.pdf"));
        assert!(!plausible_title("doi:10.1148/rg.250127"));
        assert!(!plausible_title("Untitled"));
        assert!(!plausible_title("Microsoft Word - paper.docx"));
        let text = "RSNA\nOriginal Research\nImaging of Deep Endometriosis Involving Lateral Pelvic Structures\nBrandão AC\n";
        let v = title_candidates(Some("Imaging of Deep Endometriosis Involving Lateral Pelvic Structures"), text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures");
        assert!(v[1].starts_with("RSNA Original Research Imaging"));
        // Info が不適切ならテキスト先頭だけ
        let v = title_candidates(Some("paper.pdf"), text);
        assert_eq!(v.len(), 1);
        // 空テキスト → 候補なし
        assert!(title_candidates(None, "").is_empty());
    }

    #[test]
    fn normalize_folds_unicode_and_punct() {
        assert_eq!(normalize("Ｉｍａｇｉｎｇ: of—Deep (Endometriosis)!"), "imaging of deep endometriosis");
        assert_eq!(normalize("Brandão, Müller"), "brandao muller");
    }
}
