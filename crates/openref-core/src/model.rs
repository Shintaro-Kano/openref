use serde_json::Value;

/// items の 1 行。GUI（Tauri）は search::ItemDetail の中でそのまま JSON にする（camelCase）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRow {
    pub id: i64,
    pub key: String,
    pub csl: Value,
    pub starred: bool,
    pub date_added: String,
}

/// CSL-JSON の文字列フィールドを引く
pub fn s<'a>(csl: &'a Value, key: &str) -> Option<&'a str> {
    csl.get(key).and_then(|v| v.as_str())
}

/// 題名に混じる JATS/HTML マークアップ（Crossref は `<scp>`・`<sup>`・`<sub>`・`<i>` を生で返す）を剥がし、
/// 実体参照（名前付き・数値 `&#8211;` `&#x2013;`）を戻し、空白を畳む。**比較・表示用**（pdfingest の題名ガード、
/// popup の表示）。保存する CSL 題名には掛けない（`<i>` `<sup>` `<sub>` は citeproc が書式として使う = Crossref の文字列をそのまま保存する）。
/// - `</sup>` `</sub>` 直後の改行入り空白は、続く文字が英大文字なら詰める（Crossref の整形改行
///   "Protocol for\n <sup>18</sup>\n F-PSMA" → "Protocol for 18F-PSMA"。同位体・元素記号は大文字で後続語に密着する一方、
///   "CO<sub>2</sub>\n uptake" のような語境界は小文字で始まるので空白を残す）
/// - 復号はタグ除去の後（`&lt;i&gt;` を生タグにしない）
pub fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut after_supsub = false;
    while let Some(c) = chars.next() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let t = tag.trim().to_lowercase();
                after_supsub = t == "/sup" || t == "/sub";
                tag.clear();
            } else {
                tag.push(c);
            }
            continue;
        }
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c.is_whitespace() {
            let mut run = String::from(c);
            while let Some(&n) = chars.peek() {
                if n.is_whitespace() {
                    run.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let next_upper = chars.peek().map(|n| n.is_ascii_uppercase()).unwrap_or(false);
            if !(after_supsub && run.contains('\n') && next_upper) {
                out.push(' ');
            }
            after_supsub = false;
            continue;
        }
        after_supsub = false;
        out.push(c);
    }
    let out = decode_entities(&out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 名前付き実体参照（基本 7 種）と数値参照を復号する
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';').filter(|&k| k <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let decoded: Option<String> = match ent {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" | "#39" => Some("'".into()),
            "nbsp" => Some(" ".into()),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix('x')
                        .or_else(|| n.strip_prefix('X'))
                        .map(|h| u32::from_str_radix(h, 16).ok())
                        .unwrap_or_else(|| n.parse::<u32>().ok())
                })
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
        };
        match decoded {
            Some(d) => {
                out.push_str(&d);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn year(csl: &Value) -> Option<i64> {
    csl.get("issued")?
        .get("date-parts")?
        .get(0)?
        .get(0)?
        .as_i64()
}

pub fn authors(csl: &Value) -> Vec<String> {
    csl.get("author")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    if let Some(l) = a.get("literal").and_then(|v| v.as_str()) {
                        l.to_string()
                    } else {
                        let fam = a.get("family").and_then(|v| v.as_str()).unwrap_or("");
                        let giv = a.get("given").and_then(|v| v.as_str()).unwrap_or("");
                        if giv.is_empty() {
                            fam.to_string()
                        } else {
                            format!("{fam} {giv}")
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 誌名の正規化キー（journal_keys テーブルと誌名チップの照合に使う）。
/// NFKC → 小文字 → 英数字(Unicode)以外は空白 → 連続空白を 1 つ → 先頭の "the " を除去。
/// 実際の書誌で見られる表記ゆれをすべて同じキーに畳む:
///   "Abdom. Radiol. (NY)"（Paperpile）/ "Abdom Radiol (NY)"（PubMed）→ "abdom radiol ny"
///   "Radiographics : a review …, Inc" / "Radiographics: A Review …, Inc" → 同一
///   "Abdominal Radiology (New York)" / "Abdominal radiology (New York)" → 同一
pub fn journal_norm(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = s.nfkc().collect::<String>().to_lowercase();
    let mut out = String::with_capacity(folded.len());
    let mut pending_space = false;
    for ch in folded.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(ch);
        } else {
            pending_space = true;
        }
    }
    match out.strip_prefix("the ") {
        Some(rest) => rest.to_string(),
        None => out,
    }
}

/// item の誌名識別キー群（journal_keys に格納）。同じキーを 1 つでも共有する item 同士を同一誌とみなし、
/// 検索時に推移的に束ねる（print/electronic ISSN の違いは誌名キー経由で、short の有無は full キー経由で繋がる）:
///   "issn:2366-004X"（ISSN は文字列 / 配列 / 区切り文字列のいずれも受ける）
///   "t:<journal_norm(container-title)>"  "t:<journal_norm(container-title-short)>"
pub fn journal_keys(csl: &Value) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut push = |k: String| {
        if !keys.contains(&k) {
            keys.push(k);
        }
    };
    let mut issns: Vec<String> = Vec::new();
    match csl.get("ISSN") {
        Some(Value::String(v)) => issns.push(v.clone()),
        Some(Value::Array(arr)) => issns.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_string)),
        _ => {}
    }
    for raw in issns {
        for part in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
            let v: String = part.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>().to_uppercase();
            if v.len() >= 8 {
                push(format!("issn:{v}"));
            }
        }
    }
    for field in ["container-title", "container-title-short"] {
        if let Some(t) = s(csl, field) {
            let n = journal_norm(t);
            if !n.is_empty() {
                push(format!("t:{n}"));
            }
        }
    }
    keys
}

/// 筆頭著者の given（著者チップのラベル "Family G" と頭文字絞込に使う）。literal 著者や欠損は None
pub fn first_author_given(csl: &Value) -> Option<String> {
    csl.get("author")
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.first())
        .and_then(|a| a.get("given"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string)
}

pub fn first_author_family(csl: &Value) -> String {
    csl.get("author")
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.first())
        .map(|a| {
            a.get("family")
                .or_else(|| a.get("literal"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string()
        })
        .unwrap_or_else(|| "?".to_string())
}

/// CJK（漢字・かな・半角カナ）判定
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF       // ひらがな・カタカナ
        | 0x3400..=0x4DBF     // CJK 拡張 A
        | 0x4E00..=0x9FFF     // CJK 統合漢字
        | 0xF900..=0xFAFF     // CJK 互換漢字
        | 0xFF66..=0xFF9D     // 半角カナ
        | 0x3005              // 々
    )
}

pub fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// CJK 文字を 1 文字 1 トークンに分かち書きする（FTS の cjk 列・query 前処理の共通変換）。
/// 例: "小児の胆道疾患" → "小 児 の 胆 道 疾 患"、"MRI検査" → "MRI 検 査"。
/// index 側と query 側で同じ変換を使うことで、unicode61 のままCJK部分一致（phrase）を成立させる。
pub fn cjk_spaced(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut run = String::new();
    for c in s.chars() {
        if is_cjk(c) {
            if !run.is_empty() {
                out.push(std::mem::take(&mut run));
            }
            out.push(c.to_string());
        } else if c.is_whitespace() {
            if !run.is_empty() {
                out.push(std::mem::take(&mut run));
            }
        } else {
            run.push(c);
        }
    }
    if !run.is_empty() {
        out.push(run);
    }
    out.join(" ")
}

/// char 単位の安全な切り詰め（末尾 …）
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let t: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
