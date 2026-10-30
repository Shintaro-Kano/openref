//! Ingest: PDF ドロップ / 一括取込。
//!
//! 冒頭 2 ページのテキストから DOI/PMID を機械抽出 → resolver で書誌確定 → 登録＋添付。
//! 本文の DOI は**先頭の 1 件だけ**を候補にし、**題名ガード**は「その候補を採用するか、
//! 要確認 stub にするか」だけを決める: resolver の題名（マークアップ除去済み）が冒頭 2 頁の本文にあれば採用、
//! 無ければ不採用 → stub（import_note に理由）。本文が文字化け / 和文が主（CJK 比率）なら照合できないので
//! 従来どおり採用（lenient）。**後続候補を自動採用する経路は持たない**: 参考文献欄の DOI はその論文の題名も
//! 文献リストに載っているのでガードを通ってしまい、自分の DOI の代わりに採用される（別論文の書誌を
//! 付けるキメラレコードになる）。resolver の失敗（doi.org 404 等）も後続へ進まない（GUI / CLI は Err・
//! connectd = `stub_on_fail=false` は `NoIdentifier` + `fail` で受けて stub にする）。
//! 蔵書の実測（`title_guard_survey`・題名は Paperpile 由来）: 先頭 DOI = item の DOI、全文ガードの誤拒否 0
//! （lenient は照合対象外。冒頭 3000 字に絞ると Springer 系で題名が 3000〜4800 字目にあり誤拒否するので全文に広げた）。
//! この母集団では「別論文を登録する」退行は原理的に検出できないので、その系統は自作 PDF で確かめる。
//! 識別子が無ければタイトルを推定して Crossref 書誌検索 → 本文テキストとの照合で
//! 閾値以上のときだけ DOI を採用（捏造しない）。それでも決まらなければ「要確認」stub（添付は保全）。
//! 重複は PMID/DOI に加えファイル md5 で判定（同一ファイルはスキップ・別ファイルは 2 本目として添付）。
//! バイト列は 1 度だけ読み・md5 は 1 度だけ取り（添付と索引へ渡す）・解析は冒頭 2 頁（Info /Title も同じ解析から）と
//! 本文索引の 2 回（`ingest_pdf_bytes_unlocked` = connectd はファイルに落とさず届いた本文をそのまま渡す）。
//! 流れは 3 段（① md5 照合 → ② 抽出・resolver・題名照合 = DB なし → ③ 登録・添付）。DB を Mutex で共有する
//! Tauri / connectd は `*_unlocked` で ② をロックの外に出す（CLI / MCP は便宜関数で続けて呼ぶ）。

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
    /// 識別子なしで stub を作らない設定（connectd は ③ のロックの中で `create_review_stub`（`StubOrigin::Page`）を自分で呼ぶ）
    NoIdentifier,
}

pub struct IngestOpts {
    /// 識別子が無いときタイトル推定 → Crossref 検索を試みる
    pub title_search: bool,
    /// それでも決まらないとき「要確認」stub を作る。false なら NoIdentifier を返す（② の失敗 = 抽出できない・
    /// resolver / 書誌検索の失敗も Err にせず NoIdentifier + `IngestReport::fail` で返す = 呼び手が `create_review_stub` を自分で呼ぶ）
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
    /// ② の失敗（抽出できない・resolver / 書誌検索の失敗）の文。`stub_on_fail=false` のときだけ入る（outcome = NoIdentifier・
    /// reason = None）。true（GUI / CLI）では同じ失敗が Err になる
    pub fail: Option<String>,
    /// 冒頭解析で得た PDF Info /Title（connectd が要確認 stub の題名に使う = 解析し直さない。② の失敗でも
    /// 文書を読めた範囲で入る）。md5 一致で早期 return したときは None
    pub info_title: Option<String>,
    /// 入力バイト列の 16 進 md5（1 度だけ計算したもの。connectd は要確認 stub の添付にそのまま渡す）
    pub md5: String,
}

/// 「要確認」ラベル名（stub 登録時に付与・色は橙）
pub const REVIEW_TAG: &str = "要確認";
pub const REVIEW_TAG_COLOR: &str = "peach"; // 橙（tags::LABEL_COLORS）

pub fn ingest_pdf_report(db: &mut Db, pdf: &Path, opts: &IngestOpts) -> Result<IngestReport> {
    check_pdf_file(db, pdf)?;
    let (bytes, src_name) = read_pdf_file(pdf)?;
    ingest_pdf_bytes_report(db, &bytes, &src_name, Some(pdf), opts)
}

/// 手元のファイルの存在確認と、DB 自身のファイル（本体・WAL）の拒否（DB のパスが要る）
fn check_pdf_file(db: &Db, pdf: &Path) -> Result<()> {
    if !pdf.exists() {
        return Err(CoreError::NotFound(format!("file {}", pdf.display())));
    }
    crate::db::reject_connection_file(&db.conn, pdf)
}

/// 手元のファイルを読む（DB に触らない）。戻り値 = (本文, 元ファイル名)
fn read_pdf_file(pdf: &Path) -> Result<(Vec<u8>, String)> {
    let bytes = std::fs::read(pdf)?;
    let src_name = pdf
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "file.pdf".into());
    Ok((bytes, src_name))
}

/// バイト列版（CLI / MCP など DB を排他で持つ呼び手向けの便宜関数 = ①②③を同じ `db` で続けて行う）。
/// `src_name` = 元ファイル名（csl_filename=false の添付名・要確認 stub の題名の既定）。
/// `file` = 同じ内容が置いてあるファイル（pdftotext fallback 用。None なら必要なときだけ一時ファイルに落とす）。
/// DB を Mutex で共有する呼び手（Tauri・connectd）は `ingest_pdf_bytes_unlocked` を使う
pub fn ingest_pdf_bytes_report(
    db: &mut Db,
    bytes: &[u8],
    src_name: &str,
    file: Option<&Path>,
    opts: &IngestOpts,
) -> Result<IngestReport> {
    bytes_report_with(&Net, db, bytes, src_name, file, opts)
}

fn bytes_report_with(
    net: &dyn Lookup,
    db: &mut Db,
    bytes: &[u8],
    src_name: &str,
    file: Option<&Path>,
    opts: &IngestOpts,
) -> Result<IngestReport> {
    let plan = match ingest_pdf_prepare(db, crate::pdffetch::md5_hex(bytes))? {
        Prepared::Fresh(plan) => plan,
        Prepared::Duplicate(report) => return Ok(report),
    };
    let analyzed = analyze_with(net, bytes, file, opts);
    ingest_pdf_commit(db, plan, analyzed, bytes, src_name, opts)
}

