//! コマンド登録表 — エージェント（MCP の `call`）が GUI と同じ動詞で読み書きするための一覧。
//! 各エントリは既存の core 関数を呼ぶだけ（Tauri command と同じ関数を指す = GUI parity は構造で担保。
//! `app/src-tauri` の parity test が「Tauri command ⊆ 登録表 ∪ GUI 専用 allowlist」を固定する）。
//! 引数はエージェント向けの形: item は `key`（8 桁）・フォルダは key かパス（'親/子'・大小無視）・ラベルは名前。
//! **数値 id は受けない**（collections.id / tags.id は削除後に再利用される = 握った id が別のものを指しうる）。
//! 旧 MCP の 15 tool の名前は、同じ動詞なら別名（引数は上位互換）、形が違うものは別エントリで残す。

use crate::{
    annotate, bibtex, citestyle, collections, edit, fulltext, kbmanifest, model, pdffetch, pdfingest, resolver,
    search, tags, types, CoreError, Db, Result,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::Path;

/// 登録表の 1 行
pub struct Command {
    pub name: &'static str,
    /// エージェントが読む説明（日本語 1〜2 行）
    pub doc: &'static str,
    /// JSON Schema 風（type: object・properties・required）
    pub args: Value,
    /// 状態を変えるか（MCP は最初の writes の前に DB のスナップショットを取る）
    pub writes: bool,
    /// 旧 MCP の名前など（`find` / `call` で同じエントリに解決する）
    pub aliases: &'static [&'static str],
    pub run: fn(&mut Db, &Value) -> Result<Value>,
}

pub fn registry() -> &'static [Command] {
    static R: std::sync::OnceLock<Vec<Command>> = std::sync::OnceLock::new();
    R.get_or_init(build)
}

/// 名前（別名も）でエントリを引く
pub fn find(name: &str) -> Option<&'static Command> {
    registry().iter().find(|c| c.name == name || c.aliases.contains(&name))
}

/// コマンドを実行する。args は object（null は {}）。未知の引数・必須の欠落はここで弾く（綴り違いを黙って無視しない）
pub fn call(db: &mut Db, name: &str, args: &Value) -> Result<Value> {
    let cmd = find(name).ok_or_else(|| bad(format!("未知のコマンド: {name}（schema の一覧を参照）")))?;
    let empty = json!({});
    let args = if args.is_null() { &empty } else { args };
    check_args(cmd, args)?;
    (cmd.run)(db, args)
}

/// 一覧の Markdown（`schema` tool の「コマンド一覧」節と、手引き文書の commands ブロック = 同じ文字列）
pub fn markdown() -> String {
    let mut out = String::new();
    for c in registry() {
        let mut head = format!("- `{}`（{}", c.name, if c.writes { "書込" } else { "読み" });
        if !c.aliases.is_empty() {
            let a: Vec<String> = c.aliases.iter().map(|a| format!("`{a}`")).collect();
            head.push_str(&format!("・別名 {}", a.join(" ")));
        }
        out.push_str(&format!("{head}）— {}\n", c.doc));
        let required: Vec<&str> = c.args["required"]
            .as_array()
            .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let props = c.args["properties"].as_object();
        let args: Vec<String> = props
            .map(|p| {
                // 必須を先に（required の順）、残りは名前順
                let mut keys: Vec<(&String, &Value)> = p.iter().collect();
                keys.sort_by_key(|(k, _)| required.iter().position(|r| r == k).unwrap_or(usize::MAX));
                keys.into_iter()
                    .map(|(k, v)| {
                        let ty = match v["type"].as_str() {
                            Some("array") => format!("{}[]", v["items"]["type"].as_str().unwrap_or("any")),
                            Some(t) => t.to_string(),
                            None => "any".into(),
                        };
                        let req = if required.contains(&k.as_str()) { "（必須）" } else { "" };
                        let desc = v["description"].as_str().unwrap_or("");
                        if desc.is_empty() {
                            format!("`{k}` {ty}{req}")
                        } else {
                            format!("`{k}` {ty}{req}: {desc}")
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if args.is_empty() {
            out.push_str("  - 引数なし\n");
        } else {
            out.push_str(&format!("  - 引数: {}\n", args.join(" / ")));
        }
    }
    out
}

// ---------------- 引数 ----------------

fn bad(msg: impl Into<String>) -> CoreError {
    CoreError::BadInput(msg.into())
}

fn check_args(cmd: &Command, args: &Value) -> Result<()> {
    let obj = args
        .as_object()
        .ok_or_else(|| bad(format!("{}: args は object（{{...}}）で渡してください", cmd.name)))?;
    let props = cmd.args["properties"].as_object();
    for k in obj.keys() {
        if !props.is_some_and(|p| p.contains_key(k)) {
            let accepted: Vec<&str> = props.map(|p| p.keys().map(String::as_str).collect()).unwrap_or_default();
            return Err(bad(format!(
                "{}: 未知の引数 {k}（受け付ける引数: {}）",
                cmd.name,
                if accepted.is_empty() { "なし".to_string() } else { accepted.join(", ") }
            )));
        }
    }
    for r in cmd.args["required"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
        if obj.get(r).is_none_or(|v| v.is_null()) {
            return Err(bad(format!("{}: 引数 {r} は必須です", cmd.name)));
        }
    }
    Ok(())
}

/// 引数スキーマ: (名前, 型, 説明)。型は string / integer / boolean / object / string[]（文字列の配列）
fn schema(props: &[(&str, &str, &str)], required: &[&str]) -> Value {
    let mut p = serde_json::Map::new();
    for (name, ty, desc) in props {
        let mut v = match *ty {
            "string[]" => json!({"type": "array", "items": {"type": "string"}}),
            t => json!({"type": t}),
        };
        if !desc.is_empty() {
            v["description"] = json!(desc);
        }
        p.insert(name.to_string(), v);
    }
    json!({"type": "object", "properties": p, "required": required})
}

fn opt_str(a: &Value, k: &str) -> Result<Option<String>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad(format!("引数 {k} は文字列で渡してください"))),
    }
}

fn req_str(a: &Value, k: &str) -> Result<String> {
    opt_str(a, k)?.ok_or_else(|| bad(format!("引数 {k} は必須です")))
}

fn opt_bool(a: &Value, k: &str) -> Result<Option<bool>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("引数 {k} は true / false で渡してください"))),
    }
}

fn opt_int(a: &Value, k: &str) -> Result<Option<i64>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| bad(format!("引数 {k} は整数で渡してください"))),
    }
}

fn str_list(a: &Value, k: &str) -> Result<Vec<String>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(v)) => v
            .iter()
            .map(|x| x.as_str().map(str::to_string).ok_or_else(|| bad(format!("引数 {k} は文字列の配列で渡してください"))))
            .collect(),
        Some(_) => Err(bad(format!("引数 {k} は文字列の配列で渡してください"))),
    }
}

// ---------------- フォルダ・ラベルの解決 ----------------

/// フォルダの指定（key かパス）を id に解決する。数値は拒否（id は削除後に再利用される）。
/// key（8 桁）が一致すればそれ、無ければパス: 'Parent/Child' は root から辿る・区切りの無い名前は全体から探す
/// （同名が複数なら曖昧としてパスを求める）。名前の照合は大小無視（DB の NOCASE = ASCII のみ）
pub fn resolve_collection(db: &Db, v: &Value) -> Result<i64> {
    let s = match v {
        Value::String(s) => s.trim(),
        Value::Number(_) => {
            return Err(bad(
                "フォルダの数値 id は受け付けません（id は削除後に別のフォルダへ再利用されるため）。\
                 key（8 桁・collections_tree か sql の collections.key）かパス（'親/子'）で指定してください",
            ))
        }
        _ => return Err(bad("フォルダは key かパス（文字列）で指定してください")),
    };
    if s.is_empty() {
        return Err(bad("フォルダの指定が空です"));
    }
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_alphanumeric()) {
        let by_key: Option<i64> = db
            .conn
            .query_row("SELECT id FROM collections WHERE key = ?1 AND deleted_at IS NULL", [s], |r| r.get(0))
            .optional()?;
        if let Some(id) = by_key {
            return Ok(id);
        }
    }
    let segs: Vec<&str> = collections::segments(s).collect();
    if segs.is_empty() {
        return Err(bad("フォルダの指定が空です"));
    }
    if segs.len() == 1 {
        let mut stmt = db
            .conn
            .prepare("SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE AND deleted_at IS NULL")?;
        let ids: Vec<i64> = stmt.query_map([segs[0]], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        return match ids.len() {
            1 => Ok(ids[0]),
            0 => Err(CoreError::NotFound(format!("フォルダ '{s}'（collections_tree で確認できます）"))),
            _ => Err(bad(format!("フォルダ名 '{s}' が複数あります。'親/{s}' のパスか key で指定してください"))),
        };
    }
    match collections::walk(&db.conn, None, s, false)? {
        collections::Walked::At(Some(id), _) => Ok(id),
        collections::Walked::At(None, _) => Err(bad("フォルダの指定が空です")),
        collections::Walked::Missing(seg) => Err(CoreError::NotFound(format!(
            "フォルダパス '{s}' の '{seg}'（collections_tree で確認できます）"
        ))),
    }
}

/// 実表記のパス（'親/子'。NOCASE で解決したときも DB 上の表記を返す）
fn collection_path(db: &Db, id: i64) -> Result<String> {
    collections::path_of(&db.conn, id)
}

fn collection_key(db: &Db, id: i64) -> Result<String> {
    Ok(db.conn.query_row("SELECT key FROM collections WHERE id = ?1", [id], |r| r.get(0))?)
}

fn collection_ref(db: &Db, id: i64) -> Result<Value> {
    Ok(json!({"key": collection_key(db, id)?, "path": collection_path(db, id)?}))
}

/// ラベルを名前（大小無視）で引く → (id, DB 上の名前)。数値は拒否
fn resolve_tag(db: &Db, v: &Value) -> Result<(i64, String)> {
    let name = match v {
        Value::String(s) => s.trim(),
        Value::Number(_) => {
            return Err(bad("ラベルの数値 id は受け付けません（id は削除後に再利用されるため）。名前で指定してください"))
        }
        _ => return Err(bad("ラベルは名前（文字列）で指定してください")),
    };
    tags::tag_by_name(&db.conn, name)?
        .ok_or_else(|| CoreError::NotFound(format!("ラベル '{name}'（list_tags で確認できます）")))
}

