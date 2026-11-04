//! OpenRef MCP server — stdio JSON-RPC 2.0 手実装。
//! tool は 3 つだけ: `schema`（スキーマ + 注記 + コマンド一覧）/ `sql`（読み取り専用 SQL）/ `call`（core の登録表
//! = GUI と同じ動詞）。用途はコードに書かない（何をするかはその場の指示とエージェント向け手引きの文章で決まる）。
//! 安全網は 1 つ: このプロセスで最初の（DB を変える）書込コマンドの前に DB のスナップショット（core::backup・失敗したら書かない）。
//! env `OPENREF_MCP_ALLOW`（分類 read / wiki とコマンド名のカンマ区切り）で起動すると、許可外のコマンドは一覧に出さず呼べない。
//! 無い・空なら全部。env `OPENREF_MCP_CHAT=1`（OpenNote のチャットが付ける）は写しの接頭辞を `pre-chat-` にするだけ。
//! env `OPENREF_WIKI_ROOT`（OpenNote のチャットが子の root を付ける）は root を省いた `wiki_*` と許可一覧の path の検査をその root に固定する。
//! 依存は serde_json + rusqlite + openref-core のみ（SDK 非依存）。stdout = プロトコル専用（改行区切り JSON-RPC）、stderr = ログ。

use openref_core::{backup, commands, datadir, wiki, Db};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SQL_ROWS_DEFAULT: u64 = 200;
const SQL_ROWS_MAX: u64 = 2000;
/// 応答の文字数（セルの文字列の合計）の上限。超えたら打ち切って truncated
const SQL_CHARS_MAX: usize = 100_000;
const SQL_TIME_BUDGET: Duration = Duration::from_secs(10);
/// `sql` の接続の SQLITE_LIMIT_LENGTH（1 つの文字列・BLOB・行の最大バイト数）
const SQL_MAX_VALUE_BYTES: i32 = 16 * 1024 * 1024;

fn main() {
    let db_path = match datadir::db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("openref-mcp: {e}");
            std::process::exit(1);
        }
    };
    let db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("openref-mcp: DB open 失敗 {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };
    let mut allow = Allow::parse(std::env::var("OPENREF_MCP_ALLOW").ok().as_deref());
    allow.chat = std::env::var("OPENREF_MCP_CHAT").is_ok_and(|v| v.trim() == "1");
    for w in &allow.warnings {
        eprintln!("openref-mcp: OPENREF_MCP_ALLOW: {w}");
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--schema") {
        print!("{}", schema_text(&db, &allow));
        return;
    }
    if args.iter().any(|a| a == "--commands") {
        // `schema` の §3（コマンド一覧）と同じ Markdown（手引き文書の commands ブロックに貼る用）
        print!("{}", commands::markdown());
        return;
    }
    eprintln!("openref-mcp: ready ({})", db_path.display());
    if let Some(n) = &allow.names {
        eprintln!("openref-mcp: 許可一覧で起動（{} コマンド）", n.len());
    }
    let mut srv = Server::new(db);
    srv.allow = allow;
    srv.wiki_root = std::env::var("OPENREF_WIKI_ROOT").ok().filter(|r| !r.trim().is_empty());
    if let Some(r) = &srv.wiki_root {
        eprintln!("openref-mcp: wiki_* のワークスペースを {r} に固定（OPENREF_WIKI_ROOT）");
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue; // parse 不能な行は黙って捨てる（stdio ノイズ耐性）
        };
        if let Some(resp) = srv.handle_message(&msg) {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
            let _ = out.flush();
        }
    }
}

/// 許可一覧（env `OPENREF_MCP_ALLOW`）。分類 `read` = 登録表の読みのコマンド全部・`wiki` = `wiki_*` 全部・他はコマンド名（別名可）。
/// 新しい分類は作らない。`sql` は読み取り専用なので対象外（いつも使える）
#[derive(Default)]
struct Allow {
    /// None = 全部許す（env が無い・空）
    names: Option<BTreeSet<&'static str>>,
    /// 未知の分類・コマンド名（起動時に stderr へ出す・落とさない）
    warnings: Vec<String>,
    /// env `OPENREF_MCP_CHAT=1` = OpenRef のチャットのセッション（写しの接頭辞だけ変える・コマンドは絞らない）
    chat: bool,
}

/// 許可一覧で起動したとき、ホストのファイルのパス（引数 path）は vault の中だけ受けるコマンド
const HOST_PATH_COMMANDS: [&str; 3] = ["import_pdf", "attach_pdf", "cite_install_file"];

impl Allow {
    fn parse(env: Option<&str>) -> Allow {
        let mut out = Allow::default();
        let words: Vec<&str> = env.unwrap_or("").split(',').map(str::trim).filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            // 値が設定されているのに空白・区切りだけ = 起動側の不具合の可能性（全部許すのは env が無いときと同じ）
            if env.is_some_and(|e| !e.is_empty()) {
                out.warnings.push(format!("値 {:?} にコマンド名がありません。全部のコマンドを許します", env.unwrap_or_default()));
            }
            return out;
        }
        let mut names = BTreeSet::new();
        for w in words {
            let hit: Vec<&'static str> = match w {
                "read" => commands::registry().iter().filter(|c| !c.writes).map(|c| c.name).collect(),
                "wiki" => commands::registry().iter().filter(|c| commands::is_wiki(c)).map(|c| c.name).collect(),
                other => commands::find(other).map(|c| vec![c.name]).unwrap_or_default(),
            };
            if hit.is_empty() {
                out.warnings.push(format!("未知の分類・コマンド名 {w} は無視します（分類は read / wiki・他は schema の一覧のコマンド名）"));
            }
            names.extend(hit);
        }
        out.names = Some(names);
        out
    }

    fn restricted(&self) -> bool {
        self.names.is_some()
    }

    fn allows(&self, c: &commands::Command) -> bool {
        self.names.as_ref().is_none_or(|n| n.contains(c.name))
    }

    /// DB の写しの接頭辞: OpenRef のチャット（OPENREF_MCP_CHAT=1）と許可一覧のセッションは `pre-chat-`・他は `pre-agent-`（新しい 5 個は接頭辞ごと）
    fn snapshot_prefix(&self) -> &'static str {
        if self.chat || self.restricted() {
            backup::CHAT_PREFIX
        } else {
            backup::AGENT_PREFIX
        }
    }
}

struct Server {
    db: Db,
    allow: Allow,
    /// env `OPENREF_WIKI_ROOT`（チャットの子は始めた root に縛る）。Some なら root を省いた wiki_* の args にこの値を入れ
    /// （照合は core の Ctx::pin = 履歴にある root だけ）、許可一覧の path の検査もこの root で行う
    wiki_root: Option<String>,
    /// `sql` 用の読み取り専用の別接続（初回に遅延で開く）
    ro: Option<Connection>,
    /// このプロセスで書込の前のスナップショットを済ませたか
    snapshotted: bool,
    sql_budget: Duration,
}

/// 終了時は `sql` の読み取り専用接続を先に閉じる（フィールドは宣言順 = db → ro で drop される）。書込接続が最後の接続として
/// 閉じれば SQLite が checkpoint して `-wal` を本体に畳む。ro が後に残ると、最後に閉じる ro は READ_ONLY で checkpoint できず
/// `-wal` が残る。ただし SIGTERM や他のプロセス（connectd・app・他のセッションの MCP）が開いている間は `-wal` が残るのが常態 =
/// スナップショットから戻すのは cp ではなく `.restore`（`schema` の §4）
impl Drop for Server {
    fn drop(&mut self) {
        self.ro.take();
    }
}

/// tool のトップレベル引数（arguments 直下）の検査: 未知のキーは受け付けるキーの一覧付きでエラー（綴り違いを黙って無視しない）
fn check_tool_args(tool: &str, args: &Value, accepted: &[&str]) -> Result<(), String> {
    let obj = args.as_object().ok_or_else(|| format!("{tool}: arguments は object（{{...}}）で渡してください"))?;
    match obj.keys().find(|k| !accepted.contains(&k.as_str())) {
        Some(k) => Err(format!("{tool}: 未知の引数 {k}（受け付ける引数: {}）", accepted.join(", "))),
        None => Ok(()),
    }
}

