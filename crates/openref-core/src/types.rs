//! 参照タイプのカタログ（「型カタログで検証する」思想の実装）。
//! Zotero の 40 型 123 フィールドを CSL 語彙で再構成し、手動入力 UI と検証の SSOT にする。

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Multiline,
    /// CSL name-variable（family/given の配列）
    Names,
    /// CSL date-variable（date-parts）
    Date,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    /// 既定で表示するか（false = 「フィールドを追加」から出す）
    pub core: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TypeDef {
    pub id: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub fields: Vec<FieldDef>,
}

const fn f(key: &'static str, label: &'static str, kind: FieldKind, core: bool) -> FieldDef {
    FieldDef { key, label, kind, core }
}

/// Paperpile 互換のサブタイプ語彙（`custom.subtype` に格納）
pub const SUBTYPES: [&str; 9] = [
    "Research Article",
    "Review",
    "Commentary",
    "Editorial",
    "Letter to the Editor",
    "News",
    "Opinion",
    "Erratum",
    "Retracted",
];

/// 全型に共通の末尾フィールド
fn common_tail() -> Vec<FieldDef> {
    vec![
        f("abstract", "抄録", FieldKind::Multiline, true),
        f("DOI", "DOI", FieldKind::Text, false),
        f("URL", "URL", FieldKind::Text, false),
        f("language", "言語", FieldKind::Text, false),
        f("keyword", "キーワード", FieldKind::Text, false),
        f("note", "メモ", FieldKind::Multiline, false),
    ]
}

fn head(title_label: &'static str) -> Vec<FieldDef> {
    vec![
        f("title", title_label, FieldKind::Text, true),
        f("author", "著者", FieldKind::Names, true),
    ]
}

pub fn catalog() -> Vec<TypeDef> {
    let mut out = Vec::new();

    let mut journal = head("タイトル");
    journal.extend(vec![
        f("container-title", "雑誌名（フル）", FieldKind::Text, true),
        f("container-title-short", "雑誌名（略）", FieldKind::Text, true),
        f("issued", "発行日", FieldKind::Date, true),
        f("volume", "巻", FieldKind::Text, true),
        f("issue", "号", FieldKind::Text, true),
        f("page", "ページ", FieldKind::Text, true),
        f("PMID", "PMID", FieldKind::Text, false),
        f("PMCID", "PMCID", FieldKind::Text, false),
        f("ISSN", "ISSN", FieldKind::Text, false),
        f("publisher", "出版社", FieldKind::Text, false),
    ]);
    journal.extend(common_tail());
    out.push(TypeDef {
        id: "article-journal",
        label: "雑誌論文",
        desc: "学術雑誌に掲載された論文",
        fields: journal,
    });

    let mut book = head("書名");
    book.extend(vec![
        f("editor", "編者", FieldKind::Names, true),
        f("publisher", "出版社", FieldKind::Text, true),
        f("publisher-place", "出版地", FieldKind::Text, true),
        f("issued", "発行日", FieldKind::Date, true),
        f("edition", "版", FieldKind::Text, true),
        f("ISBN", "ISBN", FieldKind::Text, true),
        f("number-of-pages", "総ページ数", FieldKind::Text, false),
        f("collection-title", "シリーズ名", FieldKind::Text, false),
        f("volume", "巻", FieldKind::Text, false),
    ]);
    book.extend(common_tail());
    out.push(TypeDef { id: "book", label: "書籍", desc: "書籍全体", fields: book });

    let mut chapter = head("章タイトル");
    chapter.extend(vec![
        f("container-title", "書名", FieldKind::Text, true),
        f("editor", "編者", FieldKind::Names, true),
        f("publisher", "出版社", FieldKind::Text, true),
        f("publisher-place", "出版地", FieldKind::Text, true),
        f("issued", "発行日", FieldKind::Date, true),
        f("page", "ページ", FieldKind::Text, true),
        f("edition", "版", FieldKind::Text, false),
        f("ISBN", "ISBN", FieldKind::Text, false),
        f("volume", "巻", FieldKind::Text, false),
    ]);
    chapter.extend(common_tail());
    out.push(TypeDef {
        id: "chapter",
        label: "書籍の章",
        desc: "独自のタイトルと著者を持つ章",
        fields: chapter,
    });

    let mut conf = head("演題");
    conf.extend(vec![
        f("container-title", "抄録集・会議録名", FieldKind::Text, true),
        f("event-title", "学会名", FieldKind::Text, true),
        f("event-place", "開催地", FieldKind::Text, true),
        f("issued", "発表日", FieldKind::Date, true),
        f("page", "ページ", FieldKind::Text, false),
        f("publisher", "発行元", FieldKind::Text, false),
    ]);
    conf.extend(common_tail());
    out.push(TypeDef {
        id: "paper-conference",
        label: "学会発表",
        desc: "学会・会議録の発表",
        fields: conf,
    });

    let mut web = head("ページタイトル");
    web.extend(vec![
        f("container-title", "サイト名", FieldKind::Text, true),
        f("URL", "URL", FieldKind::Text, true),
        f("issued", "公開日", FieldKind::Date, true),
        f("accessed", "閲覧日", FieldKind::Date, true),
        f("publisher", "発行者", FieldKind::Text, false),
    ]);
    web.extend(
        common_tail()
            .into_iter()
            .filter(|x| x.key != "URL")
            .collect::<Vec<_>>(),
    );
    out.push(TypeDef {
        id: "webpage",
        label: "Web ページ",
        desc: "オンラインで閲覧したページ",
        fields: web,
    });

    let mut report = head("タイトル");
    report.extend(vec![
        f("publisher", "発行機関", FieldKind::Text, true),
        f("publisher-place", "発行地", FieldKind::Text, true),
        f("issued", "発行日", FieldKind::Date, true),
        f("number", "報告書番号", FieldKind::Text, true),
        f("genre", "種別（ガイドライン等）", FieldKind::Text, true),
        f("collection-title", "シリーズ名", FieldKind::Text, false),
    ]);
    report.extend(common_tail());
    out.push(TypeDef {
        id: "report",
        label: "報告書・ガイドライン",
        desc: "学会ガイドライン・技術報告書など",
        fields: report,
    });

    let mut thesis = head("論文題目");
    thesis.extend(vec![
        f("publisher", "大学・機関", FieldKind::Text, true),
        f("publisher-place", "所在地", FieldKind::Text, true),
        f("issued", "取得年", FieldKind::Date, true),
        f("genre", "学位種別", FieldKind::Text, true),
    ]);
    thesis.extend(common_tail());
    out.push(TypeDef {
        id: "thesis",
        label: "学位論文",
        desc: "博士・修士論文",
        fields: thesis,
    });

    let mut news = head("記事タイトル");
    news.extend(vec![
        f("container-title", "紙誌名", FieldKind::Text, true),
        f("issued", "発行日", FieldKind::Date, true),
        f("section", "セクション", FieldKind::Text, true),
        f("page", "ページ", FieldKind::Text, false),
        f("edition", "版", FieldKind::Text, false),
    ]);
    news.extend(common_tail());
    out.push(TypeDef {
        id: "article-newspaper",
        label: "新聞・一般誌記事",
        desc: "新聞・雑誌などの記事",
        fields: news,
    });

    let mut misc = head("タイトル");
    misc.extend(vec![
        f("publisher", "発行元", FieldKind::Text, true),
        f("issued", "日付", FieldKind::Date, true),
        f("genre", "種別", FieldKind::Text, true),
        f("container-title", "収録先", FieldKind::Text, false),
        f("number", "番号", FieldKind::Text, false),
    ]);
    misc.extend(common_tail());
    out.push(TypeDef {
        id: "document",
        label: "その他",
        desc: "上記に当てはまらない資料",
        fields: misc,
    });

    out
}