// ---------- 3 段 API（pdffetch の fetch_pdf_prepare / download / attach と同型）----------
// Tauri / connectd は DB を Mutex で持つ。② の抽出（冒頭 2 頁）とネットワーク（resolver・Crossref 書誌検索。
// timeout 30 s・429 で再試行）の間にロックを握ると、一括取込中に一覧・Word・/capture/status が止まるので、
// 「① md5 照合（DB 読取）→ ② ロック外で抽出・識別子・resolver・題名照合 → ③ 再ロックして登録・添付」に分ける。
// ① と ③ の間に同じファイルが別経路で入った場合は、③ の md5 再照合が ① と同じ DuplicateFile で受ける
// （DOI / PMID の重複は add_item の Duplicate → 既存 item への添付判断 = 1 段のときと同じ枝）。
// ③ の添付（edit::write_managed のファイル書込と本文索引）はロックの中のまま。

/// ① の結果: 計画（② へ進む）か、同一ファイルが添付済みで完了か
enum Prepared {
    Fresh(IngestPlan),
    Duplicate(IngestReport),
}

/// ①〜③で持ち回る計画（入力バイト列の 16 進 md5）
struct IngestPlan {
    md5: String,
}

/// ② の結果（DB に触れずに決めたこと）: 確定した書誌（None = 要確認 / 識別子なし）とその理由、または失敗
/// （抽出できない・resolver / 書誌検索の失敗）。Info /Title は失敗でも文書を読めた範囲で入る
struct Analyzed {
    found: Result<(Option<Resolved>, Option<String>)>,
    info_title: Option<String>,
}

/// ① 同一ファイル（md5）が既にどこかの item に添付済みなら、抽出も照会もせず完了（DB 読取のみ・短い）。
/// 要確認 stub の二重作成・同じ PDF の再ドロップを防ぐ。md5 未記録の旧添付は DOI 経路（③）で拾う。
/// `md5` = 本文の 16 進 md5（`pdffetch::md5_hex`。ロックの外で取っておく）
fn ingest_pdf_prepare(db: &Db, md5: String) -> Result<Prepared> {
    Ok(match find_item_by_md5(db, &md5)? {
        Some(key) => Prepared::Duplicate(duplicate_file_report(key, md5)),
        None => Prepared::Fresh(IngestPlan { md5 }),
    })
}

fn duplicate_file_report(key: String, md5: String) -> IngestReport {
    IngestReport { outcome: PdfIngestOutcome::DuplicateFile { key }, warnings: Vec::new(), reason: None, fail: None, info_title: None, md5 }
}