fn tag_arg(a: &Value, k: &str) -> Result<Value> {
    a.get(k).cloned().ok_or_else(|| bad(format!("引数 {k} は必須です")))
}

/// item の詳細（ゴミ箱の item も引く）。無ければ NotFound("item {key}")
fn detail(db: &Db, key: &str) -> Result<search::ItemDetail> {
    search::item_detail(db, key)?.ok_or_else(|| CoreError::NotFound(format!("item {key}")))
}

/// 書き込み応答用のラベル名（詳細と同じ順）。詳細を丸ごと引き直さない（添付の stat まで走る）
fn item_tags(db: &Db, key: &str) -> Result<Vec<String>> {
    search::item_tag_names(db, key)?.ok_or_else(|| CoreError::NotFound(format!("item {key}")))
}

/// MCP の list_items / quick_add の 1 行。**ここは json! で書く snake_case が契約**（エージェント向けの応答形式）。core の行型に付いている
/// Serialize は GUI 向けの camelCase なので、このファイルでは serde_json::to_value(row) を使わない
fn item_summary(li: &search::ListedItem) -> Value {
    let c = &li.item.csl;
    json!({
        "key": li.item.key,
        "title": model::s(c, "title"),
        "first_author": model::first_author_family(c),
        "year": model::year(c),
        "journal": model::s(c, "container-title-short").or_else(|| model::s(c, "container-title")),
        "doi": model::s(c, "DOI"),
        "pmid": model::s(c, "PMID"),
        "starred": li.item.starred,
        "has_pdf": li.has_pdf,
        "has_note": li.has_note,
        "retracted": li.retracted,
        "date_added": li.item.date_added,
        "tags": li.tags.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
        "fulltext_snippet": li.fulltext_snippet.as_deref()
            .map(|s| s.replace(fulltext::HL_START, "[").replace(fulltext::HL_END, "]"))
    })
}

/// keys[] / collection 指定から対象 item key 群を集める（どちらも無ければ空）
fn collect_keys(db: &Db, a: &Value, limit: usize) -> Result<Vec<String>> {
    let keys = str_list(a, "keys")?;
    if !keys.is_empty() {
        return Ok(keys.into_iter().take(limit).collect());
    }
    if let Some(col) = a.get("collection").filter(|v| !v.is_null()) {
        let cid = resolve_collection(db, col)?;
        let rows = search::list_items(db, &search::ListFilter { collection_id: Some(cid), limit, ..Default::default() })?;
        return Ok(rows.iter().map(|r| r.item.key.clone()).collect());
    }
    Ok(Vec::new())
}

// ---------------- 読み ----------------

fn list_items(db: &mut Db, a: &Value) -> Result<Value> {
    let collection_id = a.get("collection").filter(|v| !v.is_null()).map(|v| resolve_collection(db, v)).transpose()?;
    // ラベルは存在を確かめてから id で絞る（綴り違いを「0 件」と見分けられるように）
    let tag_id = a.get("tag").filter(|v| !v.is_null()).map(|v| resolve_tag(db, v).map(|t| t.0)).transpose()?;
    let query = match opt_str(a, "query")? {
        Some(q) => Some(q),
        None => opt_str(a, "search")?,
    };
    let limit = opt_int(a, "limit")?.unwrap_or(50).max(1) as usize;
    // 空白だけの query / author / journal は core（search::list_items）が未指定として扱う
    let f = search::ListFilter {
        search: query,
        collection_id,
        tag_id,
        starred_only: opt_bool(a, "starred_only")?.unwrap_or(false),
        trashed: opt_bool(a, "trashed")?.unwrap_or(false),
        limit,
        author_family: opt_str(a, "author")?,
        author_initial: opt_str(a, "author_initial")?,
        journal: opt_str(a, "journal")?,
        metadata_only: opt_bool(a, "metadata_only")?.unwrap_or(false),
    };
    let rows = search::list_items(db, &f)?;
    Ok(json!({"count": rows.len(), "limit": limit, "items": rows.iter().map(item_summary).collect::<Vec<_>>()}))
}

fn get_item(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let d = detail(db, &key)?;
    let annotation_count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM annotations an JOIN attachments a ON a.id = an.attachment_id WHERE a.item_id = ?1",
        [d.item.id],
        |r| r.get(0),
    )?;
    let notes = &d.notes;
    let cols: Vec<Value> = d.collections.iter().map(|c| collection_ref(db, c.id)).collect::<Result<_>>()?;
    Ok(json!({
        "key": d.item.key,
        "starred": d.item.starred,
        "deleted_at": d.deleted_at,
        "date_added": d.item.date_added,
        "date_modified": d.date_modified,
        "csl": d.item.csl,
        "collections": cols,
        "tags": d.tags.iter().map(|t| json!({"name": t.name, "color": t.color})).collect::<Vec<_>>(),
        "attachments": d.attachments.iter().map(|x| json!({
            "key": x.key,
            "mode": x.mode,
            "path": x.path,
            "filename": Path::new(&x.path).file_name().map(|f| f.to_string_lossy().to_string()),
            "exists": x.exists,
            "page_count": x.page_count,
            "doi_check": x.doi_check
        })).collect::<Vec<_>>(),
        "retraction": d.retraction,
        "annotation_count": annotation_count,
        "note_count": notes.len(),
        "notes": notes.iter().map(note_json).collect::<Vec<_>>()
    }))
}

fn note_json(n: &annotate::NoteRow) -> Value {
    json!({"key": n.key, "author": n.author, "content_md": n.content_md, "date_added": n.date_added, "date_modified": n.date_modified})
}

fn get_fulltext(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let ft = fulltext::get_or_extract(db, &key)?;
    let from = opt_int(a, "from_page")?.unwrap_or(1).max(1) as usize;
    let to = opt_int(a, "to_page")?.map(|v| v.max(1) as usize).unwrap_or(ft.page_count);
    let (text, from, to) = fulltext::slice_pages(&ft.text, from, to);
    let max_chars = opt_int(a, "max_chars")?.unwrap_or(60000).max(1) as usize;
    let total = text.chars().count();
    let text: String = text.chars().take(max_chars).collect();
    Ok(json!({
        "key": key, "page_count": ft.page_count, "from_page": from, "to_page": to,
        "truncated": total > max_chars, "text": text
    }))
}

fn list_annotations(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let ann = annotate::list_annotations(db, &key)?;
    let notes = annotate::list_notes(db, &key)?;
    Ok(json!({
        "annotations": ann.iter().map(|x| json!({
            "key": x.key, "author": x.author, "kind": x.kind, "color": x.color,
            "page": x.page, "quote": x.quote, "comment": x.comment,
            "prefix": x.prefix, "suffix": x.suffix, "date_added": x.date_added
        })).collect::<Vec<_>>(),
        "notes": notes.iter().map(note_json).collect::<Vec<_>>()
    }))
}

fn list_notes(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    Ok(Value::Array(annotate::list_notes(db, &key)?.iter().map(note_json).collect()))
}

fn collections_tree(db: &mut Db, _a: &Value) -> Result<Value> {
    let rows = search::collections(db)?;
    let by_id: std::collections::HashMap<i64, &search::CollectionRow> = rows.iter().map(|r| (r.id, r)).collect();
    let paths = search::collection_paths(&rows);
    Ok(Value::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "key": r.key, "name": r.name, "path": paths[&r.id],
                    "parent": r.parent_id.and_then(|p| by_id.get(&p)).map(|p| p.key.clone()),
                    "n_items": r.n_items
                })
            })
            .collect(),
    ))
}

fn list_tags(db: &mut Db, a: &Value) -> Result<Value> {
    let sort = if opt_bool(a, "by_count")?.unwrap_or(false) { tags::TagSort::Count } else { tags::TagSort::Alphabetical };
    Ok(Value::Array(
        tags::list_tags(db, sort)?
            .iter()
            .map(|t| json!({"name": t.name, "color": t.color, "n_items": t.n_items}))
            .collect(),
    ))
}

fn view_counts(db: &mut Db, _a: &Value) -> Result<Value> {
    let c = search::view_counts(db)?;
    Ok(json!({"all": c.all, "starred": c.starred, "trash": c.trash}))
}

fn membership(db: &mut Db, a: &Value) -> Result<Value> {
    let keys = str_list(a, "keys")?;
    let m = search::membership(db, &keys)?;
    let col_ids: Vec<i64> = m.collections.iter().map(|(id, _)| *id).collect();
    let tag_ids: Vec<i64> = m.tags.iter().map(|(id, _)| *id).collect();
    let col_info = search::collection_keys_and_paths(db, &col_ids)?;
    let tag_names = search::tag_names_by_id(db, &tag_ids)?;
    let cols: Vec<Value> = m
        .collections
        .iter()
        .map(|(id, n)| {
            let (key, path) = col_info.get(id).ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(json!({"key": key, "path": path, "n": n}))
        })
        .collect::<Result<_>>()?;
    let tags: Vec<Value> = m
        .tags
        .iter()
        .map(|(id, n)| {
            let name = tag_names.get(id).ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok(json!({"name": name, "n": n}))
        })
        .collect::<Result<_>>()?;
    Ok(json!({"collections": cols, "tags": tags}))
}

fn bibtex_entries(db: &mut Db, a: &Value) -> Result<Value> {
    Ok(Value::Array(
        bibtex::entries(db, &str_list(a, "keys")?)?
            .into_iter()
            .map(|e| json!({"key": e.key, "citekey": e.citekey, "bibtex": e.bibtex, "doi": e.doi}))
            .collect(),
    ))
}

fn export_bibtex(db: &mut Db, a: &Value) -> Result<Value> {
    let keys = collect_keys(db, a, 10_000)?;
    if keys.is_empty() {
        return Err(bad("keys か collection を指定してください"));
    }
    let mut entries = Vec::new();
    for k in &keys {
        let item = search::get_by_key(db, k)?.ok_or_else(|| CoreError::NotFound(format!("item {k}")))?;
        entries.push(bibtex::item_to_bibtex(&item.csl, &item.key));
    }
    Ok(json!({"count": entries.len(), "bibtex": entries.join("\n\n")}))
}