/// 型カタログに無い旧い型名を、今のカタログの型に読み替える。`article`（CSL の汎用記事 = 1.1.0 までの GUI 取込の要確認 stub・
/// Zotero 移行の preprint が書いた）は編集できない型だったので `document`（その他）として扱う。他の型名はそのまま返す
pub fn canonical_type(id: &str) -> &str {
    if id == "article" {
        "document"
    } else {
        id
    }
}

pub fn type_def(id: &str) -> Option<TypeDef> {
    catalog().into_iter().find(|t| t.id == id)
}

/// その型で妥当なフィールドか（未知フィールドの混入を防ぐ検証）
pub fn is_valid_field(type_id: &str, field: &str) -> bool {
    // custom / id 等の内部キーは常に許可
    if matches!(field, "type" | "id" | "custom") {
        return true;
    }
    type_def(type_id)
        .map(|t| t.fields.iter().any(|f| f.key == field))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同じキーの kind は全型で同じ。参照エディタ（app cslForm.ts の cslToForm）は型に関係なく
    /// CSL の全キーを「どれかの型のカタログの kind」で欄の値に読むので、これが崩れると型を変えたとき誤変換する
    #[test]
    fn same_key_has_same_kind_in_every_type() {
        let mut seen: std::collections::HashMap<&str, (FieldKind, &str)> = std::collections::HashMap::new();
        for t in catalog() {
            for f in &t.fields {
                if let Some((kind, first)) = seen.get(f.key) {
                    assert_eq!(*kind, f.kind, "{} の kind が {first} と {} で違う", f.key, t.id);
                } else {
                    seen.insert(f.key, (f.kind, t.id));
                }
            }
        }
        assert!(seen.len() > 10);
    }

    #[test]
    fn catalog_shape() {
        let c = catalog();
        assert!(c.len() >= 9);
        let j = type_def("article-journal").unwrap();
        assert!(j.fields.iter().any(|f| f.key == "container-title" && f.core));
        assert!(j.fields.iter().any(|f| f.key == "PMID" && !f.core));
        // webpage は URL を core に持ち、共通側の重複が無い
        let w = type_def("webpage").unwrap();
        assert_eq!(w.fields.iter().filter(|f| f.key == "URL").count(), 1);
        assert!(w.fields.iter().find(|f| f.key == "URL").unwrap().core);
        // 型に無いフィールドは弾く
        assert!(is_valid_field("book", "ISBN"));
        assert!(!is_valid_field("book", "PMID"));
        assert!(is_valid_field("book", "custom"));
    }
}
