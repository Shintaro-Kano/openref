//! OpenNote のチャットのログ: 会話をワークスペースの `chat-log/<日時> <最初の発言>.md` に追記する
//! （以前は ingest/chat/ だった。ingest/ は未処理の素材の置き場・chat-log/ は会話の記録。どちらも木の下の箱に出る）。
//! Settings › OpenNote › 「チャットのログを chat-log/ に残す」（既定 ON・`wiki_chat_log` = "0" で OFF）。
//! - 最初の発言で初めてファイルを作る（空のログを作らない）。同じ会話（session）のログが chat-log/ に既にあれば続きを書く
//! - 追記だけ（上書きしない）。書けなければ 1 度だけ stderr の知らせを出し、以後その会話では書かない
//! - 形: frontmatter（type: chat・session・started）→ `## 👤 HH:MM` 発言 + `> 文脈:` → `## 🤖 HH:MM (model)` 本文・`- 🔧 道具: 要点`
//!   → ターンの終わり（result）に `- 変更: …`（Tracker の記録）。ワークスペースの中なので watcher が索引し、Chat log 箱に出る
use crate::tr;
use crate::wiki_chat::Change;
// symlink でないフォルダ / 通常のファイルか（ログはワークスペースの外へ出ない = chat-log/ が symlink なら書かない）
use openref_core::wiki::paths::{is_real_dir, is_real_file};
use serde_json::Value;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// ログの置き場（ワークスペース直下・ingest/ と同列）
pub const LOG_DIR: &str = "chat-log";
/// ファイル名に使う最初の発言の長さ（文字）
const TITLE_CHARS: usize = 40;
/// 道具の要点の長さ（文字）
const SUMMARY_CHARS: usize = 100;

/// 試験: そのワークスペースのログの追記を 1 回ごとに遅らせる（wiki_chat の詰まり試験 = Drive 上で追記が待つ場合の写し）
#[cfg(test)]
pub static SLOW_APPEND: std::sync::Mutex<Vec<(PathBuf, std::time::Duration)>> =
    std::sync::Mutex::new(Vec::new());

pub struct ChatLog {
    pub root: PathBuf,
    pub session: String,
    en: bool,
    /// 作ったログ（最初の発言まで None）
    file: Option<PathBuf>,
    /// system/init の model（🤖 の見出しに添える）
    model: Option<String>,
    /// 書けなかった = 以後書かない
    disabled: bool,
    /// このターンに書いた応答の block（message id + 内容。同じ block を 2 度書かない）
    written: HashSet<String>,
    /// このターンに 🤖 の見出しを書いたか
    heading: bool,
}

/// ローカル時刻 (年, 月, 日, 時, 分)
fn local_now() -> (i32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    local_at(secs)
}

/// unix 秒 → ローカル時刻 (年, 月, 日, 時, 分)。変換できなければ 1970-01-01 00:00
fn local_at(secs: i64) -> (i32, u32, u32, u32, u32) {
    openref_core::wiki::edit::local_time(secs)
        .map_or((1970, 1, 1, 0, 0), |t| (t.year, t.month, t.day, t.hour, t.minute))
}

fn stamp_date(t: (i32, u32, u32, u32, u32)) -> String {
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.0, t.1, t.2, t.3, t.4)
}

fn stamp_time(t: (i32, u32, u32, u32, u32)) -> String {
    format!("{:02}:{:02}", t.3, t.4)
}

/// 先頭 n 文字（超えたら …）。改行・制御文字は空白にして畳む
fn clip(s: &str, n: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        flat
    } else {
        flat.chars()
            .take(n)
            .collect::<String>()
            .trim_end()
            .to_string()
            + "…"
    }
}

/// ファイル名の題 = 最初の発言の 1 行目（`/ \ : * ? " < > |` と制御文字・見えない書式文字（core の invisible = 貼付・取込の
/// ファイル名と同じ表）を外し、空白を畳み、40 字まで）。何も残らなければ既定
pub fn log_title(text: &str, en: bool) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let cleaned: String = first
        .chars()
        .filter(|c| {
            !c.is_control()
                && !openref_core::wiki::edit::invisible(*c)
                && !matches!(
                    c,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '[' | ']' | '#' | '^'
                )
        })
        .collect();
    let mut t = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > TITLE_CHARS {
        t = t
            .chars()
            .take(TITLE_CHARS)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    let t = t.trim_matches('.').trim().to_string();
    if t.is_empty() {
        if en {
            "chat".into()
        } else {
            "チャット".into()
        }
    } else {
        t
    }
}