fn export_kb_manifest(db: &mut Db, a: &Value) -> Result<Value> {
    let mut keys = collect_keys(db, a, 10_000)?;
    if keys.is_empty() {
        let since = opt_str(a, "since")?;
        let mut stmt = db.conn.prepare(
            "SELECT key FROM items WHERE deleted_at IS NULL AND (?1 IS NULL OR date_added >= ?1) ORDER BY date_added",
        )?;
        keys = stmt.query_map(rusqlite::params![since], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    }
    let mut records = Vec::new();
    for k in &keys {
        if let Some(item) = search::get_by_key(db, k)? {
            records.push(kbmanifest::manifest_record(&item.key, &item.csl, &item.date_added));
        }
    }
    Ok(json!({"count": records.len(), "records": records}))
}

fn item_types(_db: &mut Db, _a: &Value) -> Result<Value> {
    Ok(Value::Array(
        types::catalog()
            .into_iter()
            .map(|t| {
                json!({
                    "id": t.id, "label": t.label, "desc": t.desc,
                    "fields": t.fields.iter().map(|f| json!({
                        "key": f.key, "label": f.label, "kind": f.kind, "core": f.core
                    })).collect::<Vec<_>>()
                })
            })
            .collect(),
    ))
}

fn subtypes(_db: &mut Db, _a: &Value) -> Result<Value> {
    Ok(json!(types::SUBTYPES))
}

fn lookup(_db: &mut Db, a: &Value) -> Result<Value> {
    let parsed = resolver::parse_input(&req_str(a, "input")?)?;
    Ok(resolver::resolve(&parsed)?.csl)
}

fn cite_prefs(db: &mut Db, _a: &Value) -> Result<Value> {
    citestyle::prefs(db)
}

fn cite_installed(db: &mut Db, _a: &Value) -> Result<Value> {
    Ok(Value::Array(citestyle::installed(&citestyle::styles_dir(db))))
}

fn cite_style_xml(db: &mut Db, a: &Value) -> Result<Value> {
    let id = req_str(a, "id")?;
    let xml = citestyle::style_xml(&citestyle::styles_dir(db), &id)?;
    Ok(json!({"id": id, "xml": xml}))
}

fn cite_locale_xml(db: &mut Db, a: &Value) -> Result<Value> {
    let lang = req_str(a, "lang")?;
    let xml = citestyle::locale_xml(&citestyle::styles_dir(db), &lang)?;
    Ok(json!({"lang": lang, "xml": xml}))
}

// ---------------- 書込: item ----------------

fn set_star(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let starred = opt_bool(a, "starred")?.ok_or_else(|| bad("引数 starred は必須です"))?;
    edit::set_starred(db, &key, starred)?;
    Ok(json!({"key": key, "starred": starred}))
}

fn trash_item(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    edit::trash(db, &key)?;
    Ok(json!({"key": key, "trashed": true}))
}

fn restore_item(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    edit::restore(db, &key)?;
    Ok(json!({"key": key, "trashed": false}))
}

fn purge_item(db: &mut Db, a: &Value) -> Result<Value> {
    let p = edit::purge(db, &req_str(a, "key")?)?;
    Ok(json!({"key": p.key, "files_removed": p.files_removed, "files_kept": p.files_kept, "files_failed": p.files_failed}))
}

fn create_manual(db: &mut Db, a: &Value) -> Result<Value> {
    let csl = a.get("csl").filter(|v| v.is_object()).ok_or_else(|| bad("引数 csl は CSL-JSON の object です"))?;
    Ok(json!({"key": edit::create_manual(db, csl)?}))
}

fn update_item(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let csl = a.get("csl").filter(|v| v.is_object()).ok_or_else(|| bad("引数 csl は CSL-JSON の object です"))?;
    edit::update_csl(db, &key, csl)?;
    Ok(json!({"key": key}))
}

fn quick_add(db: &mut Db, a: &Value) -> Result<Value> {
    let parsed = resolver::parse_input(&req_str(a, "input")?)?;
    let resolved = resolver::resolve(&parsed)?;
    let added = crate::ingest::add_item(db, &resolved)?;
    let d = search::item_detail(db, &added.key)?.ok_or_else(|| CoreError::NotFound("登録直後の item".into()))?;
    let pmcid = model::s(&d.item.csl, "PMCID").map(str::to_string);
    let mut v = item_summary(&d.into());
    v["pmcid"] = json!(pmcid);
    v["retraction_warning"] = json!(resolved.retracted);
    v["source"] = json!(resolved.source);
    v["warnings"] = json!(resolved.warnings);
    Ok(v)
}

fn import_pdf(db: &mut Db, a: &Value) -> Result<Value> {
    let s = pdfingest::import_summary(db, Path::new(&req_str(a, "path")?));
    Ok(json!({"status": s.status, "key": s.key, "message": s.message}))
}

fn attach_pdf(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let path = req_str(a, "path")?;
    let attached = edit::attach_auto(db, &key, Path::new(&path))?;
    let (placement, dest, warning) = match attached.placement {
        edit::Placement::Drive(p) => ("drive", p, None),
        edit::Placement::Managed(p) => ("managed", p, None),
        edit::Placement::ManagedFallback { path, reason } => ("managed", path, Some(reason)),
    };
    // 添付直後の本文の索引（失敗しても添付は成立する = 結果だけ添える。GUI は stderr にしか出さない）
    let ft: Option<(Option<String>, Option<String>)> = db
        .conn
        .query_row(
            "SELECT f.status, f.detail FROM attachments a LEFT JOIN fulltext f ON f.attachment_id = a.id WHERE a.id = ?1",
            [attached.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let fulltext = match ft {
        Some((Some(s), _)) if s == "ok" => "indexed".to_string(),
        Some((Some(s), d)) if s == "partial" => format!("indexed（一部: {}）", d.unwrap_or_default()),
        Some((Some(s), _)) if s == "empty" => "empty（テキスト層なし = スキャン PDF の可能性）".to_string(),
        Some((Some(_), d)) => format!("failed: {}", d.unwrap_or_default()),
        _ => "failed: 本文を索引できませんでした（実体を読めない等）".to_string(),
    };
    Ok(json!({
        "attached": true, "key": key, "placement": placement, "path": dest.to_string_lossy(),
        "warning": warning, "fulltext": fulltext
    }))
}

fn fetch_pdf(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let f = pdffetch::fetch_pdf(db, &key)?;
    Ok(json!({"fetched": true, "key": key, "filename": f.filename, "bytes": f.bytes_len, "source": f.source}))
}

// ---------------- 書込: ラベル ----------------

fn add_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    edit::add_tag(db, &key, &req_str(a, "tag")?)?;
    Ok(json!({"key": key, "tags": item_tags(db, &key)?}))
}

fn remove_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    edit::remove_tag(db, &key, req_str(a, "tag")?.trim())?;
    Ok(json!({"key": key, "tags": item_tags(db, &key)?}))
}

fn set_tags(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    for t in str_list(a, "add")? {
        edit::add_tag(db, &key, &t)?;
    }
    for t in str_list(a, "remove")? {
        edit::remove_tag(db, &key, t.trim())?;
    }
    Ok(json!({"key": key, "tags": item_tags(db, &key)?}))
}

fn tag_json(db: &Db, id: i64) -> Result<Value> {
    Ok(db.conn.query_row("SELECT name, color FROM tags WHERE id = ?1", [id], |r| {
        Ok(json!({"name": r.get::<_, String>(0)?, "color": r.get::<_, Option<String>>(1)?}))
    })?)
}

fn create_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let id = tags::create_tag(db, &req_str(a, "name")?, opt_str(a, "color")?.as_deref())?;
    tag_json(db, id)
}

fn update_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let (id, _) = resolve_tag(db, &tag_arg(a, "name")?)?;
    let new_name = opt_str(a, "new_name")?;
    let color = opt_str(a, "color")?;
    if new_name.is_none() && color.is_none() {
        return Err(bad("new_name か color を指定してください"));
    }
    tags::update_tag(db, id, new_name.as_deref(), color.as_deref())?;
    tag_json(db, id)
}

fn delete_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let (id, name) = resolve_tag(db, &tag_arg(a, "name")?)?;
    tags::delete_tag(db, id)?;
    Ok(json!({"deleted": name}))
}

fn merge_tags(db: &mut Db, a: &Value) -> Result<Value> {
    let (target, target_name) = resolve_tag(db, &tag_arg(a, "target")?)?;
    let sources: Vec<i64> =
        str_list(a, "sources")?.iter().map(|s| resolve_tag(db, &json!(s)).map(|t| t.0)).collect::<Result<_>>()?;
    let n = tags::merge_tags(db, &sources, target)?;
    Ok(json!({"target": target_name, "n_items": n}))
}

fn find_replace_tags(db: &mut Db, a: &Value) -> Result<Value> {
    let out = tags::find_replace_tags(
        db,
        &req_str(a, "find")?,
        &req_str(a, "replace")?,
        opt_bool(a, "dry_run")?.unwrap_or(false),
    )?;
    Ok(Value::Array(out.iter().map(|o| json!({"old": o.old, "new": o.new, "status": o.status})).collect()))
}

fn attach_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let (id, _) = resolve_tag(db, &tag_arg(a, "tag")?)?;
    tags::attach_tag(db, &key, id)?;
    Ok(json!({"key": key, "tags": item_tags(db, &key)?}))
}

fn detach_tag(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let (id, _) = resolve_tag(db, &tag_arg(a, "tag")?)?;
    tags::detach_tag(db, &key, id)?;
    Ok(json!({"key": key, "tags": item_tags(db, &key)?}))
}

// ---------------- 書込: フォルダ ----------------

fn col_arg(db: &Db, a: &Value, k: &str) -> Result<i64> {
    resolve_collection(db, a.get(k).filter(|v| !v.is_null()).ok_or_else(|| bad(format!("引数 {k} は必須です")))?)
}

