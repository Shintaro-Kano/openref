//! 本棚の表紙。PDF の 1 ページ目を長辺 768px の PNG にし、`<app_dir>/shelf-thumbs/<id>-<mtime>.png` に置く（root の下には書かない）。
//! 経路 = プロセス内の QuickLook（QLThumbnailGenerator）→ qlmanage → sips。qlmanage は `qlmanage.app` として LaunchServices に
//! Foreground で載る = 1 冊ごとに Dock に枠が出る（冊数の多い本棚の初回は数分出続ける）ので、先にプロセス内で作る。
//! 温め（開いた本棚の全部を作る）は画面が `shelf_thumb` を直列に呼ぶ（本棚を離れたら止める = 画面の都合なのでここは 1 冊の単位だけ）

use crate::{CoreError, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 表紙の置き場（app_dir の下の名前）
pub const DIR: &str = "shelf-thumbs";

/// 表紙作りの同時多発を防ぐ（1 冊 ~0.06〜0.1 秒の実測・1.4GB でも 0.13 秒 = 直列で十分）
static GEN_LOCK: Mutex<()> = Mutex::new(());

/// 1 回の経路（プロセス内の QuickLook / qlmanage / sips）の上限。返らない（Drive の本体待ち等）と GEN_LOCK を握ったまま全部の表紙が止まる
const TOOL_LIMIT: Duration = Duration::from_secs(30);

const QLMANAGE: &str = "/usr/bin/qlmanage";
const SIPS: &str = "/usr/bin/sips";

/// プロセス内の経路（pdf, 一時フォルダ）→ 一時フォルダの下に作った PNG / 作れなかった理由
type Inproc<'a> = &'a dyn Fn(&Path, &Path) -> std::result::Result<PathBuf, String>;

/// 表紙の置き場
pub fn dir(app_dir: &Path) -> PathBuf {
    app_dir.join(DIR)
}

pub fn thumb_path(cache: &Path, id: &str, mtime: i64) -> PathBuf {
    cache.join(format!("{id}-{mtime}.png"))
}

/// 無ければ作る。成功したら PNG のパスを返す。mtime が変わった古い表紙は掃除する
pub fn ensure_thumb(cache: &Path, pdf: &Path, id: &str, mtime: i64) -> Result<PathBuf> {
    ensure_using(cache, pdf, id, mtime, &inproc_thumb, Path::new(QLMANAGE), Path::new(SIPS))
}

/// ensure_thumb の本体（経路を受ける = 試験は偽のプロセス内の経路と偽の qlmanage / sips で「順番」と「出力先は cache の下だけ」を見る）
fn ensure_using(cache: &Path, pdf: &Path, id: &str, mtime: i64, inproc: Inproc, ql: &Path, sips: &Path) -> Result<PathBuf> {
    ensure_with(cache, id, mtime, |tmp| make_png(inproc, ql, sips, pdf, tmp))
}

/// ensure_thumb の本体（make = 一時フォルダへ PNG を作って返す・Err = 作れなかった理由 = 試験は差し替える）
fn ensure_with(cache: &Path, id: &str, mtime: i64, make: impl FnOnce(&Path) -> std::result::Result<PathBuf, String>) -> Result<PathBuf> {
    let dest = thumb_path(cache, id, mtime);
    if dest.exists() {
        return Ok(dest);
    }
    let _guard = GEN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if dest.exists() {
        return Ok(dest); // 鍵を待つ間に他のスレッドが作った
    }
    std::fs::create_dir_all(cache)?;
    // 旧 mtime の表紙を消す
    let prefix = format!("{id}-");
    for e in std::fs::read_dir(cache)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(".png") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let tmp = cache.join(format!(".tmp-{id}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let made = make(&tmp).map_err(|why| std::io::Error::other(format!("表紙を作れません（{why}）")));
    let r = made.and_then(|src| std::fs::rename(&src, &dest)).map(|()| dest).map_err(CoreError::from);
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

/// 1 ページ目を長辺 768px の PNG に。プロセス内の QuickLook → qlmanage → sips（1 ページ目を 72dpi でラスタライズ）の順に試す
fn make_png(inproc: Inproc, ql: &Path, sips: &Path, pdf: &Path, tmp: &Path) -> std::result::Result<PathBuf, String> {
    let why = match inproc(pdf, tmp) {
        Ok(p) if p.exists() => return Ok(p),
        Ok(p) => format!("{} がありません", p.display()),
        Err(e) => e,
    };
    // 退路の qlmanage は Dock に出る = プロセス内が落ちた理由はここで残す（全部だめなら Err の文にも入る）
    eprintln!("[shelf] 表紙: プロセス内の QuickLook で作れません（{why}）→ qlmanage / sips: {}", pdf.display());
    let ok = run_bounded(Command::new(ql).args(["-t", "-s", "768", "-o"]).arg(tmp).arg(pdf), TOOL_LIMIT);
    if let Some(p) = ok.then(|| first_png(tmp)).flatten() {
        return Ok(p);
    }
    let out = tmp.join("s.png");
    let ok = run_bounded(Command::new(sips).args(["-s", "format", "png", "-Z", "768"]).arg(pdf).arg("--out").arg(&out), TOOL_LIMIT);
    if ok && out.exists() {
        return Ok(out);
    }
    Err(format!("QuickLook: {why}・qlmanage も sips も PNG を作りませんでした"))
}

/// プロセス内の QuickLook（macOS 以外は無い = 道具の経路へ）
fn inproc_thumb(pdf: &Path, tmp: &Path) -> std::result::Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let out = tmp.join("q.png");
        ql_inproc::thumbnail(pdf, &out, 768.0, TOOL_LIMIT).map(|()| out)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pdf, tmp);
        Err("この OS にはありません".into())
    }
}

/// QuickLookThumbnailing.framework の QLThumbnailGenerator をプロセス内で呼ぶ（selector と値は SDK のヘッダ
/// QLThumbnailGenerator.h / QLThumbnailGenerationRequest.h の通り）。qlmanage と違って LaunchServices に載らない = Dock に出ない
#[cfg(target_os = "macos")]
mod ql_inproc {
    use block2::RcBlock;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, sel, Encode, Encoding};
    use objc2_foundation::{NSError, NSString, NSURL};
    use std::path::Path;
    use std::sync::{mpsc, OnceLock};
    use std::time::Duration;

    /// QLThumbnailGenerationRequestRepresentationTypeThumbnail（= 1 << 2・要求の寸法を満たす本物のサムネイル。icon / 低画質は要らない）
    const REPRESENTATION_THUMBNAIL: usize = 1 << 2;

    /// CoreGraphics の CGSize（64bit = CGFloat は f64）。encoding は `{CGSize=dd}`
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    // SAFETY: repr(C) の 2 つの f64 = CGSize の配置と同じ
    unsafe impl Encode for CGSize {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }

    /// QuickLookThumbnailing.framework は使う時に初めて dlopen する（link しない = cli / mcp / connectd には載らない）。
    /// ただし Foundation.framework と libobjc は objc2 / objc2-foundation 経由で openref-core を使う全部のバイナリに link される。読めたかを覚える
    fn framework_loaded() -> bool {
        static LOADED: OnceLock<bool> = OnceLock::new();
        *LOADED.get_or_init(|| {
            let path = c"/System/Library/Frameworks/QuickLookThumbnailing.framework/QuickLookThumbnailing";
            // SAFETY: NUL 終端の固定文字列。handle は閉じない（クラスはプロセスの寿命の間使う）
            !unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) }.is_null()
        })
    }

    /// pdf の 1 ページ目を長辺 `side` px の PNG にして out に書く。完了ハンドラを limit まで待ち、時間切れは request を cancel して Err
    pub fn thumbnail(pdf: &Path, out: &Path, side: f64, limit: Duration) -> Result<(), String> {
        thumbnail_with(pdf, out, side, limit, true)
    }

    /// thumbnail の本体（uttype = false は UTType の版があっても文字列の版を使う = 試験が macOS 15 未満の経路を通す）
    pub(super) fn thumbnail_with(pdf: &Path, out: &Path, side: f64, limit: Duration, uttype: bool) -> Result<(), String> {
        let (Some(pdf_s), Some(out_s)) = (pdf.to_str(), out.to_str()) else {
            return Err("パスが UTF-8 ではありません".into());
        };
        if !framework_loaded() {
            return Err("QuickLookThumbnailing.framework を読めません".into());
        }
        let (Some(gen_cls), Some(req_cls)) = (AnyClass::get(c"QLThumbnailGenerator"), AnyClass::get(c"QLThumbnailGenerationRequest")) else {
            return Err("QLThumbnailGenerator がありません".into());
        };
        // 保存の selector: UTType を渡す版（非推奨でない）があればそれ・無ければ文字列の版（macOS 11〜・15 で非推奨 = 動く）
        let as_uttype = sel!(saveBestRepresentationForRequest:toFileAtURL:asContentType:completionHandler:);
        let with_string = sel!(saveBestRepresentationForRequest:toFileAtURL:withContentType:completionHandler:);
        let ut_cls = AnyClass::get(c"UTType").filter(|_| uttype && gen_cls.responds_to(as_uttype));
        if ut_cls.is_none() && !gen_cls.responds_to(with_string) {
            return Err("saveBestRepresentationForRequest がありません（macOS 11 以上）".into());
        }
        autoreleasepool(|_| {
            // SAFETY: selector と引数の型はヘッダの宣言の通り（encoding は debug ビルドで objc2 が照合する）。
            // 戻り値の nil は Option で受ける
            unsafe {
                let file = NSURL::fileURLWithPath(&NSString::from_str(pdf_s));
                let dest = NSURL::fileURLWithPath(&NSString::from_str(out_s));
                let size = CGSize { width: side, height: side };
                let alloc: Allocated<AnyObject> = msg_send![req_cls, alloc];
                let req: Option<Retained<AnyObject>> =
                    msg_send![alloc, initWithFileAtURL: &*file, size: size, scale: 1.0f64, representationTypes: REPRESENTATION_THUMBNAIL];
                let req = req.ok_or("QLThumbnailGenerationRequest を作れません")?;
                let gen: Option<Retained<AnyObject>> = msg_send![gen_cls, sharedGenerator];
                let gen = gen.ok_or("sharedGenerator がありません")?;

                // 完了ハンドラは QuickLook の内部の queue から呼ばれる（main ではない = 待つのは呼び出し側のスレッドだけ）。
                // 送り手は block が持つ = 時間切れで受け手を捨てた後に遅れて呼ばれても send が Err になるだけ（解放済みの物に触らない）
                let (tx, rx) = mpsc::channel::<Result<(), String>>();
                let done = RcBlock::new(move |err: *mut NSError| {
                    let r = match err.as_ref() {
                        None => Ok(()),
                        Some(e) => Err(autoreleasepool(|_| e.localizedDescription().to_string())),
                    };
                    let _ = tx.send(r);
                });
                match ut_cls {
                    Some(ut) => {
                        let png: Option<Retained<AnyObject>> = msg_send![ut, typeWithIdentifier: &*NSString::from_str("public.png")];
                        let png = png.ok_or("UTType public.png がありません")?;
                        let () = msg_send![&*gen, saveBestRepresentationForRequest: &*req, toFileAtURL: &*dest, asContentType: &*png, completionHandler: &*done];
                    }
                    None => {
                        let png = NSString::from_str("public.png");
                        let () = msg_send![&*gen, saveBestRepresentationForRequest: &*req, toFileAtURL: &*dest, withContentType: &*png, completionHandler: &*done];
                    }
                }
                match rx.recv_timeout(limit) {
                    Ok(r) => r,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let () = msg_send![&*gen, cancelRequest: &*req];
                        Err(format!("{} 秒で返りません（取り消した）", limit.as_secs_f32()))
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => Err("完了ハンドラが呼ばれずに捨てられました".into()),
                }
            }
        })
    }
}

