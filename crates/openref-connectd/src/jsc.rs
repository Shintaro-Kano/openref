//! JavaScriptCore（macOS のシステムフレームワーク）で JS を評価する最小の FFI。
//! Word の Startup テンプレート版（render）が citeproc（word/vendor/citeproc.js）を動かすためだけに使う。
//! JS エンジンの crate は入れない。C API のうち使うのは context の作成・破棄・評価・文字列化だけ。macOS 以外はエラーを返す。

/// 1 つの global context（使い捨て。Send ではない = 作ったスレッドの中で使い切る）
pub struct Context {
    #[cfg(target_os = "macos")]
    ctx: imp::JSGlobalContextRef,
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::{c_int, c_void};

    pub type JSGlobalContextRef = *mut c_void;
    pub type JSContextRef = *const c_void;
    pub type JSStringRef = *mut c_void;
    pub type JSValueRef = *const c_void;
    pub type JSObjectRef = *mut c_void;
    pub type JSClassRef = *mut c_void;

    #[link(name = "JavaScriptCore", kind = "framework")]
    extern "C" {
        pub fn JSGlobalContextCreate(global_object_class: JSClassRef) -> JSGlobalContextRef;
        pub fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
        pub fn JSStringCreateWithCharacters(chars: *const u16, num_chars: usize) -> JSStringRef;
        pub fn JSStringRelease(string: JSStringRef);
        pub fn JSStringGetLength(string: JSStringRef) -> usize;
        pub fn JSStringGetCharactersPtr(string: JSStringRef) -> *const u16;
        pub fn JSEvaluateScript(
            ctx: JSContextRef,
            script: JSStringRef,
            this_object: JSObjectRef,
            source_url: JSStringRef,
            starting_line_number: c_int,
            exception: *mut JSValueRef,
        ) -> JSValueRef;
        pub fn JSValueToStringCopy(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> JSStringRef;
    }

    /// Rust の文字列 → JSString（UTF-16 で渡す = NUL を含んでもよい）。呼び手が JSStringRelease する
    pub fn js_string(s: &str) -> JSStringRef {
        let u: Vec<u16> = s.encode_utf16().collect();
        unsafe { JSStringCreateWithCharacters(u.as_ptr(), u.len()) }
    }

    /// JSString → Rust の文字列（解放はしない）
    pub fn rust_string(s: JSStringRef) -> String {
        unsafe {
            let n = JSStringGetLength(s);
            let p = JSStringGetCharactersPtr(s);
            if n == 0 || p.is_null() {
                return String::new();
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }

    /// 値を文字列化（JS の String(v)）。失敗は None
    pub fn value_to_string(ctx: JSContextRef, v: JSValueRef) -> Option<String> {
        if v.is_null() {
            return None;
        }
        let mut exc: JSValueRef = std::ptr::null();
        let s = unsafe { JSValueToStringCopy(ctx, v, &mut exc) };
        if s.is_null() {
            return None;
        }
        let out = rust_string(s);
        unsafe { JSStringRelease(s) };
        Some(out)
    }
}

#[cfg(target_os = "macos")]
impl Context {
    pub fn new() -> Result<Self, String> {
        let ctx = unsafe { imp::JSGlobalContextCreate(std::ptr::null_mut()) };
        if ctx.is_null() {
            return Err("JavaScriptCore の context を作れません".into());
        }
        Ok(Self { ctx })
    }

    /// スクリプトを評価し、結果を文字列（JS の String(結果)）で返す。例外は Err（例外値の文字列）。
    /// name = エラー文に出すファイル名
    pub fn eval(&self, src: &str, name: &str) -> Result<String, String> {
        let script = imp::js_string(src);
        let url = imp::js_string(name);
        let mut exc: imp::JSValueRef = std::ptr::null();
        let v = unsafe {
            imp::JSEvaluateScript(self.ctx as imp::JSContextRef, script, std::ptr::null_mut(), url, 1, &mut exc)
        };
        unsafe {
            imp::JSStringRelease(script);
            imp::JSStringRelease(url);
        }
        if !exc.is_null() {
            let msg = imp::value_to_string(self.ctx as imp::JSContextRef, exc).unwrap_or_else(|| "（例外の内容を読めません）".into());
            return Err(format!("{name}: {msg}"));
        }
        Ok(imp::value_to_string(self.ctx as imp::JSContextRef, v).unwrap_or_default())
    }
}

#[cfg(target_os = "macos")]
impl Drop for Context {
    fn drop(&mut self) {
        unsafe { imp::JSGlobalContextRelease(self.ctx) };
    }
}

#[cfg(not(target_os = "macos"))]
impl Context {
    pub fn new() -> Result<Self, String> {
        Err("整形エンジン（JavaScriptCore）は macOS でのみ使えます".into())
    }
    pub fn eval(&self, _src: &str, _name: &str) -> Result<String, String> {
        Err("整形エンジン（JavaScriptCore）は macOS でのみ使えます".into())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::Context;

    #[test]
    fn eval_returns_string_and_utf16() {
        let c = Context::new().unwrap();
        assert_eq!(c.eval("1 + 2", "t.js").unwrap(), "3");
        // 非 ASCII・サロゲートペア・NUL を往復する
        assert_eq!(c.eval("'Corrêa 小児 \\u{1F600}' + String.fromCharCode(0)", "t.js").unwrap(), "Corrêa 小児 \u{1F600}\u{0}");
        // 状態は context の中で続く
        c.eval("var x = 40;", "t.js").unwrap();
        assert_eq!(c.eval("x + 2", "t.js").unwrap(), "42");
    }

    #[test]
    fn eval_reports_exceptions() {
        let c = Context::new().unwrap();
        let e = c.eval("throw new TypeError('boom')", "x.js").unwrap_err();
        assert!(e.contains("x.js") && e.contains("TypeError: boom"), "{e}");
        let e = c.eval("this is not js", "y.js").unwrap_err();
        assert!(e.contains("SyntaxError"), "{e}");
        // 例外の後も使える
        assert_eq!(c.eval("'ok'", "z.js").unwrap(), "ok");
    }
}