impl Server {
    fn new(db: Db) -> Self {
        Server { db, allow: Allow::default(), wiki_root: None, ro: None, snapshotted: false, sql_budget: SQL_TIME_BUDGET }
    }

    fn handle_message(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        match method {
            "initialize" => {
                let requested = msg.pointer("/params/protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-06-18");
                Some(ok(id?, json!({
                    "protocolVersion": requested,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "openref", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "OpenRef（文献管理）の蔵書。まず schema を読む。読むのは sql、書くのは call。"
                })))
            }
            "ping" => Some(ok(id?, json!({}))),
            "tools/list" => Some(ok(id?, json!({ "tools": tools_json() }))),
            "tools/call" => {
                let id = id?;
                let name = msg.pointer("/params/name").and_then(|v| v.as_str()).unwrap_or("");
                let default_args = json!({});
                let args = msg.pointer("/params/arguments").unwrap_or(&default_args);
                match self.call_tool(name, args) {
                    Ok(content) => Some(ok(id, json!({ "content": content, "isError": false }))),
                    Err(e) => Some(ok(id, json!({
                        "content": [{ "type": "text", "text": e }],
                        "isError": true
                    }))),
                }
            }
            m if m.starts_with("notifications/") => None,
            _ => id.map(|id| err(id, -32601, &format!("method not found: {method}"))),
        }
    }

    /// 戻り値 = MCP の content（text / image）の並び
    fn call_tool(&mut self, name: &str, args: &Value) -> Result<Vec<Value>, String> {
        match name {
            "schema" => Ok(vec![text_content(schema_text(&self.db, &self.allow))]),
            "sql" => {
                check_tool_args("sql", args, &["sql", "params", "max_rows"])?;
                self.run_sql(args).map(|v| vec![text_content(serde_json::to_string_pretty(&v).unwrap())])
            }
            "call" => {
                check_tool_args("call", args, &["name", "args"])?;
                let cmd = args.get("name").and_then(|v| v.as_str()).ok_or("call: name（コマンド名）は必須です（一覧は schema）")?;
                let cargs = args.get("args").cloned().unwrap_or(Value::Null);
                self.run_command(cmd, &cargs)
            }
            // 登録表の名前（旧 15 tool の名前を含む）で直接呼ばれたとき（古い tool 一覧を持ったままのクライアント）は call と同じに扱う
            // （許可一覧の検査も run_command の中 = 同じ）
            other if commands::find(other).is_some() => self.run_command(other, args),
            other => Err(format!("unknown tool: {other}（tool は schema / sql / call）")),
        }
    }

    /// 登録表のコマンドを実行する（call と旧 tool 名の直接呼出しの両方がここを通る = 許可一覧の検査はここだけ）。
    /// DB を変える書込（commands::writes_db）はこのプロセスで最初の 1 回だけ、先に DB のスナップショットを取る
    /// （取れなければ実行しない = fail-closed）。結果の JSON の後ろにスナップショットの場所を添える。
    /// 結果の予約キー `_mcp_images` は抜いて image content にする（text に base64 を流さない）
    fn run_command(&mut self, name: &str, args: &Value) -> Result<Vec<Value>, String> {
        let cmd = commands::find(name).ok_or_else(|| format!("未知のコマンド: {name}（schema の一覧を参照）"))?;
        if !self.allow.allows(cmd) {
            return Err(format!("{name} はこのセッションでは許可されていません（使えるのは schema の一覧のコマンドだけ）"));
        }
        let vaulted;
        let args = if self.allow.restricted() && HOST_PATH_COMMANDS.contains(&cmd.name) {
            vaulted = self.vault_path_arg(args)?;
            &vaulted
        } else {
            args
        };
        let pinned;
        let args = match self.pin_wiki_args(cmd, args) {
            Some(a) => {
                pinned = a;
                &pinned
            }
            None => args,
        };
        let mut note = None;
        if commands::writes_db(cmd) && !self.snapshotted {
            match backup::snapshot(&self.db, self.allow.snapshot_prefix(), backup::AGENT_KEEP) {
                Ok(p) => {
                    self.snapshotted = true;
                    note = p.map(|p| {
                        format!("（安全網）このセッション最初の書込の前に DB のスナップショットを作りました: {}", p.display())
                    });
                }
                Err(e) => {
                    return Err(format!("書込の前の DB スナップショットに失敗したので {name} は実行していません: {e}"))
                }
            }
        }
        match commands::call(&mut self.db, name, args) {
            Ok(v) => Ok(to_content(v, note)),
            Err(e) => Err(match note {
                Some(n) => format!("{e}\n{n}"),
                None => e.to_string(),
            }),
        }
    }

    /// env `OPENREF_WIKI_ROOT` があれば、root を省いた（無い・null）wiki_* の args にその値を入れたもの（None = そのまま渡す）。
    /// root を渡した呼び出しはそのまま（履歴にある別の root も core の pin が受ける）。args が object でも null でもなければ触らない（登録表の検査に任せる）
    fn pin_wiki_args(&self, cmd: &commands::Command, args: &Value) -> Option<Value> {
        let root = self.wiki_root.as_deref().filter(|_| commands::is_wiki(cmd))?;
        let mut a = match args {
            Value::Null => serde_json::Map::new(),
            Value::Object(m) if m.get("root").is_none_or(Value::is_null) => m.clone(),
            _ => return None,
        };
        a.insert("root".into(), json!(root));
        Some(Value::Object(a))
    }

    /// 許可一覧のセッションでホストのファイルを読むコマンドの path を、vault の中の canonical なパスに替える
    /// （vault の外・vault の中の symlink で外を指すもの・Wiki が無効 / 未設定は拒否。検査したパスそのものを渡す）。
    /// vault = env `OPENREF_WIKI_ROOT` があればその root（履歴に無ければ拒否）・無ければ設定 wiki_root
    fn vault_path_arg(&self, args: &Value) -> Result<Value, String> {
        let mut a = args.clone();
        if let Some(p) = args.get("path").and_then(Value::as_str) {
            // 文言は core のものを 1 度だけ（BadInput の「入力を解釈できません」を重ねない）
            let msg = |e: openref_core::CoreError| match e {
                openref_core::CoreError::BadInput(m) => m,
                e => e.to_string(),
            };
            let mut ctx = wiki::Ctx::from_db(&self.db).map_err(msg)?;
            if let Some(r) = &self.wiki_root {
                ctx.pin(r).map_err(msg)?;
            }
            let canon = ctx
                .host_path_in_vault(Path::new(p))
                .map_err(|e| format!("このセッションでは path にワークスペースの中のファイル（絶対パスかワークスペース相対）だけ渡せます（{}）", msg(e)))?;
            a["path"] = json!(canon.to_string_lossy());
        }
        Ok(a)
    }

