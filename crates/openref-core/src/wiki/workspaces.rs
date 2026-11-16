//! 設定 `wiki_workspaces` = 最近のワークスペースの純関数。書くのは GUI だけ
//! （切替・選び直し・起動時・参照フォルダの追加 / 外す）。MCP / CLI は `Ctx::from_db` で読むだけ（`wiki_status` の `workspaces`）。
//!
//! 形 = JSON object `{ "<canonical な root>": { "last_used": <unix 秒>, "ref_dirs": ["<絶対パス>", …],
//! "exclude": ["<vault 相対>", …], "include": ["<vault 相対>", …] } }`。
//! `ref_dirs` / `exclude` / `include` は任意（無ければ空）・**未知の項目は保つ**（後の版が足した項目を古い版の書込で消さない）。
//! `exclude` / `include` = 索引から除く / 既定の除外（node_modules・CACHEDIR.TAG）を越えて入れるフォルダ（`set_excluded`）。
//! 要素は vault 相対・NFC・`/` 区切り（glob なし・完全一致）。無くなったフォルダの要素は残ってよい（掃除しない）
//! 上限 `MAX`（超えたら `last_used` の古い順に落とす・現在の root は落とさない）。`last_used` が無い・整数でない項目は 0 として扱う

use crate::{CoreError, Result};
use serde_json::{json, Value};

/// settings の鍵
pub const KEY: &str = "wiki_workspaces";
/// 履歴の上限（索引はワークスペースごと = app_dir の増分の上限でもある）
pub const MAX: usize = 10;

/// root → その root の object（`last_used` / `ref_dirs` / 未知の項目）
pub type Map = serde_json::Map<String, Value>;

/// 履歴（`wiki_workspaces` / `shelf_workspaces`）の 1 回の読み → 変更 → 書きを BEGIN IMMEDIATE で囲む（同じ DB を開いた別の接続 =
/// 別のプロセスの書込を、読んでから書くまでの間に混ぜない。後から来た方は busy_timeout まで待ち、待ちきれなければ SQLITE_BUSY の Err）。
/// 戻り値を commit せずに落とすと rollback
pub fn begin_immediate(db: &crate::Db) -> crate::Result<rusqlite::Transaction<'_>> {
    Ok(rusqlite::Transaction::new_unchecked(&db.conn, rusqlite::TransactionBehavior::Immediate)?)
}

/// 設定の値 → Map。未設定・空白は空。JSON の object でなければ Err（BadInput = 黙って空にしない。壊れた値の退避は GUI の仕事）。
/// 値が object でない項目・鍵が空白の項目は捨てる
pub fn parse(v: Option<&str>) -> Result<Map> {
    parse_for(KEY, v)
}

/// parse の本体。key = エラー文に出す設定の鍵（本棚の履歴 `shelf_workspaces` も同じ形 = crate::shelf が共用する）
pub fn parse_for(key: &str, v: Option<&str>) -> Result<Map> {
    let Some(s) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Map::new());
    };
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Object(m)) => Ok(m.into_iter().filter(|(k, v)| v.is_object() && !k.trim().is_empty()).collect()),
        Ok(_) => Err(CoreError::BadInput(format!("設定 {key} を解釈できません（JSON の object ではありません）"))),
        Err(e) => Err(CoreError::BadInput(format!("設定 {key} を解釈できません（JSON の object ではありません）: {e}"))),
    }
}

/// この項目の last_used（無い・数でない = 0）
fn last_used(v: &Value) -> i64 {
    v.as_object().and_then(lu_of).unwrap_or(0)
}

/// last_used の値（無い = None・数でない = Some(0)）
fn lu_of(o: &Map) -> Option<i64> {
    o.get("last_used").map(|n| n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0))
}