/// 道具の 1 行の要点: `name: 要点`（openref の call は `openref call <コマンド>: path / key`・他は path / file_path / pattern / command / query / url の最初のもの）。
/// path / file_path の絶対パスはワークスペース（root）の中なら相対にする（外 = 参照フォルダ等は絶対のまま）。pattern / command 等は触らない
pub fn tool_summary(name: &str, input: &Value, root: &Path) -> String {
    let pick = |v: &Value| {
        [
            "path",
            "file_path",
            "pattern",
            "command",
            "query",
            "url",
            "key",
            "title",
        ]
        .iter()
        .find_map(|k| v.get(k).and_then(Value::as_str).map(|s| (*k, s)))
        .map(|(k, s)| match k {
            "path" | "file_path" => clip(
                &rel_in(root, s).unwrap_or_else(|| s.to_string()),
                SUMMARY_CHARS,
            ),
            _ => clip(s, SUMMARY_CHARS),
        })
    };
    if name == crate::wiki_chat::CALL_TOOL {
        let cmd = input.get("name").and_then(Value::as_str).unwrap_or("?");
        let args = input.get("args").unwrap_or(&Value::Null);
        match pick(args) {
            Some(s) => format!("openref call {cmd}: {s}"),
            None => format!("openref call {cmd}"),
        }
    } else {
        match pick(input) {
            Some(s) => format!("{name}: {s}"),
            None => name.to_string(),
        }
    }
}

/// 絶対パス s が root（そのままか実体のパス = 子の cwd は実体で見えうる）の中なら root からの相対（root 自身は "."）
fn rel_in(root: &Path, s: &str) -> Option<String> {
    let p = Path::new(s);
    if !p.is_absolute() {
        return None;
    }
    let rel = p
        .strip_prefix(root)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            let real = std::fs::canonicalize(root).ok()?;
            p.strip_prefix(real).ok().map(Path::to_path_buf)
        })?;
    let rel = rel.to_str()?;
    Some(if rel.is_empty() {
        ".".into()
    } else {
        rel.into()
    })
}

/// chat-log/ の中で、frontmatter（先頭の `---` から次の `---` まで・12 行以内）に `session: <id>` を持つ既存のログ。symlink は見ない
fn find_existing(dir: &Path, session: &str) -> Option<PathBuf> {
    let want = format!("session: {session}");
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    names.sort();
    names.into_iter().find(|p| {
        let Ok(f) = std::fs::File::open(p) else {
            return false;
        };
        use std::io::BufRead;
        let mut lines = std::io::BufReader::new(f)
            .lines()
            .take(12)
            .map_while(Result::ok);
        if lines.next().as_deref().map(str::trim) != Some("---") {
            return false;
        }
        for l in lines {
            let l = l.trim();
            if l == "---" {
                return false;
            }
            if l == want {
                return true;
            }
        }
        false
    })
}

impl ChatLog {
    pub fn new(root: PathBuf, session: String, en: bool) -> ChatLog {
        ChatLog {
            root,
            session,
            en,
            file: None,
            model: None,
            disabled: false,
            written: HashSet::new(),
            heading: false,
        }
    }

    /// 書いているログ（絶対パス）。最初の発言まで None
    #[cfg(test)]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// 追記。ファイルが消えた / 移された / symlink に替わったら忘れる（この行は書かない。次の発言で作り直す = 消したログを蘇らせない・
    /// 移したログは find_existing が見つける）
    fn append(&mut self, s: &str) -> Result<(), String> {
        let Some(f) = self.file.as_ref() else {
            return Ok(());
        };
        if !is_real_file(f) {
            self.file = None;
            return Ok(());
        }
        #[cfg(test)]
        if let Some(d) = SLOW_APPEND
            .lock()
            .unwrap()
            .iter()
            .find(|(r, _)| *r == self.root)
            .map(|(_, d)| *d)
        {
            std::thread::sleep(d);
        }
        let r = std::fs::OpenOptions::new()
            .append(true)
            .open(f)
            .and_then(|mut h| h.write_all(s.as_bytes()));
        r.map_err(|e| {
            self.disabled = true;
            format!("{}: {e}", f.display())
        })
    }