    /// `sql` の接続: db.path から別の接続を READ_ONLY で開き、query_only + authorizer（読み以外を拒否）を掛ける
    fn ro_conn(&mut self) -> Result<&Connection, String> {
        if self.ro.is_none() {
            let path: PathBuf = self.db.path.clone().ok_or("in-memory DB には sql を使えません")?;
            let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .map_err(|e| format!("読み取り専用で DB を開けません {}: {e}", path.display()))?;
            c.busy_timeout(Duration::from_secs(3)).map_err(|e| e.to_string())?;
            c.execute_batch("PRAGMA query_only = 1").map_err(|e| e.to_string())?;
            // 文字列・BLOB・行の上限（randomblob(9e8) などでメモリを食わない。本文の最大セルは 63 万字 = 数 MB）
            c.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, SQL_MAX_VALUE_BYTES);
            c.authorizer(Some(authorize));
            self.ro = Some(c);
        }
        Ok(self.ro.as_ref().unwrap())
    }

    fn run_sql(&mut self, a: &Value) -> Result<Value, String> {
        let sql = a.get("sql").and_then(|v| v.as_str()).ok_or("sql: 引数 sql（文字列）は必須です")?.to_string();
        let params: Vec<rusqlite::types::Value> = match a.get("params") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(v)) => v.iter().map(to_sql_value).collect(),
            Some(_) => return Err("sql: params は位置パラメータの配列（?1 ?2 … に入る）".into()),
        };
        let max_rows = match a.get("max_rows") {
            None | Some(Value::Null) => SQL_ROWS_DEFAULT,
            Some(v) => v.as_u64().ok_or("sql: max_rows は正の整数")?.clamp(1, SQL_ROWS_MAX),
        } as usize;
        let budget = self.sql_budget;
        let conn = self.ro_conn()?;
        let mut batch = rusqlite::Batch::new(conn, &sql);
        let mut stmt = batch.next().map_err(sql_err)?.ok_or("sql: SQL が空です")?;
        // 単文のみ（; の後の 2 文目は準備だけして拒否。末尾の空白・コメントは文にならないので通る）
        match batch.next() {
            Ok(None) => {}
            Ok(Some(_)) => return Err("sql: 単文のみです（; の後に 2 文目があります。1 回の sql に 1 文）".into()),
            Err(e) => return Err(format!("sql: 単文のみです（2 文目があります: {}）", sql_err(e))),
        }
        if !stmt.readonly() {
            return Err("sql: 読み取り専用です（書込は call で。一覧は schema）".into());
        }
        let deadline = Instant::now() + budget;
        conn.progress_handler(1000, Some(move || Instant::now() > deadline));
        let out = collect_rows(&mut stmt, &params, max_rows);
        conn.progress_handler(0, None::<fn() -> bool>);
        out.map_err(|e| match &e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => {
                format!("sql: 時間切れ（{} 秒）で中断しました。LIMIT や索引の効く条件で絞ってください", budget.as_secs_f32())
            }
            // 実行中の not authorized = 準備は通った文の中で拒否したもの（load_extension・表の情報以外の pragma_xxx('…') 等）
            _ if e.to_string().contains("not authorized") => format!(
                "sql: この sql では許していない操作です（拡張の読込・表の情報以外の引数付き PRAGMA など）: {e}"
            ),
            _ if e.to_string().contains("too big") => {
                format!("sql: 値が大きすぎます（1 つの文字列・BLOB は {} MB まで）: {e}", SQL_MAX_VALUE_BYTES / 1024 / 1024)
            }
            _ => sql_err(e),
        })
    }
}

/// `sql` の接続の authorizer: 読み（SELECT・列の読み・関数・再帰 CTE・値なしの PRAGMA と表の情報の PRAGMA）だけ許す。
/// 書込・DDL・ATTACH / DETACH・トランザクション・値付きの PRAGMA（query_only=0 / writable_schema=1 等）は準備の段階で拒否
fn authorize(ctx: AuthContext<'_>) -> Authorization {
    const READ_PRAGMAS_WITH_ARG: [&str; 9] = [
        "table_info", "table_xinfo", "table_list", "index_list", "index_info", "index_xinfo",
        "foreign_key_list", "integrity_check", "quick_check",
    ];
    match ctx.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Function { .. } | AuthAction::Recursive => {
            Authorization::Allow
        }
        AuthAction::Pragma { pragma_name, pragma_value } => {
            if pragma_value.is_none() || READ_PRAGMAS_WITH_ARG.contains(&pragma_name.to_ascii_lowercase().as_str()) {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        _ => Authorization::Deny,
    }
}

fn sql_err(e: rusqlite::Error) -> String {
    let s = e.to_string();
    if s.contains("not authorized") {
        "sql: 読み取り専用の sql では許していない文です（書込・DDL・ATTACH・トランザクション・値付きの PRAGMA）。書込は call で".into()
    } else if s.contains("readonly") || s.contains("read-only") {
        format!("sql: 読み取り専用です（書込は call で）: {s}")
    } else {
        format!("sql: {s}")
    }
}

fn to_sql_value(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(V::Integer).unwrap_or_else(|| V::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => V::Text(s.clone()),
        other => V::Text(other.to_string()),
    }
}

/// 行を JSON に。BLOB は中身を返さず長さだけ。行数（max_rows）と文字数（SQL_CHARS_MAX）で打ち切り、打ち切ったら truncated
fn collect_rows(
    stmt: &mut rusqlite::Statement<'_>,
    params: &[rusqlite::types::Value],
    max_rows: usize,
) -> rusqlite::Result<Value> {
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let ncol = columns.len();
    let mut rows_out: Vec<Value> = Vec::new();
    let mut used = 0usize;
    let mut truncated_by: Option<&str> = None;
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
    while let Some(row) = rows.next()? {
        if rows_out.len() >= max_rows {
            truncated_by = Some("max_rows");
            break;
        }
        let mut cells = Vec::with_capacity(ncol);
        for i in 0..ncol {
            let cell = match row.get_ref(i)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
                ValueRef::Text(t) => {
                    let s = String::from_utf8_lossy(t);
                    let n = s.chars().count();
                    let room = SQL_CHARS_MAX.saturating_sub(used);
                    if n > room {
                        truncated_by = Some("chars");
                        used = SQL_CHARS_MAX;
                        Value::String(format!("{}…（{} 字のうち {} 字で打ち切り）", s.chars().take(room).collect::<String>(), n, room))
                    } else {
                        used += n;
                        Value::String(s.into_owned())
                    }
                }
                ValueRef::Blob(b) => json!({"blob_bytes": b.len()}),
            };
            used += 8; // 数値・null・区切りの分（文字列だけの勘定にしない）
            cells.push(cell);
        }
        rows_out.push(Value::Array(cells));
        if truncated_by.is_some() || used >= SQL_CHARS_MAX {
            truncated_by.get_or_insert("chars");
            break;
        }
    }
    let mut out = json!({
        "columns": columns,
        "row_count": rows_out.len(),
        "truncated": truncated_by.is_some(),
        "rows": rows_out,
    });
    if let Some(by) = truncated_by {
        out["truncated_by"] = json!(by);
    }
    Ok(out)
}

/// コマンドの結果 → MCP の content: 予約キー `_mcp_images` を抜いて image に・残りの JSON を text に・最後に安全網の知らせ
fn to_content(mut v: Value, note: Option<String>) -> Vec<Value> {
    let images = v.as_object_mut().and_then(|o| o.remove(wiki::commands::MCP_IMAGES));
    let mut out = vec![text_content(serde_json::to_string_pretty(&v).unwrap())];
    for img in images.as_ref().and_then(Value::as_array).into_iter().flatten() {
        out.push(json!({"type": "image", "data": img["data"], "mimeType": img["mimeType"]}));
    }
    out.extend(note.map(text_content));
    out
}