/// ③ 再ロックして登録・添付。② の間にロックを離していたので、まず md5 を取り直す（同じファイルが別経路で
/// 入っていれば ① と同じ DuplicateFile）。② が失敗ならその後で Err（`stub_on_fail=false` は NoIdentifier + fail）を返す
fn ingest_pdf_commit(
    db: &mut Db,
    plan: IngestPlan,
    analyzed: Analyzed,
    bytes: &[u8],
    src_name: &str,
    opts: &IngestOpts,
) -> Result<IngestReport> {
    let file_md5 = plan.md5;
    if let Some(key) = find_item_by_md5(db, &file_md5)? {
        return Ok(duplicate_file_report(key, file_md5));
    }
    let Analyzed { found, info_title } = analyzed;
    let (resolved, reason) = match found {
        Ok(found) => found,
        // connectd は stub を自分で作る（題名の既定・URL・source が違う）ので、失敗の文・Info /Title・md5 を返す（呼び手が PDF を読み直さない）
        Err(e) if !opts.stub_on_fail => {
            let outcome = PdfIngestOutcome::NoIdentifier;
            return Ok(IngestReport { outcome, warnings: Vec::new(), reason: None, fail: Some(e.to_string()), info_title, md5: file_md5 });
        }
        Err(e) => return Err(e),
    };
    let mut warnings: Vec<String> = Vec::new();
    let Some(resolved) = resolved else {
        // ② は書誌が決まらないとき理由を必ず入れる
        let reason = reason.unwrap_or_default();
        if !opts.stub_on_fail {
            let outcome = PdfIngestOutcome::NoIdentifier;
            return Ok(IngestReport { outcome, warnings, reason: Some(reason), fail: None, info_title, md5: file_md5 });
        }
        let origin = StubOrigin::File { name: src_name };
        let (key, _) = create_review_stub(db, &origin, info_title.as_deref(), bytes, &file_md5, &reason, &mut warnings)?;
        let outcome = PdfIngestOutcome::NeedsReview { key };
        return Ok(IngestReport { outcome, warnings, reason: Some(reason), fail: None, info_title, md5: file_md5 });
    };
    let outcome = match ingest::add_item(db, &resolved) {
        Ok(added) => {
            // resolver の警告（PubMed の照合に失敗して doi.org で代用 = 抄録・MeSH・撤回を見ていない）は登録に使ったときだけ添える
            warnings.extend(resolved.warnings.iter().cloned());
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
    Ok(IngestReport { outcome, warnings, reason: None, fail: None, info_title, md5: file_md5 })
}

/// DB を Mutex で共有する呼び手（Tauri・connectd）向け: ①と③だけ `lock` でロックを取り、② はロックの外で行う。
/// 戻り値の guard = 最後に取ったロック（③。① で完了したときは ①）。呼び手は続けて同じロックの中で要約・
/// 要確認 stub の作成をしてよい（③ の md5 再照合と stub 作成の間に別の取込が割り込まない）。
/// 外側の Err は `lock` の失敗だけ。内側が取込の結果（`ingest_pdf_bytes_report` と同じ）
pub fn ingest_pdf_bytes_unlocked<G, E>(
    lock: impl Fn() -> std::result::Result<G, E>,
    bytes: &[u8],
    src_name: &str,
    file: Option<&Path>,
    opts: &IngestOpts,
) -> std::result::Result<(G, Result<IngestReport>), E>
where
    G: std::ops::DerefMut<Target = Db>,
{
    bytes_unlocked_with(&Net, lock, bytes, src_name, file, opts)
}

/// `ingest_pdf_report` の Mutex 版（手元のファイル。存在確認・DB 自身のファイルの拒否は短いロックで、読み込みはロックの外）
fn report_unlocked_with<G, E>(
    net: &dyn Lookup,
    lock: impl Fn() -> std::result::Result<G, E>,
    pdf: &Path,
    opts: &IngestOpts,
) -> std::result::Result<(G, Result<IngestReport>), E>
where
    G: std::ops::DerefMut<Target = Db>,
{
    let checked = {
        let g = lock()?;
        check_pdf_file(&g, pdf)
    };
    match checked.and_then(|()| read_pdf_file(pdf)) {
        Ok((bytes, src_name)) => bytes_unlocked_with(net, lock, &bytes, &src_name, Some(pdf), opts),
        Err(e) => Ok((lock()?, Err(e))),
    }
}

fn bytes_unlocked_with<G, E>(
    net: &dyn Lookup,
    lock: impl Fn() -> std::result::Result<G, E>,
    bytes: &[u8],
    src_name: &str,
    file: Option<&Path>,
    opts: &IngestOpts,
) -> std::result::Result<(G, Result<IngestReport>), E>
where
    G: std::ops::DerefMut<Target = Db>,
{
    let md5 = crate::pdffetch::md5_hex(bytes);
    let plan = {
        let g = lock()?;
        match ingest_pdf_prepare(&g, md5) {
            Ok(Prepared::Fresh(plan)) => plan,
            Ok(Prepared::Duplicate(report)) => return Ok((g, Ok(report))),
            Err(e) => return Ok((g, Err(e))),
        }
    };
    let analyzed = analyze_with(net, bytes, file, opts); // ② ロックの外
    let mut g = lock()?;
    let res = ingest_pdf_commit(&mut g, plan, analyzed, bytes, src_name, opts);
    Ok((g, res))
}

/// ② のネットワーク（resolver・Crossref 書誌検索）。テストでは遅い / 固定のスタブに差し替える
trait Lookup {
    fn resolve(&self, input: &resolver::AddInput) -> Result<Resolved>;
    fn search_bibliographic(&self, q: &str, rows: usize) -> Result<Vec<Candidate>>;
}

struct Net;

impl Lookup for Net {
    fn resolve(&self, input: &resolver::AddInput) -> Result<Resolved> {
        resolver::resolve(input)
    }
    fn search_bibliographic(&self, q: &str, rows: usize) -> Result<Vec<Candidate>> {
        resolver::search_bibliographic(q, rows)
    }
}

/// ② 冒頭 2 頁の抽出 → 識別子 → resolver（題名ガード）/ 題名検索。**DB に触らない**（ロックの外で呼ぶ）。
/// 失敗（抽出できない・resolver の失敗）も Info /Title と一緒に `ingest_pdf_commit` へ渡す（③ が md5 を取り直してから返す）
fn analyze_with(net: &dyn Lookup, bytes: &[u8], file: Option<&Path>, opts: &IngestOpts) -> Analyzed {
    match pdftools::extract_pages_bytes(bytes, file, 1, 2) {
        Ok(ex) => Analyzed { found: identify(net, &ex, opts), info_title: ex.info_title },
        Err(e) => Analyzed { found: Err(e.error), info_title: e.info_title },
    }
}

/// ② の続き: 冒頭 2 頁の本文から書誌を決める（None = 要確認 / 識別子なし。そのときは理由を必ず入れる）
fn identify(net: &dyn Lookup, ex: &pdftools::Extracted, opts: &IngestOpts) -> Result<(Option<Resolved>, Option<String>)> {
    let text = ex.pages.join("\n");
    let lenient = lenient_reason(&ex.pages, &text).is_some();
    // 先頭の識別子 1 件だけ（後続候補は見ない）。解決失敗は Err（connectd は stub にする）
    let (resolved, reason): (Option<Resolved>, Option<String>) = match find_ids_in_text(&text) {
        Some(input) => {
            let r = net.resolve(&input)?;
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
        None if opts.title_search => match title_lookup(net, ex.info_title.as_deref(), &text)? {
            Some(doi) => (Some(net.resolve(&resolver::AddInput::Doi(doi))?), None),
            None => (None, None),
        },
        None => (None, None),
    };
    let reason = match resolved {
        Some(_) => None,
        None => Some(reason.unwrap_or_else(|| {
            if pdftools::has_text(&ex.pages) {
                "識別子なし・タイトル検索も未一致（DOI/PMID を入力して書誌を引き直してください）".to_string()
            } else {
                "テキスト層なし（スキャン PDF の可能性。OCR は未対応）".to_string()
            }
        })),
    };
    Ok((resolved, reason))
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
/// 数えられなければ Err（「stub でない」= 案内なしに畳まない。呼び手が案内の代わりに理由を出す）
pub fn is_review_stub(db: &Db, key: &str) -> Result<bool> {
    Ok(db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id JOIN items i ON i.id = it.item_id
             WHERE i.key = ?1 AND t.name = ?2",
            rusqlite::params![key, REVIEW_TAG],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?)
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
    let res = ingest_pdf_report(db, pdf, &IngestOpts::default());
    summarize_import(db, res)
}

/// `import_summary` の Mutex 版（Tauri の取込モーダル）: ② の抽出・ネットワークをロックの外で行い、
/// 要約は ③ のロックの中で作る。外側の Err は `lock` の失敗だけ
pub fn import_summary_unlocked<G, E>(lock: impl Fn() -> std::result::Result<G, E>, pdf: &Path) -> std::result::Result<ImportSummary, E>
where
    G: std::ops::DerefMut<Target = Db>,
{
    let (db, res) = report_unlocked_with(&Net, lock, pdf, &IngestOpts::default())?;
    Ok(summarize_import(&db, res))
}

/// 取込 1 件の結果を 1 行の要約にする（duplicate の相手が要確認 stub かを DB で見る）
fn summarize_import(db: &Db, res: Result<IngestReport>) -> ImportSummary {
    use PdfIngestOutcome as O;
    let (res, warnings, reason, fail) = match res {
        Ok(r) => (Ok(r.outcome), r.warnings, r.reason, r.fail),
        Err(e) => (Err(e), Vec::new(), None, None),
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
            let hint = match is_review_stub(db, &key) {
                Ok(true) => "（相手は要確認 stub。その item の編集から DOI/PMID を引き直せます）".to_string(),
                Ok(false) => String::new(),
                Err(e) => format!("（相手が要確認 stub かを確かめられません: {e}）"),
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
        Ok(O::NoIdentifier) => match fail {
            // ② の失敗（stub_on_fail=false の報告）は識別子なしではなく失敗 = Err と同じ表示
            Some(f) => ImportSummary { status: "error", key: None, message: f },
            None => ImportSummary {
                status: "no_id",
                key: None,
                message: reason.clone().unwrap_or_else(|| "冒頭 2 頁に DOI / PMID が見つからず登録を見送りました".into()),
            },
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
fn title_lookup(net: &dyn Lookup, info_title: Option<&str>, text: &str) -> Result<Option<String>> {
    if !pdftools::has_text(&[text.to_string()]) {
        return Ok(None);
    }
    for q in title_candidates(info_title, text).iter().take(2) {
        let cands = net.search_bibliographic(q, 5)?;
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

/// 要確認 stub の出どころ。型 document・ラベル「要確認」（色 peach）・custom.import_note は共通で、残りが経路で違う
/// （GUI / CLI / MCP の stub と connectd の stub の差はそのまま残している）
pub enum StubOrigin<'a> {
    /// GUI / CLI / MCP の手元ファイル（`name` = 元ファイル名）: 題名の既定 = ファイル名から .pdf / .PDF を落としたもの
    /// （空ならファイル名）・添付名 = 元ファイル名・custom.source = pdf-import-unresolved・custom.import_file = 元ファイル名・
    /// URL の鍵なし・ラベルの作成 / 付与の失敗は Err
    File { name: &'a str },
    /// connectd（`/capture/pdf` = source capture・saveStandaloneAttachment = connector-standalone）: 題名の既定 =
    /// タブ / ページの題名から .pdf を落としたもの（空なら "Untitled PDF"）・添付名 = 題名から・URL（空なら null）・
    /// ラベルの作成 / 付与の失敗は warnings に入れて続ける（stub と添付は残る）
    Page { title: Option<&'a str>, url: Option<&'a str>, source: &'a str },
}

/// 識別子が無い / 解決できない PDF の stub（捏造しない）を作って添付し、「要確認」ラベルを付ける。
/// 題名は Info /Title（冒頭解析で得たもの = `info_title`。解析し直さない）が妥当ならそれ、無ければ `origin` の既定。
/// `md5` = `bytes` の 16 進 md5（取り直さない）。Drive → managed 降格の理由は `warnings` へ。戻り値 = (item key, 題名)
pub fn create_review_stub(
    db: &mut Db,
    origin: &StubOrigin,
    info_title: Option<&str>,
    bytes: &[u8],
    md5: &str,
    note: &str,
    warnings: &mut Vec<String>,
) -> Result<(String, String)> {
    let info_title = info_title.filter(|t| plausible_title(t)).map(str::to_string);
    // 型は document。型カタログ（types.rs の 9 型）に無い型だと編集の sanitize が「未知の参照タイプ」で弾き、
    // ReferenceEditor も空のフォームになる = 要確認 stub を手で直せない
    let (title, csl, filename) = match origin {
        StubOrigin::File { name } => {
            let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
            let title = info_title.unwrap_or_else(|| if stem.trim().is_empty() { name.to_string() } else { stem.to_string() });
            let csl = json!({
                "type": "document",
                "title": title,
                "custom": { "source": "pdf-import-unresolved", "import_note": note, "import_file": name }
            });
            (title, csl, name.to_string())
        }
        StubOrigin::Page { title: hint, url, source } => {
            let title = info_title
                .or_else(|| hint.map(strip_pdf_suffix).filter(|t| !t.is_empty()))
                .unwrap_or_else(|| "Untitled PDF".into());
            let csl = json!({
                "type": "document",
                "title": title,
                "URL": url.filter(|u| !u.is_empty()),
                "custom": { "source": source, "import_note": note }
            });
            let filename = format!("{}.pdf", safe_filename_stem(&title));
            (title, csl, filename)
        }
    };
    // retracted: None なので source は使われない（撤回の印を書くときだけの値）
    let resolved = resolver::Resolved { csl, retracted: None, source: "pdf-import", warnings: vec![] };
    let added = ingest::add_item(db, &resolved)?;
    note_placement(edit::attach_auto_with_md5(db, &added.key, &filename, bytes, md5)?.placement, warnings);
    // 一覧から見つけられるよう「要確認」ラベル
    let labeled = tags::ensure_tag(db, REVIEW_TAG, Some(REVIEW_TAG_COLOR))
        .map_err(|e| ("要確認ラベル作成失敗", e))
        .and_then(|tid| tags::attach_tag(db, &added.key, tid).map_err(|e| ("要確認ラベル付与失敗", e)));
    match (labeled, origin) {
        (Ok(_), _) => {}
        (Err((_, e)), StubOrigin::File { .. }) => return Err(e),
        (Err((what, e)), StubOrigin::Page { .. }) => warnings.push(format!("{what} {e}")),
    }
    Ok((added.key, title))
}

/// タブ題名のヒントから拡張子を落とす（Chrome の PDF タブ題名はファイル名のことが多い）
pub fn strip_pdf_suffix(t: &str) -> String {
    let t = t.trim();
    let l = t.to_lowercase();
    if l.ends_with(".pdf") {
        t[..t.len() - 4].trim().to_string()
    } else {
        t.to_string()
    }
}

/// managed 添付のファイル名用（Drive モードでは drive_layout が CSL から命名するので使われない）
pub fn safe_filename_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' => ' ',
            c => c,
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = model::truncate_chars(&s, 140);
    let s = s.trim_end_matches('.').trim().to_string();
    if s.is_empty() { "attachment".into() } else { s }
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

    // ---------- 3 段 API（resolver はスタブ = ネットワークなし）----------

    /// 固定の書誌を返すスタブ。`gate` があれば resolve の中で「入った」を送り、「離せ」を待つ（ロック保持の検査用）
    struct Stub {
        gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    }

    const T1: &str = "Imaging of Deep Endometriosis Involving Lateral Pelvic Structures";
    const T2: &str = "Multimodal Large Language Model Retrieval for Follow-up Recommendations";

    impl Lookup for Stub {
        fn resolve(&self, input: &AddInput) -> Result<Resolved> {
            if let Some((entered, release)) = &self.gate {
                entered.send(()).unwrap();
                release.recv_timeout(std::time::Duration::from_secs(10)).expect("テスト側が離さない");
            }
            let (doi, title) = match input {
                AddInput::Doi(d) if d.eq_ignore_ascii_case("10.1148/rg.250127") => ("10.1148/rg.250127", T1),
                AddInput::Doi(d) if d == "10.2/multimodal" => ("10.2/multimodal", T2),
                AddInput::Doi(d) if d == "10.9999/other" => ("10.9999/other", "Structure of the cited protein complex at atomic resolution"),
                AddInput::Doi(d) if d == "10.5555/fallback" => ("10.5555/fallback", T1),
                other => return Err(CoreError::Http(format!("stub: {other:?} は 404"))),
            };
            // 10.5555/fallback = resolver が「PubMed の照合に失敗 → doi.org で代用」の警告を付けた形
            let warnings = if doi == "10.5555/fallback" { vec![FALLBACK_WARNING.to_string()] } else { vec![] };
            Ok(Resolved {
                csl: json!({ "type": "article-journal", "title": title, "DOI": doi, "issued": { "date-parts": [[2026]] },
                             "author": [{ "family": "Brandao", "given": "A" }] }),
                retracted: None,
                source: "stub",
                warnings,
            })
        }
        fn search_bibliographic(&self, _q: &str, _rows: usize) -> Result<Vec<Candidate>> {
            Ok(vec![Candidate {
                doi: "10.2/multimodal".into(),
                title: T2.into(),
                year: Some(2026),
                first_author_family: None,
                kind: Some("journal-article".into()),
            }])
        }
    }

    fn no_gate() -> Stub {
        Stub { gate: None }
    }

    const FALLBACK_WARNING: &str = "PubMed の照合に失敗（stub）→ doi.org の書誌で代用";

    /// resolver の警告は登録（added）の要約の message に ⚠ で載る。既存に添付（書誌を使わない）は載せない
    #[test]
    fn resolver_warnings_reach_the_import_summary() {
        let (dir, mut db) = temp_db("resolver-warn");
        let pdf = |t: &str| crate::pdftools::minimal_pdf(t, None);
        let first = pdf(&format!("RadioGraphics 2026 {T1} Brandao AC doi 10.5555/fallback"));
        let r = bytes_report_with(&no_gate(), &mut db, &first, "a.pdf", None, &IngestOpts::default());
        let s = summarize_import(&db, r);
        assert_eq!(s.status, "added");
        assert_eq!(s.message, format!("書誌を確定して登録・添付しました。⚠ {FALLBACK_WARNING}"));
        let other = pdf(&format!("Supplement 2026 {T1} Brandao AC doi 10.5555/fallback appendix"));
        let r = bytes_report_with(&no_gate(), &mut db, &other, "b.pdf", None, &IngestOpts::default());
        let s = summarize_import(&db, r);
        assert_eq!(s.status, "attached_additional");
        assert!(!s.message.contains("⚠"), "{}", s.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn temp_db(tag: &str) -> (std::path::PathBuf, Db) {
        let dir = std::env::temp_dir().join(format!("openref-ingest3-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("t.sqlite")).unwrap();
        (dir, db)
    }

    /// 区分ごとの PDF（本文 1 行）。同じ DOI でも本文が違えば別ファイル（md5 不一致）
    fn cases() -> Vec<(&'static str, Vec<u8>)> {
        let pdf = |t: &str, info: Option<&str>| crate::pdftools::minimal_pdf(t, info);
        vec![
            ("added", pdf(&format!("RadioGraphics 2026 {T1} Brandao AC doi 10.1148/rg.250127"), None)),
            ("same file", pdf(&format!("RadioGraphics 2026 {T1} Brandao AC doi 10.1148/rg.250127"), None)),
            ("same doi other file", pdf(&format!("Supplement 2026 {T1} Brandao AC doi 10.1148/RG.250127 appendix"), None)),
            ("guard reject", pdf("Footer of another journal issue 2026 with doi 10.9999/other and nothing else here", None)),
            ("title search", pdf(&format!("2026 {T2} discussion of results"), Some(T2))),
            ("no match", pdf("A local hospital newsletter with no identifiers at all inside it", None)),
            ("resolver error", pdf("Some text long enough for the extractor doi 10.4040/missing here", None)),
            ("unparsable", b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n".to_vec()),
        ]
    }

    fn kind(o: &PdfIngestOutcome) -> &'static str {
        match o {
            PdfIngestOutcome::Added { .. } => "Added",
            PdfIngestOutcome::AttachedToExisting { .. } => "AttachedToExisting",
            PdfIngestOutcome::AttachedAdditional { .. } => "AttachedAdditional",
            PdfIngestOutcome::DuplicateFile { .. } => "DuplicateFile",
            PdfIngestOutcome::NeedsReview { .. } => "NeedsReview",
            PdfIngestOutcome::NoIdentifier => "NoIdentifier",
        }
    }

    /// 結果区分・理由・Info /Title・警告・md5（key は乱数なので除く）
    fn label(r: &Result<IngestReport>) -> String {
        match r {
            Ok(r) => format!(
                "{} reason={:?} fail={:?} info={:?} warn={:?} md5={}",
                kind(&r.outcome),
                r.reason,
                r.fail,
                r.info_title,
                r.warnings,
                r.md5
            ),
            Err(e) => format!("Err {e}"),
        }
    }

    /// DB の中身（item の題名・型・import_note・ラベル・添付の名前と md5）を 1 行ずつ
    fn digest(db: &Db) -> Vec<String> {
        let mut stmt = db
            .conn
            .prepare(
                "SELECT i.title, json_extract(i.csl,'$.type'), json_extract(i.csl,'$.custom.import_note'),
                        (SELECT group_concat(a.mode||'|'||a.path||'|'||a.md5, ';') FROM attachments a WHERE a.item_id = i.id),
                        (SELECT group_concat(t.name) FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = i.id)
                 FROM items i ORDER BY i.id",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{:?}|{:?}|{:?}|{:?}|{:?}",
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?
                ))
            })
            .unwrap();
        // 添付のパスは DB ごとの一時ディレクトリと乱数の key を含むので伏せる
        let dir = db.path.as_ref().and_then(|p| p.parent()).map(|d| d.display().to_string()).unwrap_or_default();
        let key = regex::Regex::new(r"/storage/[A-Za-z0-9]+/").unwrap();
        rows.map(|r| key.replace_all(&r.unwrap().replace(&dir, "<DIR>"), "/storage/<KEY>/").into_owned()).collect()
    }

    /// 便宜関数（①②③を同じ db で）と Mutex 版（①③だけロック）で、同じ入力列の結果・DB の中身が一致する。
    /// stub_on_fail の両方（GUI/CLI = true、connectd = false）で
    #[test]
    fn three_stage_unlocked_matches_convenience() {
        for stub_on_fail in [true, false] {
            let opts = IngestOpts { title_search: true, stub_on_fail, csl_filename: !stub_on_fail };
            let (dir_a, mut a) = temp_db(&format!("conv-{stub_on_fail}"));
            let (dir_b, b) = temp_db(&format!("lock-{stub_on_fail}"));
            let b = std::sync::Mutex::new(b);
            for (name, bytes) in cases() {
                let src = format!("{name}.pdf");
                let ra = bytes_report_with(&no_gate(), &mut a, &bytes, &src, None, &opts);
                let (_g, rb) = bytes_unlocked_with(&no_gate(), || b.lock(), &bytes, &src, None, &opts).unwrap();
                assert_eq!(label(&ra), label(&rb), "{name} (stub_on_fail={stub_on_fail})");
            }
            let da = digest(&a);
            assert_eq!(da, digest(&b.lock().unwrap()), "stub_on_fail={stub_on_fail}");
            // 新規 2 件（DOI・題名検索）+ stub_on_fail なら要確認 stub 2 件（題名ガード不採用・未一致）
            assert_eq!(da.len(), if stub_on_fail { 4 } else { 2 }, "{da:#?}");
            std::fs::remove_dir_all(&dir_a).ok();
            std::fs::remove_dir_all(&dir_b).ok();
        }
    }

    /// 区分の表（GUI/CLI = stub_on_fail=true）: 新規 / 同一ファイル / 同 DOI の別ファイル / 題名ガード不採用 / 題名検索 / 未一致 / resolver 失敗 / 解析不能
    #[test]
    fn three_stage_outcomes() {
        let (dir, mut db) = temp_db("outcomes");
        let opts = IngestOpts::default();
        let got: Vec<String> = cases()
            .into_iter()
            .map(|(name, bytes)| match bytes_report_with(&no_gate(), &mut db, &bytes, &format!("{name}.pdf"), None, &opts) {
                Ok(r) => format!("{name}: {}{}", kind(&r.outcome), r.reason.map(|x| format!(" {}", x.chars().take(8).collect::<String>())).unwrap_or_default()),
                Err(_) => format!("{name}: Err"),
            })
            .collect();
        assert_eq!(
            got,
            [
                "added: Added",
                "same file: DuplicateFile",
                "same doi other file: AttachedAdditional",
                "guard reject: NeedsReview 本文の DOI ",
                "title search: Added",
                "no match: NeedsReview 識別子なし・タイ",
                "resolver error: Err",
                "unparsable: Err",
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// GUI / CLI の要確認 stub は型 document（capture の stub と同じ）= 型カタログにある型なので、手で書誌を直す
    /// update_csl（ReferenceEditor の保存）の sanitize を通る。旧の型 article は「未知の参照タイプ」で弾かれていた（いまは
    /// sanitize の入口で document に読み替えるので、1.1.0 までの article の stub も通る）
    #[test]
    fn review_stub_is_document_and_editable() {
        let (dir, mut db) = temp_db("stub-type");
        let bytes = cases().into_iter().find(|(n, _)| *n == "no match").unwrap().1;
        let r = bytes_report_with(&no_gate(), &mut db, &bytes, "newsletter.pdf", None, &IngestOpts::default()).unwrap();
        let PdfIngestOutcome::NeedsReview { key } = r.outcome else { panic!("{:?}", r.outcome) };
        let it = crate::search::get_by_key(&db, &key).unwrap().unwrap();
        assert_eq!(it.csl["type"], "document");
        assert!(crate::types::type_def("document").is_some());
        let mut fixed = it.csl.clone();
        fixed["title"] = json!("Hand-corrected title");
        fixed["URL"] = json!("https://example.org/n");
        edit::update_csl(&mut db, &key, &fixed).expect("要確認 stub の手直しが sanitize を通る");
        let after = crate::search::get_by_key(&db, &key).unwrap().unwrap();
        assert_eq!(after.csl["title"], "Hand-corrected title");
        assert_eq!(after.csl["custom"]["source"], "pdf-import-unresolved", "custom は残る");
        let mut old_type = fixed.clone();
        old_type["type"] = json!("article");
        edit::update_csl(&mut db, &key, &old_type).expect("旧の型 article も document に読み替えて通る");
        let after = crate::search::get_by_key(&db, &key).unwrap().unwrap();
        assert_eq!(after.csl["type"], "document");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ① と ③ の間（② = ロックを離している間）に同じファイルが別経路で入ったら、③ は ① と同じ DuplicateFile を返す。
    /// ② が失敗（resolver 失敗）でも同じ（connectd が二重に stub を作らない）。入っていなければ失敗のまま
    /// （stub_on_fail=false = NoIdentifier + fail）
    #[test]
    fn same_file_between_prepare_and_commit_is_duplicate() {
        let (dir, mut db) = temp_db("race");
        let opts = IngestOpts { title_search: true, stub_on_fail: false, csl_filename: true };
        let all = cases();
        let bytes = &all[0].1;
        let md5 = crate::pdffetch::md5_hex(bytes);
        let Prepared::Fresh(plan) = ingest_pdf_prepare(&db, md5.clone()).unwrap() else { panic!("初回は Fresh") };
        let analyzed = analyze_with(&no_gate(), bytes, None, &opts);
        // ② の間に別の取込が同じファイルを入れる
        let other = bytes_report_with(&no_gate(), &mut db, bytes, "other.pdf", None, &opts).unwrap();
        let PdfIngestOutcome::Added { key: other_key } = other.outcome else { panic!("{:?}", other.outcome) };
        let r = ingest_pdf_commit(&mut db, plan, analyzed, bytes, "late.pdf", &opts).unwrap();
        assert!(matches!(&r.outcome, PdfIngestOutcome::DuplicateFile { key } if *key == other_key), "{:?}", r.outcome);
        assert_eq!(r.md5, md5);
        let n_att = |db: &Db| db.conn.query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(n_att(&db), 1);

        // ② が失敗: 同じファイルが入っていなければ失敗のまま（NoIdentifier + fail）、入っていれば DuplicateFile（stub を作らせない）
        let err_bytes = &all[6].1;
        let md5e = crate::pdffetch::md5_hex(err_bytes);
        let fresh = |db: &Db| match ingest_pdf_prepare(db, md5e.clone()).unwrap() {
            Prepared::Fresh(p) => p,
            Prepared::Duplicate(_) => panic!("まだ入っていない"),
        };
        let (plan_a, plan_b) = (fresh(&db), fresh(&db));
        let r = ingest_pdf_commit(&mut db, plan_a, analyze_with(&no_gate(), err_bytes, None, &opts), err_bytes, "e.pdf", &opts).unwrap();
        assert!(matches!(r.outcome, PdfIngestOutcome::NoIdentifier), "{:?}", r.outcome);
        assert!(r.fail.as_deref().is_some_and(|f| f.contains("404")), "入っていなければ失敗のまま: {:?}", r.fail);
        assert_eq!(r.md5, md5e);
        let stub = ingest::add_item(&mut db, &Resolved { csl: json!({ "type": "document", "title": "stub" }), retracted: None, source: "test", warnings: vec![] }).unwrap();
        edit::attach_auto_with_md5(&db, &stub.key, "s.pdf", err_bytes, &md5e).unwrap();
        let r = ingest_pdf_commit(&mut db, plan_b, analyze_with(&no_gate(), err_bytes, None, &opts), err_bytes, "e.pdf", &opts).unwrap();
        assert!(matches!(&r.outcome, PdfIngestOutcome::DuplicateFile { key } if *key == stub.key), "{:?}", r.outcome);
        assert_eq!(n_att(&db), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// connectd（stub_on_fail=false）は ② の失敗（resolver 失敗・抽出失敗）でも Err にせず NoIdentifier + fail で受け、
    /// 要確認 stub の題名・添付に使う Info /Title と md5 を報告から取る（PDF を読み直さない）。報告の値は、文書を単独で
    /// 読み直した /Title（`info_title_reparse`）と本文の md5 に一致する。GUI / CLI（stub_on_fail=true）は ② の失敗を Err で返す
    #[test]
    fn connectd_failure_report_carries_title_and_md5() {
        let (dir, mut db) = temp_db("fail-report");
        // pdftotext（poppler）の有無に依らず fallback も失敗させる: 別の壊れたファイルを「同じ内容のファイル」として渡す
        let garbage = dir.join("garbage.pdf");
        std::fs::write(&garbage, b"not a pdf").unwrap();
        let pdf = crate::pdftools::minimal_pdf;
        let titled = "Resolver Failure Paper Title For Stub";
        // 頁の抽出で pdf-extract が panic する（フォントの参照先が無い）が、文書と /Info は読める PDF（置換は同じ長さ = xref は有効）
        let font_panic: Vec<u8> = String::from_utf8(pdf("Body text of a paper whose font cannot be decoded", Some("Extraction Failure Title")))
            .unwrap()
            .replace("/Font << /F1 5 0 R >>", "/Font << /F1 9 0 R >>")
            .into_bytes();
        // 名前, 本文, pdftotext に渡すファイル, 期待する Info /Title, ② の失敗か
        type Case<'a> = (&'a str, Vec<u8>, Option<&'a Path>, Option<&'a str>, bool);
        let cases: Vec<Case> = vec![
            ("resolver error", pdf("Some text long enough for the extractor doi 10.4040/missing here", Some(titled)), None, Some(titled), true),
            ("extract panic", font_panic, Some(garbage.as_path()), Some("Extraction Failure Title"), true),
            ("unparsable", b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n".to_vec(), Some(garbage.as_path()), None, true),
            ("no text layer", pdf("", Some("Scanned Page Title")), None, Some("Scanned Page Title"), false),
        ];
        let connectd = IngestOpts { title_search: true, stub_on_fail: false, csl_filename: true };
        for (name, bytes, file, want_title, failed) in &cases {
            let r = bytes_report_with(&no_gate(), &mut db, bytes, "x.pdf", *file, &connectd).unwrap_or_else(|e| panic!("{name}: Err {e}"));
            assert!(matches!(r.outcome, PdfIngestOutcome::NoIdentifier), "{name}: {:?}", r.outcome);
            assert_eq!(r.info_title.as_deref(), *want_title, "{name}");
            assert_eq!(r.info_title, crate::pdftools::info_title_reparse(bytes), "{name}: 文書を単独で読み直した題名と同じ");
            assert_eq!(r.md5, crate::pdffetch::md5_hex(bytes), "{name}: 本文の md5");
            if *failed {
                assert!(r.fail.is_some() && r.reason.is_none(), "{name}: fail={:?} reason={:?}", r.fail, r.reason);
            } else {
                assert_eq!((r.fail.as_deref(), r.reason.as_deref()), (None, Some("テキスト層なし（スキャン PDF の可能性。OCR は未対応）")), "{name}");
            }
            if *name == "resolver error" {
                assert!(r.fail.as_deref().is_some_and(|f| f.contains("10.4040/missing") && f.contains("404")), "{:?}", r.fail);
            }
            // GUI / CLI は ② の失敗を Err のまま返す（stub を作らない）
            if *failed {
                let gui = bytes_report_with(&no_gate(), &mut db, bytes, "x.pdf", *file, &IngestOpts::default());
                assert!(gui.is_err(), "{name}: {}", label(&gui));
            }
            // 要約（GUI モーダル・MCP の import_summary の形）は ② の失敗を「識別子なし」ではなく失敗として出す
            let fail = r.fail.clone();
            let s = summarize_import(&db, Ok(r));
            match fail {
                Some(f) => assert_eq!((s.status, s.key.is_none(), s.message), ("error", true, f), "{name}"),
                None => assert_eq!(s.status, "no_id", "{name}"),
            }
        }
        // connectd の報告では DB に何も作らない（stub は connectd が create_review_stub で作る）。GUI は ② の失敗の 3 件だけ流した = Err で何も作らない
        assert_eq!(db.conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ② の間は DB のロックを握らない: resolver（スタブ）の中で止めている間に、別スレッドがロックを取り DB を読める。
    /// 手元ファイル版（Tauri の import_pdf）とバイト列版（connectd）の両方
    #[test]
    fn lock_is_released_during_network() {
        use std::sync::{mpsc, Arc, Mutex};
        use std::time::{Duration, Instant};
        let (dir, db) = temp_db("lock");
        let pdf_path = dir.join("added.pdf");
        std::fs::write(&pdf_path, &cases()[0].1).unwrap();
        let db = Arc::new(Mutex::new(db));
        for by_path in [true, false] {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let worker = {
                let db = db.clone();
                let pdf_path = pdf_path.clone();
                std::thread::spawn(move || {
                    let stub = Stub { gate: Some((entered_tx, release_rx)) };
                    let opts = IngestOpts::default();
                    let (_g, r) = if by_path {
                        report_unlocked_with(&stub, || db.lock(), &pdf_path, &opts).unwrap()
                    } else {
                        let bytes = crate::pdftools::minimal_pdf(&format!("Second copy 2026 {T1} Brandao doi 10.1148/rg.250127"), None);
                        bytes_unlocked_with(&stub, || db.lock(), &bytes, "b.pdf", None, &opts).unwrap()
                    };
                    r.map(|r| kind(&r.outcome))
                })
            };
            entered_rx.recv_timeout(Duration::from_secs(10)).expect("resolver に入らない");
            // resolver の中で止まっている間に、こちらのスレッドがロックを取れる（握られていれば 2 秒で失敗）
            let t0 = Instant::now();
            let n: i64 = loop {
                match db.try_lock() {
                    Ok(g) => break g.conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0)).unwrap(),
                    Err(_) if t0.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(5)),
                    Err(e) => panic!("② の間に DB のロックが取れない（by_path={by_path}）: {e}"),
                }
            };
            assert_eq!(n, if by_path { 0 } else { 1 }, "② の時点ではまだ登録していない");
            release_tx.send(()).unwrap();
            let outcome = worker.join().unwrap().unwrap();
            assert_eq!(outcome, if by_path { "Added" } else { "AttachedAdditional" });
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Tauri の `import_summary_unlocked` と MCP の `import_summary` が同じ要約を返す（ネットワークに出ない枝:
    /// ファイルなし・DB 自身のファイル・解析できない PDF・md5 一致 = 相手が要確認 stub なら案内付き）
    #[test]
    fn import_summary_unlocked_matches_locked() {
        let (dir_a, mut a) = temp_db("sum-a");
        let (dir_b, b) = temp_db("sum-b");
        let b = std::sync::Mutex::new(b);
        let unparsable = dir_a.join("broken.pdf");
        std::fs::write(&unparsable, b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n").unwrap();
        let dup = dir_a.join("dup.pdf");
        let dup_bytes = crate::pdftools::minimal_pdf("A scanned page that is already attached to a review stub item", None);
        std::fs::write(&dup, &dup_bytes).unwrap();
        for db in [&mut a, &mut *b.lock().unwrap()] {
            let mut w = Vec::new();
            let origin = StubOrigin::File { name: "dup.pdf" };
            create_review_stub(db, &origin, None, &dup_bytes, &crate::pdffetch::md5_hex(&dup_bytes), "test", &mut w).unwrap();
        }
        let strip = |s: ImportSummary| (s.status, s.key.is_some(), s.message.replace(&dir_a.display().to_string(), "<A>").replace(&dir_b.display().to_string(), "<B>"));
        for (name, pa, pb) in [
            ("missing", dir_a.join("missing.pdf"), dir_a.join("missing.pdf")),
            ("connection file", dir_a.join("t.sqlite"), dir_b.join("t.sqlite")),
            ("unparsable", unparsable.clone(), unparsable.clone()),
            ("md5 dup of stub", dup.clone(), dup.clone()),
        ] {
            let sa = strip(import_summary(&mut a, &pa));
            let sb = strip(import_summary_unlocked(|| b.lock(), &pb).unwrap());
            assert_eq!(sa.0, sb.0, "{name}");
            assert_eq!(sa.1, sb.1, "{name}");
            assert_eq!(sa.2.replace("<A>", "<D>"), sb.2.replace("<B>", "<D>").replace("<A>", "<D>"), "{name}");
        }
        assert_eq!(strip(import_summary_unlocked(|| b.lock(), &dup).unwrap()).0, "duplicate");
        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }

    /// 要確認 stub の中身は出どころで違う（GUI / CLI / MCP = File・connectd = Page）。CSL の全体・添付名・ラベルを固定する
    #[test]
    fn review_stub_shape_by_origin() {
        let (dir, mut db) = temp_db("stub-shape");
        let page = |title, url, source| StubOrigin::Page { title, url, source };
        let cases: Vec<(StubOrigin, Option<&str>, &str, &str)> = vec![
            // 題名: Info /Title が妥当ならそれ（どちらの出どころでも）
            (StubOrigin::File { name: "scan 1.pdf" }, Some("A Plausible Title Of The Scanned Article"), r#"{"custom":{"import_file":"scan 1.pdf","import_note":"n","source":"pdf-import-unresolved"},"title":"A Plausible Title Of The Scanned Article","type":"document"}"#, "scan 1.pdf"),
            // File: ファイル名から .pdf / .PDF を落とす・空ならファイル名
            (StubOrigin::File { name: "scan 2.PDF" }, None, r#"{"custom":{"import_file":"scan 2.PDF","import_note":"n","source":"pdf-import-unresolved"},"title":"scan 2","type":"document"}"#, "scan 2.PDF"),
            (StubOrigin::File { name: ".pdf" }, None, r#"{"custom":{"import_file":".pdf","import_note":"n","source":"pdf-import-unresolved"},"title":".pdf","type":"document"}"#, ".pdf"),
            // Page: タブ題名から .pdf を落とす（大小無視）・添付名は題名から・URL は空なら null
            (page(Some(" Tab: title.Pdf "), Some("https://example.org/a.pdf"), "capture"), Some("x"), r#"{"URL":"https://example.org/a.pdf","custom":{"import_note":"n","source":"capture"},"title":"Tab: title","type":"document"}"#, "Tab title.pdf"),
            (page(Some(".pdf"), Some(""), "connector-standalone"), None, r#"{"URL":null,"custom":{"import_note":"n","source":"connector-standalone"},"title":"Untitled PDF","type":"document"}"#, "Untitled PDF.pdf"),
            (page(None, None, "capture"), None, r#"{"URL":null,"custom":{"import_note":"n","source":"capture"},"title":"Untitled PDF","type":"document"}"#, "Untitled PDF.pdf"),
        ];
        for (i, (origin, info, csl, file)) in cases.iter().enumerate() {
            let bytes = crate::pdftools::minimal_pdf(&format!("stub body {i}"), None);
            let mut w = Vec::new();
            let (key, title) = create_review_stub(&mut db, origin, *info, &bytes, &crate::pdffetch::md5_hex(&bytes), "n", &mut w).unwrap();
            assert!(w.is_empty(), "{w:?}");
            let (got_csl, path, tags): (String, String, String) = db
                .conn
                .query_row(
                    "SELECT i.csl, a.path, (SELECT group_concat(t.name||'|'||t.color) FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = i.id)
                     FROM items i JOIN attachments a ON a.item_id = i.id WHERE i.key = ?1",
                    [&key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(got_csl, *csl, "case {i}");
            assert_eq!(serde_json::from_str::<serde_json::Value>(csl).unwrap()["title"], title.as_str(), "case {i}");
            assert_eq!(Path::new(&path).file_name().unwrap().to_str().unwrap(), *file, "case {i}");
            assert_eq!(tags, "要確認|peach", "case {i}");
            assert!(is_review_stub(&db, &key).unwrap());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