/// 書き込み応答用の所属フォルダのパス（詳細と同じ順）。詳細を丸ごと引き直さない（添付の stat まで走る）
fn item_collections(db: &Db, key: &str) -> Result<Vec<String>> {
    search::item_collection_paths(db, key)?.ok_or_else(|| CoreError::NotFound(format!("item {key}")))
}

fn add_to_collection(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let cid = col_arg(db, a, "collection")?;
    edit::add_to_collection(db, &key, cid)?;
    Ok(json!({"key": key, "collections": item_collections(db, &key)?}))
}

fn remove_from_collection(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let cid = col_arg(db, a, "collection")?;
    edit::remove_from_collection(db, &key, cid)?;
    Ok(json!({"key": key, "collections": item_collections(db, &key)?}))
}

fn move_to_collection(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let to = col_arg(db, a, "to")?;
    let from = a.get("from").filter(|v| !v.is_null()).map(|v| resolve_collection(db, v)).transpose()?;
    edit::add_to_collection(db, &key, to)?;
    if let Some(f) = from.filter(|&f| f != to) {
        edit::remove_from_collection(db, &key, f)?;
    }
    Ok(json!({"key": key, "collections": item_collections(db, &key)?}))
}

fn create_collection(db: &mut Db, a: &Value) -> Result<Value> {
    let name = req_str(a, "name")?;
    // 親の下から辿る（親をパスに戻して root から辿り直さない = 祖先に大小違いの同名があっても親の下に着地する）
    let parent = a.get("parent").filter(|v| !v.is_null()).map(|p| resolve_collection(db, p)).transpose()?;
    let (id, created) = collections::ensure_path(&db.conn, parent, &name)?;
    Ok(json!({
        "key": collection_key(db, id)?,
        "path": collection_path(db, id)?,
        "created": !created.is_empty(),
        "created_segments": created
    }))
}

fn create_collection_in(db: &mut Db, a: &Value) -> Result<Value> {
    let parent = a.get("parent").filter(|v| !v.is_null()).map(|p| resolve_collection(db, p)).transpose()?;
    let id = collections::create_collection(db, &req_str(a, "name")?, parent)?;
    collection_ref(db, id)
}

fn rename_collection(db: &mut Db, a: &Value) -> Result<Value> {
    let id = col_arg(db, a, "collection")?;
    collections::rename_collection(db, id, &req_str(a, "name")?)?;
    collection_ref(db, id)
}

fn delete_collection(db: &mut Db, a: &Value) -> Result<Value> {
    let id = col_arg(db, a, "collection")?;
    let path = collection_path(db, id)?;
    let n = collections::delete_collection(db, id)?;
    Ok(json!({"deleted": path, "folders_removed": n}))
}

fn find_replace_collections(db: &mut Db, a: &Value) -> Result<Value> {
    let out = collections::find_replace_collections(
        db,
        &req_str(a, "find")?,
        &req_str(a, "replace")?,
        opt_bool(a, "dry_run")?.unwrap_or(false),
    )?;
    out.iter()
        .map(|o| Ok(json!({"key": collection_key(db, o.id)?, "old": o.old, "new": o.new, "status": o.status})))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

// ---------------- 書込: 注釈・ノート ----------------

fn add_annotation(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let na = annotate::NewAnnotation {
        kind: opt_str(a, "kind")?.unwrap_or_else(|| "highlight".into()),
        color: Some(opt_str(a, "color")?.unwrap_or_else(|| "purple".into())),
        comment: opt_str(a, "comment")?,
        page: opt_int(a, "page")?,
        quote: opt_str(a, "quote")?,
        prefix: opt_str(a, "prefix")?,
        suffix: opt_str(a, "suffix")?,
        author: opt_str(a, "author")?.unwrap_or_else(|| "claude".into()),
    };
    let ann_key = annotate::add_annotation(db, &key, &na)?;
    Ok(json!({"annotation_key": ann_key, "item_key": key}))
}

/// comment / color は「キーが無い = 変えない・null = 消す・文字列 = 設定」（GUI の clear_* 旗も受ける）
fn update_annotation(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let tri = |k: &str, clear: &str| -> Result<Option<Option<String>>> {
        if opt_bool(a, clear)?.unwrap_or(false) {
            return Ok(Some(None));
        }
        match a.get(k) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
            Some(_) => Err(bad(format!("引数 {k} は文字列か null で渡してください"))),
        }
    };
    let patch = annotate::AnnotationPatch {
        comment: tri("comment", "clear_comment")?,
        color: tri("color", "clear_color")?,
        kind: opt_str(a, "kind")?,
    };
    annotate::update_annotation(db, &key, &patch)?;
    Ok(json!({"key": key}))
}

fn delete_annotation(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    annotate::delete_annotation(db, &key)?;
    Ok(json!({"deleted": key}))
}

fn add_note(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    let author = opt_str(a, "author")?.unwrap_or_else(|| "claude".into());
    if author.trim().is_empty() {
        return Err(bad("author が空です"));
    }
    let note_key = annotate::add_note(db, &key, &author, &req_str(a, "content_md")?)?;
    Ok(json!({"note_key": note_key, "item_key": key}))
}

fn update_note(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    annotate::update_note(db, &key, &req_str(a, "content_md")?)?;
    Ok(json!({"key": key}))
}

fn delete_note(db: &mut Db, a: &Value) -> Result<Value> {
    let key = req_str(a, "key")?;
    annotate::delete_note(db, &key)?;
    Ok(json!({"deleted": key}))
}

// ---------------- 書込: 引用スタイル ----------------

/// 渡さなかった項目は今の設定のまま（core `citestyle::set_prefs_partial`。GUI の cite_set_prefs も同じ関数 = 同じ振る舞い）
fn cite_set_prefs(db: &mut Db, a: &Value) -> Result<Value> {
    citestyle::set_prefs_partial(db, opt_str(a, "style")?.as_deref(), opt_str(a, "locale")?.as_deref(), opt_bool(a, "doi_always")?)
}

fn cite_install_file(db: &mut Db, a: &Value) -> Result<Value> {
    citestyle::install_file(&citestyle::styles_dir(db), Path::new(&req_str(a, "path")?))
}

// ---------------- 表 ----------------

const KEY: (&str, &str, &str) = ("key", "string", "item の key（8 桁）");
const COLLECTION: &str = "フォルダの key（8 桁）かパス（'親/子'・大小無視）。数値 id は不可";

fn c(
    name: &'static str,
    writes: bool,
    aliases: &'static [&'static str],
    doc: &'static str,
    args: Value,
    run: fn(&mut Db, &Value) -> Result<Value>,
) -> Command {
    Command { name, doc, args, writes, aliases, run }
}