fn text_content(t: String) -> Value {
    json!({"type": "text", "text": t})
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools_json() -> Value {
    json!([
        {
            "name": "schema",
            "description": "まずこれを読む。OpenRef（放射線科医の文献管理）の DB スキーマ（DDL）・注記（CSL-JSON・和文検索・添付ファイルのパス・id の扱い）・call のコマンド一覧・環境（DB の場所・スナップショット）をテキストで返す。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "sql",
            "description": "DB を読む SQL（読み取り専用・単文。SELECT / WITH / 読みの PRAGMA）。一覧・検索・件数・本文の grep もこれで。行数は既定 200・最大 2000、応答は 10 万字・10 秒で打ち切る（truncated）。書込は call で。",
            "inputSchema": { "type": "object", "properties": {
                "sql": { "type": "string", "description": "1 文の SQL" },
                "params": { "type": "array", "description": "位置パラメータ（?1 ?2 …）" },
                "max_rows": { "type": "integer", "description": "既定 200・最大 2000" }
            }, "required": ["sql"] }
        },
        {
            "name": "call",
            "description": "コマンドを実行する（書込はすべてここ・GUI と同じ動詞）。name と args は schema の一覧のとおり。item は key（8 桁）・フォルダは key かパス（'親/子'）・ラベルは名前で指す（数値 id は受けない）。注釈・ノートの author は 'claude:<model>' 推奨。このプロセスで最初の（DB を変える）書込の前に DB のスナップショットを backup/ に作る。",
            "inputSchema": { "type": "object", "properties": {
                "name": { "type": "string", "description": "コマンド名（例 list_items / add_tag）" },
                "args": { "type": "object", "description": "コマンドの引数" }
            }, "required": ["name"] }
        }
    ])
}

// ---------------- schema ----------------

/// `schema` の注記。手引き文書の「スキーマの注記」節と同じ文（drift test が固定）
const SCHEMA_NOTES: &str = "\
- 書誌の正は `items.csl`（CSL-JSON の文字列）。`title` `year` `doi`（小文字）`pmid` `pmcid` `container_title` `item_type` は csl から作る生成列（読むだけ・WHERE に使える）。他のフィールドは `json_extract(csl, '$.volume')` のように読む。著者は csl の `author`（配列）か `creators_index`（item_id・position・family・given。items と JOIN して使う）。
- ゴミ箱 = `items.deleted_at IS NOT NULL`。一覧や件数はふつう `deleted_at IS NULL` で絞る。`collections.deleted_at` は使っていない（フォルダの削除は行ごと即時）。
- フォルダ = `collections`（`parent_id` で木・`key` は不変の 8 桁）と `collection_items`。ラベル = `tags`（name は大小無視で一意）と `item_tags`。**id は削除後に再利用される**ので、call ではフォルダを key かパス（'親/子'）、ラベルを名前で指す（数値 id は受けない）。
- `tags.sort_order` は使っていない（migration v3 の名残・値は作成経路で違い意味が無い = ORDER BY に使わない）。ラベルの並びは `name COLLATE NOCASE`（app・call と同じ）。
- 添付 = `attachments`（1 item に 0..n・kind = 'pdf'）。`path` は絶対パスで、ホストのツールでそのまま読める。mode = 'managed' は `<app_dir>/storage/<添付の key>/<ファイル名>`、mode = 'linked' は Google Drive の Paperpile フォルダなど外部のファイル（Drive のオンライン専用ファイルは読むときに取得を待つことがある）。`doi_check` = 自動取得した PDF の同一性照合（'match' / 'unknown' / NULL）。
- 本文 = `fulltext`（attachment_id・`text`。頁の区切りは \\f・status = ok / partial（内蔵抽出が文字化けしたまま入れた・検索には一部使える。pdftotext を入れて `openref fulltext index` で取り直す）/ empty（スキャン PDF）/ error）。本文の grep は sql で `fulltext.text` を `instr` / `LIKE` で読める。全文検索の索引は `items_fts`（書誌）と `fulltext_fts`（本文）。和文は 1 字ずつの phrase（`胆道` → `\"胆 道\"` を cjk 列に）なので、MATCH を手で書くより `call(list_items, {query})` が確実。
- 注釈 = `annotations`（attachment_id 経由で item へ。quote・prefix・suffix・page が位置の手がかり・rects と sort_key は未使用 = 常に NULL）。ノート = `notes`（item 単位の markdown）。author は user が書いたものが 'user'、エージェントは 'claude:<model>'（例 'claude:opus-5.5'）を付ける。
- 「前回以降」は `items.date_added` / `date_modified`・`notes.date_modified`・`annotations.date_added` で引ける（UTC の 'YYYY-MM-DD HH:MM:SS'）。
- `retraction_flags` = 撤回の印。`journal_keys` = 誌名の同一性のキー。`settings` の主なキー: `pdf_write_mode`（drive = 添付を Drive の Paperpile フォルダへ書く / managed = storage/）・`cite_style` `cite_locale` `cite_doi_always`（引用の既定）・`ui_*`（app の表示状態）・`update_check`（\"0\" = 起動時の新版確認を止める）`update_checked_at`（最後に確認した unix 秒）`update_latest`（取れた最新の版番号）`update_error`（直近の確認の失敗理由 `<unix 秒>: <理由>`・取れたら消える）。
- `import_sources` / `import_runs` / `import_objects` / `import_results` / `import_files` はライブラリ移行の由来・履歴・対応表・結果・コピー台帳。元レコードは引用用 CSL や全文索引とは別のローカル資料。削除済み対象の対応も残すので、履歴を削除して再作成・復元する操作はしない。
- `PRAGMA data_version` は他の接続の書込で増える（app はこれを 2 秒ごとに見て一覧を読み直す = call の書込はすぐ画面に出る）。
- sql の PRAGMA は値なしの読み（`PRAGMA user_version` 等）と表の情報（table_info・table_xinfo・table_list・index_list・index_info・index_xinfo・foreign_key_list・integrity_check・quick_check）だけ。表の情報は `SELECT * FROM pragma_table_info('items')` の形でも読める（JOIN・WHERE に使える）。
- OpenNote（OpenRef のページ機能・任意・既定は無効。コマンドは `wiki_*`）: ワークスペース = user が選んだ 1 フォルダの md / html / 画像等。正はディスクのファイルで、索引 `<app_dir>/wiki-index/<hash>.sqlite`（ワークスペースごと）は再生成できるキャッシュ（sql からは見えない）。読むのは call の `wiki_status` / `wiki_list` / `wiki_read`（画像は画像そのもの・PDF は抽出テキスト）/ `wiki_backlinks` / `wiki_search`（題名 + 本文の全文検索・和文可・tag の AND 絞込）/ `wiki_tags` / `wiki_inbox`（`ingest/` に置かれた素材の一覧）、書くのは `wiki_write`（md / html）/ `wiki_move`（パスはワークスペース相対）。`wiki_write` で既存を上書きすると上書き前を `<app_dir>/wiki-history/<16 桁>/<unix 秒>.<拡張子>` に退避して応答の `backup` に返す（ファイルごとに新しい 10 世代・同じフォルダの `path.txt` が元のファイルの絶対パス）= 戻すときはその中身を `wiki_write` で書き戻す。有効化は GUI だけ（Settings › OpenNote・フォルダは左の木のワークスペース名のメニュー）。最近のワークスペースは `wiki_status` の `workspaces`（settings の `wiki_workspaces`・切替は GUI だけ）。settings の `wiki_enabled`（\"1\" = 有効）`wiki_root`（ワークスペースの絶対パス）。ページから item へのリンクは `[題名](openref://open?key=KEY)`（KEY は実在の key だけ・`wiki_backlinks` の key で逆に引ける）。
- OpenNote のワークスペースの固定: `wiki_*` は任意の引数 `root`（`wiki_status` の `workspaces[].root` の文字列をそのまま = 履歴にあるものだけ・他はエラー）で読み書きするワークスペースを固定できる。省略 = settings の `wiki_root` = 利用者が OpenRef の前面のタブで見ているワークスペース（タブを行き来すると変わる。OpenRef のチャットの中では省略 = チャットを始めたワークスペース = env `OPENREF_WIKI_ROOT`）= 複数のワークスペースを使うとき・書いている間に既定が変わると困るときは `root` を渡す。`wiki_status {root}` の `root`・件数は固定した root の物で、`workspaces[].current` は settings の `wiki_root`（前面）のまま。
- OpenNote の用例（雛形。何を書くかはその場の指示で決める）: (1) ingest/ の素材を記事に = `wiki_inbox` → `wiki_read`（素材）→ 関係する論文を `list_items` で探す → `wiki_write`（記事。frontmatter の `sources` に素材のパス・`items` に key・`tags`）→ `wiki_move`（素材を ingest/ の外の置き場へ）。(2) 選んだ論文の要約ページを作る = `get_item` と `get_fulltext` で読む → `wiki_write`（`[題名](openref://open?key=KEY)` 付き）→ `wiki_backlinks {key}` で載ったかを確かめる。書誌は get_item の値だけを使う（作らない）。
";

/// スナップショットからの戻し方（`schema` の §4 の末尾・手引き文書と同文 = drift test）
const RESTORE_TEXT: &str = "\
- 戻し方（スナップショット → DB）: **cp / mv で openref.sqlite を置き換えない**（`-wal` が残っているうちに本体ファイルを置き換えると壊れる。connectd・app・他のセッションの MCP が DB を開いている間は `-wal` が残るのが常態）。
  1. 書き手を止める: OpenRef.app を終了する・connectd は `launchctl bootout gui/$(id -u)/dev.openref.connectd`（再開は `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.openref.connectd.plist`）・openref を使っている他の Claude セッションは書込をしない状態にする
  2. `sqlite3 \"<DB>\" \".restore '<backup>/pre-agent-<日時>.sqlite'\"`（SQLite の backup API = `-wal` があっても・DB を開いたままの読み手がいても整合する。OpenRef のチャットのセッション（OPENREF_MCP_CHAT=1）と許可一覧で起動したセッションの写しは `pre-chat-<日時>.sqlite`）
  3. `sqlite3 \"<DB>\" \"pragma integrity_check\"` が ok を返すことを確かめてから書き手を戻す
";

/// FTS5 の内部表（shadow table）の接尾辞。DDL の一覧では 1 行にまとめる
const FTS_SHADOW: [&str; 5] = ["_data", "_idx", "_content", "_docsize", "_config"];

fn schema_text(db: &Db, allow: &Allow) -> String {
    let mut out = String::from("# OpenRef スキーマ\n\n読むのは `sql`（読み取り専用・単文）、書くのは `call`（下の §3 のコマンド）。\n\n## 1. テーブル（sqlite_master から生成）\n\n```sql\n");
    let rows: Vec<(String, String, String)> = db
        .conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL
             ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 1 WHEN 'index' THEN 2 ELSE 3 END, tbl_name, name",
        )
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
        .unwrap_or_default();
    let fts: Vec<&str> = rows
        .iter()
        .filter(|(t, _, sql)| t == "table" && sql.to_ascii_uppercase().contains("USING FTS5"))
        .map(|(_, n, _)| n.as_str())
        .collect();
    let is_shadow = |n: &str| fts.iter().any(|f| FTS_SHADOW.iter().any(|s| n == format!("{f}{s}")));
    let mut shadows = Vec::new();
    for (_, name, sql) in &rows {
        if is_shadow(name) {
            shadows.push(name.clone());
        } else {
            out.push_str(sql.trim());
            out.push_str(";\n");
        }
    }
    out.push_str("```\n");
    if !shadows.is_empty() {
        out.push_str(&format!("\nFTS5 の内部表（直接は読まない）: {}\n", shadows.join(", ")));
    }
    out.push_str("\n## 2. 注記\n\n");
    out.push_str(SCHEMA_NOTES);
    out.push_str("\n## 3. call のコマンド一覧\n\n");
    out.push_str("`call(name, args)`。item は key・フォルダは key かパス・ラベルは名前。DB を変える書込のコマンドはこのプロセスで最初の 1 回の前に DB のスナップショットを取る（`wiki_*` の書込は DB を変えない = 上書き前のファイルを wiki-history/ に退避）。\n\n");
    if allow.restricted() {
        out.push_str("**このセッションで使えるのは下の一覧のコマンドだけ**（許可一覧 = 起動時の OPENREF_MCP_ALLOW）。ホストのファイルのパスを受けるコマンドはワークスペースの中のファイルだけ受ける。\n\n");
    }
    out.push_str(&commands::markdown_where(|c| allow.allows(c)));
    out.push_str("\n## 4. 環境\n\n");
    // どれも整数を返す問い合わせ（user_version・件数）
    let q = |sql: &str| -> String { db.conn.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n.to_string()).unwrap_or_else(|e| format!("?（{e}）")) };
    let path = db.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(in-memory)".into());
    let app_dir = db.app_dir().map(|p| p.display().to_string()).unwrap_or_else(|| "(なし)".into());
    let bdir = backup::backup_dir(db).map(|p| p.display().to_string()).unwrap_or_else(|| "(なし)".into());
    out.push_str(&format!("- DB: `{path}`（migration = `PRAGMA user_version` {}）\n", q("PRAGMA user_version")));
    out.push_str(&format!("- app_dir: `{app_dir}`（storage/ = managed の添付・styles/ = 引用スタイル・backup/ = スナップショット）\n"));
    out.push_str(&format!("- pdf_write_mode: {}\n", db.pdf_write_mode().unwrap_or_else(|e| format!("?（{e}）"))));
    out.push_str(&format!(
        "- 件数: items {}（うちゴミ箱 {}）・添付 {}・注釈 {}・ノート {}・フォルダ {}・ラベル {}\n",
        q("SELECT count(*) FROM items WHERE deleted_at IS NULL"),
        q("SELECT count(*) FROM items WHERE deleted_at IS NOT NULL"),
        q("SELECT count(*) FROM attachments"),
        q("SELECT count(*) FROM annotations"),
        q("SELECT count(*) FROM notes"),
        q("SELECT count(*) FROM collections"),
        q("SELECT count(*) FROM tags"),
    ));
    out.push_str(&format!(
        "- スナップショット: このプロセスで最初の DB を変える書込コマンドの前に `{bdir}/{}<日時>.sqlite` を 1 個作る（新しい {} 個を残す・DB だけ = purge_item で消えた managed の PDF は戻らない）\n",
        allow.snapshot_prefix(),
        backup::AGENT_KEEP
    ));
    out.push_str(RESTORE_TEXT);
    out
}