/// root の last_used を now にする（無ければ作る・他の項目は保つ）。max を超えた分を last_used の古い順に落とし、落とした root を
/// 古い順に返す。`root` 自身は落とさない（max が 0 でも残る）
pub fn touch(m: &mut Map, root: &str, now: i64, max: usize) -> Vec<String> {
    let e = m.entry(root.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !e.is_object() {
        *e = Value::Object(Map::new());
    }
    if let Some(o) = e.as_object_mut() {
        o.insert("last_used".into(), json!(now));
    }
    let mut evicted = Vec::new();
    if m.len() > max {
        // 新しい順の末尾（古い）から落とす。root は候補に入れない
        let mut cands: Vec<String> = order(m).into_iter().map(|(r, _)| r).filter(|r| r != root).collect();
        while m.len() > max.max(1) {
            let Some(r) = cands.pop() else { break };
            m.remove(&r);
            evicted.push(r);
        }
    }
    evicted
}

/// 履歴の並び = last_used の新しい順・同点は root の文字列順（決定的）
pub fn order(m: &Map) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = m.iter().map(|(k, v)| (k.clone(), last_used(v))).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// 履歴から外す。戻り値 = あった
pub fn remove(m: &mut Map, root: &str) -> bool {
    m.remove(root).is_some()
}

/// この root の項目に `ref_dirs` があるか（空の配列でも true = rekey が生の綴りの鍵の一覧を引き継ぐかの判定）
pub fn has_ref_dirs(m: &Map, root: &str) -> bool {
    m.get(root).and_then(|v| v.get("ref_dirs")).is_some()
}

/// この root の参照フォルダ（無い・配列でない = 空。文字列でない・空白の要素は飛ばす・重複は 1 つに）
pub fn ref_dirs(m: &Map, root: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(a) = m.get(root).and_then(|v| v.get("ref_dirs")).and_then(Value::as_array) {
        for s in a.iter().filter_map(Value::as_str) {
            if !s.trim().is_empty() && !out.iter().any(|o| o == s) {
                out.push(s.to_string());
            }
        }
    }
    out
}

/// この root の参照フォルダを書く。root の項目が無ければ作る（last_used は入れない = 履歴の時刻は touch だけが決める）・他の項目は保つ
pub fn set_ref_dirs(m: &mut Map, root: &str, dirs: &[String]) {
    let e = m.entry(root.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !e.is_object() {
        *e = Value::Object(Map::new());
    }
    if let Some(o) = e.as_object_mut() {
        o.insert("ref_dirs".into(), json!(dirs));
    }
}

/// 同じフォルダの別の綴り（symlink・末尾 `/` = 設定 wiki_root の生の値）の項目 from を canonical な to へ寄せて from を消す。
/// last_used は大きい方（片方だけならその値）・ref_dirs は to の後に from の分を足す（重複なし）・他の項目は to を優先して from の分を足す。
/// 戻り値 = from があった（from == to は何もしない）
pub fn rekey(m: &mut Map, from: &str, to: &str) -> bool {
    if from == to {
        return false;
    }
    let Some(Value::Object(src)) = m.remove(from) else {
        return false;
    };
    let src_lu = lu_of(&src);
    let src_has_dirs = src.contains_key("ref_dirs");
    let src_dirs = {
        let one: Map = std::iter::once((from.to_string(), Value::Object(src.clone()))).collect();
        ref_dirs(&one, from)
    };
    let to_has_dirs = has_ref_dirs(m, to);
    let mut dirs = ref_dirs(m, to);
    for d in src_dirs {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    let e = m.entry(to.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !e.is_object() {
        *e = Value::Object(Map::new());
    }
    let Some(t) = e.as_object_mut() else {
        return true;
    };
    let lu = match (lu_of(t), src_lu) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    for (k, v) in src {
        t.entry(k).or_insert(v);
    }
    if let Some(lu) = lu {
        t.insert("last_used".into(), json!(lu));
    }
    if to_has_dirs || src_has_dirs {
        t.insert("ref_dirs".into(), json!(dirs));
    }
    true
}

/// 文字列の配列の項目（無い・配列でない = 空。文字列でない要素は飛ばす）
fn str_list(m: &Map, root: &str, k: &str) -> Vec<String> {
    m.get(root)
        .and_then(|v| v.get(k))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// この root の除外の規則（`include` / `exclude` = vault 相対のフォルダの配列）。
/// 無い・配列でない = 既定の規則だけ。正規化（NFC・前後の `/`・不正な要素を捨てる）は `scan::Rules::new`
pub fn rules(m: &Map, root: &str) -> super::scan::Rules {
    super::scan::Rules::new(str_list(m, root, "include"), str_list(m, root, "exclude"))
}

/// フォルダ dir（vault 相対・NFC・`/` 区切り）を索引から除く / 入れる（GUI の木の右クリック = wiki_exclude_set）。
/// excluded = true: `include` から外し、既定の規則に当たらなければ `exclude` に足す。
/// excluded = false: `exclude` から外し、既定の規則に当たれば `include` に足す。default_hit = dir が既定の規則
/// （`scan::Rules::default_excludes`）に当たるか（呼び手がファイルシステムで判定する = ここは純関数）。
/// root の項目が無ければ作る（last_used は入れない）・他の項目は保つ。配列に重複は作らない。
/// 外すときの比較は走査の規則と同じ正規化（`scan::norm_dir` = NFC・前後の `/`）= 手で書いた `"proj/testdata/"` や NFD の綴りも外れる
/// （外れずに除外されたまま・画面は成功、を作らない）。正規化できない既存の要素は触らない。
/// 戻り値 = 書いた。dir そのものが正規化できない（`""`・`"."`・`"a//b"`・`.` 始まりの区画）なら何も書かずに false
/// （走査の規則が捨てる値を設定に入れて成功に見せない）
pub fn set_excluded(m: &mut Map, root: &str, dir: &str, excluded: bool, default_hit: bool) -> bool {
    let norm = super::scan::norm_dir;
    let Some(dir) = norm(dir) else { return false };
    let e = m.entry(root.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !e.is_object() {
        *e = Value::Object(Map::new());
    }
    let Some(o) = e.as_object_mut() else { return false };
    let list = |o: &Map, k: &str| -> Vec<String> {
        o.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    };
    let (mut inc, mut exc) = (list(o, "include"), list(o, "exclude"));
    inc.retain(|d| norm(d).as_deref() != Some(dir.as_str()));
    exc.retain(|d| norm(d).as_deref() != Some(dir.as_str()));
    if excluded && !default_hit {
        exc.push(dir.clone());
    }
    if !excluded && default_hit {
        inc.push(dir.clone());
    }
    for (k, v) in [("include", inc), ("exclude", exc)] {
        if v.is_empty() && !o.contains_key(k) {
            continue;
        }
        o.insert(k.into(), json!(v));
    }
    true
}

/// Map → 設定の値（JSON の文字列）
pub fn serialize(m: &Map) -> String {
    serde_json::to_string(m).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(v: Value) -> Map {
        match v {
            Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    fn roots(m: &Map) -> Vec<String> {
        order(m).into_iter().map(|(r, _)| r).collect()
    }

    /// parse_for: エラー文の鍵だけが違う（parse = wiki_workspaces の文のまま・共用しても wiki 側の文は変わらない）
    #[test]
    fn parse_for_names_the_key_in_the_error() {
        for raw in ["not json", "[1]"] {
            let wiki = parse(Some(raw)).unwrap_err().to_string();
            let other = parse_for("shelf_workspaces", Some(raw)).unwrap_err().to_string();
            assert!(wiki.contains("設定 wiki_workspaces を解釈できません"), "{wiki}");
            assert!(other.contains("設定 shelf_workspaces を解釈できません") && !other.contains("wiki_workspaces"), "{other}");
            assert_eq!(wiki.replace("wiki_workspaces", "shelf_workspaces"), other);
        }
        let raw = r#"{"/a": {"last_used": 1}, "/b": 3}"#;
        assert_eq!(parse_for("shelf_workspaces", Some(raw)).unwrap(), parse(Some(raw)).unwrap());
    }

    /// 未設定・空白は空・壊れた JSON / object でない値は Err・object でない項目と空白の鍵は捨てる
    #[test]
    fn parse_rules() {
        assert!(parse(None).unwrap().is_empty());
        assert!(parse(Some("")).unwrap().is_empty());
        assert!(parse(Some("  \n")).unwrap().is_empty());
        assert!(parse(Some("{}")).unwrap().is_empty());
        for broken in ["not json", "[]", "[\"/a\"]", "\"/a\"", "1", "null", "{\"/a\": "] {
            let e = parse(Some(broken)).unwrap_err();
            assert!(matches!(e, CoreError::BadInput(_)), "{broken}: {e:?}");
            assert!(e.to_string().contains(KEY), "{broken}: {e}");
        }
        let m = parse(Some(r#"{"/a": {"last_used": 5}, "/b": 3, "/c": null, "/d": [], " ": {}, "/e": {}}"#)).unwrap();
        assert_eq!(m.keys().cloned().collect::<Vec<_>>(), vec!["/a".to_string(), "/e".to_string()]);
    }

    /// touch: last_used を入れる・他の項目（ref_dirs・未知の項目）は保つ・新しい root は作る
    #[test]
    fn touch_keeps_other_fields() {
        let mut m = map(json!({"/a": {"last_used": 1, "ref_dirs": ["/notes"], "future": {"x": 1}}}));
        assert!(touch(&mut m, "/a", 100, MAX).is_empty());
        assert_eq!(m["/a"], json!({"last_used": 100, "ref_dirs": ["/notes"], "future": {"x": 1}}));
        assert!(touch(&mut m, "/b", 200, MAX).is_empty());
        assert_eq!(m["/b"], json!({"last_used": 200}));
        // 往復しても未知の項目は残る
        let back = parse(Some(&serialize(&m))).unwrap();
        assert_eq!(back, m);
    }

    /// 上限: 超えた分を古い順に落とし（落とした root を古い順に返す）、現在の root は古くても落とさない
    #[test]
    fn touch_evicts_oldest_but_never_current() {
        let mut m = Map::new();
        for i in 0..MAX {
            assert!(touch(&mut m, &format!("/w{i}"), 100 + i as i64, MAX).is_empty());
        }
        assert_eq!(m.len(), MAX);
        // 11 件目 = 最も古い /w0 が落ちる
        assert_eq!(touch(&mut m, "/new", 1000, MAX), vec!["/w0".to_string()]);
        assert_eq!(m.len(), MAX);
        assert!(!m.contains_key("/w0") && m.contains_key("/new"));
        // 現在の root の時刻が最も古くても（時計が戻った・last_used の無い項目）落とさない = 次に古いものを落とす
        let mut m2 = Map::new();
        for i in 0..MAX {
            touch(&mut m2, &format!("/w{i}"), 100 + i as i64, MAX);
        }
        assert_eq!(touch(&mut m2, "/cur", 1, MAX), vec!["/w0".to_string()]);
        assert!(m2.contains_key("/cur"));
        // 上限を下げる（max = 3）と一度に複数落とす・古い順
        assert_eq!(touch(&mut m2, "/w5", 2000, 3), vec!["/cur".to_string(), "/w1".into(), "/w2".into(), "/w3".into(), "/w4".into(), "/w6".into(), "/w7".into()]);
        assert_eq!(roots(&m2), vec!["/w5".to_string(), "/w9".into(), "/w8".into()]);
        // max = 0 でも現在の root は残る
        assert_eq!(touch(&mut m2, "/w9", 3000, 0), vec!["/w8".to_string(), "/w5".into()]);
        assert_eq!(roots(&m2), vec!["/w9".to_string()]);
    }

    /// 並び = 新しい順・同点は root の文字列順・last_used の無い / 数でない項目は 0
    #[test]
    fn order_is_newest_first_and_deterministic() {
        let m = map(json!({
            "/b": {"last_used": 5},
            "/a": {"last_used": 5},
            "/c": {"last_used": 9},
            "/d": {},
            "/e": {"last_used": "x"},
            "/f": {"last_used": 7.9},
        }));
        assert_eq!(
            order(&m),
            vec![("/c".to_string(), 9), ("/f".into(), 7), ("/a".into(), 5), ("/b".into(), 5), ("/d".into(), 0), ("/e".into(), 0)]
        );
    }

    /// rekey: 生の綴りの項目を canonical へ寄せる = last_used は大きい方・ref_dirs は和（to が先）・他の項目は to を優先・from は消える
    #[test]
    fn rekey_merges_spellings() {
        let mut m = map(json!({
            "/v/notes": {"last_used": 0, "ref_dirs": ["/ref", "/both"], "future": "raw", "only_raw": 1},
            "/v/notes-real": {"last_used": 50, "ref_dirs": ["/both", "/c"], "future": "canon"},
            "/v/other": {"last_used": 7},
        }));
        assert!(rekey(&mut m, "/v/notes", "/v/notes-real"));
        assert_eq!(
            m["/v/notes-real"],
            json!({"last_used": 50, "ref_dirs": ["/both", "/c", "/ref"], "future": "canon", "only_raw": 1})
        );
        assert!(!m.contains_key("/v/notes"));
        assert_eq!(m["/v/other"], json!({"last_used": 7}), "他の root は触らない");
        // to が無い = from の中身をそのまま（last_used が大きい方 = from の値）
        let mut m = map(json!({"/v/ref/": {"last_used": 9, "ref_dirs": ["/ref"]}}));
        assert!(rekey(&mut m, "/v/ref/", "/v/ref"));
        assert_eq!(m, map(json!({"/v/ref": {"last_used": 9, "ref_dirs": ["/ref"]}})));
        // from の last_used の方が新しい・どちらにも ref_dirs が無い = ref_dirs を作らない
        let mut m = map(json!({"/a": {"last_used": 90}, "/b": {"last_used": 10}}));
        assert!(rekey(&mut m, "/a", "/b"));
        assert_eq!(m, map(json!({"/b": {"last_used": 90}})));
        // from が無い・from == to = 何もしない
        let before = m.clone();
        assert!(!rekey(&mut m, "/none", "/b"));
        assert!(!rekey(&mut m, "/b", "/b"));
        assert_eq!(m, before);
    }

    /// set_excluded の 4 通り（除く × 既定に当たる / 当たらない・入れる × 同）と往復・未知の項目を保つ・rules が読む
    #[test]
    fn set_excluded_transitions() {
        let mut m = map(json!({"/v": {"last_used": 3, "ref_dirs": ["/notes"], "future": 1}}));
        // 1. 普通のフォルダを除く = exclude に足す
        set_excluded(&mut m, "/v", "proj/testdata", true, false);
        assert_eq!(m["/v"], json!({"last_used": 3, "ref_dirs": ["/notes"], "future": 1, "exclude": ["proj/testdata"]}));
        // 2. それを入れ直す = exclude から外す（既定に当たらない = include に足さない）
        set_excluded(&mut m, "/v", "proj/testdata", false, false);
        assert_eq!(m["/v"], json!({"last_used": 3, "ref_dirs": ["/notes"], "future": 1, "exclude": []}));
        // 3. 既定で除かれるフォルダを入れる = include に足す
        set_excluded(&mut m, "/v", "proj/target", false, true);
        assert_eq!(m["/v"]["include"], json!(["proj/target"]));
        assert_eq!(m["/v"]["exclude"], json!([]));
        // 4. それを除き直す = include から外す（既定に当たる = exclude には足さない）
        set_excluded(&mut m, "/v", "proj/target", true, true);
        assert_eq!(m["/v"], json!({"last_used": 3, "ref_dirs": ["/notes"], "future": 1, "exclude": [], "include": []}));
        // 重複を作らない・他のフォルダは保つ
        set_excluded(&mut m, "/v", "a", true, false);
        set_excluded(&mut m, "/v", "a", true, false);
        set_excluded(&mut m, "/v", "b", true, false);
        assert_eq!(m["/v"]["exclude"], json!(["a", "b"]));
        // rules が読む（正規化つき）
        let r = rules(&m, "/v");
        assert_eq!(r.exclude(), ["a".to_string(), "b".into()]);
        assert!(r.include().is_empty());
        // root の項目が無ければ作る（last_used は入れない）
        set_excluded(&mut m, "/w", "x", true, false);
        assert_eq!(m["/w"], json!({"exclude": ["x"]}));
        // 往復しても残る
        assert_eq!(parse(Some(&serialize(&m))).unwrap(), m);
        // 配列でない値は空として扱う
        let mut odd = map(json!({"/v": {"exclude": "x", "include": 3}}));
        set_excluded(&mut odd, "/v", "y", true, false);
        assert_eq!(odd["/v"], json!({"exclude": ["y"], "include": []}));
        assert!(rules(&map(json!({"/v": {"exclude": "x"}})), "/v").exclude().is_empty());
    }

    /// 正規化できない dir（空・`.`・空の区画・`.` 始まり）は何も書かず false（root の項目も作らない）・書いたら true
    #[test]
    fn set_excluded_refuses_unnormalizable_dir() {
        let mut m = map(json!({"/v": {"last_used": 1, "exclude": ["a"]}}));
        let before = m.clone();
        for bad in ["", ".", "a//b", "/", " ", "x/../y", ".git"] {
            for (excluded, hit) in [(true, false), (false, true), (true, true), (false, false)] {
                assert!(!set_excluded(&mut m, "/v", bad, excluded, hit), "{bad:?}");
                assert!(!set_excluded(&mut m, "/new", bad, excluded, hit), "{bad:?}");
            }
        }
        assert_eq!(m, before, "設定を変えない");
        assert!(set_excluded(&mut m, "/v", "b/", true, false));
        assert_eq!(m["/v"]["exclude"], json!(["a", "b"]));
    }

    /// 手で書いた綴り（末尾 `/`・先頭 `/`・NFD）の既存の要素も、走査の規則と同じ正規化で比べて外す
    #[test]
    fn set_excluded_matches_normalized_spellings() {
        use unicode_normalization::UnicodeNormalization;
        let nfd: String = "データ/raw".nfd().collect();
        assert_ne!(nfd, "データ/raw");
        let mut m = map(json!({"/v": {"exclude": ["proj/testdata/", nfd.clone(), "/keep/", "../bad"], "include": [" proj/target/ "]}}));
        // 走査はこれらを除外として読む
        assert_eq!(rules(&m, "/v").exclude(), ["proj/testdata".to_string(), "データ/raw".into(), "keep".into()]);
        set_excluded(&mut m, "/v", "proj/testdata", false, false);
        set_excluded(&mut m, "/v", "データ/raw", false, false);
        // 入れ直した 2 つは外れ（走査の規則からも消える）・他の要素（正規化できない物も）は触らない
        assert_eq!(m["/v"]["exclude"], json!(["/keep/", "../bad"]));
        assert_eq!(rules(&m, "/v").exclude(), ["keep".to_string()]);
        // include 側も同じ（既定に当たるフォルダを除き直す = include から外す）
        set_excluded(&mut m, "/v", "proj/target", true, true);
        assert_eq!(m["/v"]["include"], json!([]));
        // 書く側は正規化した綴り（NFD で渡されても NFC で 1 つ）
        set_excluded(&mut m, "/v", &nfd, true, false);
        set_excluded(&mut m, "/v", "データ/raw/", true, false);
        assert_eq!(m["/v"]["exclude"], json!(["/keep/", "../bad", "データ/raw"]));
    }

    /// remove・ref_dirs・set_ref_dirs（無い root は last_used なしで作る・他の項目は保つ）
    #[test]
    fn ref_dirs_and_remove() {
        let mut m = map(json!({"/a": {"last_used": 1, "ref_dirs": ["/notes", "", 3, "/notes", "/x"], "k": true}}));
        assert_eq!(ref_dirs(&m, "/a"), vec!["/notes".to_string(), "/x".into()]);
        assert!(has_ref_dirs(&m, "/a"));
        assert!(ref_dirs(&m, "/none").is_empty() && !has_ref_dirs(&m, "/none"));
        set_ref_dirs(&mut m, "/a", &["/y".to_string()]);
        assert_eq!(m["/a"], json!({"last_used": 1, "ref_dirs": ["/y"], "k": true}));
        set_ref_dirs(&mut m, "/b", &[]);
        assert_eq!(m["/b"], json!({"ref_dirs": []}));
        assert!(has_ref_dirs(&m, "/b"), "空の配列でもある");
        // ref_dirs が配列でない = 空
        let odd = map(json!({"/a": {"ref_dirs": "/notes"}}));
        assert!(ref_dirs(&odd, "/a").is_empty());
        assert!(remove(&mut m, "/b"));
        assert!(!remove(&mut m, "/b"));
        assert_eq!(m.keys().cloned().collect::<Vec<_>>(), vec!["/a".to_string()]);
    }
}
