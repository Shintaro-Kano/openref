//! 本棚の表紙。qlmanage（QuickLook）→ sips の fallback で PDF の 1 ページ目を PNG にし、
//! `<app_dir>/shelf-thumbs/<id>-<mtime>.png` に置く（root の下には書かない）。
//! 温め（開いた本棚の全部を作る）は画面が `shelf_thumb` を直列に呼ぶ（本棚を離れたら止める = 画面の都合なのでここは 1 冊の単位だけ）

use crate::{CoreError, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 表紙の置き場（app_dir の下の名前）
pub const DIR: &str = "shelf-thumbs";

/// qlmanage の同時多発の起動を防ぐ（1 冊 ~0.1 秒の実測・1.4GB でも 0.13 秒 = 直列で十分）
static GEN_LOCK: Mutex<()> = Mutex::new(());

/// 1 回の qlmanage / sips の上限。返らない（Drive の本体待ち等）と GEN_LOCK を握ったまま全部の表紙が止まる
const TOOL_LIMIT: Duration = Duration::from_secs(30);

const QLMANAGE: &str = "/usr/bin/qlmanage";
const SIPS: &str = "/usr/bin/sips";

/// 表紙の置き場
pub fn dir(app_dir: &Path) -> PathBuf {
    app_dir.join(DIR)
}

pub fn thumb_path(cache: &Path, id: &str, mtime: i64) -> PathBuf {
    cache.join(format!("{id}-{mtime}.png"))
}

/// 無ければ作る。成功したら PNG のパスを返す。mtime が変わった古い表紙は掃除する
pub fn ensure_thumb(cache: &Path, pdf: &Path, id: &str, mtime: i64) -> Result<PathBuf> {
    ensure_using(cache, pdf, id, mtime, Path::new(QLMANAGE), Path::new(SIPS))
}

/// ensure_thumb の本体（道具のパスを受ける = 試験は偽の qlmanage / sips で「出力先は cache の下だけ」を見る）
fn ensure_using(cache: &Path, pdf: &Path, id: &str, mtime: i64, ql: &Path, sips: &Path) -> Result<PathBuf> {
    ensure_with(cache, id, mtime, |tmp| quicklook(ql, sips, pdf, tmp))
}

/// ensure_thumb の本体（make = 一時フォルダへ PNG を作って返す = 試験は差し替える）
fn ensure_with(cache: &Path, id: &str, mtime: i64, make: impl FnOnce(&Path) -> Option<PathBuf>) -> Result<PathBuf> {
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
    let made = make(&tmp).ok_or_else(|| std::io::Error::other("表紙を作れません（qlmanage も sips も PNG を作りませんでした）"));
    let r = made.and_then(|src| std::fs::rename(&src, &dest)).map(|()| dest).map_err(CoreError::from);
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

/// QuickLook で 1 ページ目を長辺 768px の PNG に。だめなら sips（1 ページ目を 72dpi でラスタライズ）
fn quicklook(ql: &Path, sips: &Path, pdf: &Path, tmp: &Path) -> Option<PathBuf> {
    let ok = run_bounded(Command::new(ql).args(["-t", "-s", "768", "-o"]).arg(tmp).arg(pdf), TOOL_LIMIT);
    if let Some(p) = ok.then(|| first_png(tmp)).flatten() {
        return Some(p);
    }
    let out = tmp.join("s.png");
    let ok = run_bounded(Command::new(sips).args(["-s", "format", "png", "-Z", "768"]).arg(pdf).arg("--out").arg(&out), TOOL_LIMIT);
    (ok && out.exists()).then_some(out)
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

    fn fake_png(t: &Path) -> Option<PathBuf> {
        let p = t.join("x.png");
        std::fs::write(&p, b"png").ok()?;
        Some(p)
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
        let e = ensure_with(&cache, "aaaa", 1, |_| None).unwrap_err().to_string();
        assert!(e.contains("表紙を作れません"), "{e}");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    }

    /// 道具の出力先は cache の下の一時フォルダだけ（root = PDF の隣に書かない）。偽の qlmanage / sips は受けた出力先を自分で検査する
    /// = `-o` / `--out` を PDF の隣に変えると、一度そこへ書いて cache へ rename する形（事後の隣の確認では見えない）でも落ちる。実物の qlmanage は要らない
    #[test]
    fn tools_write_under_the_cache_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("tools");
        let shelf = d.0.join("root");
        let bin = d.0.join("bin");
        std::fs::create_dir_all(&shelf).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let pdf = shelf.join("a.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let cache = d.0.join("shelf-thumbs");
        let script = |name: &str, body: &str| {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        // qlmanage -t -s 768 -o <出力先> <pdf> / sips -s format png -Z 768 <pdf> --out <出力先>
        let under = |arg: &str| format!(r#"case "{arg}" in "{}"/.tmp-*) ;; *) exit 4 ;; esac"#, cache.display());
        let ql = script("ql", &format!(r#"[ "$4" = -o ] || exit 3; {}; printf ql > "$5/a.pdf.png""#, under("$5")));
        let sips = script("sips", &format!(r#"[ "$7" = --out ] || exit 3; {}; printf sips > "$8""#, under("$8")));
        let fail = script("fail", "exit 1");
        let neighbours = || -> Vec<String> { std::fs::read_dir(&shelf).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect() };

        let p = ensure_using(&cache, &pdf, "aaaa", 1, &ql, &fail).unwrap();
        assert_eq!((p.clone(), std::fs::read(&p).unwrap()), (cache.join("aaaa-1.png"), b"ql".to_vec()));
        assert_eq!(neighbours(), ["a.pdf"]);
        let p = ensure_using(&cache, &pdf, "bbbb", 1, &fail, &sips).unwrap();
        assert_eq!((p.clone(), std::fs::read(&p).unwrap()), (cache.join("bbbb-1.png"), b"sips".to_vec()));
        assert_eq!(neighbours(), ["a.pdf"]);
        assert!(ensure_using(&cache, &pdf, "cccc", 1, &fail, &fail).is_err());
        assert_eq!(neighbours(), ["a.pdf"]);
        assert!(!cache.join(".tmp-aaaa").exists() && !cache.join(".tmp-bbbb").exists() && !cache.join(".tmp-cccc").exists());
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

    /// 実物の qlmanage（ignored = GUI のセッションが要る。CI の無人では QuickLook が動かないことがある）
    #[test]
    #[ignore]
    fn ensure_thumb_real_pdf() {
        let d = tmp("real");
        let pdf = d.0.join("a.pdf");
        std::fs::write(&pdf, crate::pdftools::minimal_pdf("shelf", None)).unwrap();
        let p = ensure_thumb(&d.0.join("c"), &pdf, "aaaa", 1).expect("png");
        assert!(std::fs::metadata(&p).unwrap().len() > 100, "{}", p.display());
    }
}