// ---------------- tests ----------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 一時ファイル DB（sql の別接続・スナップショットは in-memory では試せない）
    fn file_server(tag: &str) -> (Server, PathBuf) {
        let dir = std::env::temp_dir().join(format!("openref-mcp-{tag}-{}-{}", std::process::id(), openref_core::keys::new_key()));
        let mut db = Db::open(&dir.join("openref.sqlite")).unwrap();
        seed(&mut db);
        (Server::new(db), dir)
    }

    fn seed(db: &mut Db) -> String {
        let r = openref_core::resolver::Resolved {
            csl: json!({
                "type": "article-journal", "title": "MCP Seed Article on pancreatic imaging",
                "author": [{"family": "Kano", "given": "S"}], "container-title": "Test Journal",
                "issued": {"date-parts": [[2026]]}, "DOI": "10.9999/mcp.seed", "PMID": "88888888"
            }),
            retracted: None,
            source: "test",
            warnings: vec![],
        };
        openref_core::ingest::add_item(db, &r).unwrap().key
    }

    fn tool(srv: &mut Server, name: &str, args: Value) -> Value {
        srv.handle_message(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": name, "arguments": args}}))
            .unwrap()["result"]
            .clone()
    }

    fn text(res: &Value) -> String {
        res["content"][0]["text"].as_str().unwrap().to_string()
    }

    fn sql(srv: &mut Server, q: &str) -> Result<Value, String> {
        let r = tool(srv, "sql", json!({"sql": q}));
        if r["isError"] == true {
            Err(text(&r))
        } else {
            Ok(serde_json::from_str(&text(&r)).unwrap())
        }
    }

    #[test]
    fn protocol_initialize_and_list() {
        let mut srv = Server::new(Db::open_in_memory().unwrap());
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}});
        let resp = srv.handle_message(&init).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(resp["result"]["serverInfo"]["name"], "openref");
        assert!(srv.handle_message(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let list = srv.handle_message(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> =
            list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["schema", "sql", "call"]);
        // 旧 tool 名で直接呼ばれても call と同じに通る
        let r = tool(&mut srv, "list_collections", json!({}));
        assert_eq!(r["isError"], false, "{r}");
        let r = tool(&mut srv, "nope", json!({}));
        assert_eq!(r["isError"], true);
    }

    #[test]
    fn schema_has_ddl_notes_commands_env() {
        let (mut srv, dir) = file_server("schema");
        let s = text(&tool(&mut srv, "schema", json!({})));
        assert!(s.contains("CREATE TABLE items"), "DDL");
        assert!(s.contains("CREATE TRIGGER fulltext_ai"), "trigger");
        assert!(s.contains("FTS5 の内部表（直接は読まない）: ") && s.contains("items_fts_data"), "shadow を 1 行に");
        assert!(!s.contains("CREATE TABLE 'items_fts_data'") && !s.contains("CREATE TABLE \"items_fts_data\""));
        assert!(s.contains(SCHEMA_NOTES) && s.contains(RESTORE_TEXT));
        assert!(s.contains("`search_library`") && s.contains("`create_collection_in`"));
        assert!(s.contains("user_version` 10") && s.contains("pre-agent-"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sql は読み取り専用: 別接続 READ_ONLY + query_only + authorizer + 単文 + readonly 判定。
    /// どの経路の書込も拒否され、DB は変わらない
    #[test]
    fn sql_is_read_only() {
        let (mut srv, dir) = file_server("sqlro");
        let v = sql(&mut srv, "SELECT count(*) AS n FROM items WHERE deleted_at IS NULL").unwrap();
        assert_eq!(v["columns"], json!(["n"]));
        assert_eq!(v["rows"], json!([[1]]));
        assert_eq!(v["truncated"], false);
        // 末尾の ; と空白・コメントは単文のうち
        assert!(sql(&mut srv, "SELECT 1;  \n -- end\n").is_ok());
        // 読みの PRAGMA・表の情報・EXPLAIN は通る
        assert!(sql(&mut srv, "PRAGMA data_version").is_ok());
        assert!(sql(&mut srv, "PRAGMA table_info(items)").is_ok());
        assert!(sql(&mut srv, "SELECT name FROM pragma_table_info('items')").is_ok());
        assert!(sql(&mut srv, "EXPLAIN QUERY PLAN SELECT * FROM items WHERE doi = 'x'").is_ok());
        // 和文 FTS と本文の表も読める
        assert!(sql(&mut srv, "SELECT rowid FROM items_fts WHERE items_fts MATCH 'pancreatic'").is_ok());

        let attempts = [
            "INSERT INTO tags (name) VALUES ('x')",
            "UPDATE items SET starred = 1",
            "DELETE FROM items",
            "WITH t AS (SELECT 1) INSERT INTO tags (name) SELECT 'cte' FROM t",
            "CREATE TABLE t (x)",
            "CREATE TEMP TABLE t (x)",
            "DROP TABLE notes",
            "ALTER TABLE items ADD COLUMN x",
            "PRAGMA query_only = 0",
            "PRAGMA query_only(0)",
            "PRAGMA writable_schema = 1",
            "PRAGMA user_version = 99",
            "PRAGMA journal_mode = DELETE",
            "ATTACH DATABASE ':memory:' AS m",
            "ATTACH DATABASE 'x.sqlite' AS m",
            "VACUUM",
            "VACUUM INTO '/tmp/openref-sql-vacuum-test.sqlite'",
            "BEGIN",
            "SAVEPOINT s",
            "REINDEX",
            "ANALYZE",
            "SELECT 1; DELETE FROM items",
            "SELECT 1; SELECT 2",
        ];
        for q in attempts {
            assert!(sql(&mut srv, q).is_err(), "拒否されるはず: {q}");
        }
        // query_only=0 を試みた後も書けない（接続を使い回しても）
        assert!(sql(&mut srv, "INSERT INTO tags (name) VALUES ('after')").is_err());
        assert!(!std::path::Path::new("/tmp/openref-sql-vacuum-test.sqlite").exists());
        assert!(!dir.join("x.sqlite").exists());
        let q = |s: &Server, q: &str| -> i64 { s.db.conn.query_row(q, [], |r| r.get(0)).unwrap() };
        assert_eq!(q(&srv, "SELECT count(*) FROM items"), 1);
        assert_eq!(q(&srv, "SELECT count(*) FROM tags"), 0);
        assert_eq!(q(&srv, "PRAGMA user_version"), 10);
        assert_eq!(q(&srv, "SELECT starred FROM items"), 0);
        // 書込のコマンドでないので スナップショットも作らない
        assert!(!dir.join("backup").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sql_limits_params_blob_and_time() {
        let (mut srv, dir) = file_server("sqllim");
        // 位置パラメータ
        let r = tool(&mut srv, "sql", json!({"sql": "SELECT ?1 + ?2 AS s, ?3 AS t, ?4 AS n", "params": [2, 3, "和文", null]}));
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["rows"], json!([[5, "和文", null]]));
        // 行数の上限（既定 200・max_rows）
        let gen = "WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < 5000) SELECT i FROM c";
        let v = sql(&mut srv, gen).unwrap();
        assert_eq!(v["row_count"], 200);
        assert_eq!(v["truncated_by"], "max_rows");
        let r = tool(&mut srv, "sql", json!({"sql": gen, "max_rows": 99999}));
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["row_count"], 2000, "最大 2000");
        let r = tool(&mut srv, "sql", json!({"sql": "SELECT 1 UNION ALL SELECT 2", "max_rows": 2}));
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["truncated"], false, "ちょうどなら打ち切りではない");
        // 文字数の上限
        let v = sql(&mut srv, "SELECT printf('%.*c', 150000, 'x') AS big").unwrap();
        assert_eq!(v["truncated_by"], "chars");
        assert!(v["rows"][0][0].as_str().unwrap().chars().count() < 101_000);
        // BLOB は長さだけ
        let v = sql(&mut srv, "SELECT zeroblob(1234) AS b").unwrap();
        assert_eq!(v["rows"][0][0], json!({"blob_bytes": 1234}));
        // 時間の予算で中断
        srv.sql_budget = Duration::from_millis(200);
        let t0 = Instant::now();
        let e = sql(&mut srv, "WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c) SELECT count(*) FROM c").unwrap_err();
        assert!(e.contains("時間切れ"), "{e}");
        assert!(t0.elapsed() < Duration::from_secs(5));
        // 中断の後も接続は使える
        assert!(sql(&mut srv, "SELECT 1").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 最初の書込の前に 1 個だけスナップショット。読みのコマンドでは作らない。6 個目で最古が消える
    #[test]
    fn snapshot_before_first_write_only() {
        let (mut srv, dir) = file_server("snap");
        let bdir = dir.join("backup");
        let count = || -> usize {
            std::fs::read_dir(&bdir)
                .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("pre-agent-")).count())
                .unwrap_or(0)
        };
        let key: String = srv.db.conn.query_row("SELECT key FROM items", [], |r| r.get(0)).unwrap();
        let r = tool(&mut srv, "call", json!({"name": "list_items", "args": {"query": "pancreatic"}}));
        assert_eq!(r["isError"], false);
        assert_eq!(count(), 0, "読みでは作らない");

        // 最古を消すのを見るために古い写しを 5 個置いておく
        std::fs::create_dir_all(&bdir).unwrap();
        for i in 0..5 {
            std::fs::write(bdir.join(format!("pre-agent-2000010{i}-000000.sqlite")), b"old").unwrap();
        }
        let r = tool(&mut srv, "call", json!({"name": "add_tag", "args": {"key": key, "tag": "膵臓"}}));
        assert_eq!(r["isError"], false, "{r}");
        let note = r["content"][1]["text"].as_str().unwrap();
        assert!(note.contains("スナップショット") && note.contains("pre-agent-"), "{note}");
        assert_eq!(count(), 5);
        assert!(!bdir.join("pre-agent-20000100-000000.sqlite").exists(), "最古が消える");
        // 写しは書込の前の状態（ラベルが無い）
        let snap = std::path::PathBuf::from(note.rsplit(": ").next().unwrap());
        let c = Connection::open(&snap).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM tags", [], |r| r.get::<_, i64>(0)).unwrap(), 0);

        // 2 回目の書込（旧名でも）では作らない
        let r = tool(&mut srv, "call", json!({"name": "set_tags", "args": {"key": key, "add": ["b"]}}));
        assert_eq!(r["isError"], false);
        assert!(r["content"].as_array().unwrap().len() == 1);
        assert!(bdir.join("pre-agent-20000101-000000.sqlite").exists(), "2 回目は作らない（消しもしない）");
        // sql からも書込が見える（別接続）
        let v = sql(&mut srv, "SELECT count(*) FROM item_tags").unwrap();
        assert_eq!(v["rows"], json!([[2]]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OPENREF_MCP_CHAT=1（チャット）は写しの接頭辞だけ変える（コマンドは絞らない）。許可一覧も pre-chat-
    #[test]
    fn chat_flag_changes_prefix_only() {
        let mut a = Allow::parse(None);
        assert_eq!(a.snapshot_prefix(), backup::AGENT_PREFIX);
        a.chat = true;
        assert_eq!(a.snapshot_prefix(), backup::CHAT_PREFIX);
        assert!(!a.restricted() && commands::registry().iter().all(|c| a.allows(c)), "コマンドは全部");
        assert_eq!(Allow::parse(Some("read")).snapshot_prefix(), backup::CHAT_PREFIX);
        let (mut srv, dir) = file_server("chatflag");
        srv.allow.chat = true;
        let s = text(&tool(&mut srv, "schema", json!({})));
        assert!(s.contains("/pre-chat-<日時>.sqlite") && !s.contains("このセッションで使えるのは下の一覧のコマンドだけ"), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 許可一覧のセッション（チャット）の写しは pre-chat-。新しい 5 個は接頭辞ごと = pre-agent- を押し出さない
    #[test]
    fn chat_snapshot_prefix_keeps_agent_snapshots() {
        let (mut srv, dir) = file_server("chatsnap");
        let key: String = srv.db.conn.query_row("SELECT key FROM items", [], |r| r.get(0)).unwrap();
        let bdir = dir.join("backup");
        std::fs::create_dir_all(&bdir).unwrap();
        for i in 0..5 {
            std::fs::write(bdir.join(format!("pre-agent-2000010{i}-000000.sqlite")), b"old").unwrap();
            std::fs::write(bdir.join(format!("pre-chat-2000010{i}-000000.sqlite")), b"old").unwrap();
        }
        srv.allow = Allow::parse(Some("read,wiki,quick_add,import_pdf,set_star"));
        assert!(text(&tool(&mut srv, "schema", json!({}))).contains("/pre-chat-<日時>.sqlite"));
        let r = tool(&mut srv, "call", json!({"name": "set_star", "args": {"key": key, "starred": true}}));
        assert_eq!(r["isError"], false, "{r}");
        assert!(text(&json!({"content": [r["content"][1]]})).contains("pre-chat-"), "{r}");
        let names = |p: &str| -> usize {
            std::fs::read_dir(&bdir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with(p)).count()
        };
        assert_eq!(names("pre-chat-"), 5, "チャットの写しは 5 個");
        assert!(!bdir.join("pre-chat-20000100-000000.sqlite").exists(), "チャットの最古が消える");
        assert_eq!(names("pre-agent-"), 5, "pre-agent- は押し出さない");
        assert!(bdir.join("pre-agent-20000100-000000.sqlite").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// スナップショットを取れなければ書込を実行しない（fail-closed）
    #[test]
    fn snapshot_failure_blocks_write() {
        let (mut srv, dir) = file_server("snapfail");
        let key: String = srv.db.conn.query_row("SELECT key FROM items", [], |r| r.get(0)).unwrap();
        std::fs::write(dir.join("backup"), b"not a dir").unwrap(); // backup/ を作れない
        let r = tool(&mut srv, "call", json!({"name": "set_star", "args": {"key": key, "starred": true}}));
        assert_eq!(r["isError"], true);
        assert!(text(&r).contains("実行していません"), "{}", text(&r));
        let starred: i64 = srv.db.conn.query_row("SELECT starred FROM items", [], |r| r.get(0)).unwrap();
        assert_eq!(starred, 0);
        // 読みは通る
        let r = tool(&mut srv, "call", json!({"name": "get_item", "args": {"key": key}}));
        assert_eq!(r["isError"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sql を使ったプロセスでも、終了（Server の drop）で書込接続が最後に閉じて `-wal` を本体へ畳む
    /// （ro を後に残すと READ_ONLY の ro が最後になり checkpoint できず `-wal` が残る = 写しを cp で戻すと壊れた）
    #[test]
    fn no_wal_left_after_sql_and_write() {
        let (mut srv, dir) = file_server("wal");
        let key: String = srv.db.conn.query_row("SELECT key FROM items", [], |r| r.get(0)).unwrap();
        assert!(sql(&mut srv, "SELECT count(*) FROM items").is_ok());
        let r = tool(&mut srv, "call", json!({"name": "add_tag", "args": {"key": key, "tag": "wal"}}));
        assert_eq!(r["isError"], false, "{r}");
        let wal = dir.join("openref.sqlite-wal");
        assert!(wal.metadata().map(|m| m.len()).unwrap_or(0) > 0, "書込の後は -wal に未反映の分がある");
        drop(srv);
        assert_eq!(wal.metadata().map(|m| m.len()).unwrap_or(0), 0, "-wal が残った");
        // 本体だけで読める（-wal なしで書込が見える）
        let c = Connection::open(dir.join("openref.sqlite")).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM tags WHERE name = 'wal'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 結果の _mcp_images は image content になり、text には base64 が残らない（安全網の知らせは最後）
    #[test]
    fn images_become_image_content() {
        let v = json!({"path": "ingest/a.png", "size": 3, "_mcp_images": [{"mimeType": "image/png", "data": "QUJD"}]});
        let c = to_content(v, Some("note".into()));
        assert_eq!(c.len(), 3);
        assert_eq!(c[1], json!({"type": "image", "data": "QUJD", "mimeType": "image/png"}));
        let t = c[0]["text"].as_str().unwrap();
        assert!(!t.contains("QUJD") && !t.contains("_mcp_images") && t.contains("ingest/a.png"), "{t}");
        assert_eq!(c[2]["text"], "note");
        assert_eq!(to_content(json!([1]), None).len(), 1);
    }

    /// 許可一覧: 分類 read / wiki と名前（別名可）・未知は警告・空なら全部
    #[test]
    fn allow_list_parse() {
        assert!(!Allow::parse(None).restricted() && Allow::parse(None).warnings.is_empty());
        // 設定されているのに実質空 = 全部許すが警告
        let blank = Allow::parse(Some(" , "));
        assert!(!blank.restricted() && blank.warnings.len() == 1, "{:?}", blank.warnings);
        assert!(Allow::parse(Some("")).warnings.is_empty());
        let a = Allow::parse(Some("read, wiki,create_item,import_pdf,nope,writes"));
        let n = a.names.as_ref().unwrap();
        for ok in ["list_items", "get_fulltext", "wiki_read", "wiki_write", "wiki_move", "quick_add", "import_pdf"] {
            assert!(n.contains(ok), "{ok}");
        }
        for no in ["purge_item", "add_tag", "update_item", "fetch_pdf", "cite_set_prefs", "attach_pdf", "delete_collection"] {
            assert!(!n.contains(no), "{no}");
        }
        assert_eq!(a.warnings.len(), 2, "{:?}", a.warnings);
        assert!(a.warnings[0].contains("nope") && a.warnings[1].contains("writes"));
    }

    /// 許可一覧で起動: schema に許可外が出ない・call も旧 tool 名の直呼びも拒否・sql と読みは通る・
    /// import_pdf は vault の外を拒否（Wiki 無効も拒否）・env 無しは今どおり
    #[test]
    fn allow_list_enforced() {
        let (mut srv, dir) = file_server("allow");
        let key: String = srv.db.conn.query_row("SELECT key FROM items", [], |r| r.get(0)).unwrap();
        srv.allow = Allow::parse(Some("read,wiki,quick_add,import_pdf"));
        let s = text(&tool(&mut srv, "schema", json!({})));
        assert!(s.contains("`list_items`") && s.contains("`wiki_write`") && s.contains("`import_pdf`") && s.contains("このセッションで使えるのは"));
        assert!(!s.contains("`purge_item`") && !s.contains("`add_tag`") && !s.contains("`fetch_pdf`"));
        for r in [
            tool(&mut srv, "call", json!({"name": "purge_item", "args": {"key": key}})),
            tool(&mut srv, "purge_item", json!({"key": key})),
            tool(&mut srv, "set_tags", json!({"key": key, "add": ["x"]})),
            tool(&mut srv, "call", json!({"name": "trash_item", "args": {"key": key}})),
        ] {
            assert_eq!(r["isError"], true);
            assert!(text(&r).contains("このセッションでは許可されていません"), "{}", text(&r));
        }
        assert_eq!(tool(&mut srv, "call", json!({"name": "get_item", "args": {"key": key}}))["isError"], false);
        assert_eq!(tool(&mut srv, "get_item", json!({"key": key}))["isError"], false);
        assert!(sql(&mut srv, "SELECT count(*) FROM items").is_ok());
        let pdf = dir.join("x.pdf");
        std::fs::write(&pdf, openref_core::pdftools::minimal_pdf("x", None)).unwrap();
        let r = tool(&mut srv, "call", json!({"name": "import_pdf", "args": {"path": pdf.to_string_lossy()}}));
        assert_eq!(r["isError"], true);
        assert!(text(&r).contains("ワークスペースの中のファイル") && text(&r).contains("無効") && !text(&r).contains("入力を解釈できません"), "{}", text(&r));
        // wiki の書込は DB を変えない = 失敗しても成功しても DB のスナップショットを作らない。拒否も作らない
        assert!(!dir.join("backup").exists());
        let n: i64 = srv.db.conn.query_row("SELECT count(*) FROM items WHERE deleted_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // env 無し = 全部（今どおり）
        srv.allow = Allow::parse(None);
        let s = text(&tool(&mut srv, "schema", json!({})));
        assert!(s.contains("`purge_item`") && !s.contains("このセッションで使えるのは"));
        assert_eq!(tool(&mut srv, "call", json!({"name": "trash_item", "args": {"key": key}}))["isError"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// wiki_* の書込は DB のスナップショットを作らない（vault のファイルだけ・上書き前は wiki-history）
    #[test]
    fn wiki_writes_take_no_db_snapshot() {
        let (mut srv, dir) = file_server("wikisnap");
        let r = tool(&mut srv, "call", json!({"name": "wiki_write", "args": {"path": "a.md", "content": "x"}}));
        assert_eq!(r["isError"], true, "Wiki は無効");
        let r = tool(&mut srv, "wiki_move", json!({"from": "a.md", "to": "b.md"}));
        assert_eq!(r["isError"], true);
        assert!(!dir.join("backup").exists(), "DB の写しを作らない");
        assert!(!srv.snapshotted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// env `OPENREF_WIKI_ROOT`（チャットの子 = 始めた root に縛る）: root を省いた wiki_* はその root に固定・root を渡せばそれ・
    /// 履歴に無い root は拒否・許可一覧の path の検査もその root・wiki 以外の args には触らない・env 無しは設定 wiki_root（今どおり）
    #[test]
    fn wiki_root_env_pins_wiki_args() {
        let (mut srv, dir) = file_server("wikiroot");
        let (a, b) = (dir.join("vault-a"), dir.join("vault-b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (a, b) = (a.to_string_lossy().to_string(), b.to_string_lossy().to_string());
        srv.db.set_setting("wiki_enabled", "1").unwrap();
        srv.db.set_setting("wiki_root", &a).unwrap();
        srv.db
            .set_setting("wiki_workspaces", &json!({a.clone(): {"last_used": 2}, b.clone(): {"last_used": 1}}).to_string())
            .unwrap();
        let status = |srv: &mut Server, args: Value| -> Value {
            let r = tool(srv, "call", json!({"name": "wiki_status", "args": args}));
            assert_eq!(r["isError"], false, "{}", text(&r));
            serde_json::from_str(&text(&r)).unwrap()
        };
        let current = |st: &Value| -> Vec<String> {
            st["workspaces"].as_array().unwrap().iter().filter(|w| w["current"] == true).map(|w| w["root"].as_str().unwrap().to_string()).collect()
        };
        // env 無し = 設定 wiki_root（tempdir はホームの外 = 検査に落ちて error の枝・root と workspaces は返る）
        let st = status(&mut srv, json!({}));
        assert_eq!((st["root"].as_str(), current(&st)), (Some(a.as_str()), vec![a.clone()]));
        // env = B: 省けば B・渡せばそれ・current は設定（前面）の A のまま
        srv.wiki_root = Some(b.clone());
        let st = status(&mut srv, json!({}));
        assert_eq!((st["root"].as_str(), current(&st)), (Some(b.as_str()), vec![a.clone()]));
        assert_eq!(status(&mut srv, Value::Null)["root"], json!(b), "args 無し（null）も固定");
        assert_eq!(status(&mut srv, json!({"root": a}))["root"], json!(a), "root を渡せばそれ");
        assert_eq!(status(&mut srv, json!({"root": null}))["root"], json!(b), "null = 省いた");
        // 旧 tool 名の直呼びも同じ（run_command を通る）・読みの先も固定した root（検査に落ちる文に B が出る）
        let r = tool(&mut srv, "wiki_list", json!({}));
        assert!(text(&r).contains(&b) && !text(&r).contains(&a), "{}", text(&r));
        // wiki 以外は触らない・形の違う args も触らない（登録表の検査に任せる）
        let find = |n: &str| commands::find(n).unwrap();
        assert!(srv.pin_wiki_args(find("get_item"), &json!({})).is_none());
        assert!(srv.pin_wiki_args(find("wiki_list"), &json!(["x"])).is_none());
        assert!(srv.pin_wiki_args(find("wiki_list"), &json!({"root": a})).is_none());
        assert_eq!(srv.pin_wiki_args(find("wiki_write"), &json!({"path": "p.md"})), Some(json!({"path": "p.md", "root": b})));
        // 許可一覧の path の検査も B（検査に落ちる文に B が出る）
        srv.allow = Allow::parse(Some("read,wiki,import_pdf"));
        let pdf = dir.join("x.pdf");
        std::fs::write(&pdf, openref_core::pdftools::minimal_pdf("x", None)).unwrap();
        let r = tool(&mut srv, "call", json!({"name": "import_pdf", "args": {"path": pdf.to_string_lossy()}}));
        assert_eq!(r["isError"], true);
        assert!(text(&r).contains(&b) && !text(&r).contains(&format!("{a}（")), "{}", text(&r));
        // 履歴に無い root = wiki_* も path の検査も拒否
        srv.wiki_root = Some(dir.join("vault-c").to_string_lossy().to_string());
        for r in [
            tool(&mut srv, "call", json!({"name": "wiki_status", "args": {}})),
            tool(&mut srv, "call", json!({"name": "wiki_write", "args": {"path": "p.md", "content": "x"}})),
            tool(&mut srv, "call", json!({"name": "import_pdf", "args": {"path": pdf.to_string_lossy()}})),
        ] {
            assert_eq!(r["isError"], true);
            assert!(text(&r).contains("履歴に無いワークスペースです"), "{}", text(&r));
        }
        assert!(!dir.join("vault-c").exists() && !dir.join("vault-a/p.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// tool のトップレベル引数の綴り違い・値の大きさの上限・実行時の拒否の文言
    #[test]
    fn tool_args_and_value_limit() {
        let (mut srv, dir) = file_server("targs");
        let r = tool(&mut srv, "sql", json!({"sql": "SELECT 1", "max_row": 5}));
        assert_eq!(r["isError"], true);
        assert!(text(&r).contains("未知の引数 max_row") && text(&r).contains("max_rows"), "{}", text(&r));
        let r = tool(&mut srv, "call", json!({"name": "view_counts", "arg": {}}));
        assert_eq!(r["isError"], true);
        assert!(text(&r).contains("未知の引数 arg"), "{}", text(&r));
        let r = tool(&mut srv, "sql", json!(["SELECT 1"]));
        assert_eq!(r["isError"], true);
        // 正しい綴りは通る
        assert_eq!(tool(&mut srv, "call", json!({"name": "view_counts", "args": {}}))["isError"], false);
        let r = tool(&mut srv, "sql", json!({"sql": "SELECT ?1", "params": [1], "max_rows": 1}));
        assert_eq!(r["isError"], false, "{r}");
        // 1 つの値は 16 MB まで（randomblob(9e8) でメモリを食わない）・本文程度の大きさは通る
        let e = sql(&mut srv, "SELECT length(randomblob(900000000))").unwrap_err();
        assert!(e.contains("大きすぎ"), "{e}");
        assert!(sql(&mut srv, "SELECT length(zeroblob(1000000))").is_ok());
        // 実行時に拒否するもの（拡張の読込・表の情報以外の pragma_xxx）は「書込」の文言にしない
        for q in ["SELECT load_extension('/tmp/nope')", "SELECT * FROM pragma_foreign_key_check('items')"] {
            let e = sql(&mut srv, q).unwrap_err();
            assert!(e.contains("許していない操作") && !e.contains("書込"), "{q}: {e}");
        }
        assert!(sql(&mut srv, "SELECT * FROM pragma_index_info('idx_items_doi')").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 手引き文書のドリフト: 注記・戻し方は同文・コマンド一覧のブロックは commands::markdown() と一致（= 全コマンドの名前が載る）。
    /// 手引き文書 docs/agent.md はリポジトリに同梱しないことがあるので、無ければ飛ばす（include_str! だとビルドが落ちる）
    #[test]
    fn docs_agent_md_in_sync() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/agent.md");
        let doc = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("docs/agent.md が無いので飛ばす");
                return;
            }
        };
        assert!(doc.contains(SCHEMA_NOTES), "docs/agent.md の注記が SCHEMA_NOTES と違う");
        assert!(doc.contains(RESTORE_TEXT), "docs/agent.md の戻し方が RESTORE_TEXT と違う");
        let begin = "<!-- commands:begin -->\n";
        let end = "<!-- commands:end -->";
        let s = doc.find(begin).expect("commands:begin が無い") + begin.len();
        let e = doc.find(end).expect("commands:end が無い");
        assert_eq!(
            &doc[s..e],
            commands::markdown(),
            "docs/agent.md のコマンド一覧が古い: `target/release/openref-mcp --commands` の出力で置き換える"
        );
        for c in commands::registry() {
            assert!(doc.contains(&format!("`{}`", c.name)), "{} が docs/agent.md に無い", c.name);
        }
    }
}