    /// 最初の発言でファイルを作る（既にこの会話のログがあればそれに続ける）
    fn ensure_file(&mut self, text: &str) -> Result<(), String> {
        if self.file.is_some() {
            return Ok(());
        }
        let dir = self.root.join(LOG_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| {
            self.disabled = true;
            format!("{}: {e}", dir.display())
        })?;
        // chat-log/ が symlink なら書かない（ワークスペースの外へ出ない = core の edit の規則と同じ）
        if !is_real_dir(&dir) {
            self.disabled = true;
            return Err(format!(
                "{}: {}",
                dir.display(),
                tr(self.en, "symlink には書きません", "symlinks are not written to")
            ));
        }
        if let Some(p) = find_existing(&dir, &self.session) {
            self.file = Some(p);
            return self.append("\n");
        }
        let now = local_now();
        let stem = format!(
            "{:04}-{:02}-{:02} {:02}{:02} {}",
            now.0,
            now.1,
            now.2,
            now.3,
            now.4,
            log_title(text, self.en)
        );
        let head = format!(
            "---\ntype: chat\nsession: {}\nstarted: {}\n---\n",
            self.session,
            stamp_date(now)
        );
        let mut n = 1;
        loop {
            let p = dir.join(if n == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem} ({n}).md")
            });
            // create_new = 同名（壊れた symlink も）があれば失敗 → 次の番号
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
            {
                Ok(mut h) => {
                    h.write_all(head.as_bytes()).map_err(|e| {
                        self.disabled = true;
                        format!("{}: {e}", p.display())
                    })?;
                    self.file = Some(p);
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
                Err(e) => {
                    self.disabled = true;
                    return Err(format!("{}: {e}", p.display()));
                }
            }
        }
    }

    /// 発言（send）。context = ページ / 文献 / 選択範囲
    pub fn user(
        &mut self,
        text: &str,
        path: Option<&str>,
        selection: Option<&str>,
        item: Option<(&str, &str)>,
    ) -> Result<(), String> {
        if self.disabled {
            return Ok(());
        }
        // 前の発言の後にファイルが消えた / 移されたら忘れる（移されたものは ensure_file が session で見つけて続ける）
        if self.file.as_ref().is_some_and(|f| !is_real_file(f)) {
            self.file = None;
        }
        self.ensure_file(text)?;
        self.written.clear();
        self.heading = false;
        let mut s = format!(
            "\n## 👤 {}\n\n{}\n",
            stamp_time(local_now()),
            text.trim_end()
        );
        let mut ctx: Vec<String> = Vec::new();
        if let Some(p) = path.map(str::trim).filter(|p| !p.is_empty()) {
            // 参照フォルダ（絶対パス）は vault のページでないので [[ ]] にしない
            let link = if p.starts_with('/') {
                p.to_string()
            } else {
                format!("[[{p}]]")
            };
            ctx.push(format!("{} {link}", tr(self.en, "ページ", "page")));
        }
        if let Some((k, t)) = item
            .map(|(k, t)| (clip(k, SUMMARY_CHARS), clip(t, SUMMARY_CHARS)))
            .filter(|(k, _)| !k.is_empty())
        {
            ctx.push(format!(
                "{} {k}{}{t}",
                tr(self.en, "文献", "item"),
                if t.is_empty() { "" } else { " " }
            ));
        }
        if let Some(sel) = selection.filter(|t| !t.trim().is_empty()) {
            ctx.push(format!(
                "{} {} {}",
                tr(self.en, "選択範囲", "selection"),
                sel.chars().count(),
                tr(self.en, "字", "chars")
            ));
            s.push_str(&format!(
                "\n> {}: {}\n",
                tr(self.en, "文脈", "context"),
                ctx.join(" · ")
            ));
            s.push('>');
            s.push('\n');
            for l in sel.trim_end().lines() {
                s.push_str(&format!("> {l}\n"));
            }
        } else if !ctx.is_empty() {
            s.push_str(&format!(
                "\n> {}: {}\n",
                tr(self.en, "文脈", "context"),
                ctx.join(" · ")
            ));
        }
        self.append(&s)
    }

    /// 応答中に子が終わった（result が来ない）= その印を残す
    pub fn ended_early(&mut self) -> Result<(), String> {
        if self.disabled || self.file.is_none() {
            return Ok(());
        }
        self.ensure_heading()?;
        self.append(&format!(
            "\n> {}\n",
            tr(
                self.en,
                "応答は途中で終わりました（子プロセスが終了）",
                "The reply ended early (the process exited)"
            )
        ))
    }

    fn ensure_heading(&mut self) -> Result<(), String> {
        if self.heading {
            return Ok(());
        }
        self.heading = true;
        let model = self
            .model
            .as_deref()
            .map(|m| format!(" ({m})"))
            .unwrap_or_default();
        self.append(&format!("\n## 🤖 {}{model}\n", stamp_time(local_now())))
    }

    /// 子の 1 行（JSON）。changes = result の行のときのこのターンの変更
    pub fn line(&mut self, v: &Value, changes: &[Change]) -> Result<(), String> {
        if self.disabled || self.file.is_none() {
            // init の model はファイルを作る前にも覚える
            if let Some(m) = (v.get("type").and_then(Value::as_str) == Some("system")
                && v.get("subtype").and_then(Value::as_str) == Some("init"))
            .then(|| v.get("model").and_then(Value::as_str))
            .flatten()
            {
                self.model = Some(m.to_string());
            }
            return Ok(());
        }
        // 下位の agent（Task）の行は画面の会話にも出さない（reducer と同じ）
        if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
            return Ok(());
        }
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "system" => {
                if v.get("subtype").and_then(Value::as_str) == Some("init") {
                    if let Some(m) = v.get("model").and_then(Value::as_str) {
                        self.model = Some(m.to_string());
                    }
                }
                Ok(())
            }
            "assistant" => {
                let id = v
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
                    return Ok(());
                };
                let mut out = String::new();
                for b in blocks {
                    let key = format!("{id}\u{0}{b}");
                    if !self.written.insert(key) {
                        continue;
                    }
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if !t.is_empty() {
                                out.push_str(&format!("\n{t}\n"));
                            }
                        }
                        Some("tool_use") => {
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                            out.push_str(&format!(
                                "- 🔧 {}\n",
                                tool_summary(
                                    name,
                                    b.get("input").unwrap_or(&Value::Null),
                                    &self.root
                                )
                            ));
                        }
                        _ => {}
                    }
                }
                if out.is_empty() {
                    return Ok(());
                }
                self.ensure_heading()?;
                self.append(&out)
            }
            "result" => {
                let mut out = String::new();
                if !changes.is_empty() {
                    self.ensure_heading()?;
                    out.push('\n');
                    for c in changes {
                        let what = match c {
                            Change::Write { path, .. } => {
                                format!("{} [[{path}]]", tr(self.en, "上書き", "overwrote"))
                            }
                            Change::Create { path, .. } => {
                                format!("{} [[{path}]]", tr(self.en, "新規", "created"))
                            }
                            Change::Move { from, to } => {
                                format!("{} {from} → [[{to}]]", tr(self.en, "移動", "moved"))
                            }
                            Change::Item { key, title } => format!(
                                "{} {key}{}",
                                tr(self.en, "蔵書に登録", "added to the library"),
                                title
                                    .as_deref()
                                    .map(|t| format!(" {}", clip(t, SUMMARY_CHARS)))
                                    .unwrap_or_default()
                            ),
                        };
                        out.push_str(&format!("- {}: {what}\n", tr(self.en, "変更", "change")));
                    }
                }
                if v.get("is_error").and_then(Value::as_bool) == Some(true)
                    || v.get("subtype")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s != "success")
                {
                    self.ensure_heading()?;
                    out.push_str(&format!(
                        "\n> {}\n",
                        tr(
                            self.en,
                            "応答は途中で終わりました（停止 / エラー）",
                            "The reply ended early (stopped / error)"
                        )
                    ));
                }
                self.append(&out)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn title_is_first_line_cleaned_and_clipped() {
        assert_eq!(
            log_title("  \n 胆道/の話: a*b?\"<c>|d\\e  \n2 行目", false),
            "胆道の話 abcde"
        );
        assert_eq!(log_title("あ".repeat(60).as_str(), false), "あ".repeat(40));
        assert_eq!(log_title("   \n\t", false), "チャット");
        assert_eq!(log_title("", true), "chat");
        assert_eq!(log_title("...", false), "チャット");
        assert_eq!(log_title("#tag [[x]] ^b \u{202E}y", false), "tag x b y");
    }

    /// 見えない書式文字は core の invisible の表（貼付・取込のファイル名と同じ）で落とす = 行 / 段落区切り・soft hyphen・
    /// BOM・タグ文字も題に残らない（旧の表は 200B-200F・202A-202E・2060-2064・2066-2069・FEFF だけ）
    #[test]
    fn title_drops_core_invisible_chars() {
        for (input, want) in [
            ("a\u{2028}b", "ab"),
            ("a\u{2029}b", "ab"),
            ("soft\u{00AD}hyphen", "softhyphen"),
            ("\u{FEFF}bom", "bom"),
            ("tag\u{E0001}\u{E0041}\u{E007F}s", "tags"),
            ("a\u{061C}b\u{180E}c", "abc"),
            ("a\u{206A}b\u{206F}c", "abc"),
            ("a\u{FFF9}b\u{FFFB}c", "abc"),
            ("z\u{200B}w\u{202E}v", "zwv"),
            ("\u{2028}\u{00AD}", "チャット"),
        ] {
            assert_eq!(log_title(input, false), want, "{input:?}");
        }
        for c in (0u32..0x11_0000)
            .filter_map(char::from_u32)
            .filter(|c| openref_core::wiki::edit::invisible(*c))
        {
            assert_eq!(log_title(&format!("a{c}b"), false), "ab", "U+{:04X}", c as u32);
        }
    }

    /// 旧の libc の写し（core の local_time に寄せる前）と同じ (年, 月, 日, 時, 分)・整形も同じ
    #[test]
    fn local_at_matches_old_libc() {
        fn old(secs: i64) -> (i32, u32, u32, u32, u32) {
            // SAFETY: localtime_r に有効なポインタを渡すだけ
            unsafe {
                let t = secs as libc::time_t;
                let mut tm: libc::tm = std::mem::zeroed();
                if libc::localtime_r(&t, &mut tm).is_null() {
                    return (1970, 1, 1, 0, 0);
                }
                (
                    tm.tm_year + 1900,
                    (tm.tm_mon + 1) as u32,
                    tm.tm_mday as u32,
                    tm.tm_hour as u32,
                    tm.tm_min as u32,
                )
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        for s in [0, 1, 59, 86_399, 951_782_400, 1_700_000_000, 2_147_483_647, 4_102_444_800, now, now + 3_600 * 13] {
            assert_eq!(local_at(s), old(s), "{s}");
            assert_eq!(stamp_date(local_at(s)), stamp_date(old(s)));
            assert_eq!(stamp_time(local_at(s)), stamp_time(old(s)));
        }
        let (y, ..) = local_now();
        assert!(y >= 2026);
    }

    #[test]
    fn tool_summary_picks_the_point() {
        let v = Path::new("/v");
        assert_eq!(
            tool_summary(
                crate::wiki_chat::CALL_TOOL,
                &json!({"name": "wiki_write", "args": {"path": "a.md", "content": "x"}}),
                v
            ),
            "openref call wiki_write: a.md"
        );
        assert_eq!(
            tool_summary(
                crate::wiki_chat::CALL_TOOL,
                &json!({"name": "search", "args": {"q": "x"}}),
                v
            ),
            "openref call search"
        );
        // ワークスペースの中の絶対パスは相対（root 自身は "."）・外（参照フォルダ・名前が前方一致だけの /v2）は絶対のまま
        assert_eq!(
            tool_summary("Read", &json!({"file_path": "/v/notes/a.md"}), v),
            "Read: notes/a.md"
        );
        assert_eq!(tool_summary("Glob", &json!({"path": "/v"}), v), "Glob: .");
        assert_eq!(
            tool_summary("Read", &json!({"file_path": "/ref/p.pdf"}), v),
            "Read: /ref/p.pdf"
        );
        assert_eq!(
            tool_summary("Read", &json!({"file_path": "/v2/a.md"}), v),
            "Read: /v2/a.md"
        );
        // pattern / command の中のパスは触らない
        assert_eq!(
            tool_summary("Grep", &json!({"pattern": "/v/x"}), v),
            "Grep: /v/x"
        );
        assert_eq!(
            tool_summary(
                "Bash",
                &json!({"command": format!("echo {}\nls", "x".repeat(200))}),
                v
            ),
            format!("Bash: echo {}…", "x".repeat(95))
        );
        assert_eq!(
            tool_summary("TodoWrite", &json!({"todos": []}), v),
            "TodoWrite"
        );
        // 子の cwd は実体のパスで見えうる（/var → /private/var）= root の実体の中も相対
        let dir = std::env::temp_dir().join(format!(
            "openref-chatlog-rel-{}-{}",
            std::process::id(),
            crate::wiki_chat::new_uuid()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let real = std::fs::canonicalize(&dir).unwrap().join("b.md");
        assert_eq!(
            tool_summary("Write", &json!({"file_path": real.to_str().unwrap()}), &dir),
            "Write: b.md"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn log_file_is_created_on_first_user_message_and_appended() {
        let dir = std::env::temp_dir().join(format!(
            "openref-chatlog-{}-{}",
            std::process::id(),
            crate::wiki_chat::new_uuid()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lg = ChatLog::new(dir.clone(), "S1".into(), false);
        // init は作る前でも model を覚える・ファイルは作らない
        lg.line(
            &json!({"type": "system", "subtype": "init", "model": "claude-x"}),
            &[],
        )
        .unwrap();
        assert!(!dir.join(LOG_DIR).exists());
        lg.user(
            "要約して\n2 行目",
            Some("notes/a.md"),
            Some("選択\nの 2 行"),
            None,
        )
        .unwrap();
        let f = lg.file().unwrap().to_path_buf();
        assert!(
            f.starts_with(dir.join(LOG_DIR))
                && f.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .ends_with(" 要約して.md"),
            "{}",
            f.display()
        );
        let msg = |id: &str, b: Value| json!({"type": "assistant", "message": {"id": id, "content": [b]}});
        lg.line(&msg("m1", json!({"type": "text", "text": "はい。"})), &[])
            .unwrap();
        lg.line(&msg("m1", json!({"type": "tool_use", "id": "t1", "name": crate::wiki_chat::CALL_TOOL, "input": {"name": "wiki_write", "args": {"path": "a.md"}}})), &[]).unwrap();
        lg.line(&msg("m1", json!({"type": "text", "text": "はい。"})), &[])
            .unwrap(); // 同じ block は 2 度書かない
        lg.line(
            &json!({"type": "result", "subtype": "success"}),
            &[Change::Create {
                path: "a.md".into(),
                sha256_after: "x".into(),
            }],
        )
        .unwrap();
        let s = std::fs::read_to_string(&f).unwrap();
        assert!(
            s.starts_with("---\ntype: chat\nsession: S1\nstarted: "),
            "{s}"
        );
        assert!(s.contains("\n## 👤 ") && s.contains("\n要約して\n2 行目\n\n> 文脈: ページ [[notes/a.md]] · 選択範囲 8 字\n>\n> 選択\n> の 2 行\n"), "{s}");
        assert!(s.contains("## 🤖 ") && s.contains(" (claude-x)\n\nはい。\n- 🔧 openref call wiki_write: a.md\n\n- 変更: 新規 [[a.md]]\n"), "{s}");
        assert_eq!(s.matches("はい。").count(), 1);
        // 2 つ目の発言（文献の文脈）・停止で終わるターン
        lg.user("次", None, None, Some(("K1", "題名"))).unwrap();
        lg.line(
            &json!({"type": "result", "subtype": "error_during_execution", "is_error": true}),
            &[],
        )
        .unwrap();
        let s = std::fs::read_to_string(&f).unwrap();
        assert!(
            s.contains("\n次\n\n> 文脈: 文献 K1 題名\n")
                && s.contains("> 応答は途中で終わりました"),
            "{s}"
        );
        // 同じ会話を別の ChatLog（起動し直し）で続ける = 既存のファイルに追記。別の会話は新しいファイル
        let mut lg2 = ChatLog::new(dir.clone(), "S1".into(), false);
        lg2.user("続き", None, None, None).unwrap();
        assert_eq!(lg2.file().unwrap(), f.as_path());
        let mut lg3 = ChatLog::new(dir.clone(), "S2".into(), true);
        lg3.user("", None, None, None).unwrap();
        assert_ne!(lg3.file().unwrap(), f.as_path());
        assert!(lg3
            .file()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(" chat.md"));
        assert_eq!(std::fs::read_dir(dir.join(LOG_DIR)).unwrap().count(), 2);
        // 消されたログは蘇らせない: 消した後の応答の行は書かず、次の発言で新しいファイルを作る（移したログは session で見つけて続ける）
        std::fs::remove_file(&f).unwrap();
        lg.line(&msg("m9", json!({"type": "text", "text": "消えた後"})), &[])
            .unwrap();
        assert!(!f.exists() && lg.file().is_none(), "消したログを蘇らせた");
        lg.user("消した後の発言", None, None, None).unwrap();
        let f2 = lg.file().unwrap().to_path_buf();
        assert_ne!(f2, f);
        assert!(
            std::fs::read_to_string(&f2)
                .unwrap()
                .starts_with("---\ntype: chat\nsession: S1\n"),
            "frontmatter の無いファイルを作った"
        );
        let moved = dir.join(LOG_DIR).join("moved.md");
        std::fs::rename(&f2, &moved).unwrap();
        lg.user("移した後", None, None, None).unwrap();
        assert_eq!(
            lg.file().unwrap(),
            moved.as_path(),
            "移したログを session で見つけて続けない"
        );
        // 下位の agent（parent_tool_use_id）の行は書かない
        lg.line(&json!({"type": "assistant", "parent_tool_use_id": "T1", "message": {"id": "sub", "content": [{"type": "text", "text": "下位の本文"}]}}), &[]).unwrap();
        assert!(!std::fs::read_to_string(&moved)
            .unwrap()
            .contains("下位の本文"));
        // frontmatter の外の `session: S1`（本文に貼られた前のログ）では見つけない
        std::fs::write(
            dir.join(LOG_DIR).join("paste.md"),
            "---\ntype: chat\nsession: OTHER\nstarted: x\n---\n\nsession: S7\n",
        )
        .unwrap();
        let mut lg7 = ChatLog::new(dir.clone(), "S7".into(), false);
        lg7.user("x", None, None, None).unwrap();
        assert_ne!(lg7.file().unwrap().file_name().unwrap(), "paste.md");
        // chat-log がワークスペースの外への symlink なら書かない（Err 1 回・以後黙る）
        let outside = std::env::temp_dir().join(format!(
            "openref-chatlog-out-{}-{}",
            std::process::id(),
            crate::wiki_chat::new_uuid()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let dir2 = dir.join("v2");
        std::fs::create_dir_all(&dir2).unwrap();
        std::os::unix::fs::symlink(&outside, dir2.join(LOG_DIR)).unwrap();
        let mut lg5 = ChatLog::new(dir2.clone(), "S5".into(), false);
        assert!(lg5
            .user("x", None, None, None)
            .unwrap_err()
            .contains("symlink"));
        assert!(
            lg5.disabled && std::fs::read_dir(&outside).unwrap().count() == 0,
            "symlink の先に書いた"
        );
        let _ = std::fs::remove_dir_all(&outside);
        // 書けない（root が消えた）= Err を 1 回返し、以後は黙る
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("gone"), "").unwrap(); // 親がファイル = create_dir_all が失敗する
        let mut lg4 = ChatLog::new(dir.join("gone").join("deeper"), "S3".into(), false);
        assert!(lg4.user("x", None, None, None).is_err());
        assert!(lg4.disabled);
        assert!(lg4.user("y", None, None, None).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