fn build() -> Vec<Command> {
    vec![
        // ---- 読み ----
        c("list_items", false, &["search_library"],
          "蔵書の一覧・検索（GUI の一覧と同じ）。query は書誌 + 添付 PDF の本文の全文検索（和文可）で、本文だけの一致は fulltext_snippet に抜粋（一致箇所は [ ]）。count が limit と同じなら続きがある。並びは query 省略時 = 追加日（date_added）の新しい順（年の順ではない）・query ありは一致度順。戻り値の journal は誌名の短縮表示（container-title-short があればそれ）。正式な誌名・著者全員・抄録・巻号頁は返さない = get_item か sql の csl で取る（まとめて取るなら sql の json_object）",
          schema(&[
              ("query", "string", "全文検索語（英・和）。省略で追加日の新しい順"),
              ("search", "string", "query と同じ（GUI の名前）"),
              ("collection", "string", COLLECTION),
              ("tag", "string", "ラベル名（大小無視）"),
              ("starred_only", "boolean", "★ だけ"),
              ("trashed", "boolean", "ゴミ箱の中を見る"),
              ("author", "string", "著者の姓（完全一致・大小無視・位置は問わない）"),
              ("author_initial", "string", "著者の名の頭文字（author と併用）"),
              ("journal", "string", "誌名（略称でも可・表記ゆれを束ねる）"),
              ("metadata_only", "boolean", "true = PDF 本文を検索しない"),
              ("limit", "integer", "既定 50"),
          ], &[]),
          list_items),
        c("get_item", false, &[],
          "item 1 件の全部: CSL-JSON・フォルダ（key・パス）・ラベル・添付（ファイルの絶対パス）・撤回情報・ノート本文・注釈数",
          schema(&[KEY], &["key"]), get_item),
        c("get_fulltext", false, &[],
          "添付 PDF の本文テキスト（抽出済みのキャッシュ・無ければその場で抽出）。頁区切りは \\f。スキャン PDF は空",
          schema(&[KEY,
              ("from_page", "integer", "1 始まり"),
              ("to_page", "integer", "この頁まで（含む）"),
              ("max_chars", "integer", "既定 60000"),
          ], &["key"]), get_fulltext),
        c("list_annotations", false, &[],
          "item の注釈（ハイライト等・author 付き）とノートの一覧",
          schema(&[KEY], &["key"]), list_annotations),
        c("list_notes", false, &[],
          "item のノート（markdown）の一覧",
          schema(&[KEY], &["key"]), list_notes),
        c("collections_tree", false, &["list_collections"],
          "フォルダの木: key・名前・パス・親の key・直下の件数（ゴミ箱は数えない）",
          schema(&[], &[]), collections_tree),
        c("list_tags", false, &[],
          "ラベルの一覧（名前・色・件数）",
          schema(&[("by_count", "boolean", "件数の多い順（既定は名前順）")], &[]), list_tags),
        c("view_counts", false, &[],
          "すべて・★・ゴミ箱の件数",
          schema(&[], &[]), view_counts),
        c("membership", false, &[],
          "複数 item のフォルダ・ラベルの所属の集計（n = keys のうち所属する件数）",
          schema(&[("keys", "string[]", "item の key の配列")], &["keys"]), membership),
        c("bibtex_entries", false, &[],
          "item ごとの BibTeX と引用キー（keys の順・無い key は飛ばす）",
          schema(&[("keys", "string[]", "item の key の配列")], &["keys"]), bibtex_entries),
        c("export_bibtex", false, &[],
          "BibTeX を 1 本の文字列で（keys かフォルダ）",
          schema(&[("keys", "string[]", "item の key の配列"), ("collection", "string", COLLECTION)], &[]),
          export_bibtex),
        c("export_kb_manifest", false, &[],
          "下流の知識ベース用の書誌確定済み manifest（keys / フォルダ / since で絞る・省略で全件）",
          schema(&[
              ("keys", "string[]", "item の key の配列"),
              ("collection", "string", COLLECTION),
              ("since", "string", "date_added がこの日付（YYYY-MM-DD）以降"),
          ], &[]), export_kb_manifest),
        c("item_types", false, &[],
          "参照タイプのカタログ（create_manual / update_item の csl に使える type とフィールド）",
          schema(&[], &[]), item_types),
        c("subtypes", false, &[],
          "論文の種類（genre）の候補",
          schema(&[], &[]), subtypes),
        c("lookup", false, &[],
          "DOI / PMID / URL から書誌（CSL-JSON）を引くだけ（ネットワーク・DB は変えない）",
          schema(&[("input", "string", "DOI・PMID・PubMed / doi.org の URL")], &["input"]), lookup),
        c("cite_prefs", false, &[],
          "既定の引用スタイル・言語・DOI を常に含めるか",
          schema(&[], &[]), cite_prefs),
        c("cite_installed", false, &[],
          "使える引用スタイル（同梱 4 種 + 導入済み）の一覧",
          schema(&[], &[]), cite_installed),
        c("cite_style_xml", false, &[],
          "citeproc に渡せる CSL の XML（誌名スタイルは親で解決済み・未取得なら取得してキャッシュ）",
          schema(&[("id", "string", "スタイル id（cite_installed の id・CSL リポジトリのファイル名）")], &["id"]),
          cite_style_xml),
        c("cite_locale_xml", false, &[],
          "citeproc の言語ファイル（locales-xx-XX.xml）の XML",
          schema(&[("lang", "string", "例 en-US / ja-JP")], &["lang"]), cite_locale_xml),
        // ---- 書込: item ----
        c("set_star", true, &[],
          "★ を付ける / 外す",
          schema(&[KEY, ("starred", "boolean", "")], &["key", "starred"]), set_star),
        c("trash_item", true, &[],
          "ゴミ箱へ移す（復元できる）",
          schema(&[KEY], &["key"]), trash_item),
        c("restore_item", true, &[],
          "ゴミ箱から戻す",
          schema(&[KEY], &["key"]), restore_item),
        c("purge_item", true, &[],
          "ゴミ箱の中の item を完全に消す（復元不可・ゴミ箱の外は拒否・Drive の linked ファイルは消さない）",
          schema(&[KEY], &["key"]), purge_item),
        c("create_manual", true, &[],
          "書誌を手で作る（csl = CSL-JSON。type は item_types のもの・title 必須・DOI/PMID が既存と同じなら拒否）",
          schema(&[("csl", "object", "CSL-JSON")], &["csl"]), create_manual),
        c("update_item", true, &[],
          "書誌を差し替える（csl は全体を渡す = get_item の csl を直して渡す。item_types の型の欄は、無ければ消える = 型を変えると旧型の欄は残らない。元の型の欄の外にあり新しい型の欄にも無いキー（citation-key・型を変えない編集での取込由来の accessed 等）は、無ければ温存・空値なら消える。custom は温存）",
          schema(&[KEY, ("csl", "object", "CSL-JSON（全体）")], &["key", "csl"]), update_item),
        c("quick_add", true, &["create_item"],
          "PMID / DOI / PubMed URL / doi.org URL から書誌を機械確定して登録（ネットワーク・重複は既存 key を示してエラー。戻り値の source = 書誌の取得元 pubmed / doi.org / isho-ris・warnings = PubMed の照合に失敗して doi.org で代用した等の警告。普段は空）",
          schema(&[("input", "string", "PMID・DOI・URL")], &["input"]), quick_add),
        c("import_pdf", true, &[],
          "手元の PDF を取り込む（本文の DOI/PMID → 書誌確定 → 登録 + 添付。決まらなければラベル「要確認」で登録）",
          schema(&[("path", "string", "PDF の絶対パス")], &["path"]), import_pdf),
        c("attach_pdf", true, &[],
          "手元の PDF を既存の item に添付する（設定に従い Drive か storage/ へ置く。戻り値の fulltext = 本文の索引の結果）",
          schema(&[KEY, ("path", "string", "PDF の絶対パス")], &["key", "path"]), attach_pdf),
        c("fetch_pdf", true, &[],
          "PDF の無い item に OA の PDF を自動取得して添付する（PMC → Unpaywall → Crossref → 着地ページ。冒頭の DOI で同一性を照合・paywall は取らない）",
          schema(&[KEY], &["key"]), fetch_pdf),
        // ---- 書込: ラベル ----
        c("add_tag", true, &[],
          "item にラベルを付ける（無ければ作る）",
          schema(&[KEY, ("tag", "string", "ラベル名")], &["key", "tag"]), add_tag),
        c("remove_tag", true, &[],
          "item からラベルを外す",
          schema(&[KEY, ("tag", "string", "ラベル名")], &["key", "tag"]), remove_tag),
        c("set_tags", true, &[],
          "item のラベルをまとめて付ける / 外す（旧 MCP の形）",
          schema(&[KEY, ("add", "string[]", "付けるラベル名"), ("remove", "string[]", "外すラベル名")], &["key"]),
          set_tags),
        c("attach_tag", true, &[],
          "既存のラベルを item に付ける（無いラベルはエラー）",
          schema(&[KEY, ("tag", "string", "ラベル名")], &["key", "tag"]), attach_tag),
        c("detach_tag", true, &[],
          "既存のラベルを item から外す",
          schema(&[KEY, ("tag", "string", "ラベル名")], &["key", "tag"]), detach_tag),
        c("create_tag", true, &[],
          "ラベルを作る（同名があれば色だけ変える）",
          schema(&[("name", "string", "ラベル名"), ("color", "string", "パレット名（gray sky blue red green 等 25 色）か #rrggbb")], &["name"]),
          create_tag),
        c("update_tag", true, &[],
          "ラベルの改名・色の変更",
          schema(&[
              ("name", "string", "今のラベル名"),
              ("new_name", "string", "新しい名前"),
              ("color", "string", "パレット名か #rrggbb"),
          ], &["name"]), update_tag),
        c("delete_tag", true, &[],
          "ラベルを消す（論文は残る）",
          schema(&[("name", "string", "ラベル名")], &["name"]), delete_tag),
        c("merge_tags", true, &[],
          "ラベルの統合: sources の付与を target へ移して sources を消す",
          schema(&[("sources", "string[]", "統合するラベル名"), ("target", "string", "残すラベル名")], &["sources", "target"]),
          merge_tags),
        c("find_replace_tags", true, &[],
          "ラベル名の検索置換（大小無視・既存と同名になるものは conflict で飛ばす）。dry_run = 予定だけ返す",
          schema(&[("find", "string", ""), ("replace", "string", ""), ("dry_run", "boolean", "")], &["find", "replace"]),
          find_replace_tags),
        // ---- 書込: フォルダ ----
        c("add_to_collection", true, &[],
          "item をフォルダへ入れる",
          schema(&[KEY, ("collection", "string", COLLECTION)], &["key", "collection"]), add_to_collection),
        c("remove_from_collection", true, &[],
          "item をフォルダから外す",
          schema(&[KEY, ("collection", "string", COLLECTION)], &["key", "collection"]), remove_from_collection),
        c("move_to_collection", true, &[],
          "item をフォルダ to へ入れる。from を渡すとそこから外す（移動・旧 MCP の形）",
          schema(&[KEY, ("to", "string", COLLECTION), ("from", "string", COLLECTION)], &["key", "to"]),
          move_to_collection),
        c("create_collection", true, &[],
          "フォルダを確保する（冪等）。name は 'A/B' のパス可・途中の親も作る・既存（大小無視）ならそれを返す。parent の下に作ることもできる",
          schema(&[("name", "string", "フォルダ名かパス"), ("parent", "string", COLLECTION)], &["name"]),
          create_collection),
        c("create_collection_in", true, &[],
          "フォルダを 1 つ作る（GUI の New folder）。同じ親の下に同名（大小無視）があれば拒否・名前に / は不可。parent 省略で最上位",
          schema(&[("name", "string", "フォルダ名"), ("parent", "string", COLLECTION)], &["name"]),
          create_collection_in),
        c("rename_collection", true, &[],
          "フォルダの改名（兄弟と同名は拒否）",
          schema(&[("collection", "string", COLLECTION), ("name", "string", "新しい名前")], &["collection", "name"]),
          rename_collection),
        c("delete_collection", true, &[],
          "フォルダをサブフォルダごと消す（即時・論文は残る）",
          schema(&[("collection", "string", COLLECTION)], &["collection"]), delete_collection),
        c("find_replace_collections", true, &[],
          "フォルダ名の検索置換（大小無視・兄弟と同名になるものは conflict で飛ばす）。dry_run = 予定だけ返す",
          schema(&[("find", "string", ""), ("replace", "string", ""), ("dry_run", "boolean", "")], &["find", "replace"]),
          find_replace_collections),
        // ---- 書込: 注釈・ノート ----
        c("add_annotation", true, &[],
          "item の PDF に注釈を付ける。quote は本文からの逐語引用（get_fulltext の文字列をそのまま）・page と prefix/suffix で位置を絞る。author は 'claude:<model>' 推奨",
          schema(&[KEY,
              ("quote", "string", "本文からの逐語引用（アンカー）"),
              ("page", "integer", "1 始まり"),
              ("comment", "string", ""),
              ("kind", "string", "highlight（既定）/ underline / strikethrough / note"),
              ("color", "string", "既定 purple（user の黄と区別）"),
              ("prefix", "string", "quote の直前の文字列"),
              ("suffix", "string", "quote の直後の文字列"),
              ("author", "string", "既定 'claude'"),
          ], &["key"]), add_annotation),
        c("update_annotation", true, &[],
          "注釈の comment / color / kind を変える（key は注釈の key。comment・color は null で消す）",
          schema(&[
              ("key", "string", "注釈の key"),
              ("comment", "string", ""),
              ("color", "string", ""),
              ("kind", "string", ""),
              ("clear_comment", "boolean", "comment を消す（GUI の形）"),
              ("clear_color", "boolean", "color を消す（GUI の形）"),
          ], &["key"]), update_annotation),
        c("delete_annotation", true, &[],
          "注釈を消す（key は注釈の key）",
          schema(&[("key", "string", "注釈の key")], &["key"]), delete_annotation),
        c("add_note", true, &[],
          "item にノート（markdown）を足す。author は 'claude:<model>' 推奨",
          schema(&[KEY, ("content_md", "string", "本文（markdown）"), ("author", "string", "既定 'claude'")], &["key", "content_md"]),
          add_note),
        c("update_note", true, &[],
          "ノートの本文を差し替える（key はノートの key・空は不可 = 消すなら delete_note）",
          schema(&[("key", "string", "ノートの key"), ("content_md", "string", "")], &["key", "content_md"]),
          update_note),
        c("delete_note", true, &[],
          "ノートを消す（key はノートの key）",
          schema(&[("key", "string", "ノートの key")], &["key"]), delete_note),
        // ---- 書込: 引用スタイル ----
        c("cite_set_prefs", true, &[],
          "既定の引用スタイル・言語・DOI を常に含めるかを変える（渡さない項目は今のまま。style は取得・導入を兼ねる）",
          schema(&[
              ("style", "string", "スタイル id"),
              ("locale", "string", "'' = スタイルの既定・例 en-US"),
              ("doi_always", "boolean", ""),
          ], &[]), cite_set_prefs),
        c("cite_install_file", true, &[],
          "手元の .csl ファイルを導入する（id は custom- 付き）",
          schema(&[("path", "string", ".csl の絶対パス")], &["path"]), cite_install_file),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::Resolved;

    fn seeded_db() -> (Db, String) {
        let mut db = Db::open_in_memory().unwrap();
        let key = seed(&mut db);
        (db, key)
    }

    fn seed(db: &mut Db) -> String {
        let r = Resolved {
            csl: json!({
                "type": "article-journal",
                "title": "MCP Seed Article on pancreatic imaging",
                "author": [{"family": "Kano", "given": "S"}],
                "container-title": "Test Journal",
                "issued": {"date-parts": [[2026]]},
                "DOI": "10.9999/mcp.seed", "PMID": "88888888",
                "abstract": "Seed abstract.",
                "custom": {"source": "test"}
            }),
            retracted: None,
            source: "test",
            warnings: vec![],
        };
        let added = crate::ingest::add_item(db, &r).unwrap();
        db.conn.execute_batch("INSERT INTO collections (key, name) VALUES ('CCCC2222', 'Seed Folder')").unwrap();
        added.key
    }

    fn call_ok(db: &mut Db, name: &str, args: Value) -> Value {
        call(db, name, &args).unwrap_or_else(|e| panic!("{name} {args}: {e}"))
    }

    fn call_err(db: &mut Db, name: &str, args: Value) -> String {
        match call(db, name, &args) {
            Ok(v) => panic!("{name} {args}: エラーのはずが {v}"),
            Err(e) => e.to_string(),
        }
    }

    /// 一時ファイル DB（managed 添付・storage/ が要るもの）
    fn file_db(tag: &str) -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("openref-cmd-{tag}-{}-{}", std::process::id(), crate::keys::new_key()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("openref.sqlite")).unwrap();
        (db, dir)
    }

    #[test]
    fn registry_names_unique_and_described() {
        let mut seen = std::collections::HashSet::new();
        for c in registry() {
            assert!(seen.insert(c.name), "名前の重複: {}", c.name);
            for a in c.aliases {
                assert!(seen.insert(a), "別名の重複: {a}");
            }
            assert!(!c.doc.is_empty());
            assert_eq!(c.args["type"], "object", "{}", c.name);
            // required は properties にある
            for r in c.args["required"].as_array().unwrap() {
                assert!(c.args["properties"].get(r.as_str().unwrap()).is_some(), "{}: {r}", c.name);
            }
        }
        // 旧 MCP の 15 tool の名前はすべて引ける
        for old in [
            "search_library", "get_item", "get_fulltext", "list_annotations", "list_collections",
            "export_bibtex", "create_item", "attach_pdf", "fetch_pdf", "add_annotation", "add_note",
            "set_tags", "create_collection", "move_to_collection", "export_kb_manifest",
        ] {
            assert!(find(old).is_some(), "旧名 {old}");
        }
        // フォルダ 4 関数
        for f in ["create_collection_in", "rename_collection", "delete_collection", "find_replace_collections"] {
            assert!(find(f).is_some_and(|c| c.writes), "{f}");
        }
        assert!(markdown().contains("`search_library`"));
    }

    #[test]
    fn args_are_checked() {
        let (mut db, key) = seeded_db();
        assert!(call_err(&mut db, "nope", json!({})).contains("未知のコマンド"));
        assert!(call_err(&mut db, "get_item", json!({"key": key, "kye": 1})).contains("未知の引数 kye"));
        assert!(call_err(&mut db, "get_item", json!({})).contains("key は必須"));
        assert!(call_err(&mut db, "get_item", json!([key])).contains("object"));
        assert!(call_err(&mut db, "set_star", json!({"key": key, "starred": "yes"})).contains("true / false"));
        // null は {} と同じ
        assert!(call(&mut db, "view_counts", &Value::Null).is_ok());
    }

    /// 空白だけの query / author / author_initial / journal は未指定として扱う（判定は core に 1 本・登録表では二重にしない）
    #[test]
    fn list_items_blank_args_mean_unspecified() {
        let (mut db, _key) = seeded_db();
        let count = |db: &mut Db, args: Value| call_ok(db, "list_items", args)["count"].as_u64().unwrap();
        let all = count(&mut db, json!({}));
        assert!(all >= 1);
        for args in [json!({"query": "  "}), json!({"search": "\t"}), json!({"author": "   "}), json!({"journal": "  "})] {
            assert_eq!(count(&mut db, args.clone()), all, "{args}");
        }
        // 頭文字だけが空白 = 姓だけで絞る
        let by_family = count(&mut db, json!({"author": "Kano"}));
        assert_eq!(count(&mut db, json!({"author": "Kano", "author_initial": " "})), by_family);
    }

    /// 旧 MCP の tool_roundtrip（旧名・旧引数の形のまま call から通る）
    #[test]
    fn tool_roundtrip() {
        let (mut db, key) = seeded_db();

        let v = call_ok(&mut db, "search_library", json!({"query": "pancreatic"}));
        assert_eq!(v["count"], 1);
        assert_eq!(v["items"][0]["key"], key.as_str());

        let v = call_ok(&mut db, "add_note", json!({"key": key, "content_md": "要点: テスト"}));
        assert_eq!(v["note_key"].as_str().unwrap().len(), 8);

        // annotation は PDF 添付なし → 誠実にエラー
        let e = call_err(&mut db, "add_annotation", json!({"key": key, "quote": "x"}));
        assert!(e.contains("add_note"));

        let v = call_ok(&mut db, "set_tags", json!({"key": key, "add": ["pancreas", "T-anchor"]}));
        assert_eq!(v["tags"].as_array().unwrap().len(), 2);
        let v = call_ok(&mut db, "search_library", json!({"tag": "pancreas"}));
        assert_eq!(v["count"], 1);

        let v = call_ok(&mut db, "move_to_collection", json!({"key": key, "to": "Seed Folder"}));
        assert_eq!(v["collections"][0], "Seed Folder");

        let v = call_ok(&mut db, "get_item", json!({"key": key}));
        assert_eq!(v["note_count"], 1);
        assert_eq!(v["csl"]["DOI"], "10.9999/mcp.seed");
        assert_eq!(v["collections"][0]["key"], "CCCC2222");

        let v = call_ok(&mut db, "export_bibtex", json!({"keys": [key]}));
        assert!(v["bibtex"].as_str().unwrap().starts_with("@article{"));
        let v = call_ok(&mut db, "export_bibtex", json!({"collection": "CCCC2222"}));
        assert_eq!(v["count"], 1);

        let v = call_ok(&mut db, "export_kb_manifest", json!({}));
        assert_eq!(v["count"], 1);
        assert_eq!(v["records"][0]["pmid"], "88888888");

        // 不明フォルダ・不明ラベルは誠実エラー（0 件と見分けられる）
        assert!(call_err(&mut db, "search_library", json!({"collection": "Nope"})).contains("Nope"));
        assert!(call_err(&mut db, "list_items", json!({"tag": "nope-tag"})).contains("nope-tag"));
        // 数値 id は拒否して key / パスを案内
        let e = call_err(&mut db, "search_library", json!({"collection": 1}));
        assert!(e.contains("key") && e.contains("パス"), "{e}");
        let e = call_err(&mut db, "attach_tag", json!({"key": key, "tag": 1}));
        assert!(e.contains("名前"), "{e}");
    }

    /// 旧 MCP の create_collection_ensure_and_move
    #[test]
    fn create_collection_ensure_and_move() {
        let (mut db, key) = seeded_db();

        let v = call_ok(&mut db, "create_collection", json!({"name": "早期膵癌/画像所見"}));
        assert_eq!(v["created"], true);
        assert_eq!(v["path"], "早期膵癌/画像所見");
        assert_eq!(v["created_segments"].as_array().unwrap().len(), 2);
        let k = v["key"].as_str().unwrap().to_string();

        // 冪等: 再実行は同じ key・created=false
        let v2 = call_ok(&mut db, "create_collection", json!({"name": "早期膵癌/画像所見"}));
        assert_eq!(v2["created"], false);
        assert_eq!(v2["key"], k.as_str());

        // NOCASE: 'seed folder' は既存 'Seed Folder' に解決し重複を作らない。path は実表記
        let v3 = call_ok(&mut db, "create_collection", json!({"name": "seed folder"}));
        assert_eq!(v3["created"], false);
        assert_eq!(v3["path"], "Seed Folder");

        // parent の下に（key 指定）
        let v4 = call_ok(&mut db, "create_collection", json!({"name": "所見2", "parent": "CCCC2222"}));
        assert_eq!(v4["path"], "Seed Folder/所見2");

        // 作った直後のフォルダへ move（パス）→ フォルダで絞った検索で見える（key でも）
        call_ok(&mut db, "move_to_collection", json!({"key": key, "to": "早期膵癌/画像所見"}));
        assert_eq!(call_ok(&mut db, "search_library", json!({"collection": "早期膵癌/画像所見"}))["count"], 1);
        assert_eq!(call_ok(&mut db, "list_items", json!({"collection": k}))["count"], 1);
        // from 付きで移動
        let v = call_ok(&mut db, "move_to_collection", json!({"key": key, "to": "Seed Folder", "from": k}));
        assert_eq!(v["collections"], json!(["Seed Folder"]));

        assert!(call(&mut db, "create_collection", &json!({"name": " / "})).is_err());
    }

    /// create_collection の parent = 親の下から辿る（親をパスに戻して root から辿り直さない）。
    /// 旧データの形（祖先に大小違いの兄弟・`/` や前後空白を含む名前）でも指定した親の下に着地する
    #[test]
    fn create_collection_under_parent_walks_from_parent() {
        let (mut db, _) = seeded_db();
        db.conn
            .execute_batch(
                "INSERT INTO collections (key, name) VALUES ('TEACH001', 'Teach');
                 INSERT INTO collections (key, name) VALUES ('TEACH002', 'teach');
                 INSERT INTO collections (key, name, parent_id) VALUES ('XUNDER02', 'X', (SELECT id FROM collections WHERE key = 'TEACH002'));
                 INSERT INTO collections (key, name) VALUES ('SLASH001', 'A/B');
                 INSERT INTO collections (key, name) VALUES ('SPACE001', ' Pad ');",
            )
            .unwrap();
        let parent_key = |db: &Db, key: &str| -> Option<String> {
            db.conn
                .query_row(
                    "SELECT p.key FROM collections c LEFT JOIN collections p ON p.id = c.parent_id WHERE c.key = ?1",
                    [key],
                    |r| r.get(0),
                )
                .unwrap()
        };
        for parent in ["XUNDER02", "SLASH001", "SPACE001"] {
            let before: i64 = db.conn.query_row("SELECT COUNT(*) FROM collections", [], |r| r.get(0)).unwrap();
            let v = call_ok(&mut db, "create_collection", json!({"name": "Y/z", "parent": parent}));
            assert_eq!(v["created_segments"], json!(["Y", "z"]), "{parent}");
            let after: i64 = db.conn.query_row("SELECT COUNT(*) FROM collections", [], |r| r.get(0)).unwrap();
            assert_eq!(after - before, 2, "祖先を作り直さない: {parent}");
            let y = parent_key(&db, v["key"].as_str().unwrap()).unwrap();
            assert_eq!(parent_key(&db, &y).as_deref(), Some(parent), "指定した親の下: {parent}");
            // 冪等（2 回目は作らない）
            let v2 = call_ok(&mut db, "create_collection", json!({"name": "y/Z", "parent": parent}));
            assert_eq!((v2["created"].clone(), v2["key"].clone()), (json!(false), v["key"].clone()));
        }
        // 名前が段を持たないときは親そのもの（従来どおり）
        let v = call_ok(&mut db, "create_collection", json!({"name": " / ", "parent": "XUNDER02"}));
        assert_eq!((v["key"].clone(), v["created"].clone()), (json!("XUNDER02"), json!(false)));
        assert_eq!(v["path"], "teach/X");
    }

    /// 書込コマンドの往復（in-memory。添付・取込は一時ファイル DB）
    #[test]
    fn write_commands_roundtrip() {
        let (mut db, key) = seeded_db();

        // ★・ゴミ箱・復元・完全削除
        call_ok(&mut db, "set_star", json!({"key": key, "starred": true}));
        assert_eq!(call_ok(&mut db, "list_items", json!({"starred_only": true}))["count"], 1);
        assert!(call_err(&mut db, "purge_item", json!({"key": key})).contains("ゴミ箱"));
        call_ok(&mut db, "trash_item", json!({"key": key}));
        assert_eq!(call_ok(&mut db, "view_counts", json!({}))["trash"], 1);
        assert!(!call_ok(&mut db, "get_item", json!({"key": key}))["deleted_at"].is_null());
        call_ok(&mut db, "restore_item", json!({"key": key}));
        assert_eq!(call_ok(&mut db, "view_counts", json!({}))["all"], 1);

        // 手動作成・編集
        let k2 = call_ok(&mut db, "create_manual", json!({"csl": {"type": "article-journal", "title": "手入力の論文"}}))["key"]
            .as_str()
            .unwrap()
            .to_string();
        let mut csl = call_ok(&mut db, "get_item", json!({"key": k2}))["csl"].clone();
        csl["title"] = json!("手入力の論文（改）");
        call_ok(&mut db, "update_item", json!({"key": k2, "csl": csl}));
        assert_eq!(call_ok(&mut db, "get_item", json!({"key": k2}))["csl"]["title"], "手入力の論文（改）");
        call_ok(&mut db, "trash_item", json!({"key": k2}));
        let v = call_ok(&mut db, "purge_item", json!({"key": k2}));
        assert_eq!(v["files_removed"], 0);
        assert!(call(&mut db, "get_item", &json!({"key": k2})).is_err());

        // ラベル: add / remove / create / update / attach / detach / merge / find_replace / delete
        call_ok(&mut db, "add_tag", json!({"key": key, "tag": "膵臓"}));
        assert_eq!(call_ok(&mut db, "remove_tag", json!({"key": key, "tag": "膵臓"}))["tags"], json!([]));
        assert_eq!(call_ok(&mut db, "create_tag", json!({"name": "liver", "color": "sky"}))["color"], "sky");
        // 改名 + 不正な色 → エラー、改名も残らない
        assert!(call(&mut db, "update_tag", &json!({"name": "LIVER", "new_name": "y", "color": "bogus"})).is_err());
        assert_eq!(call_ok(&mut db, "list_tags", json!({}))[0]["name"], "liver", "色が不正なら改名も残らない");
        assert_eq!(call_ok(&mut db, "update_tag", json!({"name": "LIVER", "new_name": "hepatic", "color": "red"}))["name"], "hepatic");
        call_ok(&mut db, "attach_tag", json!({"key": key, "tag": "hepatic"}));
        call_ok(&mut db, "create_tag", json!({"name": "肝"}));
        call_ok(&mut db, "attach_tag", json!({"key": key, "tag": "肝"}));
        assert_eq!(call_ok(&mut db, "detach_tag", json!({"key": key, "tag": "肝"}))["tags"], json!(["hepatic"]));
        call_ok(&mut db, "attach_tag", json!({"key": key, "tag": "肝"}));
        let v = call_ok(&mut db, "merge_tags", json!({"sources": ["肝"], "target": "hepatic"}));
        assert_eq!(v["n_items"], 1);
        assert!(call(&mut db, "attach_tag", &json!({"key": key, "tag": "肝"})).is_err(), "統合で消えた");
        let v = call_ok(&mut db, "find_replace_tags", json!({"find": "hep", "replace": "Hep", "dry_run": true}));
        assert_eq!(v[0]["status"], "renamed");
        assert_eq!(call_ok(&mut db, "list_tags", json!({}))[0]["name"], "hepatic", "dry_run は書かない");
        call_ok(&mut db, "find_replace_tags", json!({"find": "hep", "replace": "Hep"}));
        assert_eq!(call_ok(&mut db, "list_tags", json!({}))[0]["name"], "Hepatic");
        let v = call_ok(&mut db, "membership", json!({"keys": [key]}));
        assert_eq!(v["tags"][0]["name"], "Hepatic");
        call_ok(&mut db, "delete_tag", json!({"name": "hepatic"}));
        assert_eq!(call_ok(&mut db, "list_tags", json!({})), json!([{"name": "膵臓", "color": null, "n_items": 0}]));

        // フォルダ: create_in / add / remove / rename / find_replace / delete
        let p = call_ok(&mut db, "create_collection_in", json!({"name": "Parent"}));
        let pk = p["key"].as_str().unwrap().to_string();
        assert!(call_err(&mut db, "create_collection_in", json!({"name": "parent"})).contains("既に"));
        assert_eq!(call_ok(&mut db, "create_collection_in", json!({"name": "Child", "parent": pk}))["path"], "Parent/Child");
        call_ok(&mut db, "add_to_collection", json!({"key": key, "collection": "Parent/Child"}));
        assert_eq!(call_ok(&mut db, "membership", json!({"keys": [key]}))["collections"][0]["path"], "Parent/Child");
        assert_eq!(call_ok(&mut db, "remove_from_collection", json!({"key": key, "collection": "Parent/Child"}))["collections"], json!([]));
        assert_eq!(call_ok(&mut db, "rename_collection", json!({"collection": "Parent/Child", "name": "Kid"}))["path"], "Parent/Kid");
        let v = call_ok(&mut db, "find_replace_collections", json!({"find": "kid", "replace": "Kind"}));
        assert_eq!(v[0]["status"], "renamed");
        let tree = call_ok(&mut db, "collections_tree", json!({}));
        assert!(tree.as_array().unwrap().iter().any(|c| c["path"] == "Parent/Kind" && c["parent"] == pk.as_str()));
        let v = call_ok(&mut db, "delete_collection", json!({"collection": pk}));
        assert_eq!(v["folders_removed"], 2);

        // 注釈・ノート（注釈は PDF が要る = 下の file DB で）
        let nk = call_ok(&mut db, "add_note", json!({"key": key, "content_md": "a", "author": "claude:test"}))["note_key"]
            .as_str()
            .unwrap()
            .to_string();
        call_ok(&mut db, "update_note", json!({"key": nk, "content_md": "b"}));
        assert_eq!(call_ok(&mut db, "list_notes", json!({"key": key}))[0]["content_md"], "b");
        call_ok(&mut db, "delete_note", json!({"key": nk}));
        assert_eq!(call_ok(&mut db, "list_notes", json!({"key": key})), json!([]));

        // 引用スタイル（同梱 = ネットワーク不要）。渡さない項目は今のまま
        call_ok(&mut db, "cite_set_prefs", json!({"style": "vancouver", "doi_always": true}));
        let v = call_ok(&mut db, "cite_set_prefs", json!({"locale": "en-US"}));
        assert_eq!(v["doi_always"], true);
        assert_eq!(call_ok(&mut db, "cite_prefs", json!({}))["locale"], "en-US");

        // ネットワークへ出ない失敗経路: 解釈できない入力・既に PDF があるかの判定前の item なし
        assert!(call_err(&mut db, "quick_add", json!({"input": "   "})).contains("空入力"));
        assert!(call(&mut db, "fetch_pdf", &json!({"key": "NOSUCHKY"})).is_err());
    }

    #[test]
    fn file_commands_roundtrip() {
        let (mut db, dir) = file_db("files");
        let key = seed(&mut db);
        let pdf = dir.join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 fake").unwrap();

        let v = call_ok(&mut db, "attach_pdf", json!({"key": key, "path": pdf.to_string_lossy()}));
        assert_eq!(v["placement"], "managed");
        assert!(std::path::Path::new(v["path"].as_str().unwrap()).starts_with(dir.join("storage")));
        // 偽の PDF は本文を抽出できない → 添付は成立し、失敗が戻り値に出る
        assert!(v["fulltext"].as_str().unwrap().starts_with("failed: "), "{v}");
        // 添付ありの item への自動取得はネットワークへ出る前に拒否
        assert!(call_err(&mut db, "fetch_pdf", json!({"key": key})).contains("既に PDF"));

        // 注釈の往復
        let ak = call_ok(&mut db, "add_annotation", json!({"key": key, "quote": "seed", "page": 1, "author": "claude:test"}))
            ["annotation_key"]
            .as_str()
            .unwrap()
            .to_string();
        call_ok(&mut db, "update_annotation", json!({"key": ak, "comment": "要点", "kind": "underline"}));
        let v = call_ok(&mut db, "list_annotations", json!({"key": key}));
        assert_eq!(v["annotations"][0]["comment"], "要点");
        assert_eq!(v["annotations"][0]["color"], "purple");
        call_ok(&mut db, "update_annotation", json!({"key": ak, "comment": null}));
        assert!(call_ok(&mut db, "list_annotations", json!({"key": key}))["annotations"][0]["comment"].is_null());
        call_ok(&mut db, "delete_annotation", json!({"key": ak}));
        assert_eq!(call_ok(&mut db, "list_annotations", json!({"key": key}))["annotations"], json!([]));

        // 同じファイルの取込は md5 一致で duplicate（ネットワークに出ない）
        let v = call_ok(&mut db, "import_pdf", json!({"path": pdf.to_string_lossy()}));
        assert_eq!(v["status"], "duplicate");
        assert_eq!(v["key"], key.as_str());
        let v = call_ok(&mut db, "import_pdf", json!({"path": dir.join("none.pdf").to_string_lossy()}));
        assert_eq!(v["status"], "error");

        // 手元の .csl の導入
        let csl = dir.join("my-style.csl");
        std::fs::write(&csl, MINI_CSL).unwrap();
        let v = call_ok(&mut db, "cite_install_file", json!({"path": csl.to_string_lossy()}));
        assert!(v["id"].as_str().unwrap().starts_with("custom-"), "{v}");
        let id = v["id"].as_str().unwrap().to_string();
        assert!(call_ok(&mut db, "cite_installed", json!({})).as_array().unwrap().iter().any(|e| e["id"] == id.as_str()));
        assert!(call_ok(&mut db, "cite_style_xml", json!({"id": id}))["xml"].as_str().unwrap().contains("<style"));

        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 書き込み応答のラベル・フォルダは詳細（item_detail）を引き直さず、membership は N+1 にしない。
    /// 旧（詳細 → 名前・パスを 1 件ずつ）と同じ結果で、クエリ数が減ることを示す
    #[test]
    fn write_responses_match_detail_and_use_fewer_queries() {
        use std::cell::Cell;
        thread_local!(static QUERIES: Cell<usize> = const { Cell::new(0) });
        fn tick(_: &str, _: std::time::Duration) {
            QUERIES.with(|c| c.set(c.get() + 1));
        }
        fn count<T>(f: impl FnOnce() -> T) -> (T, usize) {
            QUERIES.with(|c| c.set(0));
            let v = f();
            (v, QUERIES.with(|c| c.get()))
        }

        let (mut db, key) = seeded_db();
        // ラベル: 大小混在・日本語（大小無視の名前順で返る）
        for t in ["beta", "Alpha", "gamma", "膵臓", "Delta"] {
            call_ok(&mut db, "add_tag", json!({"key": key, "tag": t}));
        }
        // フォルダ: 入れ子 3 段・別の親の下の同名・soft-delete 済みの祖先の下
        let a = collections::create_collection(&db, "Zeta", None).unwrap();
        let b = collections::create_collection(&db, "Mid", Some(a)).unwrap();
        let c = collections::create_collection(&db, "Leaf", Some(b)).unwrap();
        let top = collections::create_collection(&db, "Leaf", None).unwrap();
        let old_root = collections::create_collection(&db, "Old", None).unwrap();
        let under_old = collections::create_collection(&db, "Under", Some(old_root)).unwrap();
        let gone = collections::create_collection(&db, "Gone", None).unwrap();
        // 同名（Leaf）は id の逆順（top → c）で所属に加える = 順は id 順ではなく所属に加えた順
        for id in [top, c, under_old, gone] {
            edit::add_to_collection(&db, &key, id).unwrap();
        }
        db.conn.execute("UPDATE collections SET deleted_at = '2026-01-01' WHERE id IN (?1, ?2)", [old_root, gone]).unwrap();
        // 添付（実在しないパス = 詳細だと stat が走る）
        let item_id: i64 = db.conn.query_row("SELECT id FROM items WHERE key = ?1", [&key], |r| r.get(0)).unwrap();
        db.conn
            .execute(
                "INSERT INTO attachments (key, item_id, mode, path) VALUES ('ATT00001', ?1, 'linked', '/nonexistent/x.pdf')",
                [item_id],
            )
            .unwrap();
        db.conn.profile(Some(tick));

        // 旧: 詳細を引き直して名前・パスを作る
        let old_tags = |db: &Db| -> Vec<String> { detail(db, &key).unwrap().tags.into_iter().map(|t| t.name).collect() };
        let old_cols = |db: &Db| -> Vec<String> {
            detail(db, &key).unwrap().collections.iter().map(|c| collection_path(db, c.id).unwrap()).collect()
        };
        let (want_tags, q_old_tags) = count(|| old_tags(&db));
        let (got_tags, q_new_tags) = count(|| item_tags(&db, &key).unwrap());
        assert_eq!(got_tags, want_tags);
        assert_eq!(got_tags, ["Alpha", "beta", "Delta", "gamma", "膵臓"]);
        let (want_cols, q_old_cols) = count(|| old_cols(&db));
        let (got_cols, q_new_cols) = count(|| item_collections(&db, &key).unwrap());
        assert_eq!(got_cols, want_cols);
        assert_eq!(
            got_cols,
            ["Leaf", "Zeta/Mid/Leaf", "Old/Under"],
            "名前順（同名は所属に加えた順）・soft-delete 済みのフォルダ自身は出ず、soft-delete 済みの祖先の下は名前だけ出る"
        );
        assert!(q_new_tags <= 1 && q_new_cols <= 2, "新 {q_new_tags} / {q_new_cols}");
        assert!(q_old_tags >= 5 && q_old_cols > 5, "旧 {q_old_tags} / {q_old_cols}");
        eprintln!("クエリ数 ラベル 旧 {q_old_tags} → 新 {q_new_tags} / フォルダ 旧 {q_old_cols} → 新 {q_new_cols}");
        assert!(matches!(item_tags(&db, "NOSUCHKEY"), Err(CoreError::NotFound(_))));
        assert!(matches!(item_collections(&db, "NOSUCHKEY"), Err(CoreError::NotFound(_))));

        // membership: 旧の写し（id ごとに key・パス・名前を問い合わせる）と同じ値
        let keys = vec![key.clone()];
        let m = search::membership(&db, &keys).unwrap();
        let want_m = json!({
            "collections": m.collections.iter().map(|(id, n)| json!({"key": collection_key(&db, *id).unwrap(), "path": collection_path(&db, *id).unwrap(), "n": n})).collect::<Vec<_>>(),
            "tags": m.tags.iter().map(|(id, n)| json!({"name": db.conn.query_row("SELECT name FROM tags WHERE id = ?1", [id], |r| r.get::<_, String>(0)).unwrap(), "n": n})).collect::<Vec<_>>(),
        });
        let (got_m, q_new_m) = count(|| call_ok(&mut db, "membership", json!({"keys": keys})));
        let (_, q_old_m) = count(|| {
            let m = search::membership(&db, &keys).unwrap();
            for (id, _) in &m.collections {
                collection_key(&db, *id).unwrap();
                collection_path(&db, *id).unwrap();
            }
            for (id, _) in &m.tags {
                db.conn.query_row("SELECT name FROM tags WHERE id = ?1", [id], |r| r.get::<_, String>(0)).unwrap();
            }
        });
        assert_eq!(got_m, want_m);
        assert_eq!(got_m["collections"].as_array().unwrap().len(), 4, "membership は soft-delete 済みのフォルダも数える（旧と同じ）");
        assert!(got_m["collections"].as_array().unwrap().iter().any(|c| c["path"] == "Old/Under"), "soft-delete 済みの祖先の下も数える（旧と同じ）");
        assert!(q_new_m <= 4 && q_old_m > q_new_m, "membership のクエリは固定数（新 {q_new_m} / 旧 {q_old_m}）");
        eprintln!("membership クエリ数 旧 {q_old_m} → 新 {q_new_m}");

        // ゴミ箱の item も引ける
        call_ok(&mut db, "trash_item", json!({"key": key}));
        assert_eq!(item_tags(&db, &key).unwrap(), want_tags);
        assert!(call_ok(&mut db, "get_item", json!({"key": key}))["deleted_at"].is_string());
    }

    const MINI_CSL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0">
  <info>
    <title>Mini Test Style</title>
    <id>http://www.zotero.org/styles/mini-test-style</id>
    <updated>2026-10-21T00:00:00+00:00</updated>
    <category citation-format="numeric"/>
  </info>
  <citation><layout><text variable="citation-number"/></layout></citation>
  <bibliography><layout><text variable="title"/></layout></bibliography>
</style>
"#;
}