/// 子を起こして limit まで待つ。戻り値 = 時間内に終了状態 0 で終わった（時間切れは kill して回収する）
fn run_bounded(cmd: &mut Command, limit: Duration) -> bool {
    let Ok(mut child) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if t0.elapsed() < limit => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn first_png(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "png"))
}

/// これらの id の表紙を消す（`<id>-<mtime>.png`・他の名前は触らない）。戻り値 = 消した枚数
pub fn remove_for(cache: &Path, ids: &HashSet<&str>) -> usize {
    let Ok(rd) = std::fs::read_dir(cache) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let hit = name.strip_suffix(".png").and_then(|s| s.split_once('-')).is_some_and(|(id, _)| ids.contains(id));
        if hit && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(tag: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("openref-shelf-thumbs-{tag}-{}-{:x}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn fake_png(t: &Path) -> std::result::Result<PathBuf, String> {
        let p = t.join("x.png");
        std::fs::write(&p, b"png").map_err(|e| e.to_string())?;
        Ok(p)
    }

    #[test]
    fn dir_and_names_are_fixed() {
        assert_eq!(dir(Path::new("/app")), Path::new("/app/shelf-thumbs"));
        assert_eq!(thumb_path(Path::new("/c"), "abc", 12), Path::new("/c/abc-12.png"));
    }

    /// 作る → 2 回目は作らない → mtime が変われば旧を消して作り直す・他の本は触らない・一時フォルダは残らない
    #[test]
    fn makes_once_and_replaces_stale() {
        let d = tmp("once");
        let cache = d.0.join("shelf-thumbs");
        let p = ensure_with(&cache, "aaaa", 10, fake_png).unwrap();
        assert_eq!(p, cache.join("aaaa-10.png"));
        assert_eq!(std::fs::read(&p).unwrap(), b"png");
        let again = ensure_with(&cache, "aaaa", 10, |_| panic!("2 回目は作らない")).unwrap();
        assert_eq!(again, p);
        std::fs::write(cache.join("bbbb-1.png"), b"other").unwrap();
        let p2 = ensure_with(&cache, "aaaa", 11, fake_png).unwrap();
        assert!(!p.exists() && p2.exists());
        assert!(cache.join("bbbb-1.png").exists());
        let names: Vec<String> = std::fs::read_dir(&cache).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.starts_with(".tmp")), "{names:?}");
    }

    /// 作れなければ Err（文は握りつぶさない）・一時フォルダは残らない
    #[test]
    fn failure_is_reported() {
        let d = tmp("fail");
        let cache = d.0.join("c");
        let e = ensure_with(&cache, "aaaa", 1, |_| Err("理由の文".into())).unwrap_err().to_string();
        assert!(e.contains("表紙を作れません") && e.contains("理由の文"), "{e}");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    }

    /// 偽の道具（qlmanage / sips）を置くフォルダ。`script(name, body)` は起動されたら `<name>.ran` を残してから body を走らせる
    struct Tools {
        bin: PathBuf,
    }
    impl Tools {
        fn new(bin: PathBuf) -> Self {
            std::fs::create_dir_all(&bin).unwrap();
            Tools { bin }
        }
        fn script(&self, name: &str, body: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let p = self.bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n: > \"{}.ran\"\n{body}\n", p.display())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }
        /// 起動された偽の道具の名前（並びは名前順）
        fn ran(&self) -> Vec<String> {
            let mut v: Vec<String> =
                std::fs::read_dir(&self.bin).unwrap().flatten().filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".ran").map(str::to_owned)).collect();
            v.sort();
            v
        }
        fn reset(&self) {
            for n in self.ran() {
                std::fs::remove_file(self.bin.join(format!("{n}.ran"))).unwrap();
            }
        }
    }

    /// 道具の出力先は cache の下の一時フォルダだけ（root = PDF の隣に書かない）。偽の qlmanage / sips は受けた出力先を自分で検査する
    /// = `-o` / `--out` を PDF の隣に変えると、一度そこへ書いて cache へ rename する形（事後の隣の確認では見えない）でも落ちる。実物の qlmanage は要らない
    #[test]
    fn tools_write_under_the_cache_only() {
        let d = tmp("tools");
        let shelf = d.0.join("root");
        let tools = Tools::new(d.0.join("bin"));
        std::fs::create_dir_all(&shelf).unwrap();
        let pdf = shelf.join("a.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let cache = d.0.join("shelf-thumbs");
        let script = |name: &str, body: &str| tools.script(name, body);
        // プロセス内の経路は作れない役（道具へ落ちる）。受けた一時フォルダが cache の下かも見る
        let off: Inproc = &|p, t| {
            assert_eq!(p, pdf.as_path());
            assert!(t.parent() == Some(cache.as_path()) && t.file_name().unwrap().to_string_lossy().starts_with(".tmp-"), "{}", t.display());
            Err("off".into())
        };
        // qlmanage -t -s 768 -o <出力先> <pdf> / sips -s format png -Z 768 <pdf> --out <出力先>
        let under = |arg: &str| format!(r#"case "{arg}" in "{}"/.tmp-*) ;; *) exit 4 ;; esac"#, cache.display());
        let ql = script("ql", &format!(r#"[ "$4" = -o ] || exit 3; {}; printf ql > "$5/a.pdf.png""#, under("$5")));
        let sips = script("sips", &format!(r#"[ "$7" = --out ] || exit 3; {}; printf sips > "$8""#, under("$8")));
        let fail = script("fail", "exit 1");
        let neighbours = || -> Vec<String> { std::fs::read_dir(&shelf).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect() };

        let p = ensure_using(&cache, &pdf, "aaaa", 1, off, &ql, &fail).unwrap();
        assert_eq!((p.clone(), std::fs::read(&p).unwrap()), (cache.join("aaaa-1.png"), b"ql".to_vec()));
        assert_eq!(neighbours(), ["a.pdf"]);
        let p = ensure_using(&cache, &pdf, "bbbb", 1, off, &fail, &sips).unwrap();
        assert_eq!((p.clone(), std::fs::read(&p).unwrap()), (cache.join("bbbb-1.png"), b"sips".to_vec()));
        assert_eq!(neighbours(), ["a.pdf"]);
        assert!(ensure_using(&cache, &pdf, "cccc", 1, off, &fail, &fail).is_err());
        assert_eq!(neighbours(), ["a.pdf"]);
        assert!(!cache.join(".tmp-aaaa").exists() && !cache.join(".tmp-bbbb").exists() && !cache.join(".tmp-cccc").exists());
    }

    /// 順番 = プロセス内 → qlmanage → sips。プロセス内が作れたら道具は 1 つも起こさない（= qlmanage.app が Dock に出ない）。
    /// プロセス内が失敗 / 時間切れ / 「作った」のにファイルが無いなら qlmanage へ、それもだめなら sips へ。全部だめなら理由を Err に
    #[test]
    fn inproc_first_then_tools() {
        let d = tmp("order");
        let tools = Tools::new(d.0.join("bin"));
        let pdf = d.0.join("a.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let cache = d.0.join("shelf-thumbs");
        let ql = tools.script("qlmanage", r#"printf ql > "$5/a.pdf.png""#);
        let sips = tools.script("sips", r#"printf sips > "$8""#);
        let ql_fail = tools.script("qlmanage-fail", "exit 1");
        let ok: Inproc = &|_, t| {
            let p = t.join("q.png");
            std::fs::write(&p, b"inproc").map_err(|e| e.to_string())?;
            Ok(p)
        };
        let timeout: Inproc = &|_, _| Err("30 秒で返りません（取り消した）".into());
        let phantom: Inproc = &|_, t| Ok(t.join("q.png")); // 「作った」と言うがファイルが無い

        let p = ensure_using(&cache, &pdf, "a1", 1, ok, &ql, &sips).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"inproc");
        assert_eq!(tools.ran(), Vec::<String>::new(), "プロセス内で作れたら qlmanage も sips も起こさない");

        for (id, inproc) in [("a2", timeout), ("a3", phantom)] {
            tools.reset();
            let p = ensure_using(&cache, &pdf, id, 1, inproc, &ql, &sips).unwrap();
            assert_eq!(std::fs::read(&p).unwrap(), b"ql", "{id}");
            assert_eq!(tools.ran(), ["qlmanage"], "{id}");
        }

        tools.reset();
        let p = ensure_using(&cache, &pdf, "a4", 1, timeout, &ql_fail, &sips).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"sips");
        assert_eq!(tools.ran(), ["qlmanage-fail", "sips"]);

        tools.reset();
        let fail = tools.script("fail", "exit 1");
        let e = ensure_using(&cache, &pdf, "a5", 1, timeout, &fail, &fail).unwrap_err().to_string();
        assert!(e.contains("表紙を作れません") && e.contains("QuickLook: 30 秒で返りません"), "{e}");
        assert!(std::fs::read_dir(&cache).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".tmp")));
    }

    /// 時間切れの子は kill して false・すぐ終わる子は終了状態で判定
    #[test]
    fn run_bounded_kills_on_timeout() {
        assert!(run_bounded(Command::new("/usr/bin/true").arg("x"), Duration::from_secs(5)));
        assert!(!run_bounded(&mut Command::new("/usr/bin/false"), Duration::from_secs(5)));
        let t0 = Instant::now();
        assert!(!run_bounded(Command::new("/bin/sleep").arg("20"), Duration::from_millis(50)));
        assert!(t0.elapsed() < Duration::from_secs(5));
        assert!(!run_bounded(&mut Command::new("/nonexistent-tool"), Duration::from_secs(1)));
    }

    /// remove_for: 渡した id の表紙だけ（mtime は問わない）・`-` の無い名前と他の拡張子は触らない・無いフォルダは 0
    #[test]
    fn remove_for_only_touches_named_ids() {
        let d = tmp("rm");
        for n in ["aaaa-1.png", "aaaa-2.png", "bbbb-1.png", "aaaa.png", "aaaa-1.txt", ".tmp-aaaa"] {
            std::fs::write(d.0.join(n), b"x").unwrap();
        }
        assert_eq!(remove_for(&d.0, &HashSet::from(["aaaa"])), 2);
        let mut left: Vec<String> = std::fs::read_dir(&d.0).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [".tmp-aaaa", "aaaa-1.txt", "aaaa.png", "bbbb-1.png"]);
        assert_eq!(remove_for(&d.0.join("nope"), &HashSet::from(["aaaa"])), 0);
    }

    /// PNG の幅と高さ（IHDR）
    #[cfg(target_os = "macos")]
    fn png_size(p: &Path) -> (u32, u32) {
        let b = std::fs::read(p).unwrap();
        assert_eq!(&b[..8], b"\x89PNG\r\n\x1a\n", "{}", p.display());
        let be = |i: usize| u32::from_be_bytes(b[i..i + 4].try_into().unwrap());
        (be(16), be(20))
    }

    /// 実物の QLThumbnailGenerator（ignored = GUI のセッションが要る）: 長辺 768 の PNG を、道具を起こさずに作る。
    /// 完了ハンドラが main の queue に来るなら、試験のスレッドは main の runloop を回していないので時間切れになる = 来ないことの確認も兼ねる
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn inproc_real_pdf() {
        let d = tmp("inproc");
        let (shelf, out) = (d.0.join("root"), d.0.join("out"));
        std::fs::create_dir_all(&shelf).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let pdf = shelf.join("a.pdf");
        std::fs::write(&pdf, crate::pdftools::minimal_pdf("shelf", None)).unwrap();
        let p = inproc_thumb(&pdf, &out).expect("png");
        assert!(p.starts_with(&out), "出力先の一時フォルダの下に作る: {}", p.display());
        assert_eq!(names(&shelf), ["a.pdf"], "PDF の隣には書かない");
        let (w, h) = png_size(&p);
        assert_eq!(w.max(h), 768, "{w}x{h}");
        // 読めない PDF は Err（理由の文つき）で返る = 握りつぶさない。出力は空の別フォルダ（前の出力の残りに左右されない）
        let bad = shelf.join("bad.pdf");
        std::fs::write(&bad, b"not a pdf").unwrap();
        let bad_out = d.0.join("bad-out");
        std::fs::create_dir_all(&bad_out).unwrap();
        let e = inproc_thumb(&bad, &bad_out);
        assert!(e.as_ref().is_err_and(|e| !e.is_empty()), "読めない PDF は理由つきの Err: {e:?}");
        assert_eq!(names(&bad_out), Vec::<String>::new(), "読めない PDF の出力は無い");
    }

    /// フォルダの中の名前（名前順）
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// 実物の QLThumbnailGenerator の 2 つの保存の selector（ignored）: UTType の版がこの OS にあること・文字列の版（macOS 11〜14 が使う）でも同じ寸法
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn inproc_both_save_selectors() {
        let d = tmp("inproc-sel");
        let pdf = d.0.join("a.pdf");
        std::fs::write(&pdf, crate::pdftools::minimal_pdf("shelf", None)).unwrap();
        let mut sizes = vec![];
        for uttype in [true, false] {
            let out = d.0.join(format!("{uttype}.png"));
            ql_inproc::thumbnail_with(&pdf, &out, 768.0, Duration::from_secs(30), uttype).unwrap();
            sizes.push(png_size(&out));
        }
        assert_eq!(sizes[0], sizes[1]);
        assert_eq!(sizes[0].0.max(sizes[0].1), 768);
        let gen = objc2::runtime::AnyClass::get(c"QLThumbnailGenerator").unwrap();
        println!(
            "asContentType: {} / withContentType: {}",
            gen.responds_to(objc2::sel!(saveBestRepresentationForRequest:toFileAtURL:asContentType:completionHandler:)),
            gen.responds_to(objc2::sel!(saveBestRepresentationForRequest:toFileAtURL:withContentType:completionHandler:))
        );
    }

    /// 実物の QLThumbnailGenerator の時間切れ（ignored）: 上限 0 で即 Err → cancel。遅れて完了ハンドラが呼ばれても落ちない・
    /// その後も普通に作れる
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn inproc_timeout_cancels_safely() {
        let d = tmp("inproc-to");
        let pdf = d.0.join("a.pdf");
        std::fs::write(&pdf, crate::pdftools::minimal_pdf("shelf", None)).unwrap();
        // ensure_with と同じく、時間切れの後に一時フォルダを消す → 遅れた書き込みがフォルダを作り直さないこと
        let sub = d.0.join(".tmp-x");
        std::fs::create_dir_all(&sub).unwrap();
        for i in 0..5 {
            let e = ql_inproc::thumbnail(&pdf, &sub.join(format!("t{i}.png")), 768.0, Duration::ZERO).unwrap_err();
            assert!(e.contains("返りません"), "{e}");
        }
        std::fs::remove_dir_all(&sub).unwrap();
        std::thread::sleep(Duration::from_secs(3)); // 遅れた完了ハンドラ（送り先の無い send）を待つ
        assert!(!sub.exists(), "取り消した要求が一時フォルダを作り直した");
        let out = d.0.join("ok.png");
        ql_inproc::thumbnail(&pdf, &out, 768.0, Duration::from_secs(30)).unwrap();
        let (w, h) = png_size(&out);
        assert_eq!(w.max(h), 768);
    }

    /// 実物の本のフォルダ（ignored・手元の実弾）: OPENREF_THUMBS_SRC の下の PDF 全部の表紙を OPENREF_THUMBS_CACHE（scratch だけ）へ作る。
    /// プロセス内で作れた数と道具へ落ちた数を分けて数える（PDF は読むだけ）
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn real_shelf_folder() {
        use std::cell::Cell;
        let (Ok(src), Ok(cache)) = (std::env::var("OPENREF_THUMBS_SRC"), std::env::var("OPENREF_THUMBS_CACHE")) else {
            println!("real_shelf_folder: OPENREF_THUMBS_SRC / OPENREF_THUMBS_CACHE が無い = 走らせない");
            return;
        };
        let (src, cache) = (PathBuf::from(src), PathBuf::from(cache));
        assert!(cache.to_string_lossy().contains("scratch"), "scratch の cache だけ");
        let pdfs: Vec<PathBuf> = walkdir::WalkDir::new(&src)
            .into_iter()
            .flatten()
            .map(|e| e.into_path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
            .collect();
        let (inproc_ok, inproc_err) = (Cell::new(0), Cell::new(0));
        let counted: Inproc = &|p, t| {
            let r = inproc_thumb(p, t);
            match &r {
                Ok(_) => inproc_ok.set(inproc_ok.get() + 1),
                Err(e) => {
                    inproc_err.set(inproc_err.get() + 1);
                    println!("inproc Err {}: {e}", p.display());
                }
            }
            r
        };
        let t0 = Instant::now();
        let mut made = 0;
        for (i, pdf) in pdfs.iter().enumerate() {
            match ensure_using(&cache, pdf, &format!("b{i:04}"), 1, counted, Path::new(QLMANAGE), Path::new(SIPS)) {
                Ok(_) => made += 1,
                Err(e) => println!("Err {}: {e}", pdf.display()),
            }
        }
        println!("pdfs={} made={made} inproc_ok={} inproc_err={} secs={:.2}", pdfs.len(), inproc_ok.get(), inproc_err.get(), t0.elapsed().as_secs_f64());
    }

    /// 実物の経路（ignored = GUI のセッションが要る。CI の無人では QuickLook が動かないことがある）
    #[test]
    #[ignore]
    fn ensure_thumb_real_pdf() {
        let d = tmp("real");
        let shelf = d.0.join("root");
        std::fs::create_dir_all(&shelf).unwrap();
        let pdf = shelf.join("a.pdf");
        std::fs::write(&pdf, crate::pdftools::minimal_pdf("shelf", None)).unwrap();
        let cache = d.0.join("c");
        let p = ensure_thumb(&cache, &pdf, "aaaa", 1).expect("png");
        assert_eq!(p, cache.join("aaaa-1.png"));
        assert!(std::fs::metadata(&p).unwrap().len() > 100, "{}", p.display());
        assert_eq!(names(&shelf), ["a.pdf"], "PDF の隣には書かない");
    }

    /// 実物のプロセス内の経路が書くのは cache の下の一時フォルダだけ（ignored）。本番の入口 ensure_thumb は隣へ書いた PNG を
    /// cache へ rename してしまう（事後の隣の確認では見えない）+ 書けなければ qlmanage へ落ちて成功する、ので:
    /// 本棚のフォルダを書き込み禁止にし、道具を失敗役にして ensure_using を通す = プロセス内が隣へ書こうとすると Err になる
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn inproc_writes_under_the_cache_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("inproc-where");
        let shelf = d.0.join("root");
        std::fs::create_dir_all(&shelf).unwrap();
        let pdf = shelf.join("a.pdf");
        std::fs::write(&pdf, crate::pdftools::minimal_pdf("shelf", None)).unwrap();
        let cache = d.0.join("shelf-thumbs");
        let fail = Tools::new(d.0.join("bin")).script("fail", "exit 1");
        std::fs::set_permissions(&shelf, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = ensure_using(&cache, &pdf, "aaaa", 1, &inproc_thumb, &fail, &fail);
        std::fs::set_permissions(&shelf, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = r.expect("プロセス内の経路が cache の下に作る");
        assert_eq!(p, cache.join("aaaa-1.png"));
        let (w, h) = png_size(&p);
        assert_eq!(w.max(h), 768, "{w}x{h}");
        assert_eq!(names(&shelf), ["a.pdf"]);
        assert_eq!(names(&cache), ["aaaa-1.png"], "一時フォルダは残らない");
    }
}
