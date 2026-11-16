const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const code = name => fs.readFileSync(path.join(__dirname, '..', name), 'utf8');
const article = 'https://www.sciencedirect.com/science/article/pii/S0000000000000001';
const pdf = article + '/pdfft?pid=1-s2.0-S0000000000000001-main.pdf';
const landing = 'https://pdf.sciencedirectassets.com/313380/issue/1-s2.0-S0000000000000001/main.pdf?signature=test';
const pmcPage = 'https://pmc.ncbi.nlm.nih.gov/articles/PMC3062508/';
const pmcPdf = pmcPage + 'pdf/nihms277358.pdf';
const pmcLink = 'https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/';
const pmcRoute = `{url:'${pmcLink}',pmid:'21620466'}`;
const htmlError = msg => Object.assign(new Error(msg || 'PDF ではない応答'), { html: true });
const same = (a, b) => assert.equal(JSON.stringify(a), JSON.stringify(b)); // vm の別 realm の配列を比べる
const C7_PERMISSIONS = ['webRequest', 'declarativeNetRequestWithHostAccess']; // 応答ヘッダで PDF の到着を知るために足した権限
/* popup の executeScript({files: ["pdfbytes.js", "content.js"]}) と同じ順に同じ global へ読む */
function runContent(context) {
  vm.createContext(context);
  vm.runInContext(code('pdfbytes.js'), context);
  vm.runInContext(code('content.js'), context);
  return context;
}

function analyze(url, links, fullTextLinks = links) {
  let listener;
  const context = { URL, window: {}, location: new URL(url), document: {
    title: 'Article', contentType: 'text/html', body: { innerText: '' },
    querySelector: () => null, querySelectorAll: selector => (selector === ".full-text-links-list a[href]" ? fullTextLinks : links).map(href => ({ href }))
  }, chrome: { runtime: { onMessage: { addListener: f => listener = f } } } };
  runContent(context);
  let result;
  listener({ type: 'openref-analyze' }, {}, r => result = r);
  return result.data;
}
test('ScienceDirect chooses current PII before referenced PDFs', () => {
  const result = analyze(article, [article.replace('0001', '9999') + '/pdfft?pid=other.pdf', pdf]);
  assert.equal(result.pdfUrl, pdf);
  assert.equal(result.pdfSource, 'sciencedirect');
});
for (const [name, url, links] of [
  ['search page', 'https://www.sciencedirect.com/search', [pdf]],
  ['different PII', article, [article.replace('0001', '9999') + '/pdfft']],
  ['lookalike host', article.replace('.com', '.com.example.org'), [pdf]],
  ['off-site PDF', article, ['https://example.org/science/article/pii/S0000000000000001/pdfft']],
]) test(name + ' is not recognized as article PDF', () => {
  assert.notEqual(analyze(url, links).pdfSource, 'sciencedirect');
});

/* popup.js を仮想時刻で走らせる。5 秒未満のタイマー（sleep・waitUntil）は時計を進めて即実行、
   5 秒以上（withTimeout の打ち切り）は発火しない。タブ 7 = 背景タブ。timeline の [ms, url, status] で
   chrome.tabs.get の値が変わり、変わるたびに onUpdated（{status}）が発火する */
function popup() {
  const removed = [], registrations = [], posts = [], listeners = [];
  let now = 0, timeline = [[0, pdf, 'complete']], fired = 0;
  const fire = () => {
    while (fired < timeline.length && timeline[fired][0] <= now) {
      const [, url, status] = timeline[fired++];
      for (const l of listeners.slice()) l(7, { status }, { url, status });
    }
  };
  const state = () => {
    let s = timeline[0];
    for (const e of timeline) if (e[0] <= now) s = e;
    return { url: s[1], status: s[2] };
  };
  const context = vm.createContext({ URL, URLSearchParams, Uint8Array, atob, btoa, TextDecoder,
    Date: { now: () => now },
    setTimeout: (f, ms) => { if (ms >= 5000) return 0; now += ms || 1; fire(); f(); return 0; },
    clearTimeout() {},
    document: { getElementById: () => ({ addEventListener() {}, style: {}, value: '' }) },
    chrome: { tabs: {
      create: async () => { fired = 0; fire(); return { id: 7 }; },
      get: async () => state(),
      remove: async id => removed.push(id),
      onUpdated: { addListener: l => listeners.push(l), removeListener: l => listeners.splice(listeners.indexOf(l), 1) },
      onRemoved: { addListener() {}, removeListener() {} },
    } },
  });
  vm.runInContext(code('pdfbytes.js'), context); // popup.html の <script> の順
  vm.runInContext(code('popup.js').replace(/\ninit\(\);\s*$/, ''), context);
  vm.runInContext('tab = { id: 1 }; contentScriptOk = true;', context);
  context.fakeMessage = async msg => { registrations.push(msg); return { ok: true }; };
  context.fakeApi = async url => { posts.push(url); return {}; };
  vm.runInContext('sendToBg = fakeMessage; api = fakeApi;', context);
  return {
    context, removed, registrations, posts, listeners,
    now: () => now,
    timeline: t => { timeline = t; fired = 0; },
    fetcher: f => { context.fakeFetch = f; vm.runInContext('fetchPdfViaTabThenBg = fakeFetch;', context); },
  };
}
/* chrome.storage.session の代役（popup と service worker が同じ 1 つを見る）。値は書くたび・読むたびに JSON で写す
   （実物は structured clone = 別の realm に同じ物は渡らない）。set の中で onChanged を呼ぶ（順が決まる） */
const clone = v => (v === undefined ? undefined : JSON.parse(JSON.stringify(v)));
function fakeSession(initial = {}) {
  const store = clone(initial);
  const listeners = [];
  return {
    store,
    area: {
      get: async k => { const out = {}; for (const x of (k == null ? Object.keys(store) : [].concat(k))) if (x in store) out[x] = clone(store[x]); return out; },
      set: async o => {
        const changes = {};
        for (const [k, v] of Object.entries(o)) { changes[k] = { oldValue: store[k], newValue: clone(v) }; store[k] = clone(v); }
        for (const l of listeners.slice()) l(clone(changes), 'session');
      },
    },
    onChanged: { addListener: l => listeners.push(l) },
  };
}
/* 保存の工程（job.js）を service worker と同じ並び（pdfbytes.js → job.js）で読み、popup() と同じ仮想時刻で走らせる。
   5 秒以上のタイマー（withTimeout の打ち切り・工程の上限）は発火させず longTimers に残す（fireLong で手で発火）。
   setInterval（延命）も発火させず intervals に残す。タブ 1 = 利用者のタブ（userUrl = 保存を押したページのまま）・タブ 7 = 背景タブ。
   J = 工程のオブジェクト（makeJob。旧 popup の大域 tab = {id: 1}・contentScriptOk = true に当たる）。記録は J の走行中の 1 件で始める。
   background.js の 2 つは差し替え: hiddenTab の呼び出しは registrations に、api は posts に残す */
function worker({ tabUrl = article, session = null } = {}) {
  const removed = [], registrations = [], posts = [], listeners = [], longTimers = [], intervals = [], badges = [], alarms = [], created = [];
  // 応答ヘッダの見張り: webRequest.onHeadersReceived の listener（{f, filter, extra}）・DNR のセッション規則（id → 規則）と呼び出し・tabs.update
  const wrListeners = [], dnrCalls = [], updated = [], rules = new Map();
  const order = []; // 背景タブを閉じた（remove:<id>）・規則を外した（unrule）・登録済みのタブを全部閉じた（closed）の順
  let now = 0, timeline = [[0, pdf, 'complete']], fired = 0, pings = 0, closedAll = 0, wrAdded = 0;
  const userTab = { url: tabUrl };
  /* timeline の 4 つ目に応答（webRequest の details の一部）があれば、タブの状態ではなく onHeadersReceived として配る
     （filter は見ずに全 listener へ = 絞り込みは工程の側の確かめを試す） */
  const fire = () => {
    while (fired < timeline.length && timeline[fired][0] <= now) {
      const [, url, status, wr] = timeline[fired++];
      if (wr) {
        for (const l of wrListeners.slice()) l.f(Object.assign({ tabId: 7, type: 'main_frame', frameId: 0, statusCode: 200, url }, wr));
        continue;
      }
      for (const l of listeners.slice()) l(7, { status }, { url, status });
    }
  };
  const state = () => {
    let s = timeline[0];
    for (const e of timeline) if (e[0] <= now && !e[3]) s = e;
    return { url: s[1], status: s[2] };
  };
  const ses = session || fakeSession({ openref_job: { id: 'J1', tabId: 1, tabUrl, startedAt: 0, state: 'running', line: '', cls: '' } });
  const sent = []; // chrome.tabs.sendMessage（documentId で文書に固定した送信）の [tabId, msg.type, options]
  const context = vm.createContext({ URL, URLSearchParams, Uint8Array, atob, btoa, TextDecoder, AbortController,
    Date: { now: () => now },
    navigator: { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36' },
    // 5 秒以上のタイマーは id（longTimers の位置 + 1）を返し、clearTimeout で cleared の印を付ける
    setTimeout: (f, ms) => { if (ms >= 5000) { longTimers.push({ ms, f, cleared: false }); return longTimers.length; } now += ms || 1; fire(); f(); return 0; },
    clearTimeout: id => { if (id && longTimers[id - 1]) longTimers[id - 1].cleared = true; },
    setInterval: (f, ms) => { intervals.push({ f, ms, on: true }); return intervals.length; },
    clearInterval: id => { if (intervals[id - 1]) intervals[id - 1].on = false; },
    chrome: {
      tabs: {
        create: async (o) => { created.push(o && o.url); fired = 0; fire(); return { id: 7 }; },
        // 目的の URL へ移す（about:blank で開いて見張りを付けてから）= 読込をやり直す
        update: async (id, o) => { updated.push([id, o && o.url]); fired = 0; fire(); return { id }; },
        get: async id => (id === 1 ? { id: 1, url: userTab.url, status: 'complete' } : state()),
        remove: async id => { removed.push(id); order.push('remove:' + id); },
        onUpdated: { addListener: l => listeners.push(l), removeListener: l => listeners.splice(listeners.indexOf(l), 1) },
        onRemoved: { addListener() {}, removeListener() {} },
        // 既定 = 文書がある（ページ内 fetch が PDF を返す）。docGone を真にすると文書が無い（lastError）
        sendMessage: (tabId, msg, opts, cb) => {
          if (typeof opts === 'function') { cb = opts; opts = undefined; }
          sent.push([tabId, msg.type, clone(opts)]);
          Promise.resolve().then(() => {
            if (context.docGone) { context.chrome.runtime.lastError = { message: 'Could not establish connection. Receiving end does not exist.' }; cb(undefined); context.chrome.runtime.lastError = undefined; }
            else cb({ ok: true, b64: btoa('%PDF-doc') });
          });
        },
      },
      storage: { session: ses.area },
      action: { setBadgeText: async o => badges.push(o.text), setBadgeBackgroundColor: async () => {} },
      runtime: { lastError: undefined, getPlatformInfo: async () => { pings++; return { os: 'mac' }; }, getURL: x => 'chrome-extension://ext/' + x },
      permissions: { contains: async () => true },
      alarms: { create: async (name, o) => { alarms.push(['create', name, o.when]); }, clear: async (name) => { alarms.push(['clear', name]); } },
      webRequest: { onHeadersReceived: {
        addListener: (f, filter, extra) => { wrAdded++; wrListeners.push({ f, filter: clone(filter), extra: clone(extra) }); },
        removeListener: f => { const i = wrListeners.findIndex(l => l.f === f); if (i >= 0) wrListeners.splice(i, 1); },
      } },
      // dnrFail = 規則の追加を断る文（上限・古い Chrome の「知らない鍵」の代わり）
      declarativeNetRequest: {
        updateSessionRules: async o => {
          dnrCalls.push(clone(o));
          if (context.dnrFail && (o.addRules || []).length) throw new Error(context.dnrFail);
          if (!(o.addRules || []).length && (o.removeRuleIds || []).length) order.push('unrule');
          for (const id of o.removeRuleIds || []) rules.delete(id);
          for (const r of o.addRules || []) rules.set(r.id, clone(r));
        },
        getSessionRules: async () => (context.dnrGet ? context.dnrGet() : [...rules.values()].map(clone)),
      },
    },
  });
  vm.runInContext(code('pdfbytes.js'), context);
  vm.runInContext(code('job.js'), context);
  context.fakeHidden = async (tabId, open) => { registrations.push({ tabId, open }); };
  context.fakeCloseAll = async () => { closedAll++; await null; await null; order.push('closed'); };
  context.fakeApi = async url => { posts.push(url); return {}; };
  vm.runInContext('var realApi = api; hiddenTab = fakeHidden; closeAllRegistered = fakeCloseAll; api = fakeApi;', context);
  vm.runInContext(`J = makeJob({ tabId: 1, tabUrl: ${JSON.stringify(tabUrl)}, page: {}, contentScriptOk: true }, false); J.id = 'J1';`, context);
  return {
    context, removed, registrations, posts, listeners, longTimers, intervals, badges, alarms, created, session: ses, userTab,
    wrListeners, dnrCalls, updated, rules, order, wrAdded: () => wrAdded,
    closedAll: () => closedAll, sent,
    now: () => now,
    advance: ms => { now += ms; },
    pings: () => pings,
    record: () => clone(ses.store.openref_job),
    timeline: t => { timeline = t; fired = 0; },
    fetcher: f => { context.fakeFetch = f; vm.runInContext('fetchPdfViaTabThenBg = (job, ...a) => fakeFetch(...a);', context); },
  };
}
const nav = (h, url = pdf) => vm.runInContext(`fetchPdfViaNavigation(J, ${JSON.stringify(url)}, 1, true)`, h.context);

test('HTML relay: the PDF tab moves on, the landing is fetched from the article tab, temporary tab closed', async () => {
  const h = worker();
  h.timeline([[0, pdf, 'complete'], [1500, landing, 'loading'], [2000, landing, 'complete']]);
  const fetched = [];
  h.fetcher(async (id, url) => { fetched.push(url); if (url !== landing) throw htmlError(); return { b64: btoa('%PDF-test'), how: 'タブ内' }; });
  const result = await nav(h);
  assert.equal(atob(result.b64), '%PDF-test');
  assert.match(result.how, /PDF タブ経由/);
  assert.equal(result.moved, true); // www.sciencedirect.com → pdf.sciencedirectassets.com
  same(fetched, [pdf, landing]);
  same(h.removed, [7]);
  same(h.registrations.map(x => x.open), [true, false]);
  assert.equal(h.listeners.length, 0);
});
test('PMC proof-of-work: the same URL reloads after the cookie is set, then the PDF is fetched once per load', async () => {
  const h = worker();
  h.timeline([[0, pmcPdf, 'complete'], [2000, pmcPdf, 'loading'], [2600, pmcPdf, 'complete']]);
  let calls = 0;
  h.fetcher(async (id, url) => {
    assert.equal(url, pmcPdf);
    calls++;
    if (h.now() < 2600) throw htmlError('PDF ではない応答（1817 bytes・HTML の可能性）');
    return { b64: btoa('%PDF-test'), how: 'タブ内' };
  });
  const result = await nav(h, pmcPdf);
  assert.equal(atob(result.b64), '%PDF-test');
  assert.equal(result.moved, false);
  assert.equal(calls, 2); // 中継ページの complete で 1 回・reload 後の complete で 1 回（毎秒は叩かない）
  same(h.removed, [7]);
});
test('a conclusive HTML page (login / purchase) is given up after the quiet period, fetched only once', async () => {
  const h = worker();
  let calls = 0;
  h.fetcher(async () => { calls++; throw htmlError(); });
  await assert.rejects(nav(h), /中継ページのまま/);
  assert.equal(calls, 1);
  assert.ok(h.now() <= 12000, 'quiet give-up at ' + h.now() + ' ms');
  same(h.removed, [7]);
});
test('a PDF tab that keeps reloading HTML stops at the overall deadline', async () => {
  const h = worker();
  const t = [];
  for (let ms = 0; ms < 40000; ms += 1000) t.push([ms, pdf, 'loading'], [ms + 500, pdf, 'complete']);
  h.timeline(t);
  h.fetcher(async () => { throw htmlError(); });
  await assert.rejects(nav(h), /PDF へ届きませんでした/);
  assert.ok(h.now() <= 21000, 'deadline at ' + h.now() + ' ms');
  same(h.removed, [7]);
});
test('non-HTML failure (HTTP 403) is not retried and the tab is closed', async () => {
  const h = worker();
  let calls = 0;
  h.fetcher(async () => { calls++; throw new Error('HTTP 403'); });
  await assert.rejects(nav(h), /^Error: HTTP 403$/);
  assert.equal(calls, 1);
  assert.ok(h.now() < 2000, 'gave up at ' + h.now() + ' ms');
  same(h.removed, [7]);
});
test('a non-web page in the PDF tab (error page / about:blank) is not fetched', async () => {
  const h = worker();
  h.timeline([[0, 'chrome-error://chromewebdata/', 'complete'], [1500, landing, 'loading'], [2000, landing, 'complete']]);
  const fetched = [];
  h.fetcher(async (id, url) => { fetched.push(url); return { b64: btoa('%PDF-test'), how: 'タブ内' }; });
  await nav(h);
  same(fetched, [landing]);
});
test('the PDF tab is fetched only after it finishes loading', async () => {
  const h = worker();
  h.timeline([[0, landing, 'loading'], [1600, landing, 'complete']]);
  h.fetcher(async (id, url, injected) => {
    assert.ok(h.now() >= 1600);
    assert.equal(id, 1);
    assert.equal(injected, true);
    return { b64: btoa('%PDF-test'), how: 'タブ内' };
  });
  await nav(h);
});

/* 配線: html の印が content / background → popup の各段を通って PDF タブ経路の引き金になる */
test('fetchPdfViaTabThenBg (real) marks the error html when either attempt saw HTML', async () => {
  for (const [tabHtml, bgHtml, want] of [[true, false, true], [false, true, true], [false, false, false]]) {
    const h = worker();
    h.context.answers = { tab: { ok: false, error: 'x', html: tabHtml }, bg: { ok: false, error: 'y', html: bgHtml } };
    vm.runInContext("sendToTab = async () => answers.tab; bgFetchPdf = async () => answers.bg;", h.context);
    const err = await vm.runInContext(`fetchPdfViaTabThenBg(J, 1, ${JSON.stringify(pdf)}, true).then(() => null, e => e)`, h.context);
    assert.equal(!!err.html, want);
  }
});
test('fetchPdfBytes (real) reports html when either attempt saw HTML', async () => {
  for (const [tabHtml, bgHtml, want] of [[true, false, true], [false, true, true], [false, false, false]]) {
    const h = worker();
    h.context.answers = { tab: { ok: false, error: 'x', html: tabHtml }, bg: { ok: false, error: 'y', html: bgHtml } };
    vm.runInContext("sendToTab = async () => answers.tab; bgFetchPdf = async () => answers.bg;", h.context);
    const got = await vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
    assert.equal(got.bytes, null);
    assert.equal(got.html, want);
  }
});
test('background fetch reports a non-PDF response with the html flag', async () => {
  let listener;
  const noop = { addListener() {} };
  const ctx = vm.createContext({
    URL, Uint8Array, TextDecoder, btoa,
    importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
    chrome: {
      runtime: { onMessage: { addListener: f => listener = f }, onConnect: noop, getContexts: async () => [] },
      alarms: { onAlarm: noop }, tabs: { onRemoved: noop },
      storage: { session: { get: async () => ({}), set: async () => {} } },
    },
    fetch: async () => ({ ok: true, url: pdf, arrayBuffer: async () => new TextEncoder().encode('<html>Preparing to download ...</html>').buffer }),
  });
  vm.runInContext(code('background.js'), ctx);
  const res = await ctx.bgFetchPdf(pdf);
  assert.equal(res.ok, false);
  assert.equal(res.html, true);
});

/* 論文ページでの保存（attachPdf） */
function articleSave(h, pdfSource, { html = true, sites = true, moved = false } = {}) {
  h.context.navCalls = 0;
  vm.runInContext(`
    J.page = { pdfUrl: ${JSON.stringify(pdf)}, pdfSource: ${JSON.stringify(pdfSource)} };
    J.allSites = ${sites};
    fetchPdfBytes = async () => ({ bytes: null, error: 'HTML response', html: ${html} });
    fetchPdfViaNavigation = async () => { navCalls++; return { b64: btoa('%PDF-test'), how: 'PDF タブ経由・タブ内', moved: ${moved} }; };
    api = async (url) => {
      fakeApi(url);
      if (url === '/capture/fetch-oa') return { source: 'pmc-aws' };
      if (!url.startsWith('/capture/attachment?')) throw new Error('unexpected route ' + url);
      return { status: 'attached', bytes: 9, doi_check: 'match' };
    };
  `, h.context);
  return vm.runInContext("attachPdf(J, 'KEY')", h.context);
}
for (const [source, posted] of [['sciencedirect', 'link'], ['meta', 'meta']]) test(`article page (${source}): HTML falls back to the PDF tab and attaches without an OA round trip`, async () => {
  const h = worker();
  const result = await articleSave(h, source);
  assert.match(result, /PDF 添付 ✓/);
  assert.match(result, /PDF タブ経由/);
  assert.equal(h.context.navCalls, 1);
  same(h.posts.map(u => new URLSearchParams(u.split('?')[1]).get('source')), [posted]);
});
test('article page: a PDF that ended up on another origin is posted for strict DOI matching', async () => {
  const h = worker();
  await articleSave(h, 'meta', { moved: true });
  assert.match(h.posts[0], /source=link/);
});
for (const [name, opts, source] of [
  ['a non-HTML failure', { html: false }, 'meta'],
  ['no all-sites permission', { sites: false }, 'meta'],
  ['a PDF link picked from the page (not declared)', {}, 'link'],
]) test(`article page: ${name} goes to the OA chain without opening a PDF tab`, async () => {
  const h = worker();
  const result = await articleSave(h, source, opts);
  assert.match(result, /取得元: pmc-aws/);
  assert.equal(h.context.navCalls, 0);
  same(h.posts, ['/capture/fetch-oa']);
});
test('DOI mismatch is surfaced and does not fall through to OA', async () => {
  const h = worker();
  vm.runInContext(`
    J.page = { pdfUrl: ${JSON.stringify(pdf)}, pdfSource: 'sciencedirect' };
    J.allSites = true;
    fetchPdfBytes = async () => ({ bytes: null, error: 'HTML response', html: true });
    fetchPdfViaNavigation = async () => ({ b64: btoa('%PDF-test'), how: 'PDF タブ経由', moved: true });
    api = async (url) => {
      if (!url.startsWith('/capture/attachment?')) throw new Error('unexpected OA route');
      const error = new Error('別論文'); error.status = 409; throw error;
    };
  `, h.context);
  const result = await vm.runInContext("attachPdf(J, 'KEY')", h.context);
  assert.match(result, /PDF は添付しませんでした.*別論文/);
});
test('ScienceDirect attachment keeps strict link DOI verification', async () => {
  const h = worker();
  await vm.runInContext(`postAttachment(J, 'KEY', new Uint8Array(), 'sciencedirect', 'page')`, h.context);
  assert.match(h.posts[0], /source=link/);
});

/* content.js のページ内 fetch */
function contentFetch(url, responses, origin = article) {
  let listener;
  const calls = [];
  runContent({
    window: {}, URL, location: new URL(origin), Uint8Array, btoa, TextDecoder,
    DOMParser: class { parseFromString() { return { querySelector: () => null, querySelectorAll: () => [] }; } },
    chrome: { runtime: { onMessage: { addListener: f => listener = f } } },
    fetch: async (u, opts) => {
      calls.push(opts.credentials);
      const r = responses[calls.length - 1];
      if (r instanceof Error) throw r;
      return { ok: true, url: u, arrayBuffer: async () => new TextEncoder().encode(r).buffer };
    },
  });
  return new Promise(resolve => listener({ type: 'openref-fetch-pdf', url }, {}, resolve)).then(result => ({ result, calls }));
}
test('same-origin PDF is fetched with cookies', async () => {
  const { result, calls } = await contentFetch(pdf, ['%PDF-test']);
  same(calls, ['include']);
  assert.equal(atob(result.b64), '%PDF-test');
});
test('cross-origin PDF rejected by CORS for credentials is retried without cookies (signed URL with ACAO *)', async () => {
  const { result, calls } = await contentFetch(landing, [new TypeError('Failed to fetch'), '%PDF-test']);
  same(calls, ['include', 'omit']);
  assert.equal(atob(result.b64), '%PDF-test');
});
test('same-origin network failure is not retried without cookies', async () => {
  const { result, calls } = await contentFetch(pdf, [new TypeError('Failed to fetch'), '%PDF-test']);
  same(calls, ['include']);
  assert.equal(result.ok, false);
});
test('HTML instead of PDF is reported with the html flag', async () => {
  const { result } = await contentFetch(pdf, ['<html>Preparing to download ...</html>']);
  assert.equal(result.ok, false);
  assert.equal(result.html, true);
});

test('isho PDF viewer URL supplies DOI even when PDF has no identifiers', () => {
  const h = popup();
  const url = 'https://webview.isho.jp/journal/detail/pdf/10.18885/CI.0000000000';
  const result = vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, h.context);
  assert.equal(result.doi, '10.18885/CI.0000000000');
  assert.equal(result.pdfUrl, url);
  assert.equal(result.pdfSource, 'self');
  assert.equal(result.isPdf, true);
  assert.equal(analyze(url, []).doi, result.doi);
  const fake = url.replace('isho.jp', 'isho.jp.example.org');
  assert.equal(vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(fake)})`, h.context).doi, null);
});
test('isho abstract identifies the article without pretending to be PDF', () => {
  const h = popup();
  const result = vm.runInContext("urlOnlyAnalyze('https://webview.isho.jp/journal/detail/abs/10.18885/CI.0000000000?from=test')", h.context);
  assert.equal(result.doi, '10.18885/CI.0000000000');
  assert.equal(result.pdfUrl, null);
});
test('PubMed extracts only the PMC link for this PMID', () => {
  const url = 'https://pubmed.ncbi.nlm.nih.gov/21620466/';
  assert.equal(analyze(url, [pmcLink.replace('21620466','12345678'), pmcLink]).pmcUrl, pmcLink);
  assert.equal(analyze(url, [pmcLink.replace('21620466','12345678')]).pmcUrl, null);
  assert.equal(analyze(url, [pmcLink.replace('.gov/', '.gov.example.org/')]).pmcUrl, null);
  assert.equal(analyze('https://pubmed.ncbi.nlm.nih.gov/?term=test', [pmcLink]).pmcUrl, null);
});
test('PMC links outside the full-text section are ignored', () => {
  assert.equal(analyze('https://pubmed.ncbi.nlm.nih.gov/21620466/', [pmcLink], []).pmcUrl, null);
});

/* 背景タブ経路（viaDoiTab）。pages = 着地ページの解析結果の列（1 回の読込ごとに 1 つ・最後を使い続ける）。
   reload(i) が真の読込では、解析の後に onUpdated で次の読込を起こす（JS の転送・ブラウザ確認の後の転送） */
function landingHarness(pages, { landingUrl = pmcPage, moves = () => false } = {}) {
  const h = worker();
  let load = 0;
  h.context.pages = pages;
  h.context.landingUrl = landingUrl;
  h.context.nextLoad = () => {
    const i = load++;
    return i;
  };
  h.context.afterAnalyze = (i) => {
    if (moves(i)) for (const l of h.listeners.slice()) l(7, { status: 'loading' }, {});
  };
  h.context.navigated = [];
  vm.runInContext(`
    chrome.scripting = { executeScript: async () => {} };
    let current = 0;
    waitForTabLoad = async () => { current = nextLoad(); return landingUrl; };
    sendToTab = async (msg) => {
      if (msg.type === 'openref-refresh-url') return { ok: true, url: null };
      const r = pages[Math.min(current, pages.length - 1)];
      afterAnalyze(current);
      return r;
    };
    fetchPdfViaTabThenBg = async () => ({ b64: btoa('%PDF-test'), how: 'タブ内' });
    postPdf = async (job, key, b64, how, source) => how + ' [' + source + ']';
  `, h.context);
  return h;
}
const page = (data) => ({ ok: true, data: Object.assign({ pmid: null, doi: null, pdfUrl: null, pdfSource: null }, data) });
const pmcReal = page({ pmid: '21620466', doi: '10.1016/S0140-6736(10)62307-0', pdfUrl: pmcPdf, pdfSource: 'meta' });
const blank = page({});
const via = (h, doi = "'10.1016/s0140-6736(10)62307-0'", route = pmcRoute) => vm.runInContext(`viaDoiTab(J, 'KEY', ${doi}, ${route})`, h.context);

test('PMC full text is followed, verified, fetched and the temporary tab cleaned', async () => {
  const h = landingHarness([pmcReal]);
  const result = await via(h);
  assert.match(result, /PMC 本文経由.*\[meta\]/);
  same(h.removed, [7]);
  assert.equal(h.listeners.length, 0);
});
for (const [name, data, landingUrl] of [
  ['a different PMID', { pmid: '12345678' }, pmcPage],
  ['a different DOI', { doi: '10.1000/other' }, pmcPage],
  ['a non-PMC host', {}, 'https://evil.example.org/articles/PMC3062508/'],
]) test(`PMC landing with ${name} cannot attach a PDF`, async () => {
  const h = landingHarness([page(Object.assign({}, pmcReal.data, data))], { landingUrl });
  await assert.rejects(via(h), /一致しません/);
  same(h.removed, [7]);
});
test('PMC route waits through a browser-check page for the site to move on', async () => {
  const h = landingHarness([blank, pmcReal], { moves: i => i === 0 });
  const result = await via(h);
  assert.match(result, /PMC 本文経由/);
  same(h.removed, [7]);
});
test('a page that navigates away during analysis (message port closed) is retried, not failed', async () => {
  const h = landingHarness([{ ok: false, error: 'The message port closed before a response was received.' }, pmcReal], { moves: i => i === 0 });
  const result = await via(h);
  assert.match(result, /PMC 本文経由/);
});
test('PMC route stuck on a browser-check page fails honestly within the wait budget and closes the tab', async () => {
  const h = landingHarness([blank]);
  await assert.rejects(via(h, 'null'), /識別子を確認できません.*確認ページ/);
  assert.ok(h.now() >= 7500 && h.now() <= 8500, 'gave up at ' + h.now() + ' ms'); // 遷移を待ってから諦める
  assert.equal(h.context.nextLoad(), 1); // 着地ページの読込は 1 回だけ（待たずに解析を繰り返さない）
  same(h.removed, [7]);
});
test('PMC route refuses an uninspectable landing instead of accepting a PDF by URL', async () => {
  const h = landingHarness([pmcReal], { landingUrl: 'https://example.org/wrong.pdf' });
  h.context.chrome.scripting.executeScript = async () => { throw new Error('cannot inject'); };
  await assert.rejects(via(h, 'null'), /識別子を確認できません/);
  same(h.removed, [7]);
});
for (const [name, route, data, moved, posted] of [
  ['PMC declared PDF behind proof-of-work', pmcRoute, pmcReal.data, false, 'meta'],
  ['DOI landing whose citation_pdf_url answers with an HTML relay', 'null', { doi: '10.1016/j.ctarc.2022.100563', pdfUrl: pdf, pdfSource: 'meta' }, true, 'link'],
  ['DOI landing with the article View PDF link', 'null', { doi: '10.1016/j.ctarc.2022.100563', pdfUrl: pdf, pdfSource: 'sciencedirect' }, true, 'link'],
]) test(name + ' is fetched through the PDF tab', async () => {
  const h = landingHarness([page(data)], { landingUrl: route === 'null' ? article : pmcPage });
  vm.runInContext(`
    fetchPdfViaTabThenBg = async () => { const e = new Error('PDF ではない応答'); e.html = true; throw e; };
    fetchPdfViaNavigation = async (job, url, ctx, injected) => { navigated.push([url, ctx, injected]); return { b64: btoa('%PDF-test'), how: 'PDF タブ経由・タブ内', moved: ${moved} }; };
  `, h.context);
  const result = await via(h, route === 'null' ? "'10.1016/j.ctarc.2022.100563'" : undefined, route);
  assert.match(result, new RegExp('PDF タブ経由.*\\[' + posted + '\\]'));
  same(h.context.navigated, [[data.pdfUrl, 7, true]]);
  same(h.removed, [7]);
});
test('a declared PDF failing with HTTP 403 does not open a PDF tab', async () => {
  const h = landingHarness([pmcReal]);
  vm.runInContext(`
    fetchPdfViaTabThenBg = async () => { throw new Error('HTTP 403'); };
    fetchPdfViaNavigation = async () => { navigated.push('opened'); return { b64: '' }; };
  `, h.context);
  await assert.rejects(via(h, 'null'), /403/);
  same(h.context.navigated, []);
});
test('PubMed save tries the PMC browser route before server OA or DOI routes', async () => {
  const h = worker();
  vm.runInContext(`
    J.page = {pmid:'21620466',pmcUrl:'${pmcLink}'};
    J.allSites = true;
    viaDoiTab = async (job, key, doi, pmc) => { if (pmc.pmid !== '21620466') throw new Error('wrong PMID'); return 'PMC success'; };
    api = async () => { throw new Error('unexpected OA request'); };
  `, h.context);
  assert.equal(await vm.runInContext("attachPdf(J, 'KEY')", h.context), 'PMC success');
});

/* meta refresh の規則（pdfbytes.js）: content.js（取得 HTML の追従・live document の中継判定）と background.js が同じ 1 つを使う */
const relayBase = 'https://linkinghub.elsevier.com/retrieve/pii/S0140673610623070';
const refreshCases = [
  // [名前, head に入れる HTML, 期待する飛び先（null = 追わない）]
  ['delay 0', '<meta http-equiv="refresh" content="0;url=https://example.org/a.pdf">', 'https://example.org/a.pdf'],
  ['delay 10 (upper bound)', '<meta http-equiv="refresh" content="10; url=/a.pdf">', 'https://linkinghub.elsevier.com/a.pdf'],
  ['delay 11 (session timer)', '<meta http-equiv="refresh" content="11; url=/a.pdf">', null],
  ['1800 s logout', '<meta http-equiv="refresh" content="1800;url=/logout">', null],
  ['inside noscript', '<noscript><meta http-equiv="refresh" content="0;url=/nojs"></noscript>', null],
  ['inside a comment', '<!-- <meta http-equiv="refresh" content="0;url=/old"> -->', null],
  ['relative URL with &amp;', '<meta http-equiv="refresh" content="2; url=../x/y.pdf?a=1&amp;b=2">', 'https://linkinghub.elsevier.com/retrieve/x/y.pdf?a=1&b=2'],
  ['single quotes', "<meta http-equiv='refresh' content='0; URL=/q.pdf'>", 'https://linkinghub.elsevier.com/q.pdf'],
  ['upper-case HTTP-EQUIV', '<META HTTP-EQUIV="REFRESH" CONTENT="1;URL=/u.pdf">', 'https://linkinghub.elsevier.com/u.pdf'],
  ['no url', '<meta http-equiv="refresh" content="5">', null],
  ['logout first, relay second', '<meta http-equiv="refresh" content="1800;url=/logout"><meta http-equiv="refresh" content="0;url=/r.pdf">', 'https://linkinghub.elsevier.com/r.pdf'],
  ['noscript first, relay outside', '<noscript><meta http-equiv="refresh" content="0;url=/nojs"></noscript><meta http-equiv="refresh" content="2;url=/r.pdf">', 'https://linkinghub.elsevier.com/r.pdf'],
];
const relayPage = html => '<html><head>' + html + '</head><body>Redirecting…</body></html>';
/* content 側の Document の代役: DOMParser（スクリプト無効）と同じく noscript 内の meta も要素として返し、closest で祖先を答える。
   コメントは要素にならない。属性は引用符付きだけ読む（本物の DOM との照合は実ブラウザで別に取った） */
function fakeDoc(html) {
  const metas = [];
  const re = /<!--[\s\S]*?-->|<\/?noscript\b[^>]*>|<meta\b[^>]*>/gi;
  let inNoscript = false, m;
  while ((m = re.exec(html))) {
    const tag = m[0];
    if (tag.startsWith('<!--')) continue;
    if (/^<noscript/i.test(tag)) { inNoscript = true; continue; }
    if (/^<\/noscript/i.test(tag)) { inNoscript = false; continue; }
    const attr = name => {
      const a = new RegExp('\\s' + name + '\\s*=\\s*(?:"([^"]*)"|\'([^\']*)\')', 'i').exec(tag);
      return a ? (a[1] ?? a[2]).replace(/&amp;/g, '&') : null;
    };
    if ((attr('http-equiv') || '').toLowerCase() !== 'refresh') continue;
    const ns = inNoscript;
    metas.push({ getAttribute: n => attr(n), closest: sel => (sel === 'noscript' && ns ? {} : null) });
  }
  const pick = sel => (sel === 'meta[http-equiv="refresh" i]' ? metas : []);
  return {
    querySelector: sel => pick(sel)[0] || null,
    querySelectorAll: pick,
  };
}
function shared() {
  const ctx = vm.createContext({ URL });
  vm.runInContext(code('pdfbytes.js'), ctx);
  return ctx.OpenRefShared;
}
for (const [name, html, want] of refreshCases) test(`meta refresh rule (string / Document): ${name}`, () => {
  const s = shared();
  assert.equal(s.metaRefreshUrl(relayPage(html), relayBase), want);
  assert.equal(s.metaRefreshUrlInDoc(fakeDoc(relayPage(html)), relayBase), want);
});
test('pdfbytes.js injected twice keeps the first definition (no redeclaration error)', () => {
  const ctx = vm.createContext({ URL });
  vm.runInContext(code('pdfbytes.js'), ctx);
  const first = ctx.OpenRefShared;
  vm.runInContext(code('pdfbytes.js'), ctx);
  assert.equal(ctx.OpenRefShared, first);
});
/* ページの id="OpenRefShared" / id="__openRefCaptureLoaded" の要素は Window の named property として isolated world からも見える */
const pageElement = () => ({ tagName: 'DIV', id: 'named-property' });
test('a page element named OpenRefShared does not suppress the shared definitions', () => {
  let listener;
  runContent({
    OpenRefShared: pageElement(), URL, window: {}, location: new URL('https://example.org/doi/10.1000/xyz'),
    document: { title: 'T', contentType: 'text/html', body: { innerText: '' }, querySelector: () => null, querySelectorAll: () => [] },
    chrome: { runtime: { onMessage: { addListener: f => listener = f } } },
  });
  let res;
  listener({ type: 'openref-analyze' }, {}, r => res = r);
  assert.equal(res.ok, true);
  assert.equal(res.data.doi, '10.1000/xyz');
});
test('a page element named __openRefCaptureLoaded does not stop content.js from listening; a real second injection does', () => {
  const listeners = [];
  const context = runContent({
    URL, window: { __openRefCaptureLoaded: pageElement() }, location: new URL('https://example.org/'),
    document: { title: 'T', contentType: 'text/html', body: { innerText: '' }, querySelector: () => null, querySelectorAll: () => [] },
    chrome: { runtime: { onMessage: { addListener: f => listeners.push(f) } } },
  });
  assert.equal(listeners.length, 1);
  vm.runInContext(code('pdfbytes.js'), context);
  vm.runInContext(code('content.js'), context);
  assert.equal(listeners.length, 1); // 二重注入は従来どおり 1 回だけ登録
});

/* 取得した HTML への 1 回追従: content.js（DOMParser）と background.js（文字列）が同じ飛び先を選ぶ */
const fakeFetch = (urls, body) => async u => {
  urls.push(u);
  return { ok: true, url: u, arrayBuffer: async () => new TextEncoder().encode(body(u)).buffer };
};
function contentFollow(url, body) {
  let listener;
  const urls = [];
  runContent({
    window: {}, URL, location: new URL(relayBase), Uint8Array, btoa, TextDecoder,
    DOMParser: class { parseFromString(html) { return fakeDoc(html); } },
    chrome: { runtime: { onMessage: { addListener: f => listener = f } } },
    fetch: fakeFetch(urls, body),
  });
  return new Promise(resolve => listener({ type: 'openref-fetch-pdf', url, follow: true }, {}, resolve)).then(result => ({ result, urls }));
}
function backgroundFollow(url, body) {
  let listener;
  const urls = [];
  const noop = { addListener() {} };
  const ctx = vm.createContext({
    URL, Uint8Array, TextDecoder, btoa,
    importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
    chrome: {
      runtime: { onMessage: { addListener: f => listener = f }, onConnect: noop, getContexts: async () => [] },
      alarms: { onAlarm: noop }, tabs: { onRemoved: noop },
      storage: { session: { get: async () => ({}), set: async () => {} } },
    },
    fetch: fakeFetch(urls, body),
  });
  vm.runInContext(code('background.js'), ctx);
  return ctx.bgFetchPdf(url, true).then(result => ({ result, urls }));
}
for (const [name, html, want] of refreshCases) test(`content and background follow the same meta refresh: ${name}`, async () => {
  for (const run of [contentFollow, backgroundFollow]) {
    const { result, urls } = await run(relayBase, u => (u === want ? '%PDF-test' : relayPage(html)));
    same(urls, want ? [relayBase, want] : [relayBase]);
    assert.equal(result.ok, !!want);
    if (!want) assert.equal(result.html, true); // 追わない = 「PDF タブで開いて取り直す」経路へ
  }
});

/* 背景タブの中継判定（openref-refresh-url）は live document に同じ規則を掛ける（セッション期限の refresh で空回りしない） */
for (const [name, html, want] of refreshCases) test(`live document relay check: ${name}`, () => {
  let listener;
  runContent({
    window: {}, URL, location: new URL(relayBase), document: fakeDoc(relayPage(html)),
    chrome: { runtime: { onMessage: { addListener: f => listener = f } } },
  });
  let res;
  listener({ type: 'openref-refresh-url' }, {}, r => res = r);
  assert.equal(res.ok, true);
  assert.equal(res.url, want);
});

/* 壊れた %（URIError）で解析が落ちない: popup の URL だけの解析（init の try の外）と content の analyze。
   decode できない = 「そこに DOI は無い」として次の手がかり（doi.org リンク・本文冒頭・題名のみ保存）へ進む */
const brokenPct = '%E0%A4%A';
test('safeDecode returns null for a broken % sequence and decodes a valid one', () => {
  const s = shared();
  assert.equal(s.safeDecode('10.1000/' + brokenPct), null);
  assert.equal(s.safeDecode('10.1000/a%2Fb%28c%29'), '10.1000/a/b(c)');
});
for (const [name, url, doi, isPdf] of [
  ['/doi/ path', 'https://example.org/doi/10.1000/' + brokenPct, null, false],
  ['/doi/pdf/ path (still a PDF tab, no DOI)', 'https://example.org/doi/pdf/10.1000/x' + brokenPct, null, true],
  ['isho PDF viewer (still a PDF tab, no DOI)', 'https://webview.isho.jp/journal/detail/pdf/10.18885/' + brokenPct, null, true],
  // 正しいエスケープは従来どおり decode する。URL の規則は pdfbytes.js で共有（cleanDoi を通す）: 末尾の ")" は "(" と釣り合うので DOI の一部
  ['valid escapes still decoded', 'https://example.org/doi/abs/10.1000/a%28b%29', '10.1000/a(b)', false],
]) test(`popup URL-only analysis survives a broken %: ${name}`, () => {
  const h = popup();
  const result = vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, h.context);
  assert.equal(result.doi, doi);
  assert.equal(result.isPdf, isPdf);
});
function analyzeWith(url, doiLink, bodyText = '') {
  let listener;
  runContent({
    URL, window: {}, location: new URL(url),
    document: {
      title: 'Article', contentType: 'text/html', body: { innerText: bodyText },
      querySelector: () => null,
      querySelectorAll: sel => (sel === 'a[href*="doi.org/10."]' && doiLink ? [{ href: doiLink }] : []),
    },
    chrome: { runtime: { onMessage: { addListener: f => listener = f } } },
  });
  let res;
  listener({ type: 'openref-analyze' }, {}, r => res = r);
  return res;
}
for (const [name, url, link, body, doi] of [
  ['/doi/ path → nothing else', 'https://example.org/doi/full/10.1000/' + brokenPct, null, '', null],
  ['isho abstract → nothing else', 'https://webview.isho.jp/journal/detail/abs/10.18885/' + brokenPct, null, '', null],
  ['a broken doi.org link → nothing else', 'https://example.org/article/1', 'https://doi.org/10.1000/' + brokenPct, '', null],
  ['/doi/ path → the doi.org link', 'https://example.org/doi/full/10.1000/' + brokenPct, 'https://doi.org/10.1000/good.1', '', '10.1000/good.1'],
  ['/doi/ path → the body text', 'https://example.org/doi/full/10.1000/' + brokenPct, null, 'Cite as doi:10.1000/body.2 (2026)', '10.1000/body.2'],
  ['a broken doi.org link → the body text', 'https://example.org/article/1', 'https://doi.org/10.1000/' + brokenPct, 'https://doi.org/10.1000/body.3', '10.1000/body.3'],
]) test(`content analyze stays ok with a broken % and falls through: ${name}`, () => {
  const res = analyzeWith(url, link, body);
  assert.equal(res.ok, true); // ok:false だと popup は contentScriptOk=false → HTML ページを「PDF タブ」として取り込もうとする
  assert.equal(res.data.doi, doi);
  assert.equal(res.data.title, 'Article'); // 識別子が無くても題名のみの保存に進める
});

/* ===== 識別の物差し: よくある出版社の型ごとの fixture を、本物の analyze()（content.js）と
   urlOnlyAnalyze（popup.js）にかける。fixture は出版社ページの丸写しではなく、<head> のメタとリンクだけを手で書いた物。
   expect = 正しい答え。今のコードが外す物は gap: "<理由>" を付け、「今は expect と違う」ことを確かめる
   （直れば試験が落ちる → そのときに gap の印を外す）。urlOnly = Chrome の PDF ビューア（content script を入れられない） */
const C0 = [
  { name: 'Springer: Jpn J Radiol の記事ページ', url: 'https://link.springer.com/article/10.1007/s11604-025-01811-9',
    meta: [['citation_title', 'CT findings of ...'], ['citation_doi', '10.1007/s11604-025-01811-9'], ['dc.identifier', 'doi:10.1007/s11604-025-01811-9'],
      ['citation_pdf_url', 'https://link.springer.com/content/pdf/10.1007/s11604-025-01811-9.pdf'],
      ['citation_reference', 'citation_journal_title=Radiology; citation_title=Ref A; citation_doi=10.1148/radiol.2020201234']],
    links: ['https://doi.org/10.1148/radiol.2020201234', 'https://link.springer.com/content/pdf/10.1007/s11604-025-01811-9.pdf'],
    expect: { doi: '10.1007/s11604-025-01811-9', pmid: null, pdfSource: 'meta' } },
  { name: 'Springer: Skeletal Radiol の記事ページ', url: 'https://link.springer.com/article/10.1007/s00256-025-04912-3',
    meta: [['citation_title', 'MRI of ...'], ['citation_doi', '10.1007/s00256-025-04912-3'],
      ['citation_pdf_url', 'https://link.springer.com/content/pdf/10.1007/s00256-025-04912-3.pdf']],
    links: ['https://doi.org/10.2214/AJR.19.21234', 'https://link.springer.com/content/pdf/10.1007/s00256-024-04000-1.pdf'],
    expect: { doi: '10.1007/s00256-025-04912-3', pmid: null, pdfSource: 'meta' } },
  { name: 'Springer: Abdom Radiol の記事ページ', url: 'https://link.springer.com/article/10.1007/s00261-025-05001-7',
    meta: [['citation_title', 'Pancreatic ...'], ['citation_doi', '10.1007/s00261-025-05001-7'], ['prism.doi', '10.1007/s00261-025-05001-7'],
      ['citation_pdf_url', 'https://link.springer.com/content/pdf/10.1007/s00261-025-05001-7.pdf']],
    links: [], expect: { doi: '10.1007/s00261-025-05001-7', pmid: null, pdfSource: 'meta' } },
  { name: 'Springer: PDF タブ（URL のみ）', urlOnly: true, url: 'https://link.springer.com/content/pdf/10.1007/s00256-025-04912-3.pdf',
    expect: { doi: '10.1007/s00256-025-04912-3', pmid: null, pdfSource: 'self' },
    gap: 'URL の /content/pdf/<DOI>.pdf を DOI として読まない（/capture/pdf の本文抽出で補われる）' },
  { name: 'RSNA: Radiology の記事ページ（Atypon・dc.Identifier の先頭が publisher-id）', url: 'https://pubs.rsna.org/doi/10.1148/radiol.251234',
    meta: [['dc.Title', 'Deep learning ...'], ['dc.Identifier', 'radiol.251234'], ['dc.Identifier', '10.1148/radiol.251234']],
    links: ['https://pubs.rsna.org/doi/epdf/10.1148/radiol.251234', 'https://doi.org/10.1148/radiol.2019190001'],
    expect: { doi: '10.1148/radiol.251234', pmid: null, pdfSource: 'atypon' } },
  { name: 'RSNA: RadioGraphics の本文（/doi/full/）', url: 'https://pubs.rsna.org/doi/full/10.1148/rg.250045',
    meta: [['dc.Title', 'Imaging of ...']], links: ['https://doi.org/10.1148/rg.2021200150'],
    expect: { doi: '10.1148/rg.250045', pmid: null, pdfSource: 'atypon' } },
  { name: 'RSNA: PDF リーダー（/doi/epdf/・URL のみ）', urlOnly: true, url: 'https://pubs.rsna.org/doi/epdf/10.1148/radiol.251234',
    expect: { doi: '10.1148/radiol.251234', pmid: null, pdfSource: 'self' } },
  { name: 'AJR の記事ページ（Atypon）', url: 'https://www.ajronline.org/doi/10.2214/AJR.25.33012',
    meta: [['dc.Title', 'Ultrasound ...'], ['dc.Identifier', '10.2214/AJR.25.33012']], links: ['https://doi.org/10.1148/radiol.2020201234'],
    expect: { doi: '10.2214/AJR.25.33012', pmid: null, pdfSource: 'atypon' } },
  { name: 'AJNR の記事ページ（citation_doi・citation_pmid）', url: 'https://www.ajnr.org/content/46/3/512',
    meta: [['citation_title', 'Diffusion ...'], ['citation_doi', '10.3174/ajnr.A8512'], ['citation_pmid', '39876543'],
      ['citation_pdf_url', 'https://www.ajnr.org/content/ajnr/46/3/512.full.pdf']],
    links: [], expect: { doi: '10.3174/ajnr.A8512', pmid: '39876543', pdfSource: 'meta' } },
  // J-STAGE の記事ページの型（メタを縮めた物）。参考文献は citation_reference（DOI なし）で、自分の DOI は doi.org のリンクにもある
  { name: 'J-STAGE の記事ページ', url: 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_article/-char/ja',
    meta: [['citation_title', '小児の架空症例の画像所見'], ['citation_doi', '10.20844/zzzz.42.1_5'],
      ['citation_pdf_url', 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_pdf'], ['citation_issn', '1234-5678'], ['citation_issn', '8765-4321'],
      ['citation_reference', 'citation_author=A Sample; citation_title=An example reference title; citation_publication_date=1998'],
      ['citation_reference', 'citation_author=B Example; citation_title=An example reference title: a second part; citation_publication_date=2020'],
      ['citation_reference', 'citation_author=山田 太郎; citation_title=架空の参考文献の例示の題名; citation_publication_date=2021'],
      ['dc.identifier', '10.20844/zzzz.42.1_5']],
    links: ['https://doi.org/10.20844/zzzz.42.1_5'],
    expect: { doi: '10.20844/zzzz.42.1_5', pmid: null, pdfSource: 'meta' } },
  { name: 'J-STAGE の PDF タブ（URL のみ・DOI は記事ページにだけある）', urlOnly: true,
    url: 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_pdf/-char/ja',
    landing: [['citation_title', '小児の架空症例の画像所見'], ['citation_doi', '10.20844/zzzz.42.1_5'],
      ['citation_pdf_url', 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_pdf']],
    expect: { doi: '10.20844/zzzz.42.1_5', pmid: null, pdfSource: 'self' } },
  { name: '医書.jp の PDF ビューア（URL のみ）', urlOnly: true, url: 'https://webview.isho.jp/journal/detail/pdf/10.18885/CI.0000000000',
    expect: { doi: '10.18885/CI.0000000000', pmid: null, pdfSource: 'self' } },
  { name: '医書.jp の抄録ページ', url: 'https://webview.isho.jp/journal/detail/abs/10.18885/CI.0000000000',
    meta: [['citation_title', '架空の題名：頭部画像の正常変異の例']], links: [],
    expect: { doi: '10.18885/CI.0000000000', pmid: null, pdfSource: null } },
  { name: 'PubMed の論文ページ', url: 'https://pubmed.ncbi.nlm.nih.gov/21620466/',
    meta: [['citation_title', 'Effect of ...'], ['citation_doi', '10.1016/S0140-6736(10)62307-0']],
    links: ['https://doi.org/10.1056/NEJMoa0800000', pmcLink], fullTextLinks: [pmcLink],
    expect: { doi: '10.1016/S0140-6736(10)62307-0', pmid: '21620466', pdfSource: null } },
  { name: 'PMC の論文ページ', url: pmcPage,
    meta: [['citation_title', 'Effect of ...'], ['citation_doi', '10.1016/S0140-6736(10)62307-0'], ['citation_pmid', '21620466'], ['citation_pdf_url', pmcPdf]],
    links: ['https://doi.org/10.1056/NEJMoa0800000', pmcPdf],
    expect: { doi: '10.1016/S0140-6736(10)62307-0', pmid: '21620466', pdfSource: 'meta' } },
  // ScienceDirect の論文ページの型（head を縮めた物）: citation_doi と dc.identifier がある・
  // citation_pdf_url は無い・doi.org リンクは自分の DOI が先頭で、参考文献の DOI が続く（異なる DOI が 15）
  { name: 'ScienceDirect の論文ページ', url: article,
    meta: [['citation_pii', 'S0000000000000001'], ['citation_title', 'Case report ...'], ['citation_journal_title', 'Radiology Case Reports'],
      ['citation_issn', '1930-0433'], ['citation_volume', '21'], ['citation_issue', '1'], ['citation_firstpage', '101'], ['citation_lastpage', '106'],
      ['citation_publication_date', '2026/01/01'], ['citation_doi', '10.1016/j.radcr.9999.99.999'], ['dc.identifier', '10.1016/j.radcr.9999.99.999']],
    links: ['https://doi.org/10.1016/j.radcr.9999.99.999', article.replace('0001', '9999') + '/pdfft?pid=other.pdf', pdf,
      'https://doi.org/10.1000/ref.0001', 'https://doi.org/10.1000/ref.0002', 'https://doi.org/10.1000/ref.0003'],
    expect: { doi: '10.1016/j.radcr.9999.99.999', pmid: null, pdfSource: 'sciencedirect' } },
  // Radiopaedia の記事の型: head は dc.identifier だけ（citation_* なし）・doi.org リンクは自分の DOI が先頭で参考文献が続く（異なる DOI が 17）
  { name: 'Radiopaedia の記事（dc.identifier）', url: 'https://radiopaedia.org/articles/example-article-1?lang=us',
    meta: [['dc.identifier', '10.53347/rID-0000000']],
    links: ['https://doi.org/10.53347/rID-0000000', 'https://doi.org/10.1000/ref.0004', 'https://doi.org/10.1000/ref.0005', 'https://doi.org/10.1000/ref.0006'],
    expect: { doi: '10.53347/rID-0000000', pmid: null, pdfSource: null } },
  // Ovid の抄録の型: head は dc.identifier=doi:…（前置き付き）だけ・doi.org リンクなし。URL の末尾の題名は縮めた
  { name: 'Ovid（LWW・JCAT）の抄録（dc.identifier に doi: の前置き）', url: 'https://www.ovid.com/jnls/jcat/abstract/10.1097/rct.0000000000000000~an-example-title',
    meta: [['dc.identifier', 'doi:10.1097/RCT.0000000000000000']], links: [],
    expect: { doi: '10.1097/RCT.0000000000000000', pmid: null, pdfSource: null } },
  { name: '学会抄録（citation_title だけ）', url: 'https://archive.ismrm.org/2025/1234.html',
    meta: [['citation_title', 'Accelerated ...']], links: [],
    expect: { doi: null, pmid: null, pdfSource: null } },
  { name: '雑誌の目次（.pdf リンク多数・citation_doi なし）', url: 'https://www.ajnr.org/content/46/3',
    meta: [['og:title', 'Table of Contents']],
    links: ['https://www.ajnr.org/content/ajnr/46/3/505.full.pdf', 'https://www.ajnr.org/content/ajnr/46/3/512.full.pdf'],
    // link = ページ上のリンク拾い（popup は識別子の無いページでは本体とみなさず添付しない）
    expect: { doi: null, pmid: null, pdfSource: 'link' } },
  // J-STAGE の目次の型: 各記事の DOI が doi.org のリンクで並ぶ（14 本）・PDF は _pdf/-char/ja
  { name: 'J-STAGE の目次（記事ごとの doi.org リンク）', url: 'https://www.jstage.jst.go.jp/browse/zzzz/42/1/_contents/-char/ja',
    meta: [], links: ['https://doi.org/10.20844/zzzz.42.1_1', 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_1/_pdf/-char/ja',
      'https://doi.org/10.20844/zzzz.42.1_2', 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_2/_pdf/-char/ja'],
    expect: { doi: null, pmid: null, pdfSource: null } },
  { name: '参考文献欄にだけ doi.org リンクがあるページ（自分の DOI なし）', url: 'https://www.example-society.jp/guideline/ct-2025.html',
    meta: [['og:title', 'CT 撮影ガイドライン 2025']],
    links: ['https://doi.org/10.1148/radiol.2020201234', 'https://doi.org/10.2214/AJR.19.21234', 'https://doi.org/10.1007/s11604-024-01500-9'],
    expect: { doi: null, pmid: null, pdfSource: null } },
  // 確度の低い 2 規則（doi.org リンク・本文冒頭）は、異なる DOI がちょうど 1 つのときだけ採る
  { name: 'メタの無い記事ページ: doi.org リンクは自分の DOI だけ（同じ DOI を大小・dx 違いで 2 回）', url: 'https://www.example-journal.org/article/123',
    meta: [], links: ['https://doi.org/10.1234/EJ.2025.123', 'https://www.example-journal.org/about', 'https://dx.doi.org/10.1234/ej.2025.123'],
    expect: { doi: '10.1234/EJ.2025.123', pmid: null, pdfSource: null } },
  { name: 'メタの無い記事ページ: 自分の doi.org リンク + 参考文献の doi.org リンク', url: 'https://www.example-journal.org/article/123',
    meta: [], links: ['https://doi.org/10.1234/ej.2025.123', 'https://doi.org/10.1148/radiol.2020201234', 'https://doi.org/10.2214/AJR.19.21234'],
    expect: { doi: null, pmid: null, pdfSource: null } },
  { name: '本文冒頭に DOI が 1 つだけ（同じ DOI を 2 回）', url: 'https://www.example-journal.org/article/124',
    meta: [], links: [], body: 'Original article. doi: 10.1234/ej.2025.124. Cite as https://doi.org/10.1234/ej.2025.124',
    expect: { doi: '10.1234/ej.2025.124', pmid: null, pdfSource: null } },
  { name: '本文冒頭に自分の DOI と参考文献の DOI', url: 'https://www.example-journal.org/article/124',
    meta: [], links: [], body: 'doi: 10.1234/ej.2025.124\nReferences 1. Radiology 2020. doi:10.1148/radiol.2020201234 2. AJR 2019. doi:10.2214/AJR.19.21234',
    expect: { doi: null, pmid: null, pdfSource: null } },
];
/* content.js の stub: meta は名前を大小無視で引く（同名の複数可・querySelector は最初の 1 つ・querySelectorAll は全部）・
   doi.org リンクは全部（DOM の順）・JSON-LD は f.jsonld（script の本文の列）・知らない selector は落とす */
const META_SEL = /^meta\[name="([^"]+)" i\](?:, meta\[property="\1" i\])?$/;
function fixtureDoc(f) {
  const metas = (f.meta || []).map(([name, content]) => ({ name, content, getAttribute: k => (k === 'content' ? content : null) }));
  const links = (f.links || []).map(href => ({ href }));
  const named = sel => metas.filter(x => x.name.toLowerCase() === META_SEL.exec(sel)[1].toLowerCase());
  return {
    title: f.title || '', contentType: f.contentType || 'text/html', body: { innerText: f.body || '' },
    querySelector(sel) {
      if (META_SEL.test(sel)) return named(sel)[0] || null;
      throw new Error('stub にない selector: ' + sel);
    },
    querySelectorAll(sel) {
      if (META_SEL.test(sel)) return named(sel);
      if (sel === 'script[type="application/ld+json"]') return (f.jsonld || []).map(textContent => ({ textContent }));
      if (sel === 'a[href]') return links;
      if (sel === 'a[href*="doi.org/10."]') return links.filter(a => a.href.includes('doi.org/10.'));
      if (sel === '.full-text-links-list a[href]') return (f.fullTextLinks || []).map(href => ({ href }));
      throw new Error('stub にない selector: ' + sel);
    },
  };
}
/* popup の DOMParser と記事ページの fetch の代役: 記事ページは <meta name content> だけの HTML。
   pages = {URL: メタの列 | {url: 転送後の URL, meta} | Error（fetch が投げる）}・無い URL は HTTP 404。取った URL と credentials を fetched に残す */
const metaHtml = metas => '<html><head>' + metas.map(([n, c]) => `<meta name="${n}" content="${c}">`).join('') + '</head></html>';
class MetaParser {
  parseFromString(html) { return fixtureDoc({ meta: [...html.matchAll(/<meta name="([^"]*)" content="([^"]*)">/g)].map(m => [m[1], m[2]]) }); }
}
const landingFetch = (pages, fetched = []) => async (url, opts) => {
  fetched.push([url, opts && opts.credentials]);
  const p = pages[url];
  if (p instanceof Error) throw p;
  if (!p) return { ok: false, status: 404, url, text: async () => '' };
  if (p.meta) return { ok: true, status: 200, url: p.url, text: async () => metaHtml(p.meta) };
  return { ok: true, status: 200, url, text: async () => metaHtml(p) };
};
/* init と同じ順: 解析（content script か URL だけ）→ fillFromJstageArticle（J-STAGE の PDF タブなら記事ページの DOI） */
async function identify(f) {
  const h = popup();
  h.context.DOMParser = MetaParser;
  h.context.fetch = landingFetch(f.landing ? { [shared().jstageArticleUrl(f.url)]: f.landing } : {});
  let r;
  if (f.urlOnly) {
    r = vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(f.url)})`, h.context);
  } else {
    let listener;
    runContent({ URL, window: {}, location: new URL(f.url), document: fixtureDoc(f),
      chrome: { runtime: { onMessage: { addListener: l => listener = l } } } });
    let res;
    listener({ type: 'openref-analyze' }, {}, x => res = x);
    assert.equal(res.ok, true, f.name + ': ' + res.error);
    r = res.data;
  }
  await h.context.fillFromJstageArticle(r);
  return { doi: r.doi, pmid: r.pmid, pdfSource: r.pdfSource };
}
for (const f of C0) test('baseline: ' + f.name, async () => {
  const got = await identify(f);
  if (f.gap) assert.notDeepEqual(got, f.expect, 'gap が解消した → gap の印を外す（' + f.gap + '）');
  else assert.deepEqual(got, f.expect);
});
test('baseline summary', async () => {
  let ok = 0;
  for (const f of C0) if (JSON.stringify(await identify(f)) === JSON.stringify(f.expect)) ok++;
  const gaps = C0.filter(f => f.gap);
  assert.equal(ok + gaps.length, C0.length);
  console.log(`capture baseline: 正しく識別 ${ok} / ${C0.length}（既知の欠け ${gaps.length}: ${gaps.map(f => f.name).join('・')}）`);
});

/* ===== citation_pdf_url の相対 URL はページを基準に絶対 URL へ。http(s) でない値は宣言として採らない ===== */
function analyzePdf(f) {
  let listener;
  const doc = Object.assign(fixtureDoc(f), f.baseURI ? { baseURI: f.baseURI } : {});
  runContent({ URL, window: {}, location: new URL(f.url), document: doc,
    chrome: { runtime: { onMessage: { addListener: l => listener = l } } } });
  let res;
  listener({ type: 'openref-analyze' }, {}, x => res = x);
  assert.equal(res.ok, true, res.error);
  return { pdfUrl: res.data.pdfUrl, pdfSource: res.data.pdfSource };
}
const c11Page = 'https://pub.example.org/t8/article';
for (const [name, f, expect] of [
  ['a root-relative value is resolved against the page', { url: c11Page, meta: [['citation_pdf_url', '/t8/paper.pdf']] },
    { pdfUrl: 'https://pub.example.org/t8/paper.pdf', pdfSource: 'meta' }],
  ['a path-relative value is resolved against the page', { url: c11Page + '?x=1', meta: [['citation_pdf_url', 'paper.pdf']] },
    { pdfUrl: 'https://pub.example.org/t8/paper.pdf', pdfSource: 'meta' }],
  ['a protocol-relative value takes the page\'s scheme', { url: c11Page, meta: [['citation_pdf_url', '//cdn.example.org/p.pdf']] },
    { pdfUrl: 'https://cdn.example.org/p.pdf', pdfSource: 'meta' }],
  ['the document base (<base href>) wins over the page URL, like a.href', { url: c11Page, baseURI: 'https://files.example.org/v2/', meta: [['citation_pdf_url', 'paper.pdf']] },
    { pdfUrl: 'https://files.example.org/v2/paper.pdf', pdfSource: 'meta' }],
  ['an absolute value is kept as written', { url: c11Page, meta: [['citation_pdf_url', 'https://cdn.example.org/a/b.pdf?token=x%2Fy']] },
    { pdfUrl: 'https://cdn.example.org/a/b.pdf?token=x%2Fy', pdfSource: 'meta' }],
  ['javascript: is not a declaration (nothing else on the page)', { url: c11Page, meta: [['citation_pdf_url', 'javascript:alert(1)']] },
    { pdfUrl: null, pdfSource: null }],
  ['data: is not a declaration; the page link is used instead', { url: c11Page, meta: [['citation_pdf_url', 'data:application/pdf;base64,JVBERi0=']], links: ['https://pub.example.org/t8/real.pdf'] },
    { pdfUrl: 'https://pub.example.org/t8/real.pdf', pdfSource: 'link' }],
  ['an unparsable value is not a declaration; Atypon is used instead', { url: 'https://pub.example.org/doi/10.1000/x1', meta: [['citation_pdf_url', 'http://']] },
    { pdfUrl: 'https://pub.example.org/doi/pdf/10.1000/x1?download=true', pdfSource: 'atypon' }],
  ['a non-http(s) declaration on a PDF tab falls back to the tab itself', { url: 'https://pub.example.org/t8/paper.pdf', contentType: 'application/pdf', meta: [['citation_pdf_url', 'ftp://pub.example.org/p.pdf']] },
    { pdfUrl: 'https://pub.example.org/t8/paper.pdf', pdfSource: 'self' }],
  ['a non-http(s) .pdf link is skipped for the next one', { url: c11Page, meta: [], links: ['data:text/plain,x.pdf', 'https://pub.example.org/t8/real.pdf'] },
    { pdfUrl: 'https://pub.example.org/t8/real.pdf', pdfSource: 'link' }],
  ['an absolute http: value is kept as written (http is not dropped)', { url: c11Page, meta: [['citation_pdf_url', 'http://cdn.example.org/a/b.pdf']] },
    { pdfUrl: 'http://cdn.example.org/a/b.pdf', pdfSource: 'meta' }],
  // 空白だけの値は new URL が基準（= ページ自身）を返す → HTML が「PDF タブ」扱いになっていた
  ['a value of spaces only is not a declaration (not the page itself)', { url: c11Page, meta: [['citation_pdf_url', '   ']] },
    { pdfUrl: null, pdfSource: null }],
  ['a value of tabs / newlines only is not a declaration', { url: c11Page, meta: [['citation_pdf_url', '\t\n \r\n']] },
    { pdfUrl: null, pdfSource: null }],
  ['spaces only does not block the Atypon fall-through', { url: 'https://pub.example.org/doi/10.1000/x1', meta: [['citation_pdf_url', '  ']] },
    { pdfUrl: 'https://pub.example.org/doi/pdf/10.1000/x1?download=true', pdfSource: 'atypon' }],
  ['spaces only does not block the link fall-through', { url: c11Page, meta: [['citation_pdf_url', ' \t ']], links: ['https://pub.example.org/t8/real.pdf'] },
    { pdfUrl: 'https://pub.example.org/t8/real.pdf', pdfSource: 'link' }],
  ['a fragment-only value points at the page itself: not a declaration', { url: c11Page, meta: [['citation_pdf_url', '#pdf']] },
    { pdfUrl: null, pdfSource: null }],
  ['an empty query "?" points at the page itself: not a declaration', { url: c11Page, meta: [['citation_pdf_url', '?']] },
    { pdfUrl: null, pdfSource: null }],
  ['the page\'s own path (relative) is not a declaration', { url: c11Page + '#top', meta: [['citation_pdf_url', 'article']] },
    { pdfUrl: null, pdfSource: null }],
  ['the page\'s own absolute URL is not a declaration; a link is used instead', { url: c11Page, meta: [['citation_pdf_url', c11Page]], links: ['https://pub.example.org/t8/real.pdf'] },
    { pdfUrl: 'https://pub.example.org/t8/real.pdf', pdfSource: 'link' }],
  ['the same path with another query is a different page: kept', { url: c11Page, meta: [['citation_pdf_url', '?format=pdf']] },
    { pdfUrl: c11Page + '?format=pdf', pdfSource: 'meta' }],
  // タブ自体が PDF で、meta が自分を指す: 今までどおり meta（self の枝に落とさない = popup の source も URL も同じ）
  ['a PDF tab whose declaration points at itself keeps it as meta (unchanged)', { url: 'https://pub.example.org/t8/paper.pdf', contentType: 'application/pdf', meta: [['citation_pdf_url', 'https://pub.example.org/t8/paper.pdf']] },
    { pdfUrl: 'https://pub.example.org/t8/paper.pdf', pdfSource: 'meta' }],
  ['a PDF tab whose declaration is spaces only falls back to the tab itself', { url: 'https://pub.example.org/t8/paper.pdf', contentType: 'application/pdf', meta: [['citation_pdf_url', '  ']] },
    { pdfUrl: 'https://pub.example.org/t8/paper.pdf', pdfSource: 'self' }],
]) test('relative citation_pdf_url: ' + name, () => {
  assert.deepEqual(analyzePdf(f), expect);
});

/* ===== J-STAGE の PDF タブ → 記事ページの DOI（popup が記事ページを 1 回だけ引く） ===== */
const jsPdf = 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_pdf';
const jsArticle = 'https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_article';
// J-STAGE の記事ページの型: citation_pdf_url は -char の無い …/_pdf
const jsLanding = [['citation_title', '小児の架空症例の画像所見'], ['citation_doi', '10.20844/zzzz.42.1_5'], ['citation_pdf_url', jsPdf]];
const urlOnly = url => vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, popup().context);
for (const [name, url, landing] of [
  ['-char/ja', jsPdf + '/-char/ja', jsArticle + '/-char/ja'],
  ['-char なし', jsPdf, jsArticle],
  ['-char/en とクエリ', jsPdf + '/-char/en?download=1', jsArticle + '/-char/en'],
  ['末尾の /', jsPdf + '/', jsArticle],
  ['断片', jsPdf + '/-char/ja#page=2', jsArticle + '/-char/ja'],
]) test(`J-STAGE PDF URL is a PDF tab with its article page: ${name}`, () => {
  const r = urlOnly(url);
  same([r.landingUrl, r.pdfUrl, r.pdfSource, r.isPdf, r.doi], [landing, url, 'self', true, null]);
  assert.equal(shared().jstageArticleUrl(url), landing);
});
for (const [name, url] of [
  ['the article page (_article)', jsArticle + '/-char/ja'],
  ['a lookalike host', jsPdf.replace('jst.go.jp', 'jst.go.jp.example.org') + '/-char/ja'],
  ['another host with the same path', jsPdf.replace('www.jstage.jst.go.jp', 'example.org')],
  ['http', jsPdf.replace('https:', 'http:')],
  ['an extra path segment after _pdf', jsPdf + '/-char/ja/extra'],
  ['a path with too few segments', 'https://www.jstage.jst.go.jp/article/zzzz/42/1/_pdf'],
  ['the table of contents', 'https://www.jstage.jst.go.jp/browse/zzzz/42/1/_contents/-char/ja'],
]) test(`not a J-STAGE PDF URL: ${name}`, () => {
  const r = urlOnly(url);
  same([r.landingUrl, r.pdfSource, r.isPdf], [null, null, false]);
});
test('content.js (if it can be injected into the PDF) reports the same article page', () => {
  let listener;
  runContent({ URL, window: {}, location: new URL(jsPdf + '/-char/ja'), document: fixtureDoc({ contentType: 'application/pdf' }),
    chrome: { runtime: { onMessage: { addListener: l => listener = l } } } });
  let res;
  listener({ type: 'openref-analyze' }, {}, x => res = x);
  same([res.data.landingUrl, res.data.pdfSource, res.data.doi], [jsArticle + '/-char/ja', 'self', null]);
});

async function fillJstage(url, pages, page) {
  const h = popup();
  const fetched = [];
  h.context.DOMParser = MetaParser;
  h.context.fetch = landingFetch(pages, fetched);
  const p = page || vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, h.context);
  const note = await h.context.fillFromJstageArticle(p);
  return { p, note, fetched };
}
for (const [name, tabUrl, declared] of [
  ['tab -char/ja, page …/_pdf', jsPdf + '/-char/ja', jsPdf],
  ['tab with a query', jsPdf + '/-char/ja?download=1', jsPdf],
  ['page declares -char/ja too', jsPdf + '/-char/ja', jsPdf + '/-char/ja'],
  ['relative citation_pdf_url', jsPdf + '/-char/ja', '/article/zzzz/42/1/42_5/_pdf'],
]) test(`J-STAGE PDF tab takes the DOI and title from its article page: ${name}`, async () => {
  const landing = shared().jstageArticleUrl(tabUrl);
  const { p, note, fetched } = await fillJstage(tabUrl, { [landing]: jsLanding.map(([n, c]) => [n, n === 'citation_pdf_url' ? declared : c]) });
  same([p.doi, p.title, p.pdfUrl, p.pdfSource, note], ['10.20844/zzzz.42.1_5', '小児の架空症例の画像所見', tabUrl, 'self', '']);
  same(fetched, [[landing, 'include']]); // 1 回だけ・同一オリジンの cookie 付き
});
const swap = (name, value) => jsLanding.map(([n, c]) => [n, n === name ? value : c]);
for (const [name, pages, why] of [
  ['the article page declares another article\'s PDF', { [jsArticle + '/-char/ja']: swap('citation_pdf_url', jsPdf.replace('42_5', '42_6')) }, /記事ページの PDF がこのタブと別/],
  ['the article page declares a PDF on another host', { [jsArticle + '/-char/ja']: swap('citation_pdf_url', jsPdf.replace('www.jstage.jst.go.jp', 'example.org')) }, /記事ページの PDF がこのタブと別/],
  ['the article page declares no PDF', { [jsArticle + '/-char/ja']: jsLanding.filter(([n]) => n !== 'citation_pdf_url') }, /記事ページの PDF がこのタブと別/],
  ['the fetch fails', { [jsArticle + '/-char/ja']: new TypeError('Failed to fetch') }, /Failed to fetch/],
  ['the article page answers HTTP 404', {}, /HTTP 404/],
  ['the article page has no citation_doi', { [jsArticle + '/-char/ja']: jsLanding.filter(([n]) => n !== 'citation_doi') }, /citation_doi なし/],
  ['citation_doi is not a DOI', { [jsArticle + '/-char/ja']: swap('citation_doi', 'zzzz.42.1_5') }, /citation_doi なし/],
]) test(`J-STAGE article lookup is refused when ${name}: the page stays on the PDF route`, async () => {
  const { p, note } = await fillJstage(jsPdf + '/-char/ja', pages);
  same([p.doi, p.pmid, p.title, p.pdfSource, p.isPdf], [null, null, null, 'self', true]);
  assert.match(note, why);
  assert.match(note, /PDF 本文から探します/);
});
test('the article page is not fetched when the page already has an identifier or is not a J-STAGE PDF', async () => {
  for (const page of [{ doi: '10.1000/x', landingUrl: jsArticle }, { pmid: '1', landingUrl: jsArticle }, { landingUrl: null }]) {
    const { note, fetched } = await fillJstage(null, {}, page);
    same([note, fetched], ['', []]);
  }
});

/* 配線: popup を init から開いて保存を押す（executeScript が拒まれる = Chrome の PDF ビューア） */
/* opts: longTimers（popup の 5 秒以上のタイマーを発火させて ms を残す）・allSites / declined（全サイト許可とその拒否の記録）・proxy（保存された大学プロキシの前置き）・
   routes（connectd の応答の差し替え = {パス（? の前）: 値 | Error | (path, body) => 値}）・landingUrl（背景タブ 7 の URL）・
   setup（読み込んだ後に popup と service worker の両方で走らせる JS = sendToTab・bgFetchPdf 等の chrome との境目の差し替え。
   popup では bgFetchPdf は使われない = 空の値を置いておく）・env（別の popup と service worker・記録を共有する = 開き直し）・tabId。
   保存の工程は service worker の側（background.js → job.js を読んだ別の context）で走る。popup の chrome.runtime.sendMessage は
   その onMessage へ届き、記録（chrome.storage.session）は両方が同じ 1 つを見る。保存ボタンの click は工程の終わりまで待つ
   （clickRaw = 待たない）。時計は仮想（5 秒未満のタイマーは時計を進めて即実行。service worker は 5 秒以上を発火させない） */
function swEnv({ allSites = true, routes = {}, landingUrl, fetchFn = null } = {}) {
  const calls = [], fetched = [], created = [], badges = [];
  const session = fakeSession();
  const userTabs = {}; // tab id → URL（popup が開いたタブ）
  const shared = {
    tabs: {
      query: async () => [{ id: 1, url: userTabs[1], title: '42_5.pdf' }],
      create: async (o) => { created.push(JSON.parse(JSON.stringify(o))); return { id: 7 }; },
      get: async (id) => (id in userTabs ? { id, status: 'complete', url: userTabs[id] } : { status: 'complete', url: landingUrl }),
      remove: async () => {},
      onUpdated: { addListener() {}, removeListener() {} }, onRemoved: { addListener() {}, removeListener() {} },
    },
    scripting: { executeScript: async () => { throw new Error('Cannot access contents of the page'); } },
    permissions: { contains: async () => allSites, request: async () => false },
    action: { setBadgeText: async (o) => { badges.push(o.text); }, setBadgeBackgroundColor: async () => {} },
  };
  const fakeApi = async (path, opts) => {
    const body = opts && typeof opts.body === 'string' ? JSON.parse(opts.body) : null;
    calls.push([path, body]);
    const r = routes[path.split('?')[0]];
    if (r instanceof Error) throw r;
    if (typeof r === 'function') return r(path, body);
    if (r !== undefined) return r;
    if (path === '/capture/status') return { items: 1 };
    if (path === '/capture/collections') return { collections: [] };
    if (path === '/capture/save') return { key: 'K1', status: 'created', doi: '10.20844/zzzz.42.1_5', has_pdf: false, title: '小児の架空症例の画像所見' };
    if (path.startsWith('/capture/attachment?')) return { status: 'attached', bytes: 9, doi_check: 'unknown' };
    if (path.startsWith('/capture/pdf?')) return { status: 'stub', key: 'K2' };
    throw new Error('unexpected route ' + path);
  };
  let swNow = 0, onMessage = null, alarmListener = null; // alarmListener = background.js の onAlarm（確認の窓の試験が期限を手で起こす）
  const swLong = [], swIntervals = [];
  const sw = vm.createContext({ URL, URLSearchParams, Uint8Array, atob, btoa, TextDecoder, TextEncoder,
    Date: { now: () => swNow },
    setTimeout: (f, ms) => { if (ms < 5000) { swNow += ms || 1; f(); } else swLong.push({ ms, f }); return 0; },
    clearTimeout() {},
    setInterval: (f, ms) => { swIntervals.push({ f, ms, on: true }); return swIntervals.length; },
    clearInterval: (id) => { if (swIntervals[id - 1]) swIntervals[id - 1].on = false; },
    fetch: fetchFn,
    importScripts: (...names) => names.forEach((n) => vm.runInContext(code(n), sw)),
    chrome: Object.assign({}, shared, {
      runtime: { id: 'ext', getURL: (x) => 'chrome-extension://ext/' + x, onMessage: { addListener: (f) => { onMessage = f; } }, getPlatformInfo: async () => ({ os: 'mac' }) },
      storage: { session: session.area },
      alarms: { create() {}, clear() {}, onAlarm: { addListener: (f) => { alarmListener = f; } } },
    }),
  });
  vm.runInContext(code('background.js'), sw);
  sw.fakeApi = fakeApi;
  vm.runInContext(`api = fakeApi; bgFetchPdf = async () => ({ ok: true, b64: btoa('%PDF-test') });
    var lastRun = null; const realRunSave = runSave; runSave = (job) => (lastRun = realRunSave(job));`, sw);
  /* popup からのメッセージ（実物と同じく値は写して渡す・送り手は拡張のページ） */
  const env = {};
  const deliver = (msg, cb) => {
    const sender = { id: 'ext', url: 'chrome-extension://ext/popup.html' };
    // env.hold = Promise の間は応答を popup へ渡さない（応答より後の記録の変化が先に届く順を作る）
    const async = onMessage(clone(msg), sender, (res) => Promise.resolve(env.hold).then(() => cb(clone(res))));
    if (async !== true) Promise.resolve().then(() => cb(undefined));
  };
  /* 走っている工程が終わるまで待つ（終わりの記録が popup に届くまで） */
  const settle = async () => {
    for (let p = sw.lastRun; p; p = sw.lastRun === p ? null : sw.lastRun) await p;
    await new Promise((r) => setImmediate(r));
  };
  return Object.assign(env, { calls, fetched, created, badges, session, userTabs, shared, sw, deliver, settle, swLong, swIntervals,
    alarm: (name) => alarmListener({ name }),
    record: () => clone(session.store.openref_job), fakeApi });
}
function popupApp(tabUrl, pages, { longTimers = null, allSites = true, declined = false, proxy = null, routes = {}, landingUrl = tabUrl, setup = '', env = null, tabId = 1 } = {}) {
  const fetchFn = typeof pages === 'function' ? pages : null;
  const reused = !!env;
  env = env || swEnv({ allSites, routes, landingUrl, fetchFn });
  const fetchPage = fetchFn || landingFetch(pages, env.fetched);
  env.userTabs[tabId] = tabUrl;
  const els = {};
  // document.activeElement は focus() で動く（押せるようになった保存ボタンへのフォーカス）。最初は body
  const doc = { body: { id: 'body' }, documentElement: { id: 'html' } };
  doc.activeElement = doc.body;
  // append = 状態行に文と要素を足す（結果の 1 行目の印の要素）。作った要素は doc.made に残す（class と文を試験が見る）
  const el = id => els[id] || (els[id] = { id, textContent: '', innerHTML: '', className: '', value: '', disabled: true, style: {}, handlers: {},
    addEventListener(type, f) { this.handlers[type] = f; }, appendChild() {}, focus() { doc.activeElement = this; },
    append(...xs) { for (const x of xs) this.textContent += typeof x === 'string' ? x : x.textContent; } });
  doc.getElementById = el;
  doc.made = [];
  doc.createElement = tag => { const e = { tag, className: '', textContent: '' }; doc.made.push(e); return e; };
  let now = 0;
  const context = vm.createContext({ URL, URLSearchParams, Uint8Array, atob, btoa, TextDecoder,
    Date: { now: () => now },
    // 5 秒以上のタイマー（withTimeout の打ち切り）は longTimers を渡したときだけ、ms を残して次の microtask で発火する
    setTimeout: (f, ms) => { if (ms < 5000) { now += ms || 1; f(); } else if (longTimers) { longTimers.push(ms); Promise.resolve().then(f); } return 0; },
    clearTimeout() {}, setInterval() {},
    document: doc, DOMParser: MetaParser, fetch: fetchPage,
    chrome: Object.assign({}, env.shared, {
      tabs: Object.assign({}, env.shared.tabs, { query: async () => [{ id: tabId, url: tabUrl, title: '42_5.pdf' }] }),
      runtime: { sendMessage: (msg, cb) => env.deliver(msg, cb) },
      storage: { local: { get: async () => Object.assign(declined ? { openref_allsites_declined: true } : {}, proxy !== null ? { openref_proxy_template: proxy } : {}), set: async () => {} },
        onChanged: env.session.onChanged },
    }),
  });
  vm.runInContext(code('pdfbytes.js'), context);
  vm.runInContext(code('popup.js').replace(/\ninit\(\);\s*$/, ''), context);
  context.fakeApi = env.fakeApi;
  vm.runInContext('api = fakeApi; var bgFetchPdf = null;', context);
  if (setup) {
    vm.runInContext(setup, context);
    if (!reused) vm.runInContext(setup, env.sw); // 共有の service worker には最初の popup の setup だけ
  }
  const clickRaw = els.save.handlers.click;
  els.save.handlers.click = async () => { await clickRaw(); await env.settle(); };
  return { context, sw: env.sw, env, els, calls: env.calls, fetched: env.fetched, created: env.created, doc, clickRaw, record: env.record };
}
async function openAndSave(tabUrl, pages, opts) {
  const app = popupApp(tabUrl, pages, opts);
  // init が終わらない（記事ページの取得に上限が無い）ときは本物の時計で 2 秒後に落とす
  let timer;
  await Promise.race([vm.runInContext('init()', app.context),
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('init が終わらない（記事ページの取得に上限が無い）')), 2000); })]);
  clearTimeout(timer);
  app.opened = { title: app.els.title.textContent, ids: app.els.ids.innerHTML, status: app.els.status.textContent };
  assert.equal(app.els.save.disabled, false);
  await app.els.save.handlers.click();
  return app;
}
test('J-STAGE PDF tab: the DOI from the article page goes to /capture/save, then the tab PDF is attached as source=self', async () => {
  const app = await openAndSave(jsPdf + '/-char/ja', { [jsArticle + '/-char/ja']: jsLanding });
  assert.equal(app.opened.title, '小児の架空症例の画像所見'); // タブの題名（42_5.pdf）ではなく記事ページの citation_title
  assert.match(app.opened.ids, /PDF タブ.*DOI 10\.20844\/zzzz\.42\.1_5/);
  assert.equal(app.opened.status, '');
  same(app.calls.map(c => c[0]), ['/capture/status', '/capture/collections', '/capture/save', '/capture/attachment?key=K1&source=self&via=page']);
  assert.equal(app.calls[2][1].doi, '10.20844/zzzz.42.1_5');
  assert.equal(app.calls[2][1].url, jsPdf + '/-char/ja');
  assert.match(app.record().line, /登録 K1 — PDF 添付 ✓.*DOI 未確認/);
  same(app.fetched.map(f => f[0]), [jsArticle + '/-char/ja']);
});
test('J-STAGE PDF tab whose article page does not match keeps the /capture/pdf route', async () => {
  const app = await openAndSave(jsPdf + '/-char/ja', { [jsArticle + '/-char/ja']: swap('citation_pdf_url', jsPdf.replace('42_5', '42_6')) });
  assert.equal(app.opened.title, '42_5.pdf');
  assert.match(app.opened.ids, /識別子なし → PDF 本文から/);
  assert.match(app.opened.status, /記事ページで DOI を確かめられず/);
  same(app.calls.map(c => c[0].split('?')[0]), ['/capture/status', '/capture/collections', '/capture/pdf']);
  assert.match(app.record().line, /要確認として登録 K2/);
});
test('J-STAGE article page that never answers: the time limit fires and the save goes to /capture/pdf', async () => {
  const longTimers = [];
  const app = await openAndSave(jsPdf + '/-char/ja', () => new Promise(() => {}), { longTimers });
  same(longTimers, [8000]); // 記事ページの取得の上限（JSTAGE_ARTICLE_TIMEOUT_MS）だけ
  assert.match(app.opened.status, /DOI を確かめられず（J-STAGE の記事ページ が 8 秒以内に応答しません）/);
  assert.match(app.opened.ids, /識別子なし → PDF 本文から/);
  same(app.calls.map(c => c[0].split('?')[0]), ['/capture/status', '/capture/collections', '/capture/pdf']);
});
test('a redirected article page resolves a relative citation_pdf_url against the URL it ended on', async () => {
  // 記事ページ（-char/ja 付き）が -char なしへ転送された。相対の _pdf は転送後の …/42_5/_article から見て …/42_5/_pdf
  // （転送前の …/_article/-char/ja から見ると …/_article/-char/_pdf = 別物）
  const pages = { [jsArticle + '/-char/ja']: { url: jsArticle, meta: swap('citation_pdf_url', '_pdf') } };
  const { p, note } = await fillJstage(jsPdf + '/-char/ja', pages);
  same([p.doi, note], ['10.20844/zzzz.42.1_5', '']);
});
test('an older OpenRefShared without the new functions is replaced by the new pdfbytes.js', () => {
  const old = { metaRefreshUrl() {}, metaRefreshUrlInDoc() {}, safeDecode() {} }; // 前の版の注入が同じ isolated world に残した物
  const ctx = vm.createContext({ URL, OpenRefShared: old });
  vm.runInContext(code('pdfbytes.js'), ctx);
  assert.equal(typeof ctx.OpenRefShared.cleanDoi, 'function', '古い定義が残って cleanDoi が無い');
  assert.equal(typeof ctx.OpenRefShared.jstageArticleUrl, 'function', '古い定義が残って jstageArticleUrl が無い');
  assert.equal(ctx.OpenRefShared.cleanDoi('doi:10.1000/x.'), '10.1000/x');
});

/* ===== PDF 失敗の分類と次の一手（確かなときだけ 1 行・迷ったら出さない） ===== */
const inPopup = (expr) => vm.runInContext(expr, popup().context);
const inJob = (expr) => vm.runInContext(expr, worker().context);
const NEXT = inJob('NEXT_STEP');
/* 結果の 1 行目の「PDF ✗（理由）」の理由（分類の短い言い方）。job.js から読まずに書く = 文を固定する */
const WHY = { mismatch: '別の論文だった', login: 'ログインが要る', challenge: '確認ページのまま', stuck: '途中のページで止まった', no_pdf: 'PDF が見つからない', timeout: '時間切れ' };
for (const [url, want] of [
  ['https://idp.uni.example.ac.jp/idp/profile/SAML2/Redirect/SSO', true], // ホストの段 idp
  ['https://login.openathens.net/saml/2', true],
  ['https://www.example.org/Shibboleth.sso/Login?target=x', true], // 拡張子を外して shibboleth・パスの段 login
  ['https://ezproxy.lib.example.edu/connect?url=https://x', true],
  ['https://www.example.org/user/sign-in', true],
  ['https://www.example.org/signin', true],
  ['https://wayf.example.org/', true],
  ['https://sso.example.com/x', true],
  ['https://auth.example.com/authorize', true],
  ['https://www.example.org/action/login.php', true],
  ['https://www.example.org/authors/smith', false],
  ['https://www.example.org/author/xyz', false], // author は auth ではない
  ['https://authorea.com/doi/full/10.22541/x', false],
  ['https://www.example.org/doi/10.1000/auth.123', false], // 数字の拡張子は外さない
  ['https://www.example.org/loginhelp', false],
  ['https://www.example.org/article/1?next=/login', false], // クエリは見ない
  ['not a url', false],
]) test(`login URL rule: ${url} → ${want}`, () => {
  assert.equal(inJob(`OpenRefShared.isLoginUrl(${JSON.stringify(url)})`), want);
});
for (const [name, res, want] of [
  ['HTTP 401', { ok: false, httpStatus: 401, url: pdf }, 'login'],
  ['HTTP 403 (also bot blocking)', { ok: false, httpStatus: 403, url: pdf }, null],
  ['HTTP 403 that ended on a login page', { ok: false, httpStatus: 403, url: 'https://idp.example.edu/idp/x' }, 'login'],
  ['HTML that ended on a login page', { ok: false, html: true, url: 'https://www.example.org/Shibboleth.sso/Login' }, 'login'],
  ['HTML on the article host', { ok: false, html: true, url: pdf }, null],
  ['an old response without the new fields', { ok: false, error: 'x', html: true }, null],
]) test(`fetch failure kind: ${name}`, () => {
  assert.equal(vm.runInContext(`fetchKind(J, ${JSON.stringify(res)})`, worker().context), want);
});
// connectd（core pdffetch.rs）の決まった言い回しで組んだ /capture/fetch-oa の失敗文
const oaText = (...tried) => 'PDF を自動取得できませんでした — ' + tried.join(' / ') + '。paywall の文献はサーバ側からは取得できません（設計どおり）。ブラウザで PDF を開いた状態で取り込んでください';
const noLanding = 'doi.org: 着地ページ www.example.org に PDF の所在（citation_pdf_url / .pdf リンク）がありません（text/html・5120 bytes。出版社が拡張外からの取得に本文ページを出さない bot 対策の可能性 → ブラウザ経路へ）';
const oaNone = oaText('pmc: PMCID なし', 'unpaywall: OPENREF_EMAIL 未設定（opt-in）', 'crossref: link[] に PDF なし', noLanding);
const oaBot = oaText('pmc: PMCID なし', 'unpaywall: OPENREF_EMAIL 未設定（opt-in）', 'crossref: link[] に PDF なし', 'doi.org: 出版社ページ www.example.org が HTTP 403 を返しました（拡張外からの取得を拒否・bot 対策 → ブラウザ経路へ）');
for (const [name, msg, want] of [
  ['no location at any stage', oaNone, 'no_pdf'],
  ['PMC outside the OA subset', oaNone.replace('pmc: PMCID なし', 'pmc: PMC1234567 は OA サブセット外（license none・自動取得不可）'), 'no_pdf'],
  ['the landing page answered 403 (the server stage alone does not decide)', oaBot, 'no_pdf'],
  ['HTTP 401', oaText('pmc: PMCID なし', 'unpaywall: OA 版なし', 'crossref: link[] に PDF なし', 'doi.org: www.example.org が HTTP 401'), 'login'],
  ['Crossref declared a PDF that failed', oaNone.replace('crossref: link[] に PDF なし', 'crossref: https://www.example.org/x.pdf → www.example.org が HTTP 403'), null],
  ['a PDF was found and refused', oaNone.replace('crossref: link[] に PDF なし', 'crossref: link[] に PDF なし / doi.org: 別の文献（DOI 10.1000/x）の PDF でした（9 bytes・添付せず）'), null],
  ['Unpaywall had a location', oaNone.replace('unpaywall: OPENREF_EMAIL 未設定（opt-in）', 'unpaywall: api.unpaywall.org が HTTP 500'), null],
  ['an unrelated error', 'item K1 not found', null],
]) test(`/capture/fetch-oa text kind: ${name}`, () => {
  const h = worker();
  h.context.err = new Error(msg);
  assert.equal(vm.runInContext('oaKind(err)', h.context), want);
});
for (const [name, k, want] of [
  ['mismatch', { page: 'mismatch' }, 'mismatch'],
  ['login', { page: 'login', server: 'no_pdf' }, 'login'],
  ['challenge', { page: 'no_pdf', pmc: 'challenge' }, 'challenge'],
  ['stuck', { page: 'stuck', server: 'no_pdf' }, 'stuck'],
  ['challenge beats stuck', { page: 'stuck', pmc: 'challenge' }, 'challenge'],
  ['login beats stuck', { page: 'stuck', doiTab: 'login' }, 'login'],
  ['no_pdf when the PMC stage ran (it may have had a declared PDF)', { page: 'no_pdf', pmc: null, server: 'no_pdf', doiTab: 'no_pdf' }, null],
  ['no_pdf at the page, the server chain and the DOI page', { page: 'no_pdf', server: 'no_pdf', doiTab: 'no_pdf' }, 'no_pdf'],
  ['mismatch beats login', { page: 'login', doiTab: 'mismatch' }, 'mismatch'],
  ['login beats challenge', { pmc: 'challenge', doiTab: 'login' }, 'login'],
  ['no_pdf without the DOI page (no all-sites permission)', { page: 'no_pdf', server: 'no_pdf' }, null],
  ['no_pdf while the page had a PDF', { page: null, server: 'no_pdf', doiTab: 'no_pdf' }, null],
  ['a 403 everywhere', { page: null, server: null, doiTab: null }, null],
  ['nothing', {}, null],
]) test(`failure class: ${name}`, () => {
  assert.equal(vm.runInContext(`classifyFailure(${JSON.stringify(k)})`, worker().context), want);
});
test('PDF tab that stays on an HTML page is stuck (not login without evidence); reloading until the deadline is unclassified; a login URL is login', async () => {
  const quiet = worker();
  quiet.fetcher(async () => { throw htmlError(); });
  const e1 = await nav(quiet).then(() => null, e => e);
  assert.equal(e1.kind, 'stuck');
  const loop = worker();
  const t = [];
  for (let ms = 0; ms < 40000; ms += 1000) t.push([ms, pdf, 'loading'], [ms + 500, pdf, 'complete']);
  loop.timeline(t);
  loop.fetcher(async () => { throw htmlError(); });
  const e2 = await nav(loop).then(() => null, e => e);
  assert.match(e2.message, /PDF へ届きませんでした/);
  assert.equal(e2.kind, null);
  const sso = worker();
  sso.timeline(t.map(([ms, , st]) => [ms, 'https://idp.example.edu/idp/profile/SAML2/Redirect/SSO', st]));
  sso.fetcher(async () => { throw htmlError(); });
  assert.equal((await nav(sso).then(() => null, e => e)).kind, 'login');
});
test('DOI page route marks its failures: no declaration → no_pdf, login landing → login, PMC check page → challenge', async () => {
  const noDecl = landingHarness([page({ doi: '10.1234/ej.2025.123' })], { landingUrl: 'https://www.example-journal.org/article/123' });
  assert.equal((await via(noDecl, "'10.1234/ej.2025.123'", 'null').then(() => null, e => e)).kind, 'no_pdf');
  const login = landingHarness([blank], { landingUrl: 'https://login.openathens.net/saml/2' });
  assert.equal((await via(login, "'10.1234/ej.2025.123'", 'null').then(() => null, e => e)).kind, 'login');
  const stuck = landingHarness([blank]);
  assert.equal((await via(stuck, 'null').then(() => null, e => e)).kind, 'challenge');
  const pmcLogin = landingHarness([blank], { landingUrl: 'https://login.openathens.net/saml/2' }); // PMC 本文がログイン画面に着いた
  assert.equal((await via(pmcLogin, 'null').then(() => null, e => e)).kind, 'login');
  const relay = landingHarness([blank], { landingUrl: 'https://www.example-journal.org/article/123' });
  assert.equal((await via(relay, "'10.1234/ej.2025.123'", 'null').then(() => null, e => e)).kind, null); // 中継ページのまま = 分類しない
});

/* 配線: 保存を押してから最終の状態行まで（論文ページ = content script が入る） */
const art = 'https://www.example-journal.org/article/123';
const artPdf = 'https://www.example-journal.org/article/123.pdf';
const apiError = (status, message, body = {}) => Object.assign(new Error(message), { status, body });
const articleTab = (data, fetchAnswer) => `
  chrome.scripting.executeScript = async () => {};
  const bgFetch = ${JSON.stringify(fetchAnswer)};
  sendToTab = async (msg, tabId) => {
    if (msg.type === 'openref-analyze') return { ok: true, data: Object.assign({ doi: null, pmid: null, pdfUrl: null, pdfSource: null, title: 'T', url: ${JSON.stringify(art)} }, tabId === 7 ? landingData : ${JSON.stringify(data)}) };
    if (msg.type === 'openref-refresh-url') return { ok: true, url: null };
    if (msg.type === 'openref-fetch-pdf') return bgFetch;
  };
  bgFetchPdf = async () => bgFetch;
`;
async function saveAndRead(opts, landingData = {}) {
  const app = popupApp(art, {}, opts);
  app.context.landingData = landingData;
  app.sw.landingData = landingData;
  await vm.runInContext('init()', app.context);
  await app.els.save.handlers.click();
  const rec = app.record();
  // 分類の行（次の一手）は畳まずに見せる部分（head）の最後の行。段ごとの理由は head の後ろ（畳む）
  return { app, status: rec.line, lines: rec.line.split('\n'), head: rec.head, headLines: rec.head.split('\n') };
}
const savedK1 = doi => ({ key: 'K1', status: 'created', doi, has_pdf: false, title: 'T' });
test('save flow → login: the PDF request ends on an IdP page; today\'s reasons stay and the class line is last', async () => {
  const idp = { ok: false, error: 'Error: PDF ではない応答（5000 bytes・HTML の可能性）', html: true, httpStatus: 0, url: 'https://idp.uni.example.ac.jp/idp/profile/SAML2/Redirect/SSO' };
  const { status, lines, headLines } = await saveAndRead({ allSites: false, declined: true,
    setup: articleTab({ doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' }, idp),
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/fetch-oa': apiError(422, oaBot) } });
  assert.match(status, /ブラウザ経由で取れず（ページ内 fetch: .*PDF ではない応答/);
  assert.match(status, /OpenRef 側: PDF を自動取得できませんでした/);
  assert.match(status, /DOI ページ経由: 全サイトへのアクセス許可が無いため未試行/);
  assert.equal(headLines.at(-1), NEXT.login);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（ログインが要る）', '登録 K1']);
});
test('save flow → challenge: PubMed page whose PMC full text stays on the check page', async () => {
  const { status, lines, headLines } = await saveAndRead({ landingUrl: pmcLink,
    setup: articleTab({ pmid: '21620466', pmcUrl: pmcLink }, { ok: false, error: 'x' }),
    routes: { '/capture/save': savedK1(null), '/capture/fetch-oa': apiError(422, oaText('pmc: PMC3062508 は AWS Open Data に未収載（OA サブセット外か未同期）', 'unpaywall: DOI なし', 'crossref/doi.org: DOI なし')) } });
  assert.match(status, /PMC 本文経由: PMC 本文の識別子を確認できません（pmc\.ncbi\.nlm\.nih\.gov が確認ページ/);
  assert.equal(headLines.at(-1), NEXT.challenge);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（確認ページのまま）', '登録 K1']);
});
test('save flow → no_pdf: no PDF on the page, none in the server chain, none on the DOI page opened in the browser', async () => {
  const { status, lines, headLines } = await saveAndRead({
    setup: articleTab({ doi: '10.1234/ej.2025.123' }, { ok: false, error: 'x' }),
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/fetch-oa': apiError(422, oaNone) } },
    { doi: '10.1234/ej.2025.123' });
  assert.match(status, /DOI ページ経由: 着地ページ（www\.example-journal\.org）に PDF の宣言/);
  assert.equal(headLines.at(-1), NEXT.no_pdf);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（PDF が見つからない）', '登録 K1']);
});
test('save flow → mismatch: the existing refusal message is kept and the class line follows', async () => {
  const { status, lines, headLines } = await saveAndRead({ allSites: false, declined: true,
    setup: articleTab({ doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' }, { ok: true, b64: btoa('%PDF-test') }),
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'),
      '/capture/attachment': apiError(409, '別論文（DOI 10.1000/other）の PDF です', { status: 'mismatch', found_doi: '10.1000/other' }) } });
  assert.match(status, /書誌は保存済み。PDF は添付しませんでした — 別論文（DOI 10\.1000\/other）/);
  assert.equal(headLines.at(-1), NEXT.mismatch);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（別の論文だった）', '登録 K1']);
});
test('save flow with HTTP 403 everywhere gets no class line (403 is also bot blocking)', async () => {
  const forbidden = { ok: false, error: 'Error: HTTP 403', html: false, httpStatus: 403, url: artPdf };
  const { status } = await saveAndRead({ allSites: false, declined: true,
    setup: articleTab({ doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' }, forbidden),
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/fetch-oa': apiError(422, oaBot) } });
  assert.match(status, /HTTP 403/);
  assert.doesNotMatch(status, /分類:/);
});
test('connectd reachable but the DB unreadable: the popup shows the error instead of "null 件"', async () => {
  const app = popupApp(jsPdf + '/-char/ja', { [jsArticle + '/-char/ja']: jsLanding },
    { routes: { '/capture/status': { ok: true, app: 'openref', items: null, error: '件数を数えられません: database is locked' } } });
  await vm.runInContext('init()', app.context);
  assert.equal(app.els.conn.textContent, '✓ OpenRef（件数不明）');
  assert.match(app.els.status.textContent, /蔵書を読めません: 件数を数えられません: database is locked/);
  assert.equal(app.els.status.className, 'err');
  const ok = popupApp(jsPdf + '/-char/ja', { [jsArticle + '/-char/ja']: jsLanding }, { routes: { '/capture/status': { ok: true, items: 12 } } });
  await vm.runInContext('init()', ok.context);
  assert.equal(ok.els.conn.textContent, '✓ OpenRef（12 件）');
  assert.equal(ok.els.status.textContent, '');
});
test('save flow → stuck: the PDF tab stays on an HTML page with no login evidence', async () => {
  const html = { ok: false, error: 'Error: PDF ではない応答（5000 bytes・HTML の可能性）', html: true, httpStatus: 0, url: artPdf };
  const { lines, headLines } = await saveAndRead({
    setup: articleTab({ doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' }, html),
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/fetch-oa': apiError(422, oaBot) } },
    { doi: '10.1234/ej.2025.123' });
  assert.equal(headLines.at(-1), NEXT.stuck);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（途中のページで止まった）', '登録 K1']);
});
/* 誤った分類の 2 つの再現を回帰試験に */
test('a page fetch that hit an IdP but whose PDF-tab retry got the PDF does not leak "login" when the transfer fails', async () => {
  const idp = { ok: false, error: 'Error: PDF ではない応答', html: true, httpStatus: 0, url: 'https://idp.uni.example.ac.jp/idp/profile/SAML2/Redirect/SSO' };
  const setup = articleTab({ doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' }, idp) + `
    var pageFetches = 0;
    const firstAnswer = sendToTab;
    sendToTab = async (msg, tabId) => (msg.type === 'openref-fetch-pdf' && ++pageFetches > 1 ? { ok: true, b64: btoa('%PDF-test') } : firstAnswer(msg, tabId));`;
  const { status } = await saveAndRead({ setup,
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/attachment': apiError(500, 'internal error'), '/capture/fetch-oa': apiError(422, oaBot) } },
    { doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' });
  assert.match(status, /転送失敗（internal error/);
  assert.doesNotMatch(status, /分類:/);
});
test('a PubMed page whose PMC stage had a declared PDF (HTTP 500) is not classed no_pdf', async () => {
  const fail500 = { ok: false, error: 'Error: HTTP 500', html: false, httpStatus: 500, url: pmcPdf };
  const setup = articleTab({ pmid: '21620466', pmcUrl: pmcLink }, fail500) + `
    var landings = 0;
    const firstAnswer = sendToTab;
    sendToTab = async (msg, tabId) => {
      if (tabId !== 7 || msg.type !== 'openref-analyze') return firstAnswer(msg, tabId);
      landings++; // 1 回目 = PMC 本文（宣言された PDF あり）・2 回目 = DOI ページ（宣言なし）
      return { ok: true, data: Object.assign({ doi: '10.1234/ej.2025.123', pmid: null, pdfUrl: null, pdfSource: null, title: 'T' },
        landings === 1 ? { pmid: '21620466', pdfUrl: ${JSON.stringify(pmcPdf)}, pdfSource: 'meta' } : {}) };
    };`;
  const { status } = await saveAndRead({ landingUrl: pmcLink, setup,
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/fetch-oa': apiError(422,
      oaText('pmc: PMC3062508 は OA サブセット外（author manuscript・license none・自動取得不可）', 'unpaywall: OPENREF_EMAIL 未設定（opt-in）', 'crossref: link[] に PDF なし', noLanding)) } });
  assert.match(status, /PMC 本文経由: .*HTTP 500/);
  assert.match(status, /DOI ページ経由: 着地ページ（pmc\.ncbi\.nlm\.nih\.gov）に PDF の宣言/);
  assert.doesNotMatch(status, /分類:/);
});
test('save flow → login: the DOI page\'s declared PDF answers HTTP 401', async () => {
  const unauthorized = { ok: false, error: 'Error: HTTP 401', html: false, httpStatus: 401, url: artPdf };
  const { status, lines, headLines } = await saveAndRead({
    setup: articleTab({ doi: '10.1234/ej.2025.123' }, unauthorized),
    routes: { '/capture/save': savedK1('10.1234/ej.2025.123'), '/capture/fetch-oa': apiError(422, oaNone) } },
    { doi: '10.1234/ej.2025.123', pdfUrl: artPdf, pdfSource: 'meta' });
  assert.match(status, /DOI ページ経由: タブ内: Error: HTTP 401/);
  assert.equal(headLines.at(-1), NEXT.login);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（ログインが要る）', '登録 K1']);
});
/* 印の出どころ（content.js・background.js）が httpStatus と転送の着き先を返す */
const idpUrl = 'https://idp.example.edu/idp/profile/SAML2/Redirect/SSO';
function backgroundFetchWith(answer) {
  let listener;
  const noop = { addListener() {} };
  const ctx = vm.createContext({
    URL, Uint8Array, TextDecoder, btoa,
    importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
    chrome: {
      runtime: { onMessage: { addListener: f => listener = f }, onConnect: noop, getContexts: async () => [] },
      alarms: { onAlarm: noop }, tabs: { onRemoved: noop },
      storage: { session: { get: async () => ({}), set: async () => {} } },
    },
    fetch: async () => answer,
  });
  vm.runInContext(code('background.js'), ctx);
  return ctx.bgFetchPdf(pdf);
}
function contentFetchWith(answer) {
  let listener;
  runContent({
    window: {}, URL, location: new URL(article), Uint8Array, btoa, TextDecoder,
    DOMParser: class { parseFromString() { return { querySelector: () => null, querySelectorAll: () => [] }; } },
    chrome: { runtime: { onMessage: { addListener: f => listener = f } } },
    fetch: async () => answer,
  });
  return new Promise(resolve => listener({ type: 'openref-fetch-pdf', url: pdf }, {}, resolve));
}
const htmlAt = url => ({ ok: true, status: 200, url, arrayBuffer: async () => new TextEncoder().encode('<html>Sign in</html>').buffer });
for (const [who, run] of [['background.js', backgroundFetchWith], ['content.js', contentFetchWith]]) {
  test(`${who} reports HTTP 401 with its status and final URL`, async () => {
    const res = await run({ ok: false, status: 401, url: idpUrl });
    same([res.ok, res.httpStatus, res.url, res.html], [false, 401, idpUrl, false]);
  });
  test(`${who} reports where a non-PDF answer ended up (after redirects)`, async () => {
    const res = await run(htmlAt(idpUrl));
    same([res.ok, res.html, res.httpStatus, res.url], [false, true, 0, idpUrl]);
  });
}

/* ===== ページが自分について宣言した書誌メタ（evidence）= 識別子の無いページで connectd が Crossref の候補と照合する証拠 ===== */
function analyzeFixture(f) {
  let listener;
  runContent({ URL, window: {}, location: new URL(f.url || 'https://www.example-society.org/abstracts/2025/1234'), document: fixtureDoc(f),
    chrome: { runtime: { onMessage: { addListener: l => listener = l } } } });
  let res;
  listener({ type: 'openref-analyze' }, {}, x => res = x);
  assert.equal(res.ok, true, res.error);
  return JSON.parse(JSON.stringify(res.data)); // vm の別 realm の値を素の値にする
}
const refs = [['citation_reference', 'citation_author=A Sample; citation_title=An example reference title; citation_publication_date=1998'],
  ['citation_reference', 'citation_title=Another referenced paper of sufficient length; citation_author=Smith, J; citation_date=2001']];
const ldArticle = (extra = {}) => JSON.stringify(Object.assign({ '@context': 'https://schema.org', '@type': 'ScholarlyArticle',
  headline: 'Imaging of Deep Endometriosis Involving Lateral Pelvic Structures',
  author: [{ '@type': 'Person', familyName: 'Brandão', givenName: 'Alice' }, { '@type': 'Person', name: 'Izabela V. Pires Franco' }],
  datePublished: '2026-08-01', isPartOf: { '@type': 'Periodical', name: 'RadioGraphics' } }, extra));
const LD_EV = { title: { value: 'Imaging of Deep Endometriosis Involving Lateral Pelvic Structures', source: 'json-ld' },
  authors: { value: ['Brandão, Alice', 'Izabela V. Pires Franco'], source: 'json-ld' },
  date: { value: '2026-08-01', source: 'json-ld' }, journal: { value: 'RadioGraphics', source: 'json-ld' } };
for (const [name, f, want] of [
  ['citation_* (all authors in order; citation_reference, og:title and the body are not read)',
    { meta: [['og:title', 'Site name | Abstract'], ['citation_title', 'Accelerated whole-brain MR fingerprinting'], ['citation_author', 'Chen, Wei'],
      ['citation_author', 'Tanaka, Hiroshi'], ['citation_publication_date', '2025/05/10'], ['citation_journal_title', 'Proc Intl Soc Mag Reson Med'], ...refs],
      body: 'Related: An example reference title. doi: none' },
    { title: { value: 'Accelerated whole-brain MR fingerprinting', source: 'citation_title' },
      authors: { value: ['Chen, Wei', 'Tanaka, Hiroshi'], source: 'citation_author' },
      date: { value: '2025/05/10', source: 'citation_publication_date' }, journal: { value: 'Proc Intl Soc Mag Reson Med', source: 'citation_journal_title' } }],
  ['citation_date when there is no citation_publication_date', { meta: [['citation_title', 'A title'], ['citation_date', '2024'], ['dc.date', '2019-01-01']] },
    { title: { value: 'A title', source: 'citation_title' }, date: { value: '2024', source: 'citation_date' } }],
  ['dc.* (any case) and prism.publicationName', { meta: [['DC.Title', 'Dublin core title'], ['dc.Creator', 'Smith, John'], ['dc.creator', 'Doe, Jane'],
      ['dc.Date', '2023-02-01'], ['prism.publicationName', 'Some Journal']] },
    { title: { value: 'Dublin core title', source: 'dc.title' }, authors: { value: ['Smith, John', 'Doe, Jane'], source: 'dc.creator' },
      date: { value: '2023-02-01', source: 'dc.date' }, journal: { value: 'Some Journal', source: 'prism.publicationName' } }],
  ['citation_title wins over dc.title and JSON-LD; JSON-LD fills what the metas lack',
    { meta: [['citation_title', 'Citation title'], ['dc.title', 'DC title']], jsonld: [ldArticle()] },
    Object.assign({}, LD_EV, { title: { value: 'Citation title', source: 'citation_title' } })],
  ['a single JSON-LD article', { jsonld: [ldArticle()] }, LD_EV],
  ['a single JSON-LD article inside @graph with a WebPage', { jsonld: [JSON.stringify({ '@context': 'https://schema.org',
    '@graph': [{ '@type': 'WebPage', name: 'Page' }, JSON.parse(ldArticle({ '@type': ['MedicalScholarlyArticle'] }))] })] }, LD_EV],
  ['a single JSON-LD article whose citations are Articles (nested ones are not counted)',
    { jsonld: [ldArticle({ '@type': 'https://schema.org/ScholarlyArticle', citation: [{ '@type': 'ScholarlyArticle', headline: 'Ref 1' }, { '@type': 'ScholarlyArticle', headline: 'Ref 2' }] })] }, LD_EV],
  ['a single JSON-LD Article (a blog or news post, not a ScholarlyArticle) → no JSON-LD', { jsonld: [ldArticle({ '@type': 'Article' })] }, null],
  ['a single schema:Article next to a ScholarlyArticle → the ScholarlyArticle only', { jsonld: [ldArticle({ '@type': 'schema:Article', headline: 'Blog post' }), ldArticle()] }, LD_EV],
  ['two JSON-LD articles in two scripts (a listing) → no JSON-LD', { jsonld: [ldArticle(), ldArticle({ headline: 'Another article on the list' })] }, null],
  ['two JSON-LD articles in one array → no JSON-LD', { jsonld: ['[' + ldArticle() + ',' + ldArticle({ headline: 'Another' }) + ']'] }, null],
  ['two JSON-LD articles: the metas still count', { meta: [['citation_title', 'Own title']], jsonld: [ldArticle(), ldArticle({ headline: 'Another' })] },
    { title: { value: 'Own title', source: 'citation_title' } }],
  ['broken JSON-LD and non-article JSON-LD are ignored', { jsonld: ['{ not json', JSON.stringify({ '@type': 'NewsArticle', headline: 'News' })] }, null],
  ['only og:title and citation_reference → no evidence', { meta: [['og:title', 'CT 撮影ガイドライン 2025'], ...refs] }, null],
]) test(`evidence: ${name}`, () => {
  assert.deepEqual(analyzeFixture(f).evidence, want);
});
test('evidence of the baseline pages never carries a citation_reference value', () => {
  for (const f of C0.filter(f => !f.urlOnly)) {
    const d = analyzeFixture(f);
    assert.ok(d.evidence === null || typeof d.evidence === 'object', f.name);
    assert.ok(!JSON.stringify(d.evidence || {}).includes('citation_reference'), f.name);
  }
  // J-STAGE の記事ページ: citation_title だけ（citation_reference の題名・著者は入らない）
  same(analyzeFixture(C0.find(f => f.name === 'J-STAGE の記事ページ')).evidence, { title: { value: '小児の架空症例の画像所見', source: 'citation_title' } });
});

/* 配線: 本物の content.js の解析結果を popup に渡し、保存を押す → /capture/save に evidence・最終行に reason */
const abstractPage = { url: 'https://www.example-society.org/abstracts/2025/1234',
  meta: [['og:title', 'Abstract 1234'], ['citation_title', 'Imaging of Deep Endometriosis Involving Lateral Pelvic Structures'], ['citation_author', 'Brandão, Alice'],
    ['citation_publication_date', '2026/08/01'], ...refs] };
for (const [name, saved, want, nHead] of [
  ['matched', { key: 'K1', status: 'created', doi: '10.1148/rg.250127', has_pdf: true, title: 'T', reason: 'ページの書誌メタで照合: 題名一致・年+筆頭著者一致' },
    ['書誌 ✓ ・ PDF ✓', '登録 K1 — PDF は既に添付済み', 'ページの書誌メタで照合: 題名一致・年+筆頭著者一致'], 3],
  ['not matched (title only + label)', { key: 'K2', status: 'created', doi: null, has_pdf: false, title: 'T', reason: '識別子なし・ページの書誌メタで Crossref の候補 3 件と照合し、1 件に結べませんでした' },
    ['書誌 ？（要確認） ・ PDF ✗（識別子なし）', '登録 K2', '要確認ラベルを付けました: 識別子なし・ページの書誌メタで Crossref の候補 3 件と照合し、1 件に結べませんでした', 'PDF リンクなし（識別子なしのため OA 経路は対象外）'], 3],
  ['older connectd (no reason)', { key: 'K3', status: 'created', doi: null, has_pdf: false, title: 'T' },
    ['書誌 ？（要確認） ・ PDF ✗（識別子なし）', '登録 K3', 'PDF リンクなし（識別子なしのため OA 経路は対象外）'], 2],
]) test(`wiring: /capture/save receives the page evidence; the final status shows the reason (${name})`, async () => {
  // want = line の行・nHead = そのうち畳まずに見せる行（head）の数
  const data = analyzeFixture(abstractPage);
  const { app, lines, headLines } = await saveAndRead({ setup: articleTab(data, { ok: false, error: 'x' }), routes: { '/capture/save': saved } });
  const body = app.calls.find(c => c[0] === '/capture/save')[1];
  same([body.doi, body.pmid, body.title], [null, null, 'Imaging of Deep Endometriosis Involving Lateral Pelvic Structures']);
  same(body.evidence, { title: { value: 'Imaging of Deep Endometriosis Involving Lateral Pelvic Structures', source: 'citation_title' },
    authors: { value: ['Brandão, Alice'], source: 'citation_author' }, date: { value: '2026/08/01', source: 'citation_publication_date' } });
  same(lines, want);
  same(headLines, want.slice(0, nHead));
  same(app.calls.map(c => c[0]), ['/capture/status', '/capture/collections', '/capture/save']);
});
test('wiring: a page without evidence sends evidence null (the URL-only analysis too)', async () => {
  const { app } = await saveAndRead({ setup: articleTab({ title: 'T' }, { ok: false, error: 'x' }),
    routes: { '/capture/save': { key: 'K4', status: 'created', doi: null, has_pdf: false, title: 'T' } } });
  assert.equal(app.calls.find(c => c[0] === '/capture/save')[1].evidence, null);
  assert.equal(urlOnly('https://www.example.org/a/b').evidence, undefined);
});

/* ===== 大学プロキシの前置き（実験的）。設定は 1 つ（options.html）・空なら今までどおり ===== */
const PROXY = 'https://login.ezproxy.example.ac.jp/login?url=';
const OA = 'https://go.openathens.net/redirector/example.ac.jp?url={url}';
/* EZproxy のホスト名方式（HttpsHyphens）で書き換えた URL: "-" → "--"・"." → "-"・プロキシのホスト（login. を外した物）の下へ */
const proxify = u => { const x = new URL(u); x.hostname = x.hostname.replace(/-/g, '--').replace(/\./g, '-') + '.ezproxy.example.ac.jp'; return x.href; };
const doiTarget = 'https://doi.org/10.1016/j.radcr.9999.99.999';
for (const [name, template, want] of [
  ['EZproxy: the target is appended as is', PROXY, PROXY + doiTarget],
  ['OpenAthens: {url} gets the encoded target', OA, 'https://go.openathens.net/redirector/example.ac.jp?url=https%3A%2F%2Fdoi.org%2F10.1016%2Fj.radcr.9999.99.999'],
  ['surrounding spaces are trimmed', '  ' + PROXY + ' ', PROXY + doiTarget],
  ['http:// is refused', PROXY.replace('https:', 'http:'), null],
  ['no scheme', 'login.ezproxy.example.ac.jp/login?url=', null],
  ['javascript:', 'javascript:alert(1)//', null],
  ['https:// without a host', 'https://', null],
  ['a malformed host', 'https://exa[mple.ac.jp/login?url=', null],
  ['a space inside', 'https://login.ezproxy.example.ac.jp/login?url= x', null],
  ['no path (the target would join the host)', 'https://login.ezproxy.example.ac.jp', null],
  ['{url} inside the host', 'https://{url}.example.ac.jp/x', null],
  ['user:pw@', 'https://user:pw@login.ezproxy.example.ac.jp/login?url=', null],
  ['a backslash before @ (the URL host would be evil.com)', 'https://evil.com\\@login.ezproxy.example.ac.jp/login?url=', null],
  ['a percent-encoded host (read as another host)', 'https://%6Cogin.ezproxy.example.ac.jp/login?url=', null],
  ['a query right after the host', 'https://login.ezproxy.example.ac.jp?url=', 'https://login.ezproxy.example.ac.jp?url=' + doiTarget],
  ['empty', '', null],
  ['not a string', 42, null],
  ['null', null, null],
]) test(`proxied: ${name}`, () => {
  assert.equal(shared().proxied(template, doiTarget), want);
});
test('proxied: no target → null', () => {
  assert.equal(shared().proxied(PROXY, ''), null);
  assert.equal(shared().proxied(PROXY, null), null);
});
for (const [name, host, template, want] of [
  ['ScienceDirect', 'www-sciencedirect-com.ezproxy.example.ac.jp', PROXY, 'www.sciencedirect.com'],
  ['RSNA ("-" is a dot)', 'pubs-rsna-org.ezproxy.example.ac.jp', PROXY, 'pubs.rsna.org'],
  ['a real hyphen ("--")', 'www-thieme--connect-de.ezproxy.example.ac.jp', PROXY, 'www.thieme-connect.de'],
  ['Wiley', 'onlinelibrary-wiley-com.ezproxy.example.ac.jp', PROXY, 'onlinelibrary.wiley.com'],
  ['upper case', 'WWW-SCIENCEDIRECT-COM.EZPROXY.EXAMPLE.AC.JP', PROXY, 'www.sciencedirect.com'],
  ['a host not under the proxy', 'www.sciencedirect.com', PROXY, 'www.sciencedirect.com'],
  ['a lookalike without the dot before the base', 'www-sciencedirect-comezproxy.example.ac.jp', PROXY, 'www-sciencedirect-comezproxy.example.ac.jp'],
  ['the proxy host itself', 'ezproxy.example.ac.jp', PROXY, 'ezproxy.example.ac.jp'],
  ['the proxy\'s own login host (no dot after reversing)', 'login.ezproxy.example.ac.jp', PROXY, 'login.ezproxy.example.ac.jp'],
  ['the proxy\'s own menu host', 'menu.ezproxy.example.ac.jp', PROXY, 'menu.ezproxy.example.ac.jp'],
  ['two labels under the base (dotted style: rule unknown)', 'www.sciencedirect.com.ezproxy.example.ac.jp', PROXY, 'www.sciencedirect.com.ezproxy.example.ac.jp'],
  ['no template', 'www-sciencedirect-com.ezproxy.example.ac.jp', null, 'www-sciencedirect-com.ezproxy.example.ac.jp'],
  ['an invalid (http) template', 'www-sciencedirect-com.ezproxy.example.ac.jp', PROXY.replace('https:', 'http:'), 'www-sciencedirect-com.ezproxy.example.ac.jp'],
  ['a template without a login. label', 'www-sciencedirect-com.ezproxy.example.ac.jp', 'https://ezproxy.example.ac.jp/login?url=', 'www.sciencedirect.com'],
  ['a template without login.: the base is its whole host', 'www-sciencedirect-com.example.ac.jp', 'https://ezproxy.example.ac.jp/login?url=', 'www-sciencedirect-com.example.ac.jp'],
  ['OpenAthens (no host rewriting)', 'www.sciencedirect.com', OA, 'www.sciencedirect.com'],
]) test(`deproxifyHost: ${name}`, () => {
  assert.equal(shared().deproxifyHost(host, template), want);
});
test('an older OpenRefShared without the proxy functions is replaced by the new pdfbytes.js', () => {
  const old = { metaRefreshUrl() {}, metaRefreshUrlInDoc() {}, safeDecode() {}, cleanDoi() {}, jstageArticleUrl() {} }; // プロキシの関数が入る前の版
  const ctx = vm.createContext({ URL, OpenRefShared: old });
  vm.runInContext(code('pdfbytes.js'), ctx);
  assert.equal(typeof ctx.OpenRefShared.proxied, 'function');
  assert.equal(typeof ctx.OpenRefShared.deproxifyHost, 'function');
});

/* 本物の analyze（content.js・openref-analyze の proxy）と urlOnlyAnalyze（popup.js の proxyTemplate）に、物差しの fixture を
   EZproxy のホスト名方式で書き換えた物をかける（ページのリンクもプロキシが書き換える）。J-STAGE の記事ページの citation_pdf_url は書き換えない（未確認のため） */
async function identifyProxied(f, proxy) {
  const h = popup();
  vm.runInContext(`proxyTemplate = ${JSON.stringify(proxy)};`, h.context);
  h.context.DOMParser = MetaParser;
  const url = proxify(f.url);
  h.context.fetch = landingFetch(f.landing ? { [shared().jstageArticleUrl(url, PROXY)]: f.landing } : {});
  let r;
  if (f.urlOnly) {
    r = vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, h.context);
  } else {
    let listener;
    const doc = fixtureDoc(Object.assign({}, f, { links: (f.links || []).map(proxify), fullTextLinks: (f.fullTextLinks || []).map(proxify) }));
    runContent({ URL, window: {}, location: new URL(url), document: doc, chrome: { runtime: { onMessage: { addListener: l => listener = l } } } });
    let res;
    listener({ type: 'openref-analyze', proxy }, {}, x => res = x);
    assert.equal(res.ok, true, f.name + ': ' + res.error);
    r = res.data;
  }
  await h.context.fillFromJstageArticle(r);
  return { got: { doi: r.doi, pmid: r.pmid, pdfSource: r.pdfSource }, r: JSON.parse(JSON.stringify(r)), url };
}
const C4_HOST = ['ScienceDirect の論文ページ', 'PubMed の論文ページ', 'J-STAGE の PDF タブ（URL のみ・DOI は記事ページにだけある）',
  '医書.jp の PDF ビューア（URL のみ）', '医書.jp の抄録ページ']; // ホスト名で分岐する規則に頼る物
const C4_ANY = ['J-STAGE の記事ページ', 'RSNA: Radiology の記事ページ（Atypon・dc.Identifier の先頭が publisher-id）', 'PMC の論文ページ']; // メタと URL のパスだけで決まる物
for (const name of [...C4_HOST, ...C4_ANY]) test(`proxied page with the template = the unproxied answer: ${name}`, async () => {
  const f = C0.find(x => x.name === name);
  assert.ok(f, name);
  const { got, r, url } = await identifyProxied(f, PROXY);
  assert.deepEqual(got, f.expect);
  assert.equal(r.url, url); // 取得に使う URL はプロキシのまま
  // ホスト名の規則で決めた PDF（ScienceDirect の pdfft・タブ自体）もプロキシのまま（メタの宣言は fixture では書き換えていない）
  if (['sciencedirect', 'self', 'atypon'].includes(r.pdfSource)) assert.match(r.pdfUrl, /\.ezproxy\.example\.ac\.jp\//);
  const without = (await identifyProxied(f, null)).got;
  if (C4_HOST.includes(name)) assert.notDeepEqual(without, f.expect); // 前置きが無ければホスト名の規則は当たらない（今までどおり）
  else assert.deepEqual(without, f.expect);
});
test('proxied ScienceDirect: the PDF is this article\'s pdfft on the proxy host (not a referenced PDF)', async () => {
  const { r } = await identifyProxied(C0.find(x => x.name === 'ScienceDirect の論文ページ'), PROXY);
  assert.equal(r.pdfUrl, proxify(pdf));
});
test('proxied J-STAGE PDF tab: the article page is fetched through the proxy', async () => {
  const { r } = await identifyProxied(C0.find(x => x.name === 'J-STAGE の PDF タブ（URL のみ・DOI は記事ページにだけある）'), PROXY);
  same([r.landingUrl, r.pdfUrl], [proxify(jsArticle + '/-char/ja'), proxify(jsPdf + '/-char/ja')]);
});
test('proxied PubMed: an unproxied PMC link is used; a PMC link rewritten by the proxy is not (the PMC route checks the plain URL)', () => {
  const run = links => {
    let listener;
    runContent({ URL, window: {}, location: new URL(proxify('https://pubmed.ncbi.nlm.nih.gov/21620466/')),
      document: fixtureDoc({ meta: [['citation_doi', '10.1016/S0140-6736(10)62307-0']], fullTextLinks: links }),
      chrome: { runtime: { onMessage: { addListener: l => listener = l } } } });
    let res;
    listener({ type: 'openref-analyze', proxy: PROXY }, {}, x => res = x);
    return [res.data.pmid, res.data.pmcUrl];
  };
  same(run([pmcLink]), ['21620466', pmcLink]);
  same(run([proxify(pmcLink)]), ['21620466', null]);
});
async function fillJstageProxied(declared) {
  const h = popup();
  vm.runInContext(`proxyTemplate = ${JSON.stringify(PROXY)};`, h.context);
  h.context.DOMParser = MetaParser;
  h.context.fetch = landingFetch({ [proxify(jsArticle + '/-char/ja')]: swap('citation_pdf_url', declared) });
  const p = vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(proxify(jsPdf + '/-char/ja'))})`, h.context);
  const note = await h.context.fillFromJstageArticle(p);
  return [p.doi, note];
}
for (const [name, declared, ok] of [
  ['the article page declares the publisher URL (meta not rewritten)', jsPdf, true],
  ['the article page declares the proxied URL (meta rewritten)', proxify(jsPdf), true],
  ['the article page declares another article (publisher URL)', jsPdf.replace('42_5', '42_6'), false],
  ['the article page declares another article (proxied URL)', proxify(jsPdf.replace('42_5', '42_6')), false],
  ['the article page declares a PDF on another proxied host', proxify(jsPdf.replace('www.jstage.jst.go.jp', 'www.example.org')), false],
]) test(`proxied J-STAGE identity check: ${name}`, async () => {
  const [doi, note] = await fillJstageProxied(declared);
  if (ok) same([doi, note], ['10.20844/zzzz.42.1_5', '']);
  else { assert.equal(doi, null); assert.match(note, /記事ページの PDF がこのタブと別/); }
});

/* 失敗の分類と同居: プロキシのホスト名の ezproxy の段を「ログイン画面」と読まない（前置きがあるとき）。プロキシのログイン画面は残る */
for (const [url, want] of [
  [proxify(pdf), false],
  [proxify('https://www.example-journal.org/article/123'), false],
  ['https://login.ezproxy.example.ac.jp/login?qurl=https%3a%2f%2fwww.sciencedirect.com', true],
  ['https://ezproxy.example.ac.jp/login', true],
  ['https://login.ezproxy.example.ac.jp/connect?session=x', true], // ホストだけがログインの印
  ['https://idp.example.ac.jp/idp/profile/SAML2/Redirect/SSO', true],
]) test(`login URL rule with the template: ${url} → ${want}`, () => {
  const h = worker();
  vm.runInContext(`J.proxyTemplate = ${JSON.stringify(PROXY)};`, h.context);
  assert.equal(vm.runInContext(`OpenRefShared.isLoginUrl(${JSON.stringify(url)}, J.proxyTemplate)`, h.context), want);
});
test('without the template a proxied host still reads as a login page (today\'s rule: the ezproxy label)', () => {
  assert.equal(inJob(`OpenRefShared.isLoginUrl(${JSON.stringify(proxify(pdf))}, J.proxyTemplate)`), true);
});
test('a proxied DOI landing without a PDF declaration is no_pdf with the template (login without it)', async () => {
  const landingUrl = proxify('https://www.example-journal.org/article/123');
  const run = async proxy => {
    const h = landingHarness([page({ doi: '10.1234/ej.2025.123' })], { landingUrl });
    vm.runInContext(`J.proxyTemplate = ${JSON.stringify(proxy)};`, h.context);
    return (await via(h, "'10.1234/ej.2025.123'", 'null').then(() => null, e => e)).kind;
  };
  assert.equal(await run(PROXY), 'no_pdf');
  assert.equal(await run(null), 'login');
});

/* 配線: 保存を押してから。fetch の URL と analyze に添えた前置きを記録する */
const recordC4 = `
  var fetchedUrls = [], analyzed = [];
  const tab0 = sendToTab, bg0 = bgFetchPdf;
  sendToTab = async (msg, tabId) => {
    if (msg.type === 'openref-fetch-pdf') fetchedUrls.push(msg.url);
    if (msg.type === 'openref-analyze') analyzed.push(msg.proxy);
    return tab0(msg, tabId);
  };
  bgFetchPdf = async (url, follow) => { fetchedUrls.push(url); return bg0(url, follow); };`;
const ej = '10.1234/ej.2025.123';
const unauthorized401 = { ok: false, error: 'Error: HTTP 401', html: false, httpStatus: 401, url: artPdf };
for (const [name, proxy, opened] of [
  ['the template set (EZproxy)', PROXY, PROXY + 'https://doi.org/' + ej],
  ['the template set (OpenAthens)', OA, 'https://go.openathens.net/redirector/example.ac.jp?url=' + encodeURIComponent('https://doi.org/' + ej)],
  ['no template', null, 'https://doi.org/' + ej],
  ['an invalid stored template (http)', PROXY.replace('https:', 'http:'), 'https://doi.org/' + ej],
]) test(`layer 2 with ${name}: the DOI page opens at the expected URL; the declared PDF is fetched as declared`, async () => {
  const { app, lines, headLines } = await saveAndRead({ proxy,
    setup: articleTab({ doi: ej }, unauthorized401) + recordC4,
    routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaNone) } },
    { doi: ej, pdfUrl: artPdf, pdfSource: 'meta' });
  same(app.created, [{ url: opened, active: false }]);
  same(app.sw.fetchedUrls, [artPdf, artPdf]); // タブ内 → 拡張。どちらも宣言された URL のまま（前置きは付けない）
  const sent = proxy === null || proxy.startsWith('http:') ? null : proxy;
  same([...app.context.analyzed, ...app.sw.analyzed], [sent, sent]); // 論文ページ（popup）と DOI ページ（工程）の解析に前置きを添える
  assert.equal(headLines.at(-1), NEXT.login);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（ログインが要る）', '登録 K1']);
});

/* 「プロキシ経由で開く」: 分類が login / stuck で前置きがあるときだけ。押すと前面の新しいタブで開く */
const htmlAtArt = { ok: false, error: 'Error: PDF ではない応答（5000 bytes・HTML の可能性）', html: true, httpStatus: 0, url: artPdf };
const idpAnswer = { ok: false, error: 'Error: PDF ではない応答', html: true, httpStatus: 0, url: 'https://idp.uni.example.ac.jp/idp/profile/SAML2/Redirect/SSO' };
const C4_FLOWS = {
  // DOI ページの宣言された PDF が 401。ページに PDF は無い → DOI ページを開く
  login: [{ setup: articleTab({ doi: ej }, unauthorized401), routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaNone) } },
    { doi: ej, pdfUrl: artPdf, pdfSource: 'meta' }, 'https://doi.org/' + ej],
  // ページ上のリンク拾い（link）の PDF が IdP に着いた → 本体とは限らないので開くのは DOI ページ
  'login (a link-picked PDF)': [{ allSites: false, declined: true, setup: articleTab({ doi: ej, pdfUrl: artPdf, pdfSource: 'link' }, idpAnswer),
    routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaBot) } }, {}, 'https://doi.org/' + ej],
  // ページが宣言した PDF のタブが HTML のまま → 開くのはその PDF
  stuck: [{ setup: articleTab({ doi: ej, pdfUrl: artPdf, pdfSource: 'meta' }, htmlAtArt), routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaBot) } },
    { doi: ej }, artPdf],
  no_pdf: [{ setup: articleTab({ doi: ej }, { ok: false, error: 'x' }), routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaNone) } },
    { doi: ej }, null],
  // PubMed の PMC 本文が確認ページのまま（プロキシで開き直しても通らない）
  challenge: [{ landingUrl: pmcLink, setup: articleTab({ pmid: '21620466', pmcUrl: pmcLink }, { ok: false, error: 'x' }),
    routes: { '/capture/save': savedK1(null), '/capture/fetch-oa': apiError(422, oaText('pmc: PMC3062508 は AWS Open Data に未収載（OA サブセット外か未同期）', 'unpaywall: DOI なし', 'crossref/doi.org: DOI なし')) } },
    {}, null],
  mismatch: [{ allSites: false, declined: true, setup: articleTab({ doi: ej, pdfUrl: artPdf, pdfSource: 'meta' }, { ok: true, b64: btoa('%PDF-test') }),
    routes: { '/capture/save': savedK1(ej), '/capture/attachment': apiError(409, '別論文（DOI 10.1000/other）の PDF です', { status: 'mismatch', found_doi: '10.1000/other' }) } }, {}, null],
  'no class (HTTP 403)': [{ allSites: false, declined: true, setup: articleTab({ doi: ej, pdfUrl: artPdf, pdfSource: 'meta' }, { ok: false, error: 'Error: HTTP 403', html: false, httpStatus: 403, url: artPdf }),
    routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaBot) } }, {}, null],
};
/* 「プロキシ経由で開く」（新しいタブ）は、全サイト許可の無い popup の動き。全サイト許可のある popup では、同じボタンが確認の窓
   になる（その試験は確認の窓の節）。ここの流れの多くは全サイト許可ありで保存しないと分類が出ない（PDF タブ・DOI ページ経由）ので、
   保存の後に全サイト許可を外した popup を開き直して、「プロキシ経由で開く」の動きを見る（期待する値は確認の窓が無いときと同じ） */
async function reopenWithoutAllSites(app, opts) {
  app.env.shared.permissions.contains = async () => false;
  const again = popupApp(art, {}, Object.assign({}, opts, { env: app.env }));
  again.context.landingData = app.context.landingData;
  await vm.runInContext('init()', again.context);
  return again;
}
for (const [cls, [opts, landing, target]] of Object.entries(C4_FLOWS)) {
  test(`button with the template, ${cls}: ${target ? 'shown, opens the proxied ' + (target === artPdf ? 'PDF' : 'DOI page') : 'hidden'}`, async () => {
    const o = Object.assign({ proxy: PROXY }, opts);
    const { app: first, lines, headLines } = await saveAndRead(o, landing);
    const base = cls.split(' ')[0];
    if (base in NEXT) assert.equal(headLines.at(-1), NEXT[base]);
    else assert.doesNotMatch(lines.join('\n'), /分類:/);
    assert.equal(lines[0], '書誌 ✓ ・ PDF ✗（' + (WHY[base] || '取得できず') + '）');
    const app = await reopenWithoutAllSites(first, o);
    assert.equal(app.els.viaproxy.style.display, target ? 'block' : 'none');
    if (!target) return;
    const before = app.created.length;
    await app.els.viaproxy.handlers.click();
    same(app.created.slice(before), [{ url: PROXY + target, active: true }]);
  });
  test(`button without the template, ${cls}: hidden`, async () => {
    const { app: first } = await saveAndRead(opts, landing);
    const app = await reopenWithoutAllSites(first, opts);
    assert.equal(app.els.viaproxy.style.display, 'none');
  });
}
test('the PMC route opens the plain PMC link even with the template (PMC is not proxied)', async () => {
  const [opts, landing] = C4_FLOWS.challenge;
  const { app } = await saveAndRead(Object.assign({ proxy: PROXY }, opts), landing);
  same(app.created[0], { url: pmcLink, active: false });
});
test('button on a page already inside the proxy: the target goes back to the publisher URL before the prefix (no double proxy)', async () => {
  const pAtArt = proxify(artPdf);
  const o = { proxy: PROXY,
    setup: articleTab({ doi: ej, pdfUrl: pAtArt, pdfSource: 'meta', url: proxify(art) }, Object.assign({}, htmlAtArt, { url: pAtArt })),
    routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaBot) } };
  const { app: first, lines, headLines } = await saveAndRead(o, { doi: ej });
  assert.equal(headLines.at(-1), NEXT.stuck);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（途中のページで止まった）', '登録 K1']);
  const app = await reopenWithoutAllSites(first, o);
  await app.els.viaproxy.handlers.click();
  same(app.created.at(-1), { url: PROXY + artPdf, active: true });
});
test('a proxied PDF tab without identifiers: /capture/pdf stores the publisher URL; the PDF is fetched from the proxy host', async () => {
  const tabUrl = 'https://www-example--society-org.ezproxy.example.ac.jp/files/a.pdf';
  for (const [proxy, want] of [[PROXY, 'https://www.example-society.org/files/a.pdf'], [null, tabUrl]]) {
    const app = await openAndSave(tabUrl, {}, { proxy, setup: `
      var bgUrls = [];
      const bgFirst = bgFetchPdf;
      bgFetchPdf = async (url, follow) => { bgUrls.push(url); return bgFirst(url, follow); };` });
    const call = app.calls.find(c => c[0].startsWith('/capture/pdf?'));
    assert.ok(call, 'no /capture/pdf');
    assert.equal(new URLSearchParams(call[0].split('?')[1]).get('url'), want);
    same(app.sw.bgUrls, [tabUrl]);
  }
});
test('button with an OpenAthens template opens the encoded URL', async () => {
  const [opts, landing] = C4_FLOWS.stuck;
  const o = Object.assign({ proxy: OA }, opts);
  const { app: first } = await saveAndRead(o, landing);
  const app = await reopenWithoutAllSites(first, o);
  await app.els.viaproxy.handlers.click();
  same(app.created.at(-1), { url: 'https://go.openathens.net/redirector/example.ac.jp?url=' + encodeURIComponent(artPdf), active: true });
});
test('a second save hides the button again until its own class says so', async () => {
  const [opts, landing] = C4_FLOWS.login;
  const { app } = await saveAndRead(Object.assign({ proxy: PROXY }, opts), landing);
  assert.equal(app.els.viaproxy.style.display, 'block');
  vm.runInContext(`api = async () => { throw new Error('connectd が落ちた'); };`, app.sw);
  await app.els.save.handlers.click();
  assert.equal(app.els.viaproxy.style.display, 'none');
});

/* 題名のみの保存: 保存される URL はプロキシのホストを戻した出版社の URL（前置きが無ければタブの URL のまま） */
for (const [name, proxy, want] of [['the template set', PROXY, abstractPage.url], ['no template', null, proxify(abstractPage.url)]]) {
  test(`title-only save sends the publisher URL (${name})`, async () => {
    const data = Object.assign(analyzeFixture(abstractPage), { url: proxify(abstractPage.url) }); // www-example--society-org.<proxy>
    const { app } = await saveAndRead({ proxy, setup: articleTab(data, { ok: false, error: 'x' }),
      routes: { '/capture/save': { key: 'K2', status: 'created', doi: null, has_pdf: false, title: 'T' } } });
    assert.equal(app.calls.find(c => c[0] === '/capture/save')[1].url, want);
  });
}
test('wiring: a proxied J-STAGE PDF tab takes the DOI from the proxied article page; the PDF is fetched from the proxy host', async () => {
  const tabUrl = proxify(jsPdf + '/-char/ja');
  const app = await openAndSave(tabUrl, { [proxify(jsArticle + '/-char/ja')]: jsLanding }, { proxy: PROXY, setup: `
    var bgUrls = [];
    const bgFirst = bgFetchPdf;
    bgFetchPdf = async (url, follow) => { bgUrls.push(url); return bgFirst(url, follow); };` });
  same(app.fetched.map(f => f[0]), [proxify(jsArticle + '/-char/ja')]);
  same(app.calls.map(c => c[0]), ['/capture/status', '/capture/collections', '/capture/save', '/capture/attachment?key=K1&source=self&via=page']);
  same([app.calls[2][1].doi, app.calls[2][1].url], ['10.20844/zzzz.42.1_5', jsPdf + '/-char/ja']);
  same(app.sw.bgUrls, [tabUrl]);
});

/* 設定のページ（options.html + options.js）。chrome.storage.local は差し替え */
function optionsPage(stored = {}, permissions = undefined) {
  const els = {};
  const el = id => els[id] || (els[id] = { textContent: '', className: '', value: '', handlers: {}, addEventListener(t, f) { this.handlers[t] = f; } });
  const store = Object.assign({}, stored);
  const writes = [];
  const ctx = vm.createContext({ URL, document: { getElementById: el }, chrome: { storage: { local: {
    get: async k => (k in store ? { [k]: store[k] } : {}),
    set: async o => { writes.push(['set', JSON.parse(JSON.stringify(o))]); Object.assign(store, o); },
    remove: async k => { writes.push(['remove', k]); delete store[k]; },
  } }, permissions } }); // permissions = 全サイト許可のスイッチの試験だけ（無ければ権限の段は「確かめられません」で止まる）
  vm.runInContext(code('pdfbytes.js'), ctx);
  vm.runInContext(code('options.js'), ctx);
  const submit = async value => {
    await tick(); // load（保存済みの値を欄へ）が先に終わる
    el('proxy').value = value;
    let prevented = false;
    await el('form').handlers.submit({ preventDefault() { prevented = true; } });
    assert.ok(prevented, 'submit がページを読み直す');
    return { status: el('status').textContent, cls: el('status').className };
  };
  return { ctx, els, store, writes, submit };
}
const tick = () => new Promise(r => setImmediate(r));
test('options: the stored template is shown; the key is the one the popup reads', async () => {
  const o = optionsPage({ openref_proxy_template: PROXY });
  await tick();
  assert.equal(o.els.proxy.value, PROXY);
  assert.equal(vm.runInContext('PROXY_KEY', o.ctx), inPopup('PROXY_KEY'));
});
for (const [name, value, saved, host] of [
  ['EZproxy', PROXY, PROXY, 'login.ezproxy.example.ac.jp'],
  ['OpenAthens', OA, OA, 'go.openathens.net'],
  ['surrounding spaces are trimmed', '  ' + PROXY + '  ', PROXY, 'login.ezproxy.example.ac.jp'],
]) test(`options: a valid template is saved and its destination host shown (${name})`, async () => {
  const o = optionsPage();
  const { status, cls } = await o.submit(value);
  same(o.writes, [['set', { openref_proxy_template: saved }]]);
  assert.equal(cls, 'ok');
  assert.ok(status.includes('送り先: ' + host + '\n'), status);
  assert.ok(status.includes(shared().proxied(saved, 'https://doi.org/10.1000/xyz123')), status);
});
for (const [name, value, why] of [
  ['http://', PROXY.replace('https:', 'http:'), /https:\/\/ で始まる URL/],
  ['no scheme', 'login.ezproxy.example.ac.jp/login?url=', /https:\/\/ で始まる URL/],
  ['javascript:', 'javascript:alert(1)//', /https:\/\/ で始まる URL/],
  ['a space inside', 'https://login.ezproxy.example.ac.jp/login?url= x', /空白/],
  ['no path', 'https://login.ezproxy.example.ac.jp', /パスが要ります/],
  ['{url} in the host', 'https://{url}.example.ac.jp/x', /\{url\} はホスト名の中に置けません/],
  ['user:pw@', 'https://user:pw@login.ezproxy.example.ac.jp/login?url=', /ユーザー名・パスワード/],
  ['a backslash', 'https://evil.com\\@login.ezproxy.example.ac.jp/login?url=', /\\ は使えません/],
  ['a percent-encoded host', 'https://%6Cogin.ezproxy.example.ac.jp/login?url=', /読み違え/],
]) test(`options: an invalid template is refused with its reason and the stored one kept (${name})`, async () => {
  const o = optionsPage({ openref_proxy_template: PROXY });
  const { status, cls } = await o.submit(value);
  same(o.writes, []);
  assert.equal(o.store.openref_proxy_template, PROXY);
  assert.equal(cls, 'err');
  assert.match(status, why);
  assert.match(status, /保存していません/);
});
test('options: an empty field turns the proxy off (removes the setting)', async () => {
  const o = optionsPage({ openref_proxy_template: PROXY });
  const { cls } = await o.submit('   ');
  same(o.writes, [['remove', 'openref_proxy_template']]);
  assert.equal('openref_proxy_template' in o.store, false);
  assert.equal(cls, 'ok');
});
test('manifest: the options page is registered and no permission was added', () => {
  const m = JSON.parse(code('manifest.json'));
  assert.equal(m.options_ui.page, 'options.html');
  same(m.permissions.filter(x => !C7_PERMISSIONS.includes(x)), ['activeTab', 'scripting', 'storage', 'alarms']); // 応答ヘッダの見張りの 2 つはその試験が見る
  same(m.host_permissions, ['http://127.0.0.1/*']);
  same(m.optional_host_permissions, ['https://*/*', 'http://*/*']);
  assert.match(code('options.html'), /<script src="pdfbytes\.js"><\/script>\s*<script src="options\.js"><\/script>/);
  assert.match(code('popup.html'), /id="viaproxy"[^>]*>プロキシ経由で開く</);
});

/* ===== ショートカット（manifest の _execute_action）と、押せるようになった保存ボタンへのフォーカス ===== */
const c10Tab = jsPdf + '/-char/ja';
const c10Pages = { [jsArticle + '/-char/ja']: jsLanding };
test('manifest: _execute_action is Ctrl+Shift+S (Command+Shift+S on Mac) and no permission was added', () => {
  const m = JSON.parse(code('manifest.json'));
  same(m.commands, { _execute_action: { suggested_key: { default: 'Ctrl+Shift+S', mac: 'Command+Shift+S' } } });
  same(m.permissions.filter(x => !C7_PERMISSIONS.includes(x)), ['activeTab', 'scripting', 'storage', 'alarms']);
  same(m.host_permissions, ['http://127.0.0.1/*']);
});
test('the save button gets the focus once it can be pressed (Enter saves)', async () => {
  const app = popupApp(c10Tab, c10Pages);
  assert.equal(app.doc.activeElement.id, 'body');
  await vm.runInContext('init()', app.context);
  assert.equal(app.els.save.disabled, false);
  assert.equal(app.doc.activeElement.id, 'save');
});
test('no focus while the button cannot be pressed (the article page is still loading)', async () => {
  const app = popupApp(c10Tab, () => new Promise(() => {})); // 記事ページが答えない = 解析が終わらない（上限のタイマーは発火させない）
  vm.runInContext('init()', app.context);
  for (let i = 0; i < 5; i++) await tick();
  same(app.calls.map(c => c[0]), ['/capture/status', '/capture/collections']); // 解析の途中まで進んでいる
  assert.equal(app.els.save.disabled, true);
  assert.equal(app.doc.activeElement.id, 'body');
});
test('the analysis ends with nothing to save (no identifier, no title, no PDF): disabled and not focused', async () => {
  const empty = { doi: null, pmid: null, pdfUrl: null, pdfSource: null, title: null, url: 'https://www.example.org/', isPdf: false, landingUrl: null };
  const app = popupApp('https://www.example.org/', c10Pages, { setup: `
    chrome.scripting.executeScript = async () => {};
    chrome.tabs.query = async () => [{ id: 1, url: 'https://www.example.org/', title: '' }];
    sendToTab = async () => ({ ok: true, data: ${JSON.stringify(empty)} });` });
  await vm.runInContext('init()', app.context);
  assert.equal(app.els.title.textContent, '（タイトル不明）'); // 描画まで進んだ
  assert.equal(app.els.save.disabled, true);
  assert.equal(app.doc.activeElement.id, 'body');
});
test('connectd unreachable: the button stays disabled and the focus is not placed', async () => {
  const app = popupApp(c10Tab, c10Pages, { routes: { '/capture/status': new Error('fetch failed') } });
  await vm.runInContext('init()', app.context);
  assert.equal(app.els.save.disabled, true);
  assert.equal(app.doc.activeElement.id, 'body');
});
test('the user is on the folder select: the focus is not taken', async () => {
  const app = popupApp(c10Tab, c10Pages);
  app.doc.activeElement = app.doc.getElementById('collection');
  await vm.runInContext('init()', app.context);
  assert.equal(app.els.save.disabled, false);
  assert.equal(app.doc.activeElement.id, 'collection');
});
test('the click (Enter on the focused button) disables the button at once: one save', async () => {
  const app = popupApp(c10Tab, c10Pages);
  await vm.runInContext('init()', app.context);
  assert.equal(app.doc.activeElement.id, 'save');
  const done = app.els.save.handlers.click();
  assert.equal(app.els.save.disabled, true); // 同期で押せなくなる = 2 回目の Enter は効かない
  await done;
  assert.equal(app.calls.filter(c => c[0] === '/capture/save').length, 1);
});

/* ===== 保存の工程は service worker（job.js）で走る。工程ごとのオブジェクト・記録 1 件・同時に 1 件・延命・中断と追い越し ===== */
const c6Doi = d => '10.5555/c6-' + d;
const c6Page = (d, extra = {}) => Object.assign({ doi: c6Doi(d), pmid: null, pdfUrl: 'https://pub.example.org/' + d + '.pdf', pdfSource: 'meta', title: 'T ' + d, url: 'https://pub.example.org/' + d }, extra);
const c6Input = (d, extra = {}) => ({ tabId: 1, tabUrl: 'https://pub.example.org/' + d, page: c6Page(d), contentScriptOk: false, collectionId: '', proxy: null, ...extra });
/* 工程の service worker 側だけ（記録は空から）。gates[doi] があれば、その論文の PDF の拡張 fetch は gate が開くまで待つ。
   /capture/save は送られた DOI から key を決める（K-<doi の末尾>） */
function c6Worker() {
  const h = worker({ session: fakeSession() });
  h.gates = {};
  h.context.gates = h.gates;
  vm.runInContext(`
    bgFetchPdf = async (url) => { const g = gates[url]; if (g) await g.promise; return { ok: true, b64: btoa('%PDF-test') }; };
    api = async (url, opts) => {
      fakeApi(url);
      if (url === '/capture/save') { const doi = JSON.parse(opts.body).doi; return { key: 'K-' + doi.split('-').pop(), status: 'created', doi, has_pdf: false, title: 'T' }; }
      if (url.startsWith('/capture/attachment?')) return { status: 'attached', bytes: 9, doi_check: 'match' };
      throw new Error('unexpected route ' + url);
    };`, h.context);
  h.gate = d => { let open; const promise = new Promise(r => { open = r; }); h.gates['https://pub.example.org/' + d + '.pdf'] = { promise }; return open; };
  h.start = input => vm.runInContext(`startJob(${JSON.stringify(input)})`, h.context);
  h.LIMIT = vm.runInContext('JOB_LIMIT_MS', h.context);
  return h;
}
test('a job aborted at the time limit sends nothing more to connectd and its late result does not overwrite the record', async () => {
  const h = c6Worker();
  const open = h.gate('a');
  const a = await h.start(c6Input('a'));
  assert.equal(a.ok, true);
  await tick();
  same(h.posts, ['/capture/save']); // 書誌は保存済み・PDF の取得で待っている
  same(h.record().state, 'running');
  const limit = h.longTimers.filter(t => t.ms === h.LIMIT);
  assert.equal(limit.length, 1, '工程の上限のタイマーが 1 つ');
  limit[0].f();
  await tick();
  same([h.record().state, h.record().cls], ['aborted', 'err']);
  assert.match(h.record().line, /中断: 保存が 15 分の上限を超えました/);
  assert.equal(h.intervals[0].on, false); // 延命も止まる
  open();
  await a.done;
  same(h.posts, ['/capture/save']); // 取れた PDF を /capture/attachment へ送らない
  same(h.record().state, 'aborted'); // 遅れて来た結果で記録を書き換えない
  same(h.badges.at(-1), '!');
});
test('an aborted job stays stopped even when the "aborted" record could not be written (storage refused)', async () => {
  const h = c6Worker();
  const open = h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  const set = h.session.area.set;
  h.session.area.set = async () => { throw new Error('QUOTA_BYTES quota exceeded'); };
  h.longTimers.find(t => t.ms === h.LIMIT).f();
  await tick();
  h.session.area.set = set;
  same(h.record().state, 'running'); // 記録は書けなかった（走行中のまま）
  open();
  await a.done;
  same(h.posts, ['/capture/save']); // それでも PDF は送らない（工程の中断の印）
});
test('a job overtaken by a later save (its record is older than the limit) does not send its PDF to the earlier item', async () => {
  const h = c6Worker();
  const openA = h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  same(h.posts, ['/capture/save']);
  h.advance(h.LIMIT + 1); // 上限を過ぎた（A の上限のタイマーはまだ発火していない = 中断の印は付いていない）
  const b = await h.start(c6Input('b', { tabUrl: 'https://pub.example.org/b' }));
  assert.equal(b.ok, true, '古い走行中の記録は新しい保存を塞がない');
  await b.done;
  same(h.posts, ['/capture/save', '/capture/save', '/capture/attachment?key=K-b&source=meta&via=page']);
  openA();
  await a.done;
  same(h.posts, ['/capture/save', '/capture/save', '/capture/attachment?key=K-b&source=meta&via=page']); // A の PDF は送らない
  same([h.record().id, h.record().state], [b.id, 'done']);
  assert.match(h.record().line, /^書誌 ✓ ・ PDF ✓\n登録 K-b — PDF 添付 ✓/);
});
test('a second save while one is running is refused; the running one is not disturbed', async () => {
  const h = c6Worker();
  const open = h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  const b = await h.start(c6Input('b', { tabId: 2, tabUrl: 'https://pub.example.org/b' }));
  same([b.ok, b.busy], [false, true]);
  assert.match(b.error, /別の保存が進行中です/);
  same(h.record().id, a.id);
  open();
  await a.done;
  same(h.posts, ['/capture/save', '/capture/attachment?key=K-a&source=meta&via=page']);
  same(h.record().state, 'done');
});
test('a running record older than the limit does not block a new save, and the popup reads it as aborted', async () => {
  const h = c6Worker();
  await h.session.area.set({ openref_job: { id: 'old', tabId: 3, tabUrl: 'https://x.example.org/', startedAt: 0, state: 'running', line: 'PDF を取得中…', cls: '' } });
  h.advance(h.LIMIT);
  const seen = await vm.runInContext('currentRecord()', h.context);
  same([seen.id, seen.state, seen.cls], ['old', 'aborted', 'err']);
  assert.match(seen.line, /中断: 保存が 15 分の上限を超えました/);
  await h.session.area.set({ openref_job: { id: 'old2', tabId: 3, tabUrl: 'https://x.example.org/', startedAt: 0, state: 'running', line: '', cls: '' } });
  const r = await h.start(c6Input('a'));
  assert.equal(r.ok, true);
  await r.done;
  same([h.record().id, h.record().state], [r.id, 'done']);
});
test('a running record younger than the limit is still refused (the limit is the boundary)', async () => {
  const h = c6Worker();
  await h.session.area.set({ openref_job: { id: 'run', tabId: 3, tabUrl: 'https://x.example.org/', startedAt: 0, state: 'running', line: '', cls: '' } });
  h.advance(h.LIMIT - 1);
  const r = await h.start(c6Input('a'));
  same([r.ok, r.busy], [false, true]);
  same((await vm.runInContext('currentRecord()', h.context)).state, 'running');
});
/* background.js を service worker と同じく読む（importScripts で pdfbytes.js・job.js）。session = 起動の時点の chrome.storage.session */
function bootWorker(initial) {
  const ses = fakeSession(initial);
  const removed = [], badges = [];
  let listener = null;
  const noop = { addListener() {} };
  const ctx = vm.createContext({ URL, Uint8Array, TextDecoder, btoa, atob,
    Date: { now: () => 1000 }, setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {},
    importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
    chrome: {
      runtime: { id: 'ext', getURL: x => 'chrome-extension://ext/' + x, onMessage: { addListener: f => { listener = f; } } },
      alarms: { onAlarm: noop, create() {}, clear() {} }, tabs: { onRemoved: noop, remove: async id => removed.push(id) },
      storage: { session: ses.area }, action: { setBadgeText: async o => badges.push(o.text), setBadgeBackgroundColor: async () => {} },
    },
  });
  vm.runInContext(code('background.js'), ctx);
  return { ctx, ses, removed, badges, listener: () => listener, record: () => clone(ses.store.openref_job) };
}
test('service worker start: a running record left by the previous worker becomes aborted; its background tabs are closed', async () => {
  const b = bootWorker({ openref_job: { id: 'x', tabId: 1, tabUrl: 'https://pub.example.org/a', startedAt: 900, state: 'running', line: 'PDF を取得中…（拡張から fetch）', cls: '' },
    openref_hidden_tabs: { 7: 900 } });
  await tick();
  same([b.record().state, b.record().cls], ['aborted', 'err']);
  assert.match(b.record().line, /中断: 拡張の背景処理（service worker）が保存の途中で止まりました/);
  same(b.badges, ['!']);
  same(b.removed, [7]);
  for (const state of ['done', 'error', 'aborted']) { // 終わった記録はそのまま
    const done = bootWorker({ openref_job: { id: 'y', tabId: 1, tabUrl: 'u', startedAt: 900, state, line: 'L', cls: 'done' } });
    await tick();
    same([done.record().state, done.record().line, done.badges], [state, 'L', []]);
  }
});
test('the worker takes openref-save / openref-job only from the extension\'s own pages', async () => {
  const b = bootWorker({});
  await tick();
  for (const sender of [{ id: 'ext', url: 'https://pub.example.org/a', tab: { id: 1 } }, { id: 'other', url: 'chrome-extension://other/popup.html' }, {}]) {
    let answered = false;
    const r = b.listener()({ type: 'openref-save', input: c6Input('a') }, sender, () => { answered = true; });
    await tick();
    same([r, answered, b.record()], [false, false, undefined]);
  }
  const res = await new Promise(resolve => b.listener()({ type: 'openref-job' }, { id: 'ext', url: 'chrome-extension://ext/popup.html' }, resolve));
  same(res, { ok: true, record: null });
});
test('keepalive: an extension API every 20 s while the job waits on connectd; stopped when the job ends', async () => {
  const h = c6Worker();
  let release;
  const slow = new Promise(r => { release = r; });
  h.context.slow = slow;
  vm.runInContext(`const fastApi = api; api = async (url, opts) => { if (url === '/capture/save') await slow; return fastApi(url, opts); };`, h.context);
  const r = await h.start(c6Input('a'));
  await tick();
  same(h.posts, []); // /capture/save の応答を待っている
  same(h.intervals.map(i => [i.ms, i.on]), [[20000, true]]);
  for (let i = 0; i < 3; i++) h.intervals[0].f(); // 60 秒の待ち
  await tick();
  assert.equal(h.pings(), 3);
  release();
  await r.done;
  same(h.intervals.map(i => i.on), [false]);
  same(h.record().state, 'done');
});
test('the badge: running, then ✓ when the PDF is in OpenRef, ! when it is not, ! on an error', async () => {
  const ok = c6Worker();
  await (await ok.start(c6Input('a'))).done;
  same(ok.badges, ['…', '✓']);
  same([ok.record().cls, ok.record().line.split('\n')[0]], ['done', '書誌 ✓ ・ PDF ✓']); // 色はバッジと同じ規則
  const noPdf = c6Worker();
  noPdf.context.chrome.permissions.contains = async () => false; // 層 2（DOI ページの背景タブ）へは進まない
  vm.runInContext(`bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });
    const api0 = api; api = async (url, opts) => { if (url === '/capture/fetch-oa') { fakeApi(url); throw Object.assign(new Error('PDF を自動取得できませんでした'), { status: 422 }); } return api0(url, opts); };`, noPdf.context);
  await (await noPdf.start(c6Input('a'))).done;
  same(noPdf.badges, ['…', '!']);
  assert.match(noPdf.record().line, /書誌は保存済み。PDF は取得できませんでした/);
  same([noPdf.record().cls, noPdf.record().line.split('\n')[0]], ['warn', '書誌 ✓ ・ PDF ✗（取得できず）']);
  const err = c6Worker();
  vm.runInContext(`api = async () => { throw new Error('connectd が落ちた'); };`, err.context);
  await (await err.start(c6Input('a'))).done;
  same([err.badges, err.record().state, err.record().line, err.record().cls], [['…', '!'], 'error', '書誌 ✗ ・ PDF —\n✗ connectd が落ちた', 'err']);
});
test('the user tab moved to another page: no page fetch from it; the extension fetch is still tried', async () => {
  const h = worker();
  const sent = [];
  h.context.sent = sent;
  vm.runInContext(`sendToTab = async (msg, tabId) => { sent.push([msg.type, tabId]); return { ok: true, b64: btoa('%PDF-tab') }; };
    bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });`, h.context);
  h.userTab.url = 'https://other.example.org/elsewhere';
  const got = await vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
  same(sent, []);
  assert.equal(got.bytes, null);
  assert.match(got.error, /ページ内 fetch: 利用者のタブが別のページへ移ったため未試行 \/ 拡張 fetch: HTTP 403/);
  const err = await vm.runInContext(`fetchPdfViaTabThenBg(J, 1, ${JSON.stringify(pdf)}, true).then(() => null, e => e)`, h.context);
  same(sent, []);
  assert.match(err.message, /タブ内: 利用者のタブが別のページへ移ったため未試行/);
  h.userTab.url = article + '#section-2'; // 同じページの中の移動（断片）は移ったとみなさない
  const same2 = await vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
  same([sent, same2.how], [[['openref-fetch-pdf', 1]], 'ページ内']);
});
/* popup の側: 記録の表示（開き直し・別のタブ）。保存の中身はプロキシの節の articleTab と同じ論文ページ */
const c6Setup = articleTab({ doi: ej, pdfUrl: artPdf, pdfSource: 'meta' }, { ok: true, b64: btoa('%PDF-test') });
const c6Routes = gate => ({ '/capture/save': savedK1(ej), '/capture/attachment': () => gate.then(() => ({ status: 'attached', bytes: 9, doi_check: 'match' })) });
test('reopening the popup on the same page shows the running job (save disabled), then its result; a later reopen can save again', async () => {
  let open;
  const gate = new Promise(r => { open = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gate) });
  await vm.runInContext('init()', first.context);
  await first.clickRaw();
  await tick();
  same(first.record().state, 'running'); // popup を閉じた（first はもう見ない）
  const again = popupApp(art, {}, { env: first.env, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', again.context);
  same([again.els.status.textContent, again.els.save.disabled], [first.record().line, true]);
  assert.match(again.els.status.textContent, /PDF を OpenRef へ転送中/);
  same(again.els.ids.textContent, first.record().ids);
  open();
  await first.env.settle();
  const rec = first.record();
  same([rec.state, rec.cls], ['done', 'done']);
  same([again.els.status.textContent, again.els.status.className], [rec.line, 'done']); // 状態行 = 記録
  assert.match(rec.line, /^書誌 ✓ ・ PDF ✓\n登録 K1 — PDF 添付 ✓/);
  same(again.els.save.disabled, true); // 走行を見た popup の成功は押せないまま（二重保存を避ける）
  same(first.env.badges.at(-1), ''); // 結果を出した popup がバッジを消す
  const later = popupApp(art, {}, { env: first.env, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', later.context);
  same([later.els.status.textContent, later.els.save.disabled], [rec.line, false]); // 開き直しは結果を出し、保存は押せる
});
test('a job that ends while the reopened popup is still analysing the page is shown as ended (no stale "running")', async () => {
  let openAtt, openAn;
  const gAtt = new Promise(r => { openAtt = r; });
  const gAn = new Promise(r => { openAn = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gAtt) });
  await vm.runInContext('init()', first.context);
  await first.clickRaw();
  await tick();
  same(first.record().state, 'running');
  const again = popupApp(art, {}, { env: first.env, allSites: false, declined: true, setup: c6Setup +
    "const an0 = sendToTab; sendToTab = async (m, id) => { if (m.type === 'openref-analyze') await anGate; return an0(m, id); };" });
  again.context.anGate = gAn;
  const opening = vm.runInContext('init()', again.context);
  await tick(); // openref-job は答えた（走行中）・解析は待っている
  openAtt();
  await first.env.settle(); // 解析の間に工程が終わった
  same(first.record().state, 'done');
  openAn();
  await opening;
  same([again.els.status.textContent, again.els.status.className], [first.record().line, 'done']);
  same(again.els.save.disabled, false); // 走行を見ていない（開き直し）= 押せる
});
test('a popup on another tab: a note while the other save runs; the worker refuses its save; the note goes when the other ends', async () => {
  let open;
  const gate = new Promise(r => { open = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gate) });
  await vm.runInContext('init()', first.context);
  await first.clickRaw();
  await tick();
  const other = popupApp('https://www.example-journal.org/article/456', {}, { env: first.env, tabId: 2, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', other.context);
  assert.match(other.els.status.textContent, /別のタブ（またはこのタブの前のページ）の保存が進行中です/);
  same(other.els.save.disabled, false);
  await other.clickRaw();
  assert.match(other.els.status.textContent, /^✗ 別の保存が進行中です/);
  same([other.els.status.className, other.els.save.disabled], ['err', false]);
  same(first.calls.filter(c => c[0] === '/capture/save').length, 1); // 2 件目は connectd へ行かない
  open();
  await first.env.settle();
  // 走行中の一言は、その保存の結果の 1 行に替わる（題名と結果の 1 行目・2 行目）
  // 別のタブの 1 行は状態行と詳細の後ろ（#note）。進行中の一言は消える
  same([other.els.note.textContent, other.els.status.textContent, other.els.abort.style.display], ['前の保存（T）: ' + first.record().head.split('\n').slice(0, 2).join(' / '), '', 'none']);
  assert.match(other.els.note.textContent, /^前の保存（T）: 書誌 ✓ ・ PDF ✓ \/ 登録 K1 — PDF 添付 ✓（/);
});
test('the popup that started the save shows each progress line and the final record; a failed save can be pressed again', async () => {
  const app = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: { '/capture/save': new Error('connectd が落ちた') } });
  await vm.runInContext('init()', app.context);
  const lines = [];
  app.env.session.onChanged.addListener(ch => { if (ch.openref_job) lines.push(app.els.status.textContent); });
  await app.els.save.handlers.click();
  same(lines.slice(0, 2), ['保存を始めます…', '書誌を確定中…（PubMed/Crossref）']);
  same([app.els.status.textContent, app.els.status.className, app.els.save.disabled], ['書誌 ✗ ・ PDF —\n✗ connectd が落ちた', 'err', false]);
  same(app.record().state, 'error');
});
test('all-sites permission is re-checked by the worker, not taken from the popup', async () => {
  const app = popupApp(art, {}, { allSites: false, declined: true,
    setup: articleTab({ doi: ej }, unauthorized401), routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaNone) } });
  app.context.landingData = app.sw.landingData = { doi: ej, pdfUrl: artPdf, pdfSource: 'meta' };
  await vm.runInContext('init()', app.context);
  same(vm.runInContext('allSites', app.context), false);
  app.env.shared.permissions.contains = async () => true; // 保存の前に許可された（chrome://extensions で）
  await app.els.save.handlers.click();
  same(app.created, [{ url: 'https://doi.org/' + ej, active: false }]); // 層 2 の DOI ページを開いた
  same(app.record().failClass, 'login');
});
test('the save message carries the tab, the analysed page, the content-script flag, the folder and the proxy', async () => {
  const app = popupApp(art, {}, { allSites: false, declined: true, proxy: PROXY, setup: c6Setup });
  const seen = [];
  const deliver = app.env.deliver;
  app.context.chrome.runtime.sendMessage = (msg, cb) => { seen.push(clone(msg)); deliver(msg, cb); };
  await vm.runInContext('init()', app.context);
  app.doc.getElementById('collection').value = '42';
  await app.els.save.handlers.click();
  const save = seen.find(m => m.type === 'openref-save');
  same([save.input.tabId, save.input.tabUrl, save.input.contentScriptOk, save.input.collectionId, save.input.proxy, save.input.page.doi, save.input.page.pdfUrl],
    [1, art, true, '42', PROXY, ej, artPdf]);
  same(app.calls.find(c => c[0] === '/capture/save')[1].collection_id, 42);
});

/* ===== 工程の追い越し・時間制限・中断・送り手の検査の守り ===== */
/* /capture/save を gate で止められる工程（c6Worker に足す）: saveGates[doi] があれば、その論文の /capture/save は gate が開くまで待つ */
function c6WorkerSaveGated() {
  const h = c6Worker();
  h.saveGates = {};
  h.context.saveGates = h.saveGates;
  vm.runInContext(`const apiG = api; api = async (url, opts) => { if (url === '/capture/save') { const g = saveGates[JSON.parse(opts.body).doi]; if (g) await g.promise; } return apiG(url, opts); };`, h.context);
  h.saveGate = d => { let open; const promise = new Promise(r => { open = r; }); h.saveGates[c6Doi(d)] = { promise }; return open; };
  return h;
}
test('a job whose record was replaced by another running record sends nothing and does not touch that record (the id check alone)', async () => {
  const h = c6Worker();
  const open = h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  same(h.posts, ['/capture/save']);
  // 記録だけが別の走行中の保存に替わった（中断の印は付いていない = 記録の id の確かめだけが止める）
  const foreign = { id: 'other-job', tabId: 9, tabUrl: 'https://pub.example.org/z', startedAt: h.now(), state: 'running', line: 'PDF を取得中…', cls: '', seen: false };
  await h.session.area.set({ openref_job: foreign });
  open();
  await a.done;
  same(h.posts, ['/capture/save']);
  same(h.record(), foreign);
});
test('an overtaken job resumed while the new save is still running: it sends nothing, leaves the new record alone, stops its keepalive and does no page fetch; the new save attaches to its own item', async () => {
  const h = c6WorkerSaveGated();
  h.userTab.url = 'https://pub.example.org/a'; // 利用者のタブは A のページのまま（ページ内 fetch をしないのは追い越しのため）
  const openA = h.saveGate('a');
  const a = await h.start(c6Input('a', { contentScriptOk: true }));
  await tick();
  same(h.posts, []); // A は /capture/save の応答を待っている
  h.advance(h.LIMIT + 1);
  const openB = h.gate('b');
  const b = await h.start(c6Input('b', { tabId: 2, tabUrl: 'https://pub.example.org/b' }));
  assert.equal(b.ok, true);
  await tick();
  same(h.intervals.map(i => i.on), [false, true]); // A の延命は止まり、B の延命が動く
  same(h.closedAll(), 1); // A が開いた背景タブ（登録済み）を閉じた
  const pageFetches = [];
  h.context.pageFetches = pageFetches;
  vm.runInContext(`sendToTab = async (msg, id) => { pageFetches.push([msg.type, id]); return { ok: true, b64: btoa('%PDF-a') }; };`, h.context);
  openA();
  await a.done;
  same(pageFetches, []); // 追い越された A は利用者のタブから取らない
  same(h.posts, ['/capture/save', '/capture/save']); // A の /capture/save の応答は来たが、A は以後何も送らない
  same([h.record().id, h.record().state], [b.id, 'running']);
  openB();
  await b.done;
  same(h.posts, ['/capture/save', '/capture/save', '/capture/attachment?key=K-b&source=meta&via=page']);
  same([h.record().id, h.record().state], [b.id, 'done']);
});
test('the time-limit timer of an overtaken job does not abort the newer save', async () => {
  const h = c6Worker();
  h.gate('a');
  await h.start(c6Input('a'));
  await tick();
  h.advance(h.LIMIT + 1);
  const openB = h.gate('b');
  const b = await h.start(c6Input('b', { tabId: 2, tabUrl: 'https://pub.example.org/b' }));
  await tick();
  const timers = h.longTimers.filter(t => t.ms === h.LIMIT);
  assert.equal(timers.length, 2);
  timers[0].f(); // A の上限のタイマー
  await tick();
  same([h.record().id, h.record().state], [b.id, 'running']);
  openB();
  await b.done;
  same(h.record().state, 'done');
});
test('the proxy template reaches the PDF-tab classification inside the job (a proxied host is not a login page)', async () => {
  for (const [proxy, want] of [[PROXY, 'stuck'], [null, 'login']]) {
    const h = worker();
    vm.runInContext(`J.proxyTemplate = ${JSON.stringify(proxy)};`, h.context);
    h.timeline([[0, proxify(pdf), 'complete']]);
    h.fetcher(async () => { throw Object.assign(htmlError(), { url: proxify(pdf) }); });
    const e = await nav(h, proxify(pdf)).then(() => null, x => x);
    same(e.kind, want);
    same(vm.runInContext(`fetchKind(J, ${JSON.stringify({ ok: false, html: true, url: proxify(pdf) })})`, h.context), proxy ? null : 'login');
  }
});
test('a record change that arrives before the late openref-job answer wins (the stale answer is not shown)', async () => {
  let openAtt;
  const gAtt = new Promise(r => { openAtt = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gAtt) });
  await vm.runInContext('init()', first.context);
  await first.clickRaw();
  await tick();
  let release;
  first.env.hold = new Promise(r => { release = r; }); // 以後の応答（openref-job）は止める
  const again = popupApp(art, {}, { env: first.env, allSites: false, declined: true, setup: c6Setup });
  const opening = vm.runInContext('init()', again.context);
  await tick(); // service worker は走行中の記録で答えた（まだ popup に届かない）
  openAtt();
  await first.env.settle(); // 終わりの記録の変化が先に届く
  first.env.hold = null;
  release();
  await opening;
  same([again.els.status.textContent, first.record().state], [first.record().line, 'done']);
});
test('the worker refuses a sender that breaks any one of the three conditions', async () => {
  const b = bootWorker({});
  await tick();
  const popupUrl = 'chrome-extension://ext/popup.html';
  for (const sender of [{ id: 'ext', url: popupUrl, tab: { id: 1 } }, { id: 'other', url: popupUrl }, { id: 'ext', url: 'https://pub.example.org/a' }]) {
    let answered = false;
    const r = b.listener()({ type: 'openref-job' }, sender, () => { answered = true; });
    await tick();
    same([r, answered], [false, false]);
  }
  const ok = await new Promise(resolve => b.listener()({ type: 'openref-job' }, { id: 'ext', url: popupUrl }, resolve));
  same(ok.ok, true);
});
test('an aborted or overtaken job opens no new background tab (PDF tab and DOI page)', async () => {
  for (const how of ['cancelled', 'record replaced']) {
    const h = worker();
    if (how === 'cancelled') vm.runInContext('J.cancelled = true;', h.context);
    else await h.session.area.set({ openref_job: { id: 'other-job', tabId: 9, tabUrl: 'u', startedAt: 0, state: 'running', line: '', cls: '' } });
    const e1 = await nav(h).then(() => null, e => e);
    const e2 = await via(h, "'10.1000/x'", 'null').then(() => null, e => e);
    same([h.created, e1 && e1.stale, e2 && e2.stale], [[], true, true]);
  }
});
test('a popup on the same tab after it moved to another page does not take the earlier page\'s record as its own', async () => {
  let open;
  const gate = new Promise(r => { open = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gate) });
  await vm.runInContext('init()', first.context);
  await first.clickRaw();
  await tick();
  const moved = 'https://www.example-journal.org/article/123?page=2';
  const later = popupApp(moved, {}, { env: first.env, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', later.context);
  assert.match(later.els.status.textContent, /別のタブ（またはこのタブの前のページ）の保存が進行中です/);
  same(later.els.save.disabled, false); // 自分の走行中としては扱わない
  open();
  await first.env.settle();
  assert.match(later.els.note.textContent, /^前の保存（T）: 書誌 ✓ ・ PDF ✓ \/ 登録 K1 — PDF 添付 ✓/); // 前のページの結果は 1 行だけ・自分の状態行にしない
  same(later.els.save.disabled, false);
});
test('badge ✓ for a PDF already in OpenRef (duplicate on the PDF-tab route); ! for a stub', async () => {
  // duplicate の ✓ は相手が確かな item（connectd の summarize_item = has_pdf が真）のときだけ。相手の形が分からない（古い connectd の
  // 最小形）・ゴミ箱の中・要確認 stub は !（stub の status と同じ）
  for (const [status, badge, extra = {}] of [['duplicate', '!'], ['stub', '!'], ['duplicate', '✓', { has_pdf: true, title: 'X' }],
    ['duplicate', '!', { trashed: true }], ['duplicate', '!', { has_pdf: true, reason: '同一ファイルが要確認 stub に添付済み。その item の詳細 → 編集から DOI/PMID を引き直せます' }]]) {
    const h = c6Worker();
    h.context.ans = Object.assign({ status, key: 'K9' }, extra);
    vm.runInContext(`const api1 = api; api = async (url, opts) => { if (url.startsWith('/capture/pdf?')) { fakeApi(url); return ans; } return api1(url, opts); };`, h.context);
    const r = await h.start(c6Input('x', { page: { doi: null, pmid: null, pdfUrl: 'https://pub.example.org/x.pdf', pdfSource: 'self', isPdf: true, title: 'X', url: 'https://pub.example.org/x.pdf' } }));
    await r.done;
    same([h.badges, h.record().state], [['…', badge], 'done']);
  }
});

/* 時間制限 */
test('the first page fetch and extension fetch give up after 180 s (large PDFs on slow lines); the failure is kept and classed timeout', async () => {
  const h = worker();
  vm.runInContext(`sendToTab = () => new Promise(() => {}); bgFetchPdf = () => new Promise(() => {});`, h.context);
  const p = vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
  await tick();
  same(h.longTimers.map(t => t.ms), [180000]);
  h.longTimers[0].f();
  await tick();
  same(h.longTimers.map(t => t.ms), [180000, 180000]);
  h.longTimers[1].f();
  const got = await p;
  same([got.bytes, got.kind], [null, 'timeout']);
  assert.match(got.error, /ページ内 fetch: ページ内 fetch が 180 秒以内に応答しません \/ 拡張 fetch: 拡張 fetch が 180 秒以内に応答しません/);
  same(vm.runInContext(`classifyFailure({ page: 'timeout' })`, h.context), 'timeout');
});
test('connectd requests time out per path (fetch-oa 300 s, others 120 s) and say what may still happen (save: bibliography / others: PDF)', async () => {
  for (const [path, ms, later] of [['/capture/fetch-oa', 300000, 'PDF が後から付く'], ['/capture/save', 120000, '書誌が後から登録される'],
    ['/capture/attachment?key=K1&source=meta&via=page', 120000, 'PDF が後から付く'], ['/capture/pdf?title=T', 120000, 'PDF が後から付く']]) {
    const h = worker();
    h.context.fetch = (url, opts) => new Promise((res, rej) => opts.signal.addEventListener('abort', () => rej(new Error('The operation was aborted'))));
    const p = vm.runInContext(`realApi(${JSON.stringify(path)}, { method: 'POST' })`, h.context).then(() => null, e => e);
    await tick();
    same(h.longTimers.map(t => t.ms), [ms]);
    h.longTimers[0].f();
    const e = await p;
    same([e.kind, e.connectdTimeout, e.path], ['timeout', true, path.split('?')[0]]);
    assert.match(e.message, new RegExp(path.split('?')[0] + ' が ' + ms / 1000 + ' 秒以内に応答しません（OpenRef 側では処理が続いていることがあり、' + later + 'ことがあります）'));
  }
});
test('a /capture/save that times out ends the job as an error with the timeout class line', async () => {
  const h = c6Worker();
  vm.runInContext(`api = async () => { throw Object.assign(new Error('OpenRef（connectd）の /capture/save が 120 秒以内に応答しません'), { kind: 'timeout', connectdTimeout: true, path: '/capture/save' }); };`, h.context);
  await (await h.start(c6Input('a'))).done;
  const rec = h.record();
  same([rec.state, rec.failClass, rec.line.split('\n').at(-1)], ['error', 'timeout', inJob('CONNECTD_TIMEOUT_NEXT.save')]);
});
test('a /capture/fetch-oa that times out ends the job there (no DOI page in the browser = no second PDF) with the OpenRef timeout line', async () => {
  const h = c6Worker(); // 全サイト許可あり = 時間切れで止めなければ層 2（DOI ページ）へ進む
  vm.runInContext(`bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });
    const api0 = api; api = async (url, opts) => { if (url === '/capture/fetch-oa') { fakeApi(url); throw Object.assign(new Error('OpenRef（connectd）の /capture/fetch-oa が 300 秒以内に応答しません'), { kind: 'timeout', connectdTimeout: true, path: '/capture/fetch-oa' }); } return api0(url, opts); };`, h.context);
  await (await h.start(c6Input('a'))).done;
  same([h.created, h.posts, h.record().state, h.record().failClass, h.record().line.split('\n').at(-1)],
    [[], ['/capture/save', '/capture/fetch-oa'], 'error', 'timeout', inJob('CONNECTD_TIMEOUT_NEXT.pdf')]);
});
test('a /capture/attachment that times out ends the job there (no PMC page, no server-side fetch on top)', async () => {
  const h = c6Worker();
  vm.runInContext(`const api0 = api; api = async (url, opts) => { if (url.startsWith('/capture/attachment?')) { fakeApi(url); throw Object.assign(new Error('OpenRef（connectd）の /capture/attachment が 120 秒以内に応答しません'), { kind: 'timeout', connectdTimeout: true, path: '/capture/attachment' }); } return api0(url, opts); };`, h.context);
  await (await h.start(c6Input('a', { page: c6Page('a', { pmid: '21620466', pmcUrl: pmcLink }) }))).done;
  same([h.created, h.posts, h.record().failClass, h.record().line.split('\n').at(-1)],
    [[], ['/capture/save', '/capture/attachment?key=K-a&source=meta&via=page'], 'timeout', inJob('CONNECTD_TIMEOUT_NEXT.pdf')]);
});
test('a page fetch that timed out still classes timeout when the extension fetch then fails for another reason (403)', async () => {
  const h = worker();
  vm.runInContext(`sendToTab = () => new Promise(() => {}); bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });`, h.context);
  const p = vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
  await tick();
  h.longTimers[0].f();
  const got = await p;
  same([got.bytes, got.kind], [null, 'timeout']);
  same(vm.runInContext(`classifyFailure({ page: ${JSON.stringify(got.kind)} })`, h.context), 'timeout');
});
/* 中断（popup の「中断」→ SW の abortById） */
test('the abort button stops a stuck save of this tab at once: record aborted, background tabs closed, keepalive stopped, next save starts', async () => {
  const app = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup + 'sendToTab = async (m, id) => (m.type === \'openref-fetch-pdf\' ? new Promise(() => {}) : { ok: true, data: Object.assign({ doi: null, pmid: null, pdfUrl: null, pdfSource: null, title: \'T\', url: ' + JSON.stringify(art) + ' }, ' + JSON.stringify({ doi: ej, pdfUrl: artPdf, pdfSource: 'meta' }) + ') });',
    routes: { '/capture/save': savedK1(ej) } });
  await vm.runInContext('init()', app.context);
  await app.clickRaw();
  await tick();
  same([app.record().state, app.els.abort.style.display, app.els.save.disabled], ['running', 'block', true]);
  assert.match(app.record().line, /ページ内 fetch/); // 本文の来ないページ内 fetch で止まっている
  await app.els.abort.handlers.click();
  await tick();
  const rec = app.record();
  same([rec.state, rec.cls], ['aborted', 'err']);
  same(rec.line, '✗ 中断しました。書誌や PDF は保存済みのことがあります（OpenRef で確かめてください）'); // 本人を三人称で呼ばない・再試行を勧めない
  same([app.els.status.textContent, app.els.abort.style.display, app.els.save.disabled], [rec.line, 'none', false]);
  same(app.env.swIntervals.map(i => i.on), [false]);
  // すぐ次の保存ができる（断られない）
  vm.runInContext(`sendToTab = async (m) => (m.type === 'openref-fetch-pdf' ? { ok: true, b64: btoa('%PDF-test') } : { ok: true, data: {} });`, app.sw);
  await app.els.save.handlers.click();
  same(app.record().state, 'done');
  same(app.calls.filter(c => c[0] === '/capture/save').length, 2);
});
test('the abort button on another tab stops the other save; this tab can save right away', async () => {
  let open;
  const gate = new Promise(r => { open = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gate) });
  await vm.runInContext('init()', first.context);
  await first.clickRaw();
  await tick();
  const other = popupApp('https://www.example-journal.org/article/456', {}, { env: first.env, tabId: 2, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', other.context);
  same(other.els.abort.style.display, 'block');
  await other.els.abort.handlers.click();
  await tick();
  same(first.record().state, 'aborted');
  assert.match(other.els.note.textContent, /^前の保存（T）: ✗ 中断しました。書誌や PDF は保存済みのことがあります/);
  await other.clickRaw();
  await tick();
  same([first.record().tabId, first.record().state], [2, 'running']);
  open(); // 中断した保存の添付の応答が遅れて来ても、新しい保存を乱さない
  await first.env.settle();
  same([first.record().tabId, first.record().state], [2, 'done']);
  // 中断の前に送り終えていた添付の要求（connectd 側では処理が続く）と、新しい保存の添付の 2 回
  same(first.calls.filter(c => c[0].startsWith('/capture/attachment')).length, 2);
});
test('abort after the service worker restarted (no live job) fixes only the record with that id', async () => {
  const h = c6Worker();
  await h.session.area.set({ openref_job: { id: 'x', tabId: 1, tabUrl: 'u', startedAt: h.now(), state: 'running', line: '', cls: '' } });
  same((await vm.runInContext(`abortById('y')`, h.context)).state, 'running');
  const r = await vm.runInContext(`abortById('x')`, h.context);
  same([r.state, r.seen], ['aborted', false]);
});
test('a stale record read by openref-job is not used to abort a save that started in between', async () => {
  const h = c6Worker();
  await h.session.area.set({ openref_job: { id: 'old', tabId: 3, tabUrl: 'u', startedAt: 0, state: 'running', line: '', cls: '' } });
  h.advance(h.LIMIT + 1);
  const open = h.gate('a');
  const [seen, started] = await vm.runInContext(`Promise.all([currentRecord(), startJob(${JSON.stringify(c6Input('a'))})])`, h.context);
  same([seen.id, started.ok], [started.id, true]); // 読んだ古い記録は、直す前に新しい保存に替わっていた
  same([h.record().id, h.record().state], [started.id, 'running']);
  open();
  await started.done;
});
test('a job sets a wake-up alarm at its time limit and clears it at the end; the alarm aborts a job past the limit', async () => {
  const h = c6Worker();
  const r = await h.start(c6Input('a'));
  await r.done;
  same(h.alarms, [['create', 'openref-job-limit', h.LIMIT], ['clear', 'openref-job-limit']]);
  const h2 = c6Worker();
  h2.gate('a');
  const r2 = await h2.start(c6Input('a'));
  await tick();
  same(await vm.runInContext('onJobAlarm()', h2.context), false); // 上限の前は何もしない
  h2.advance(h2.LIMIT);
  await vm.runInContext('onJobAlarm()', h2.context);
  same(h2.record().state, 'aborted');
  same(h2.intervals.map(i => i.on), [false]);
  void r2;
});
test('the page fetch is pinned to the document the popup injected into (documentId); a gone document falls back to the extension fetch', async () => {
  const h = worker();
  vm.runInContext(`J.tab.documentId = 'doc-1'; bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });`, h.context);
  h.userTab.url = article + '?view=2'; // pushState でクエリが変わった（同じ文書）= URL は比べない
  const got = await vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
  same([got.how, h.sent], ['ページ内', [[1, 'openref-fetch-pdf', { documentId: 'doc-1' }]]]);
  h.context.docGone = true; // 別のページへ移った = 文書が無い
  const gone = await vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)})`, h.context);
  same(gone.bytes, null);
  assert.match(gone.error, /^ページ内 fetch: 保存を押したページへ届きませんでした（Could not establish connection/); // 原因は断定しない
  const viaTab = await vm.runInContext(`fetchPdfViaTabThenBg(J, 1, ${JSON.stringify(pdf)}, true).then(() => null, e => e)`, h.context);
  assert.match(viaTab.message, /^タブ内: 保存を押したページへ届きませんでした/);
  same(h.sent.map(x => x[2]), [{ documentId: 'doc-1' }, { documentId: 'doc-1' }, { documentId: 'doc-1' }]);
});
test('the popup passes the documentId of its injection to the job', async () => {
  const app = popupApp(art, {}, { allSites: false, declined: true, proxy: null,
    setup: c6Setup + "chrome.scripting.executeScript = async (o) => (o.target.tabId === 7 ? [] : [{ documentId: 'doc-9', frameId: 0 }]);" });
  const seen = [];
  const deliver = app.env.deliver;
  app.context.chrome.runtime.sendMessage = (msg, cb) => { seen.push(clone(msg)); deliver(msg, cb); };
  await vm.runInContext('init()', app.context);
  await app.els.save.handlers.click();
  same(seen.find(m => m.type === 'openref-save').input.documentId, 'doc-9');
});
test('a save whose first record cannot be written is refused and the popup says so', async () => {
  const h = c6Worker();
  h.session.area.set = async () => { throw new Error('QUOTA_BYTES quota exceeded'); };
  const r = await h.start(c6Input('a'));
  same([r.ok, r.error], [false, '保存の記録（chrome.storage.session）を書けないため、保存を始められません']);
  same([h.posts, h.badges, h.intervals], [[], [], []]);
  const app = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', app.context);
  app.env.session.area.set = async () => { throw new Error('QUOTA_BYTES quota exceeded'); };
  await app.els.save.handlers.click();
  same([app.els.status.textContent, app.els.status.className, app.els.save.disabled], ['✗ 保存の記録（chrome.storage.session）を書けないため、保存を始められません', 'err', false]);
});
test('a popup opened elsewhere shows an unseen result of another tab once (title and first line) and clears the badge', async () => {
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', first.context);
  vm.runInContext('showRecord = () => {};', first.context); // 結果を表示する前に popup が閉じた（誰も見ていない）
  await first.els.save.handlers.click();
  same([first.record().state, first.record().seen, first.env.badges.at(-1)], ['done', false, '✓']);
  const other = popupApp('https://www.example-journal.org/article/456', {}, { env: first.env, tabId: 2, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', other.context);
  await tick();
  same(other.els.note.textContent, '前の保存（' + first.record().title + '）: ' + first.record().head.split('\n').slice(0, 2).join(' / '));
  assert.match(other.els.note.textContent, /）: 書誌 ✓ ・ PDF ✓ \/ 登録 K1 — PDF 添付 ✓（/);
  same([first.record().seen, first.env.badges.at(-1)], [true, '']);
  const third = popupApp('https://www.example-journal.org/article/789', {}, { env: first.env, tabId: 3, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', third.context);
  same([third.els.status.textContent, third.els.note.textContent], ['', '']); // 見た結果はもう出さない
});
test('the 90 s background-tab alarm keeps a tab of a running job and closes an orphan', async () => {
  const run = async (live) => {
    const ses = fakeSession({ openref_hidden_tabs: { 7: 900 } });
    const removed = [], created = [];
    let onAlarm = null;
    const noop = { addListener() {} };
    const ctx = vm.createContext({ URL, Uint8Array, TextDecoder, btoa, atob,
      Date: { now: () => 1000 }, setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {},
      importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
      chrome: {
        runtime: { id: 'ext', getURL: x => 'chrome-extension://ext/' + x, onMessage: noop },
        alarms: { onAlarm: { addListener: f => { onAlarm = f; } }, create: (n, o) => created.push(n), clear() {} },
        tabs: { onRemoved: noop, remove: async id => removed.push(id) },
        storage: { session: ses.area }, action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {} },
      },
    });
    vm.runInContext(code('background.js'), ctx);
    await tick();
    removed.length = 0; // 起動時の後始末の分は数えない
    await ses.area.set({ openref_hidden_tabs: { 7: 900 } });
    if (live) vm.runInContext('liveJob = { cancelled: false, startedAt: 0 };', ctx);
    onAlarm({ name: 'openref-hidden-tab-7' });
    await tick();
    return [removed, created];
  };
  same(await run(true), [[], ['openref-hidden-tab-7']]); // 走っている工程の背景タブ = 期限を延ばす
  same(await run(false), [[7], []]); // 孤児 = 閉じる
});
test('a job aborted while its page fetch hangs does not go on to the extension fetch when the page fetch times out', async () => {
  const h = worker();
  const bg = [];
  h.context.bg = bg;
  vm.runInContext(`sendToTab = () => new Promise(() => {}); bgFetchPdf = async (u) => { bg.push(u); return { ok: true, b64: btoa('%PDF-x') }; };`, h.context);
  const p = vm.runInContext(`fetchPdfBytes(J, ${JSON.stringify(pdf)}).then(() => null, e => e)`, h.context);
  await tick();
  vm.runInContext('J.cancelled = true;', h.context); // 「中断」が押された
  h.longTimers[0].f(); // ページ内 fetch の 60 秒
  const e = await p;
  same([e && e.stale, bg], [true, []]);
});
test('the time limit of an aborted job does not come back to close the background tabs of a later save', async () => {
  const h = c6Worker();
  h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  await vm.runInContext(`abortById(${JSON.stringify(a.id)})`, h.context); // 「中断」
  same(h.closedAll(), 1); // 中断で A の背景タブを閉じた
  const limitA = h.longTimers.find(t => t.ms === h.LIMIT);
  assert.equal(limitA.cleared, true, '中断で A の上限のタイマーを止める');
  const openB = h.gate('b');
  const b = await h.start(c6Input('b', { tabId: 2, tabUrl: 'https://pub.example.org/b' }));
  await tick();
  limitA.f(); // それでも A の上限が来た（タイマーの取り消しが間に合わなかった）
  await tick();
  same(h.closedAll(), 1); // B の背景タブは閉じない（A を止めるのは 1 回だけ）
  same([h.record().id, h.record().state], [b.id, 'running']);
  openB();
  await b.done;
});
test('a result that arrives while the popup is still analysing is shown after the analysis (not marked seen unseen)', async () => {
  let openAtt, openFetch;
  const gAtt = new Promise(r => { openAtt = r; });
  const gFetch = new Promise(r => { openFetch = r; });
  const first = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: c6Routes(gAtt) });
  await vm.runInContext('init()', first.context);
  vm.runInContext('showRecord = () => {};', first.context); // 結果を見る前に閉じた
  await first.clickRaw();
  await tick();
  // 別のタブの J-STAGE の PDF タブ: 記事ページの取得（最大 8 秒）の間に、最初の保存が終わる
  const jsTab = jsPdf + '/-char/ja';
  const other = popupApp(jsTab, url => gFetch.then(() => ({ ok: false, status: 404, url, text: async () => '' })), { env: first.env, tabId: 2, allSites: false, declined: true });
  const opening = vm.runInContext('init()', other.context);
  await tick();
  openAtt();
  await first.env.settle();
  same([first.record().state, first.record().seen], ['done', false]); // 解析の間は表示も seen もしない
  openFetch();
  await opening;
  await tick();
  assert.match(other.els.status.textContent, /記事ページで DOI を確かめられず/); // 解析の文は残り
  assert.match(other.els.note.textContent, /^前の保存（T）: 書誌 ✓ ・ PDF ✓ \/ 登録 K1 — PDF 添付 ✓/); // 結果の 1 行は解析の文の後ろ（#note）
  same(first.record().seen, true);
});
test('an older OpenRefShared that has every function but no constants is replaced by the new pdfbytes.js', () => {
  const ctx = vm.createContext({ URL });
  vm.runInContext(code('pdfbytes.js'), ctx);
  const fnsOnly = {};
  for (const [k, v] of Object.entries(ctx.OpenRefShared)) if (typeof v === 'function') fnsOnly[k] = v;
  const ctx2 = vm.createContext({ URL, OpenRefShared: fnsOnly });
  vm.runInContext(code('pdfbytes.js'), ctx2);
  same([ctx2.OpenRefShared.JOB_KEY, ctx2.OpenRefShared.BASE], ['openref_job', 'http://127.0.0.1:23119']);
});
test('an abort for an old id does not stop the save that is running now', async () => {
  const h = c6Worker();
  const open = h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  const r = await vm.runInContext(`abortById('an-older-save')`, h.context);
  same([r.id, r.state, h.closedAll(), h.intervals.map(i => i.on)], [a.id, 'running', 0, [true]]);
  open();
  await a.done;
  same(h.record().state, 'done');
});
test('the worker routes the job-limit alarm and the background-tab alarm apart', async () => {
  const run = async (alarmName) => {
    const ses = fakeSession({ openref_hidden_tabs: { 7: 900 } });
    const removed = [], created = [];
    let onAlarm = null;
    const noop = { addListener() {} };
    let now = 1000;
    const ctx = vm.createContext({ URL, Uint8Array, TextDecoder, btoa, atob,
      Date: { now: () => now }, setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {},
      importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
      chrome: {
        runtime: { id: 'ext', getURL: x => 'chrome-extension://ext/' + x, onMessage: noop },
        alarms: { onAlarm: { addListener: f => { onAlarm = f; } }, create: (n) => created.push(n), clear() {} },
        tabs: { onRemoved: noop, remove: async id => removed.push(id) },
        storage: { session: ses.area }, action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {} },
      },
    });
    vm.runInContext(code('background.js'), ctx);
    await tick();
    removed.length = 0;
    await ses.area.set({ openref_hidden_tabs: { 7: 900 }, openref_job: { id: 'L', tabId: 1, tabUrl: 'u', startedAt: 0, state: 'running', line: '', cls: '' } });
    vm.runInContext(`liveJob = { id: 'L', cancelled: false, startedAt: 0 };`, ctx); // 上限を過ぎて走っている工程
    now = vm.runInContext('JOB_LIMIT_MS', ctx) + 1000;
    onAlarm({ name: alarmName });
    await tick();
    return [clone(ses.store.openref_job).state, removed, created];
  };
  same(await run('openref-job-limit'), ['aborted', [7], []]); // 上限の alarm = 工程を中断（その工程の背景タブも閉じる・期限は延ばさない）
  same(await run('openref-hidden-tab-7'), ['running', [], ['openref-hidden-tab-7']]); // 背景タブの期限 = 工程を中断しない・期限を延ばす
});

/* ===== PDF の到着を応答ヘッダで知る（工程が開いた背景タブだけ・全サイト許可のある人だけ）===== */
const STOP = 'chrome-extension://ext/stopped.html';
const hdrs = o => Object.entries(o).map(([name, value]) => ({ name, value }));
const c7Relay = 'https://www.example-journal.org/article/123/pdf';
const c7File = 'https://www.example-journal.org/article/123/file.pdf';
const c7Other = 'https://cdn.example-assets.org/123/main.pdf?sig=x';
const attachPdf = hdrs({ 'Content-Type': 'application/pdf', 'Content-Disposition': 'attachment; filename="123.pdf"' });
const isPdfResp = (headers, url = c7File) => vm.runInContext(`isPdfResponse(${JSON.stringify(headers)}, ${JSON.stringify(url)})`, worker().context);
/* 全サイト許可ありの工程で、宣言された PDF の URL（中継 HTML）を背景タブで開く。fetched = 取り直した URL */
function c7Nav(t, { sites = true, answer = null } = {}) {
  const h = worker();
  vm.runInContext(`J.allSites = ${sites};`, h.context);
  h.timeline(t);
  const fetched = [];
  h.fetcher(answer || (async (id, url) => {
    fetched.push(url);
    if (url === c7Relay) throw htmlError();
    return { b64: btoa('%PDF-test'), how: 'タブ内' };
  }));
  h.fetched = fetched;
  h.run = () => vm.runInContext(`fetchPdfViaNavigation(J, ${JSON.stringify(c7Relay)}, 1, true)`, h.context).then(r => ({ ok: r }), e => ({ err: e }));
  return h;
}
const t4b = (extra = {}, at = 1500) => [[0, c7Relay, 'complete'], [at, c7File, null, Object.assign({ responseHeaders: attachPdf }, extra)], [at + 100, STOP, 'loading'], [at + 200, STOP, 'complete']];
/* listener・規則がすべて外れ、背景タブが閉じた */
function cleaned(h) {
  same(h.wrListeners.length, 0);
  same([...h.rules.keys()], []);
  same(h.removed, [7]);
  same(h.listeners.length, 0);
}
const noExtUrl = (...texts) => { for (const t of texts) assert.doesNotMatch(String(t), /chrome-extension:/); };

test('isPdfResponse: content-type and content-disposition in any letter case and with parameters', () => {
  for (const [headers, want] of [
    [{ 'content-type': 'application/pdf' }, true],
    [{ 'Content-Type': 'Application/PDF; charset=binary' }, true],
    [{ 'CONTENT-TYPE': ' application/x-pdf ' }, true],
    [{ 'content-type': 'text/html; charset=utf-8' }, false],
    [{ 'Content-Disposition': 'attachment' }, true], // 種類の無い attachment = 候補（中身は %PDF で確かめる）
    [{ 'content-disposition': 'ATTACHMENT; filename="paper.pdf"', 'content-type': 'application/octet-stream' }, true],
    [{ 'content-disposition': "attachment; filename*=UTF-8''%E8%AB%96%E6%96%87.pdf", 'content-type': 'binary/octet-stream' }, true],
    [{ 'content-disposition': 'inline; filename="paper.pdf"', 'content-type': 'text/html' }, false],
    [{ 'content-disposition': 'inline' }, false],
  ]) assert.equal(isPdfResp(hdrs(headers)), want, JSON.stringify(headers));
});
test('isPdfResponse: an attachment that is not a PDF (zip / supplement) is not taken as the article PDF', () => {
  for (const [headers, url] of [
    [{ 'content-disposition': 'attachment; filename="supp.zip"', 'content-type': 'application/zip' }, c7File],
    [{ 'content-disposition': 'attachment', 'content-type': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' }, c7File],
    [{ 'content-disposition': 'attachment; filename=data.zip', 'content-type': 'application/octet-stream' }, c7File],
    [{ 'content-disposition': 'attachment', 'content-type': 'application/octet-stream' }, 'https://www.example-journal.org/supp/data.xlsx'],
  ]) assert.equal(isPdfResp(hdrs(headers), url), false, JSON.stringify(headers));
  // attachment で content-type が何も言っていなくても、ファイル名が PDF なら候補
  assert.equal(isPdfResp(hdrs({ 'content-disposition': 'attachment; filename="x.pdf"', 'content-type': 'application/force-download' }), 'https://x.example.org/dl?id=1'), true);
});

test('attachment behind a relay (type 4b): the arrival is detected, its URL is fetched from the article tab, the tab never downloads', async () => {
  const h = c7Nav(t4b());
  const r = await h.run();
  assert.ok(r.ok, String(r.err));
  assert.equal(atob(r.ok.b64), '%PDF-test');
  assert.equal(r.ok.moved, false);
  same(h.fetched, [c7Relay, c7File]); // 中継ページの complete で 1 回・検出した URL を 1 回（拡張内のページは取らない）
  same(h.created, ['about:blank']); // 見張りを付けてから目的の URL へ移す
  same(h.updated, [[7, c7Relay]]);
  // 見張り = このタブの main_frame だけ・応答ヘッダつき
  same(h.dnrCalls[0].addRules.map(x => [x.condition.tabIds, x.condition.resourceTypes, x.action.redirect.extensionPath]), [[[7], ['main_frame'], '/stopped.html']]);
  const rule = h.dnrCalls[0].addRules[0];
  assert.ok(rule.id >= 7001 && rule.id < 8001, 'rule id ' + rule.id);
  same(rule.condition.responseHeaders.map(x => x.header), ['content-type', 'content-disposition']);
  same(h.dnrCalls[0].removeRuleIds, [rule.id]);
  same(h.dnrCalls.at(-1), { removeRuleIds: [rule.id] });
  cleaned(h);
  noExtUrl(JSON.stringify(h.record()));
});
test('the listener is filtered to the background tab\'s top frame with response headers', async () => {
  const h = c7Nav(t4b());
  let seen = null;
  const realFetch = h.context.fakeFetch;
  h.context.fakeFetch = async (...a) => { seen = seen || JSON.parse(JSON.stringify(h.wrListeners.map(l => [l.filter, l.extra]))); return realFetch(...a); };
  await h.run();
  same(seen, [[{ urls: ['http://*/*', 'https://*/*'], types: ['main_frame'], tabId: 7 }, ['responseHeaders']]]);
  assert.equal(h.wrAdded(), 1);
});
test('responses of other tabs, of frames inside the page, and non-2xx answers are not taken', async () => {
  for (const extra of [{ tabId: 9 }, { type: 'sub_frame', frameId: 4 }, { frameId: 3 }, { statusCode: 404 }, { statusCode: 302 }, { url: 'data:application/pdf;base64,JVBERg==' }]) {
    const h = c7Nav([[0, c7Relay, 'complete'], [1500, c7File, null, Object.assign({ responseHeaders: attachPdf }, extra)]]);
    const r = await h.run();
    assert.ok(r.err, JSON.stringify(extra));
    assert.equal(r.err.kind, 'stuck', JSON.stringify(extra));
    same(h.fetched, [c7Relay]); // 検出した URL を取りに行かない
    cleaned(h);
  }
});
test('moved is decided by the origin of the detected URL (the tab itself sits on the extension page)', async () => {
  const h = c7Nav(t4b({ url: c7Other }));
  const r = await h.run();
  assert.equal(r.ok.moved, true);
  same(h.fetched, [c7Relay, c7Other]);
});
test('article page: a detected PDF on another origin is posted as source=link (DOI match required)', async () => {
  const h = worker();
  vm.runInContext(`
    J.page = { pdfUrl: ${JSON.stringify(c7Relay)}, pdfSource: 'meta' };
    J.allSites = true;
    fetchPdfBytes = async () => ({ bytes: null, error: 'HTML response', html: true });
  `, h.context);
  h.timeline(t4b({ url: c7Other }));
  h.fetcher(async (id, url) => { if (url === c7Relay) throw htmlError(); return { b64: btoa('%PDF-test'), how: 'タブ内' }; });
  vm.runInContext(`api = async (url) => { fakeApi(url); return { status: 'attached', bytes: 9, doi_check: 'match' }; };`, h.context);
  const line = await vm.runInContext("attachPdf(J, 'KEY')", h.context);
  assert.match(line, /PDF 添付 ✓/);
  same(h.posts.map(u => new URLSearchParams(u.split('?')[1]).get('source')), ['link']);
  same(h.posts.map(u => new URLSearchParams(u.split('?')[1]).get('via')), ['page']);
});
test('a non-PDF attachment stopped by the rule ends as stuck without fetching it or using the extension page URL', async () => {
  const zip = hdrs({ 'content-type': 'application/zip', 'content-disposition': 'attachment; filename="supp.zip"' });
  for (const stop of [STOP, 'chrome-extension://dyn0123456789abcdef/stopped.html']) {
    const h = c7Nav([[0, c7Relay, 'complete'], [1000, 'https://www.example-journal.org/supp.zip', null, { responseHeaders: zip }], [1100, stop, 'loading'], [1200, stop, 'complete']]);
    const r = await h.run();
    assert.equal(r.err.kind, 'stuck');
    assert.ok(h.now() < 3000, 'stopped at ' + h.now() + ' ms'); // 拡張内のページで止まったら待たずに終える
    same(h.fetched, [c7Relay]);
    noExtUrl(r.err.message, JSON.stringify(h.record()));
    cleaned(h);
  }
});
test('a detected candidate whose bytes are not a PDF is not attached (fetched once, then stuck)', async () => {
  const h = c7Nav(t4b({ responseHeaders: hdrs({ 'content-type': 'application/octet-stream', 'content-disposition': 'attachment' }) }), {
    answer: async (id, url) => { h.fetched.push(url); throw htmlError('PDF ではない応答（2048 bytes）'); },
  });
  const r = await h.run();
  assert.equal(r.err.kind, 'stuck');
  same(h.fetched, [c7Relay, c7File]);
  cleaned(h);
});
test('without the all-sites permission: no listener, no rule, the tab opens at the URL (as in 0.2.8)', async () => {
  const h = c7Nav(t4b(), { sites: false });
  await h.run();
  same([h.wrAdded(), h.dnrCalls.length, h.updated.length], [0, 0, 0]);
  same(h.created, [c7Relay]);
  same(h.fetched, [c7Relay]); // 検出なし = 今の動き（取り直しは中継ページの 1 回だけ）
});
test('Chrome older than 128: no rule is added, detection still works', async () => {
  const h = c7Nav(t4b());
  h.context.navigator.userAgent = 'Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/127.0.6533.120 Safari/537.36';
  const r = await h.run();
  assert.ok(r.ok);
  same(h.fetched, [c7Relay, c7File]);
  same(h.dnrCalls, []);
  cleaned(h);
  // headless の UA（HeadlessChrome/N）も版として読む
  const hl = c7Nav(t4b());
  hl.context.navigator.userAgent = 'Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.8078.47 Safari/537.36';
  assert.ok((await hl.run()).ok);
  same(hl.dnrCalls.filter(c => (c.addRules || []).length).length, 1);
  // 版が読めない UA も規則を入れない
  const g = c7Nav(t4b());
  g.context.navigator.userAgent = 'SomeBrowser/1.0';
  assert.ok((await g.run()).ok);
  same(g.dnrCalls, []);
});
test('a rule that cannot be added does not stop the save (detection only)', async () => {
  const h = c7Nav(t4b());
  h.context.dnrFail = "Error at property 'condition': Unexpected property: 'responseHeaders'.";
  const r = await h.run();
  assert.ok(r.ok);
  same(h.fetched, [c7Relay, c7File]);
  same(h.dnrCalls.length, 1); // 追加を試みただけ（外す要求は出さない）
  cleaned(h);
});
test('an error inside the background tab still removes the listener and the rule', async () => {
  const h = c7Nav(t4b(), { answer: async () => { throw new Error('HTTP 403'); } });
  const r = await h.run();
  assert.match(String(r.err), /HTTP 403/);
  cleaned(h);
});
test('an aborted job removes the rule at once and never fetches what is detected after the abort', async () => {
  const h = c7Nav(t4b());
  let atAbort = null;
  h.fetcher(async (id, url) => {
    h.fetched.push(url);
    vm.runInContext('stopJob(J)', h.context);
    await tick();
    atAbort = { rules: h.rules.size, listeners: h.wrListeners.length };
    throw htmlError();
  });
  const r = await h.run();
  assert.ok(r.err);
  same(atAbort, { rules: 0, listeners: 0 }); // stopJob の経路で外れる
  same(h.fetched, [c7Relay]);
  same(h.removed, [7]);
});
test('an aborted job does not move the blank tab to the URL', async () => {
  const h = c7Nav(t4b());
  h.context.chrome.declarativeNetRequest.updateSessionRules = async o => { h.dnrCalls.push(o); if ((o.addRules || []).length) vm.runInContext('stopJob(J)', h.context); };
  const r = await h.run();
  assert.equal(r.err.stale, true);
  same(h.updated, []);
  same(h.wrListeners.length, 0);
  same(h.removed, [7]);
});

/* viaDoiTab: DOI ページが直接 PDF に着く型（検出した URL を先に見る） */
function c7Doi(t, route = 'null', { realWait = false } = {}) {
  const h = worker();
  vm.runInContext('J.allSites = true;', h.context);
  h.timeline(t);
  h.context.calls = [];
  vm.runInContext(`
    chrome.scripting = { executeScript: async () => { calls.push('inject'); } };
    ${realWait ? '' : 'waitForTabLoad = async (id) => (await chrome.tabs.get(id)).url;'}
    sendToTab = async (msg) => { calls.push(msg.type); return { ok: true, data: { pmid: null, doi: null, pdfUrl: null, pdfSource: null } }; };
    fetchPdfViaTabThenBg = async (job, id, url, injected) => { calls.push(['fetch', id, url, injected]); return { b64: btoa('%PDF-test'), how: '拡張' }; };
    postPdf = async (job, key, b64, how, source) => how + ' [' + source + ']';
  `, h.context);
  h.run = () => vm.runInContext(`viaDoiTab(J, 'KEY', '10.1234/ej.2025.123', ${route})`, h.context).then(r => ({ ok: r }), e => ({ err: e }));
  return h;
}
test('DOI page that lands directly on a PDF: the detected URL is the PDF (source=self), not the tab URL', async () => {
  const h = c7Doi([[0, 'https://doi.org/10.1234/ej.2025.123', 'loading'], [0, c7File, null, { responseHeaders: attachPdf }], [0, STOP, 'complete']]);
  const r = await h.run();
  assert.ok(r.ok, String(r.err));
  assert.match(r.ok, /DOI ページ経由・拡張 fetch・www\.example-journal\.org \[self\]/);
  same(h.context.calls, [['fetch', 7, c7File, false]]); // 注入も解析もしない・取るのは検出した URL
  same(h.created, ['about:blank']);
  same(h.updated, [[7, 'https://doi.org/10.1234/ej.2025.123']]);
  cleaned(h);
  noExtUrl(r.ok, JSON.stringify(h.record()));
});
test('DOI page whose landing was stopped by the rule but not taken as a PDF fails without showing the extension page URL', async () => {
  // 静的な id の URL と、use_dynamic_url の id（起動ごとの id）の URL
  for (const stop of [STOP, 'chrome-extension://dyn0123456789abcdef/stopped.html']) {
    const h = c7Doi([[0, 'https://doi.org/10.1234/ej.2025.123', 'loading'], [0, stop, 'complete']]);
    const r = await h.run();
    assert.match(r.err.message, /PDF ではないファイル/);
    noExtUrl(r.err.message, JSON.stringify(h.record()));
    same(h.context.calls, []);
    cleaned(h);
  }
});
test('the PMC route does not accept a detected PDF (the PMC page must confirm the identifiers)', async () => {
  const h = c7Doi([[0, pmcLink, 'loading'], [0, pmcPdf, null, { responseHeaders: hdrs({ 'content-type': 'application/pdf' }) }], [0, STOP, 'complete']], pmcRoute);
  const r = await h.run();
  assert.match(r.err.message, /識別子を確認できません/);
  assert.ok(!h.context.calls.some(c => Array.isArray(c) && c[0] === 'fetch'));
  cleaned(h);
});
test('the DOI route without the all-sites watch keeps today\'s URL rule (.pdf tab URL that cannot be injected)', async () => {
  const h = c7Doi([[0, c7File, 'complete']]);
  vm.runInContext("J.allSites = false; chrome.scripting.executeScript = async () => { throw new Error('cannot inject'); };", h.context);
  const r = await h.run();
  assert.match(r.ok, /\[self\]/);
  same([h.wrAdded(), h.dnrCalls.length], [0, 0]);
  same(h.created, ['https://doi.org/10.1234/ej.2025.123']);
});

/* 工程の終わり・中断・service worker の起動 */
test('the end of a job removes any watch left behind', async () => {
  const h = worker({ session: fakeSession({}) });
  vm.runInContext('runSave = async (job) => { await watchPdfHeaders(job, 7); };', h.context);
  const r = await vm.runInContext(`startJob({ tabId: 1, tabUrl: 'https://pub.example.org/a', page: {} })`, h.context);
  await r.done;
  await tick();
  same([h.wrAdded(), h.dnrCalls.filter(c => (c.addRules || []).length).length], [1, 1]);
  same([h.wrListeners.length, h.rules.size], [0, 0]);
});
test('the user\'s abort removes the watch of the running job', async () => {
  const h = worker({ session: fakeSession({}) });
  h.context.hold = new Promise(() => {});
  vm.runInContext('runSave = async (job) => { await watchPdfHeaders(job, 7); await hold; };', h.context);
  const r = await vm.runInContext(`startJob({ tabId: 1, tabUrl: 'https://pub.example.org/a', page: {} })`, h.context);
  await tick();
  same([h.wrListeners.length, h.rules.size], [1, 1]);
  await vm.runInContext(`abortById(${JSON.stringify(r.id)})`, h.context);
  await tick();
  same([h.wrListeners.length, h.rules.size], [0, 0]);
});
test('a watch started after the job was aborted is removed at once', async () => {
  const h = worker();
  vm.runInContext('J.allSites = true; stopJob(J);', h.context);
  const w = await vm.runInContext('watchPdfHeaders(J, 7)', h.context);
  assert.equal(w.active, false);
  same([h.wrListeners.length, h.rules.size, h.wrAdded()], [0, 0, 0]);
});
test('service worker start removes every session rule in the job\'s id range (and only those)', async () => {
  const rules = new Map([[5, { id: 5 }], [7001, { id: 7001 }], [7500, { id: 7500 }], [8000, { id: 8000 }], [8001, { id: 8001 }]]);
  const calls = [];
  const noop = { addListener() {} };
  const ctx = vm.createContext({ URL, Uint8Array, TextDecoder, btoa, atob,
    Date: { now: () => 1000 }, setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {},
    importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
    chrome: {
      runtime: { id: 'ext', getURL: x => 'chrome-extension://ext/' + x, onMessage: noop },
      alarms: { onAlarm: noop, create() {}, clear() {} }, tabs: { onRemoved: noop, remove: async () => {} },
      storage: { session: fakeSession({}).area }, action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {} },
      declarativeNetRequest: {
        getSessionRules: async () => [...rules.values()],
        updateSessionRules: async o => { calls.push(clone(o)); for (const id of o.removeRuleIds || []) rules.delete(id); },
      },
    },
  });
  vm.runInContext(code('background.js'), ctx);
  await tick();
  same(calls, [{ removeRuleIds: [7001, 7500, 8000] }]);
  same([...rules.keys()], [5, 8001]);
});
test('a job started right after the worker starts adds its rule only after the start-up cleanup', async () => {
  const h = worker();
  vm.runInContext('J.allSites = true;', h.context);
  h.rules.set(7001, { id: 7001, stale: true }); // 前の service worker の残り（同じ id）
  let release;
  const gate = new Promise(r => { release = r; });
  h.context.dnrGet = () => gate.then(() => [{ id: 7001 }]);
  const cleanup = vm.runInContext('removeAllPdfRules()', h.context);
  const watching = vm.runInContext('watchPdfHeaders(J, 7)', h.context);
  await tick();
  same(h.dnrCalls, []); // 後始末の一覧がまだ = 規則を入れない
  release();
  assert.equal(await cleanup, 1);
  await watching;
  same(h.dnrCalls.map(c => [c.removeRuleIds, (c.addRules || []).map(x => x.id)]), [[[7001], []], [[7001], [7001]]]);
  assert.equal(h.rules.get(7001).stale, undefined); // 残ったのは新しい工程の規則
});
test('manifest: webRequest and declarativeNetRequestWithHostAccess are required, the blank page is the only web accessible resource', () => {
  const m = JSON.parse(code('manifest.json'));
  same(m.permissions, ['activeTab', 'scripting', 'storage', 'alarms', ...C7_PERMISSIONS]);
  assert.equal(m.optional_permissions, undefined);
  same(m.web_accessible_resources, [{ resources: ['stopped.html'], matches: ['http://*/*', 'https://*/*'], use_dynamic_url: true }]);
  const page = code('stopped.html');
  assert.doesNotMatch(page, /<script|on\w+=|<iframe|<link/i); // script も読み込みも無い静的な 1 枚
  assert.equal(vm.runInContext('PDF_STOP_PAGE', worker().context), '/stopped.html');
});

/* ===== 応答ヘッダの見張りの境界: 直着・待ちの前の到着・移動の失敗・閉じる順・停止ページの判定 ===== */
const until = async (cond, n = 50) => { for (let i = 0; i < n && !cond(); i++) await tick(); assert.ok(cond(), 'condition not reached'); };
const realTimeout = (p, ms = 1000) => Promise.race([p, new Promise(r => setTimeout(() => r({ hung: true }), ms))]);
test('without a rule, a DOI that lands directly on an attachment PDF is taken from the detection (the tab stays about:blank)', async () => {
  const h = c7Doi([[0, 'about:blank', 'complete']], 'null', { realWait: true });
  h.context.dnrFail = 'rule refused'; // 規則なし = 検出だけ（128 未満・追加の失敗）
  const p = h.run();
  await until(() => h.wrListeners.length === 1 && h.listeners.length === 2); // watchTab と waitForTabLoad
  for (const l of h.wrListeners.slice()) l.f({ tabId: 7, type: 'main_frame', frameId: 0, statusCode: 200, url: c7File, responseHeaders: attachPdf });
  const r = await realTimeout(p);
  assert.ok(!r.hung, 'waitForTabLoad did not return on the detection');
  assert.match(r.ok, /DOI ページ経由・拡張 fetch・www\.example-journal\.org \[self\]/);
  same(h.context.calls, [['fetch', 7, c7File, false]]);
  cleaned(h);
});
test('a detection that arrived before the wait started also ends the wait', async () => {
  const h = c7Doi([[0, 'about:blank', 'complete'], [0, c7File, null, { responseHeaders: attachPdf }]], 'null', { realWait: true });
  h.context.dnrFail = 'rule refused';
  const r = await realTimeout(h.run());
  assert.ok(!r.hung);
  assert.match(r.ok, /\[self\]/);
});
test('the real waitForTabLoad does not take the blank tab as the landing; the landing that loads after it is analysed', async () => {
  const landingUrl = 'https://www.example-journal.org/article/123';
  const h = c7Doi([[0, 'about:blank', 'complete']], 'null', { realWait: true });
  vm.runInContext(`sendToTab = async (msg) => { calls.push(msg.type); return msg.type === 'openref-analyze'
    ? { ok: true, data: { doi: '10.1234/ej.2025.123', pmid: null, pdfUrl: ${JSON.stringify(c7File)}, pdfSource: 'meta' } } : { ok: true, url: null }; };`, h.context);
  const p = h.run();
  await until(() => h.listeners.length === 2);
  await tick();
  same(h.context.calls, []); // about:blank の complete では進まない
  for (const l of h.listeners.slice()) l(7, { status: 'loading' }, { url: landingUrl, status: 'loading' });
  for (const l of h.listeners.slice()) l(7, { status: 'complete' }, { url: landingUrl, status: 'complete' });
  const r = await realTimeout(p);
  assert.match(r.ok, /DOI ページ経由・拡張 fetch・www\.example-journal\.org \[meta\]/);
  same(h.context.calls, ['inject', 'openref-analyze', ['fetch', 7, c7File, true]]);
  cleaned(h);
});
test('if moving the blank tab to the URL fails, the watch is removed before the error goes up', async () => {
  const h = c7Nav(t4b());
  h.context.chrome.tabs.update = async () => { throw new Error('No tab with id: 7.'); };
  const r = await h.run();
  assert.match(String(r.err), /No tab with id/);
  same([h.wrListeners.length, h.rules.size], [0, 0]);
  same(h.removed, [7]);
});
test('an abort closes the background tabs first, then removes the rule', async () => {
  const h = worker();
  vm.runInContext('J.allSites = true;', h.context);
  await vm.runInContext('watchPdfHeaders(J, 7)', h.context);
  vm.runInContext('stopJob(J)', h.context);
  await until(() => h.rules.size === 0);
  same(h.order, ['closed', 'unrule']);
});
test('service worker start closes the leftover background tabs first, then removes the rules', async () => {
  const order = [];
  const noop = { addListener() {} };
  const ctx = vm.createContext({ URL, Uint8Array, TextDecoder, btoa, atob,
    Date: { now: () => 1000 }, setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {},
    importScripts: (...names) => names.forEach(n => vm.runInContext(code(n), ctx)),
    chrome: {
      runtime: { id: 'ext', getURL: x => 'chrome-extension://ext/' + x, onMessage: noop },
      alarms: { onAlarm: noop, create() {}, clear() {} },
      tabs: { onRemoved: noop, remove: async id => { await null; await null; await null; order.push('remove:' + id); } },
      storage: { session: fakeSession({ openref_hidden_tabs: { 7: 900, 8: 900 } }).area }, action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {} },
      declarativeNetRequest: {
        getSessionRules: async () => { order.push('list'); return [{ id: 7001 }]; },
        updateSessionRules: async o => { order.push('unrule:' + o.removeRuleIds.join(',')); },
      },
    },
  });
  vm.runInContext(code('background.js'), ctx);
  for (let i = 0; i < 30; i++) await tick();
  same(order, ['remove:7', 'remove:8', 'list', 'unrule:7001']);
});
test('a web page named stopped.html is not the extension page (the scheme is checked)', async () => {
  const evil = 'https://evil.example.org/stopped.html';
  const h = c7Nav([[0, evil, 'complete']]);
  const r = await h.run();
  assert.ok(r.ok, String(r.err));
  same(h.fetched, [evil]);
});
test('the background tab is closed before its rule is removed (PDF tab and DOI page)', async () => {
  const h = c7Nav(t4b());
  await h.run();
  same(h.order, ['remove:7', 'unrule']);
  const d = c7Doi([[0, 'https://doi.org/10.1234/ej.2025.123', 'loading'], [0, c7File, null, { responseHeaders: attachPdf }], [0, STOP, 'complete']]);
  await d.run();
  same(d.order, ['remove:7', 'unrule']);
});
test('only the first detected response is taken', async () => {
  const second = 'https://www.example-journal.org/article/123/second.pdf';
  const h = c7Nav([[0, c7Relay, 'complete'], [1500, c7File, null, { responseHeaders: attachPdf }], [1500, second, null, { responseHeaders: attachPdf }], [1600, STOP, 'complete']]);
  const r = await h.run();
  assert.ok(r.ok);
  same(h.fetched, [c7Relay, c7File]);
});

/* ===== 確認の窓（全サイト許可のある人だけ・利用者がボタンを押したときだけ）=====
   保存はプロキシの節の流れ（C4_FLOWS）を全サイト許可ありで走らせ、終わった後の service worker に窓の代役（c8Kit）を入れる:
   chrome.windows（窓 50・タブ 51）・tabs.update・tabs.onRemoved の listener・webRequest・DNR・executeScript の記録。
   保存の流れ自体はプロキシの節の試験と同じ（代役は保存の後に入れる） */
const WIN = 50, WTAB = 51;
const PDF_HEADERS = [{ name: 'Content-Type', value: 'application/pdf' }];
function c8Kit(env) {
  const sw = env.sw;
  const c = sw.chrome;
  // order = 窓のタブを閉じた（remove:51）・規則を外した（unrule）・背景タブを開いた（create）・取得した（fetch）の順。
  // silent = 閉じても onRemoved を配らない（閉じる知らせが来ない場合でも中断が待ちを終えるか）
  const kit = { windows: [], updated: [], removedTabs: [], tabRm: [], winRm: [], wr: [], dnr: [], rules: new Map(), injected: [], sentToWindow: 0, closed: false,
    order: [], silent: false };
  const off = (arr, f) => { const i = arr.indexOf(f); if (i >= 0) arr.splice(i, 1); };
  c.windows = {
    create: async (o) => { kit.windows.push(clone(o)); return { id: WIN, tabs: [{ id: WTAB, windowId: WIN }] }; },
    remove: async () => {},
    onRemoved: { addListener: f => kit.winRm.push(f), removeListener: f => off(kit.winRm, f) },
  };
  const t = c.tabs; // shared.tabs（popup と同じ物）を書き換える = closeAllRegistered などが呼ぶ時点の関数が代役になる
  const get0 = t.get, remove0 = t.remove;
  t.get = async (id) => { if (id === WTAB && kit.closed) throw new Error('No tab with id'); return get0(id); };
  t.remove = async (id) => {
    kit.removedTabs.push(id);
    kit.order.push('remove:' + id);
    if (id === WTAB && kit.silent) { kit.closed = true; return remove0(id); }
    if (id === WTAB && !kit.closed) { kit.closed = true; for (const f of kit.tabRm.slice()) f(WTAB, { windowId: WIN, isWindowClosing: true }); for (const f of kit.winRm.slice()) f(WIN); }
    return remove0(id);
  };
  t.update = async (id, o) => { kit.updated.push([id, o && o.url]); return { id }; };
  const create0 = t.create;
  t.create = async (o) => { kit.order.push('create'); return create0(o); };
  t.onRemoved = { addListener: f => kit.tabRm.push(f), removeListener: f => off(kit.tabRm, f) };
  const send0 = t.sendMessage;
  t.sendMessage = (id, msg, opts, cb) => {
    if (typeof opts === 'function') { cb = opts; opts = undefined; }
    if (id === WTAB) kit.sentToWindow++;
    if (send0) return send0(id, msg, opts, cb);
    Promise.resolve().then(() => cb && cb({ ok: false, error: 'no receiver' }));
  };
  // bgTabUrl = 背景タブ（7）の着いた URL（終わり方 ② の段 pdf で、取り直しが別オリジンに着く型）
  t.get = ((g) => async (id) => (id === 7 && kit.bgTabUrl ? { id: 7, status: 'complete', url: kit.bgTabUrl } : g(id)))(t.get);
  c.webRequest = { onHeadersReceived: { addListener: (f, filter) => kit.wr.push({ f, filter: clone(filter) }), removeListener: f => { const i = kit.wr.findIndex(l => l.f === f); if (i >= 0) kit.wr.splice(i, 1); } } };
  c.declarativeNetRequest = {
    updateSessionRules: async (o) => { kit.dnr.push(clone(o)); if (!(o.addRules || []).length && (o.removeRuleIds || []).length) kit.order.push('unrule'); for (const id of o.removeRuleIds || []) kit.rules.delete(id); for (const r of o.addRules || []) kit.rules.set(r.id, clone(r)); },
    getSessionRules: async () => [...kit.rules.values()].map(clone),
  };
  sw.navigator = { userAgent: 'Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36' };
  const exec0 = c.scripting.executeScript;
  c.scripting.executeScript = async (o) => { kit.injected.push(o.target.tabId); return exec0(o); };
  // 窓の工程も settle が待てるように（runSave と同じ包み）。sendToTab と拡張 fetch・/capture/attachment は試験ごとの答え（c8Answer・c8Api）
  sw.c8Fetched = [];
  vm.runInContext(`const realRunRetry = runRetry; runRetry = (job) => (lastRun = realRunRetry(job));
    const c8PrevSend = sendToTab;
    sendToTab = async (msg, tabId) => {
      if (tabId === ${WTAB}) c8SentToWindow = (typeof c8SentToWindow === 'number' ? c8SentToWindow : 0) + 1;
      if (msg.type === 'openref-fetch-pdf' && typeof c8Answer === 'function') { c8Fetched.push(['tab', tabId, msg.url]); return c8Answer(msg.url, 'tab'); }
      return c8PrevSend(msg, tabId);
    };
    const c8PrevBg = bgFetchPdf;
    bgFetchPdf = async (url, follow) => { if (typeof c8Answer === 'function') { c8Fetched.push(['bg', null, url]); return c8Answer(url, 'bg'); } return c8PrevBg(url, follow); };
    const c8PrevApi = api;
    api = async (p, o) => (typeof c8Api === 'function' && p.startsWith('/capture/attachment')) ? c8Api(p, o) : c8PrevApi(p, o);`, sw);
  kit.waiting = () => vm.runInContext('!!(liveJob && liveJob.wake)', sw);
  kit.found = (url, headers = PDF_HEADERS) => {
    for (const l of kit.wr.slice()) if (l.filter.tabId === WTAB) l.f({ tabId: WTAB, type: 'main_frame', frameId: 0, statusCode: 200, url, responseHeaders: headers });
  };
  kit.userCloses = () => {
    kit.closed = true;
    for (const f of kit.tabRm.slice()) f(WTAB, { windowId: WIN, isWindowClosing: true });
    for (const f of kit.winRm.slice()) f(WIN);
  };
  kit.waitMs = vm.runInContext('WINDOW_WAIT_MS', sw);
  kit.timeout = () => { const e = env.swLong.find(x => x.ms === kit.waitMs && !x.fired); assert.ok(e, 'no window-wait timer'); e.fired = true; e.f(); };
  kit.serve = (fn) => { sw.c8Answer = (url, how) => { kit.order.push('fetch'); return fn(url, how); }; };
  return kit;
}
const pdfOk = (tag = 'win') => ({ ok: true, b64: btoa('%PDF-' + tag) });
const attachCalls = (app) => app.calls.filter(c => c[0].startsWith('/capture/attachment')).map(c => c[0]);
async function c8Save(flow, extra = {}) {
  const [opts, landing] = C4_FLOWS[flow];
  const o = Object.assign({}, opts, extra);
  const { app, lines, headLines } = await saveAndRead(o, landing);
  const kit = c8Kit(app.env);
  return { app, kit, lines, o };
}
/* ボタンを押し、窓の工程が窓を開いて待つところまで進める */
async function pressWindow(app, kit) {
  await app.els.viaproxy.handlers.click();
  for (let i = 0; i < 300 && !kit.waiting(); i++) await tick();
  assert.ok(kit.waiting(), '窓の工程が待ちに入らない: ' + JSON.stringify(app.record()));
}
const C8_TARGETS = { login: 'https://doi.org/' + ej, stuck: artPdf, challenge: pmcLink };

for (const flow of ['login', 'stuck', 'challenge']) {
  test(`${flow}: with the all-sites permission the button is the window; it opens the stopped stage's URL in a focused popup window`, async () => {
    const { app, kit } = await c8Save(flow);
    assert.equal(app.els.viaproxy.style.display, 'block');
    assert.match(app.els.viaproxy.textContent, /窓で開いて確認・ログインする/);
    // 押す前に読める案内（窓が前面に出ると popup は閉じ、状態行は見えない）
    assert.equal(app.els.winhint.style.display, 'block');
    assert.equal(app.els.winhint.textContent, '済んだら窓を閉じてください（PDF が開けばそのまま取り込みます）');
    same(kit.windows, []); // 自動では開かない
    const rec = app.record();
    assert.equal(rec.retry.stage, { login: 'doi', stuck: 'pdf', challenge: 'pmc' }[flow]);
    assert.equal(rec.retry.key, 'K1');
    const createdBefore = app.created.length;
    await pressWindow(app, kit);
    same(kit.windows, [{ url: 'about:blank', type: 'popup', focused: true, width: 1000, height: 760 }]);
    same(kit.updated, [[WTAB, C8_TARGETS[flow]]]);
    assert.equal(app.created.length, createdBefore); // 新しいタブ（プロキシ経由で開く）は開かない
    assert.equal(app.els.viaproxy.style.display, 'none');
    assert.equal(app.record().state, 'running');
    assert.match(app.record().line, /窓を開きました（.+）。確認やログインはその窓で済ませ、済んだら窓を閉じてください。PDF が開けばそのまま取り込みます（5 分で窓を閉じます）/);
    assert.equal(app.els.winhint.style.display, 'none'); // 押した後は隠す
    // 見張りは窓のタブに絞る（最上位の読込・応答ヘッダ）
    same(kit.wr.map(l => l.filter), [{ urls: ['http://*/*', 'https://*/*'], types: ['main_frame'], tabId: WTAB }]);
    same([...kit.rules.values()].map(r => r.condition.tabIds), [[WTAB]]);
    kit.userCloses();
    await app.env.settle();
  });
  test(`${flow} with the proxy template: the window opens ${flow === 'challenge' ? 'the plain PMC link (PMC is not proxied)' : 'the proxied URL'}`, async () => {
    const { app, kit } = await c8Save(flow, { proxy: PROXY });
    // 前置きは押したときの設定（service worker が storage.local から読む）
    app.sw.chrome.storage.local = { get: async () => ({ openref_proxy_template: PROXY }) };
    // 文言は起きる事に合わせる: 前置きを付けて開く段（pdf / doi）だけ「プロキシ経由」・PMC は前置きなし
    assert.match(app.els.viaproxy.textContent, flow === 'challenge' ? /^窓で開いて確認・ログインする/ : /プロキシ経由の窓でログインする/);
    await pressWindow(app, kit);
    same(kit.updated, [[WTAB, flow === 'challenge' ? pmcLink : PROXY + C8_TARGETS[flow]]]);
    kit.userCloses();
    await app.env.settle();
  });
}
test('an OpenAthens template encodes the target; a page already inside the proxy is not proxied twice', async () => {
  {
    const { app, kit } = await c8Save('stuck', { proxy: OA });
    app.sw.chrome.storage.local = { get: async () => ({ openref_proxy_template: OA }) };
    await pressWindow(app, kit);
    same(kit.updated, [[WTAB, 'https://go.openathens.net/redirector/example.ac.jp?url=' + encodeURIComponent(artPdf)]]);
    kit.userCloses();
    await app.env.settle();
  }
  const pAtArt = proxify(artPdf);
  const o = { proxy: PROXY, setup: articleTab({ doi: ej, pdfUrl: pAtArt, pdfSource: 'meta', url: proxify(art) }, Object.assign({}, htmlAtArt, { url: pAtArt })),
    routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaBot) } };
  const { app } = await saveAndRead(o, { doi: ej });
  const kit = c8Kit(app.env);
  app.sw.chrome.storage.local = { get: async () => ({ openref_proxy_template: PROXY }) };
  await pressWindow(app, kit);
  same(kit.updated, [[WTAB, PROXY + artPdf]]);
  kit.userCloses();
  await app.env.settle();
});
for (const flow of ['no_pdf', 'mismatch', 'no class (HTTP 403)']) {
  test(`${flow}: no window button (and no record material)`, async () => {
    const { app } = await c8Save(flow);
    assert.equal(app.els.viaproxy.style.display, 'none');
    assert.equal(app.record().retry, null);
  });
}
test('without the all-sites permission: the job leaves no material, the popup never offers the window', async () => {
  const opts = Object.assign({}, C4_FLOWS['login (a link-picked PDF)'][0], { proxy: PROXY }); // 許可なしでも login になる流れ
  const { app, lines, headLines } = await saveAndRead(opts, {});
  assert.equal(headLines.at(-1), NEXT.login);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（ログインが要る）', '登録 K1']);
  assert.equal(app.record().retry, null);
  assert.equal(app.els.viaproxy.style.display, 'block'); // 「プロキシ経由で開く」（新しいタブ）
  assert.equal(app.els.viaproxy.textContent, 'プロキシ経由で開く');
});
test('a popup without the all-sites permission (revoked after the save) shows the proxy tab button, not the window', async () => {
  const { app: first, o, kit } = await c8Save('stuck', { proxy: PROXY });
  const app = await reopenWithoutAllSites(first, o);
  assert.equal(app.els.viaproxy.textContent, 'プロキシ経由で開く');
  await app.els.viaproxy.handlers.click();
  same(kit.windows, []);
  same(app.created.at(-1), { url: PROXY + artPdf, active: true });
});
test('the service worker refuses the window when the permission is gone, and never opens one', async () => {
  const { app, kit } = await c8Save('stuck');
  app.env.shared.permissions.contains = async () => false;
  const res = await new Promise(r => app.env.deliver({ type: 'openref-retry', id: app.record().id }, r));
  assert.equal(res.ok, false);
  assert.match(res.error, /全サイトへのアクセス許可/);
  same(kit.windows, []);
  assert.equal(app.record().state, 'done');
});
test('the button is only for this tab and this page: another tab or another page of the same tab shows none', async () => {
  const { app, kit, o } = await c8Save('stuck');
  const other = popupApp('https://www.example-journal.org/article/999', {}, Object.assign({}, o, { env: app.env, tabId: 2 }));
  await vm.runInContext('init()', other.context);
  assert.equal(other.els.viaproxy.style.display === 'block', false);
  const moved = popupApp(art + '?page=2', {}, Object.assign({}, o, { env: app.env, tabId: 1 }));
  await vm.runInContext('init()', moved.context);
  assert.equal(moved.els.viaproxy.style.display === 'block', false);
  same(kit.windows, []);
});
test('while the window job runs, a reopened popup shows no button; a second press of the same result is refused', async () => {
  const { app, kit, o } = await c8Save('stuck');
  const id = app.record().id;
  await pressWindow(app, kit);
  const again = popupApp(art, {}, Object.assign({}, o, { env: app.env }));
  await vm.runInContext('init()', again.context);
  assert.equal(again.els.viaproxy.style.display === 'block', false);
  const res = await new Promise(r => app.env.deliver({ type: 'openref-retry', id }, r));
  assert.equal(res.ok, false);
  same(kit.windows.length, 1);
  kit.userCloses();
  await app.env.settle();
  // 終わった後に古い結果の id で押しても始まらない（記録はもう窓の工程の物）
  const late = await new Promise(r => app.env.deliver({ type: 'openref-retry', id }, r));
  assert.equal(late.ok, false);
  same(kit.windows.length, 1);
});
test('the message carries only the record id: extra fields from the popup are not used', async () => {
  const { app, kit } = await c8Save('stuck');
  await new Promise(r => app.env.deliver({ type: 'openref-retry', id: app.record().id, url: 'https://evil.example/', key: 'K9', stage: 'doi' }, r));
  for (let i = 0; i < 300 && !kit.waiting(); i++) await tick();
  same(kit.updated, [[WTAB, artPdf]]);
  kit.serve(() => pdfOk());
  kit.userCloses();
  await app.env.settle();
  assert.ok(attachCalls(app).every(u => u.includes('key=K1')), attachCalls(app));
});

/* 終わり方 ①: 窓に PDF が届く */
test('end 1: a PDF reaching the window is detected, the window closed, refetched from the article tab, attached to the same key; /capture/save is not called again', async () => {
  const { app, kit } = await c8Save('stuck');
  const saves = app.calls.filter(c => c[0] === '/capture/save').length;
  const attachesBefore = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve((url) => (url === artPdf ? pdfOk() : { ok: false, error: 'x' }));
  kit.found(artPdf);
  await app.env.settle();
  assert.ok(kit.removedTabs.includes(WTAB), 'window not closed');
  same(app.sw.c8Fetched[0], ['tab', 1, artPdf]); // 元の論文タブから
  assert.ok(kit.order.indexOf('remove:' + WTAB) < kit.order.indexOf('fetch'), JSON.stringify(kit.order)); // 窓を閉じてから取る
  assert.ok(kit.order.indexOf('remove:' + WTAB) < kit.order.indexOf('unrule'), JSON.stringify(kit.order)); // 閉じてから規則を外す
  same(attachCalls(app).slice(attachesBefore), ['/capture/attachment?key=K1&source=meta&via=page']);
  assert.equal(app.calls.filter(c => c[0] === '/capture/save').length, saves);
  const rec = app.record();
  assert.equal(rec.state, 'done');
  assert.match(rec.line, /^書誌 ✓ ・ PDF ✓\n既存 K1 — PDF 添付 ✓（窓で確認・タブ内 fetch/);
  assert.equal(rec.retry, null);
  // バッジは走行中（…）の後に成功（✓）。popup が結果を見て消す（''）のは、その後
  assert.ok(app.env.badges.lastIndexOf('✓') > app.env.badges.lastIndexOf('…'), JSON.stringify(app.env.badges));
  same(kit.wr, []);
  same([...kit.rules.keys()], []);
  same(kit.injected.filter(id => id === WTAB), []); // 窓のタブに content script を入れない
  assert.equal(app.sw.c8SentToWindow || 0, 0);
  assert.equal(kit.sentToWindow, 0); // chrome.tabs.sendMessage（documentId つきの直接の呼び出しを含む）も窓のタブへは送らない
});
test('end 1: the PDF is fetched by the extension when the article tab is gone; the window tab itself is never fetched from', async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  app.env.userTabs[1] = 'https://www.example-journal.org/other'; // 利用者が別のページへ移った
  kit.serve((url, how) => (how === 'bg' ? pdfOk() : { ok: false, error: 'x' }));
  kit.found(artPdf);
  await app.env.settle();
  same(app.sw.c8Fetched, [['bg', null, artPdf]]);
  assert.match(app.record().line, /PDF 添付 ✓（窓で確認・拡張 fetch/);
});
for (const [name, flow, proxy, hit, want] of [
  ['the declared PDF URL itself keeps the declared source', 'stuck', null, artPdf, 'meta'],
  ['the declared PDF URL with another query and fragment keeps the declared source', 'stuck', null, artPdf + '?download=1#page=2', 'meta'],
  ['another path on the same origin (supplement / author guide the user opened) is source=link', 'stuck', null, 'https://www.example-journal.org/article/123/suppl.pdf', 'link'],
  ['another origin is source=link', 'stuck', null, 'https://cdn.example-journal.net/x.pdf', 'link'],
  ['the same path on a proxied host of the publisher (EZproxy hostname style) keeps the declared source', 'stuck', PROXY, proxify(artPdf), 'meta'],
  ['another path on a proxied host is source=link', 'stuck', PROXY, proxify('https://www.example-journal.org/guide.pdf'), 'link'],
  ['the DOI page stage is always source=link', 'login', null, artPdf, 'link'],
  ['the PMC stage is always source=link', 'challenge', null, pmcPdf, 'link'],
]) test(`end 1 source: ${name}`, async () => {
  const { app, kit } = await c8Save(flow, proxy ? { proxy } : {});
  if (proxy) app.sw.chrome.storage.local = { get: async () => ({ openref_proxy_template: proxy }) };
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  kit.found(hit);
  await app.env.settle();
  const posted = attachCalls(app).slice(before);
  assert.equal(posted.length, 1, JSON.stringify(posted));
  assert.equal(new URLSearchParams(posted[0].split('?')[1]).get('source'), want);
});
test('end 1: another article opened inside the window is refused by the DOI check and not attached (mismatch, no button)', async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  kit.serve(() => pdfOk('other'));
  app.sw.c8Api = (p) => { app.calls.push([p, null]); throw apiError(409, '取得した PDF は別論文（DOI 10.1000/other）でした', { status: 'mismatch', found_doi: '10.1000/other' }); };
  kit.found('https://www.example-journal.org/article/999.pdf');
  await app.env.settle();
  const rec = app.record();
  assert.match(rec.line, /PDF は添付しませんでした — 取得した PDF は別論文/);
  assert.equal(rec.failClass, 'mismatch');
  assert.equal(rec.retry, null);
  assert.equal(app.els.viaproxy.style.display, 'none');
});

/* 終わり方 ②: 利用者が窓を閉じる = 止まった段を 1 回だけやり直す */
test('end 2 (PDF stage): the user closes the window, the declared PDF is fetched once more and attached', async () => {
  const { app, kit } = await c8Save('stuck');
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  kit.userCloses();
  await app.env.settle();
  same(app.sw.c8Fetched, [['tab', 1, artPdf]]);
  same(attachCalls(app).slice(before), ['/capture/attachment?key=K1&source=meta&via=page']);
  assert.match(app.record().line, /^書誌 ✓ ・ PDF ✓\n既存 K1 — PDF 添付 ✓/);
  same(kit.injected.filter(id => id === WTAB), []);
  assert.equal(kit.sentToWindow, 0);
  assert.equal(app.sw.c8SentToWindow || 0, 0);
});
test('end 2 (PMC stage): the PMC route runs once more in a background tab after the window is closed', async () => {
  const { app, kit } = await c8Save('challenge');
  app.sw.landingData = { pmid: '21620466', doi: null, pdfUrl: pmcPdf, pdfSource: 'meta' };
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  const created = app.created.length;
  kit.userCloses();
  await app.env.settle();
  assert.equal(app.created.length, created + 1); // 背景タブを 1 つ（窓が閉じた後）
  assert.ok(kit.order.indexOf('remove:' + WTAB) < kit.order.indexOf('create'), JSON.stringify(kit.order));
  assert.equal(app.created.at(-1).url, 'about:blank'); // 見張りを付けてから PMC へ移す
  same(attachCalls(app).slice(before), ['/capture/attachment?key=K1&source=meta&via=doi-tab']);
  assert.ok(kit.updated.some(([id, u]) => id === 7 && u === pmcLink), JSON.stringify(kit.updated));
  assert.match(app.record().line, /PMC 本文経由/);
  same(kit.injected.filter(id => id === WTAB), []); // 注入したのは背景タブ（7）だけ
});
test('end 2 (DOI stage): the DOI page route runs once more; the window\'s URL is not reused', async () => {
  const { app, kit } = await c8Save('login');
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  kit.userCloses();
  await app.env.settle();
  same(attachCalls(app).slice(before), ['/capture/attachment?key=K1&source=meta&via=doi-tab']);
  assert.ok(kit.updated.some(([id, u]) => id === 7 && u === 'https://doi.org/' + ej), JSON.stringify(kit.updated));
});
test('the retry is once per press: a retry that fails again ends with its class and the button comes back; nothing runs by itself', async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  const fetchedBefore = app.sw.c8Fetched.length;
  kit.serve(() => htmlAtArt);
  kit.userCloses();
  await app.env.settle();
  const rec = app.record();
  assert.equal(rec.state, 'done');
  assert.equal(rec.failClass, 'stuck');
  assert.equal(rec.head.split('\n').at(-1), NEXT.stuck); // 分類の行は畳まずに見せる部分の最後
  same(rec.head.split('\n').slice(0, 2), ['書誌 ✓ ・ PDF ✗（途中のページで止まった）', '既存 K1']);
  assert.ok(rec.retry, 'no material for the next press');
  same(kit.windows.length, 1); // 窓は 1 回だけ
  // やり直しの取得は 1 巡（ページ内 → 拡張 → 背景タブで 1 回の取り直し）だけ
  const urls = app.sw.c8Fetched.slice(fetchedBefore).map(f => f[2]);
  assert.ok(urls.length <= 4, JSON.stringify(urls));
  assert.equal(app.els.viaproxy.style.display, 'block'); // また押せる
  for (let i = 0; i < 20; i++) await tick();
  same(kit.windows.length, 1);
});

/* 120 秒・中断・追い越し・service worker の起動 */
test('after 5 minutes the window is closed and the save ends with the previous class and next step (no retry, no post)', async () => {
  const { app, kit } = await c8Save('stuck');
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  kit.timeout();
  await app.env.settle();
  assert.ok(kit.removedTabs.includes(WTAB));
  same(attachCalls(app).slice(before), []);
  same(app.sw.c8Fetched, []);
  const rec = app.record();
  assert.match(rec.line, /窓で 5 分待ちましたが PDF は届きませんでした/);
  assert.equal(rec.head.split('\n').at(-1), NEXT.stuck);
  same(rec.head.split('\n').slice(0, 2), ['書誌 ✓ ・ PDF ✗（途中のページで止まった）', '既存 K1']);
  assert.equal(rec.line.split('\n').at(-1), '書誌は保存済み。窓で 5 分待ちましたが PDF は届きませんでした（窓を閉じました）'); // 畳む側
  assert.equal(rec.failClass, 'stuck');
  same(kit.wr, []);
  same([...kit.rules.keys()], []);
  assert.equal(vm.runInContext('WINDOW_WAIT_MS', app.sw), 300000);
});
test('"abort this save" while waiting: the window is closed, the watch and rule removed, nothing is fetched or sent', async () => {
  const { app, kit } = await c8Save('stuck');
  const callsBefore = app.calls.length;
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  await new Promise(r => app.env.deliver({ type: 'openref-abort', id: app.record().id }, r));
  await app.env.settle();
  for (let i = 0; i < 10; i++) await tick();
  assert.ok(kit.removedTabs.includes(WTAB));
  same(kit.wr, []);
  same([...kit.rules.keys()], []);
  same(app.sw.c8Fetched, []);
  same(app.calls.slice(callsBefore), []);
  assert.equal(app.record().state, 'aborted');
  assert.ok(kit.order.indexOf('remove:' + WTAB) >= 0 && kit.order.indexOf('remove:' + WTAB) < kit.order.indexOf('unrule'), JSON.stringify(kit.order));
  kit.found(artPdf); // 中断の後に届いた PDF は拾わない
  await app.env.settle();
  same(app.calls.slice(callsBefore), []);
});
test('the job limit (another save may then take over) closes the window and sends nothing', async () => {
  const { app, kit } = await c8Save('stuck');
  const callsBefore = app.calls.length;
  await pressWindow(app, kit);
  const limit = app.env.swLong.filter(x => x.ms === inJob('JOB_LIMIT_MS')).at(-1);
  limit.f();
  await app.env.settle();
  for (let i = 0; i < 10; i++) await tick();
  assert.ok(kit.removedTabs.includes(WTAB));
  same(kit.wr, []);
  same(app.calls.slice(callsBefore), []);
  assert.equal(app.record().state, 'aborted');
});
test('service worker start: the registered window tab is closed and the rules removed', async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  await vm.runInContext('removeAllPdfRules(closeAllRegistered())', app.sw);
  assert.ok(kit.removedTabs.includes(WTAB));
  same([...kit.rules.keys()], []);
  await app.env.settle();
});
test('the 90 s background-tab alarm does not close the window while its job runs', async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  app.env.alarm('openref-hidden-tab-' + WTAB);
  for (let i = 0; i < 10; i++) await tick();
  assert.equal(kit.removedTabs.includes(WTAB), false);
  assert.ok(kit.waiting());
  kit.userCloses();
  await app.env.settle();
});
test('the user\'s other tabs are not touched: only the window tab is watched, closed or updated', async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  kit.serve(() => pdfOk());
  kit.found(artPdf);
  await app.env.settle();
  same([...new Set(kit.removedTabs)], [WTAB]);
  same(kit.updated.map(u => u[0]), [WTAB]);
});
test('popup.html: the one button keeps its id; the new labels do not collide with existing ids or classes', () => {
  const html = code('popup.html');
  assert.equal((html.match(/id="viaproxy"/g) || []).length, 1);
  assert.match(code('popup.js'), /type: "openref-retry", id, size: screenSize\(\)/);
});

// timeout: 起こさない作りでは工程が終わらない（settle が返らない）= 試験は時間切れで落ちる
test('an abort ends the wait even when no close event arrives (the job does not sit until the wait timer)', { timeout: 5000 }, async () => {
  const { app, kit } = await c8Save('stuck');
  await pressWindow(app, kit);
  kit.silent = true;
  await new Promise(r => app.env.deliver({ type: 'openref-abort', id: app.record().id }, r));
  for (let i = 0; i < 20 && kit.waiting(); i++) await tick();
  assert.equal(kit.waiting(), false);
  await app.env.settle(); // 120 秒のタイマーを起こさずに終わる
  assert.equal(app.env.swLong.some(x => x.ms === kit.waitMs && x.fired), false);
  assert.ok(kit.removedTabs.includes(WTAB));
  same(kit.wr, []);
});
test('startJob with expectId starts only while the record is still that finished save', async () => {
  const env = swEnv();
  await env.session.area.set({ openref_job: { id: 'NEWER', tabId: 1, tabUrl: art, startedAt: 0, state: 'done', line: '' } });
  const input = { tabId: 1, tabUrl: art, page: { doi: ej }, contentScriptOk: true };
  const r = await vm.runInContext('(input) => startJob(input, { expectId: "OLD", run: async () => {} })', env.sw)(input);
  assert.equal(r.ok, false);
  assert.match(r.error, /もう新しくありません/);
  assert.equal(env.record().id, 'NEWER');
  const ok = await vm.runInContext('(input) => startJob(input, { expectId: "NEWER", run: async () => {} })', env.sw)(input);
  assert.equal(ok.ok, true);
});
test('end 1 source: the DOI page stage is source=link even when the page declared a PDF of the same origin', async () => {
  // ページの宣言した PDF は 403（分類なし）・DOI ページの宣言した PDF は 401（login）= 止まった段は DOI ページ
  const setup = `
    chrome.scripting.executeScript = async () => {};
    sendToTab = async (msg, tabId) => {
      if (msg.type === 'openref-analyze') return { ok: true, data: Object.assign({ doi: null, pmid: null, pdfUrl: null, pdfSource: null, title: 'T', url: ${JSON.stringify(art)} },
        tabId === 7 ? { doi: ${JSON.stringify(ej)}, pdfUrl: ${JSON.stringify(artPdf)}, pdfSource: 'meta' } : { doi: ${JSON.stringify(ej)}, pdfUrl: ${JSON.stringify(artPdf)}, pdfSource: 'meta' }) };
      if (msg.type === 'openref-refresh-url') return { ok: true, url: null };
      if (msg.type === 'openref-fetch-pdf') return tabId === 7 ? ${JSON.stringify(unauthorized401)} : { ok: false, error: 'Error: HTTP 403', html: false, httpStatus: 403, url: ${JSON.stringify(artPdf)} };
    };
    bgFetchPdf = async () => ({ ok: false, error: 'Error: HTTP 403', html: false, httpStatus: 403, url: ${JSON.stringify(artPdf)} });`;
  const { app, lines, headLines } = await saveAndRead({ setup, routes: { '/capture/save': savedK1(ej), '/capture/fetch-oa': apiError(422, oaNone) } });
  assert.equal(headLines.at(-1), NEXT.login);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（ログインが要る）', '登録 K1']);
  assert.equal(app.record().retry.stage, 'doi');
  const kit = c8Kit(app.env);
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  same(kit.updated, [[WTAB, 'https://doi.org/' + ej]]);
  kit.serve(() => pdfOk());
  kit.found(artPdf);
  await app.env.settle();
  const posted = attachCalls(app).slice(before);
  same(posted.map(u => new URLSearchParams(u.split('?')[1]).get('source')), ['link']);
});

test('a PMID-only save: a PDF of another path that reached the window is posted as source=link and refused when the DOI cannot be confirmed', async () => {
  const setup = articleTab({ pmid: '31000001', pdfUrl: artPdf, pdfSource: 'meta' }, htmlAtArt);
  const { app, lines, headLines } = await saveAndRead({ setup, routes: { '/capture/save': { key: 'K1', status: 'created', doi: null, has_pdf: false, title: 'T' }, '/capture/fetch-oa': apiError(422, oaBot) } });
  assert.equal(headLines.at(-1), NEXT.stuck);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（途中のページで止まった）', '登録 K1']);
  assert.equal(app.record().retry.stage, 'pdf');
  const kit = c8Kit(app.env);
  await pressWindow(app, kit);
  kit.serve(() => pdfOk('guide'));
  const posted = [];
  // connectd の照合の代わり: link（ヒューリスティック）で item に DOI が無ければ 422 unverified（Declared なら印なしで添付される）
  app.sw.c8Api = (p) => {
    posted.push(p);
    const source = new URLSearchParams(p.split('?')[1]).get('source');
    if (source === 'link') throw apiError(422, 'ページ上の .pdf リンクから取った PDF は目的 DOI を確認できないため添付しません', { status: 'unverified' });
    return { status: 'attached', bytes: 9 };
  };
  kit.found('https://www.example-journal.org/authors/guide.pdf');
  await app.env.settle();
  same(posted.map(u => new URLSearchParams(u.split('?')[1]).get('source')), ['link']);
  assert.match(app.record().line, /PDF は添付しませんでした/);
  assert.doesNotMatch(app.record().line, /PDF 添付 ✓/);
});
test('end 2 (PDF stage): a retry whose background tab lands on another origin posts source=link', async () => {
  const { app, kit } = await c8Save('stuck');
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  const cdn = 'https://cdn.example-journal.net/signed/123.pdf';
  kit.bgTabUrl = cdn;
  kit.serve((url) => (url === cdn ? pdfOk() : htmlAtArt)); // 宣言の URL は HTML（中継）→ 背景タブが別オリジンの PDF に着く
  kit.userCloses();
  await app.env.settle();
  same(attachCalls(app).slice(before), ['/capture/attachment?key=K1&source=link&via=page']);
});
test('end 1: when the PDF that reached the window cannot be fetched, the save ends without retrying the stage', async () => {
  const { app, kit } = await c8Save('stuck');
  const before = attachCalls(app).length;
  await pressWindow(app, kit);
  kit.serve(() => htmlAtArt);
  const created = app.created.length;
  kit.found(artPdf);
  await app.env.settle();
  same(app.sw.c8Fetched.map(f => f[2]), [artPdf, artPdf]); // 届いた URL を論文タブ → 拡張で 1 回ずつ。宣言の URL を取り直さない
  assert.equal(app.created.length, created); // 背景タブを開かない
  same(attachCalls(app).slice(before), []);
  assert.match(app.record().line, /窓に届いた PDF を取り込めませんでした/);
});
test('all-sites, a proxy template and no retry material: the button is the proxy tab', async () => {
  const setup = articleTab({ pmid: '31000002', pdfUrl: artPdf, pdfSource: 'link' }, idpAnswer);
  const { app, lines, headLines } = await saveAndRead({ proxy: PROXY, setup, routes: { '/capture/save': { key: 'K1', status: 'created', doi: null, has_pdf: false, title: 'T' }, '/capture/fetch-oa': apiError(422, oaBot) } });
  assert.equal(headLines.at(-1), NEXT.login);
  same([lines[0], lines[1]], ['書誌 ✓ ・ PDF ✗（ログインが要る）', '登録 K1']);
  assert.equal(app.record().retry, null); // 宣言の PDF も DOI も無い = 窓で開く先が無い
  assert.equal(app.els.viaproxy.style.display, 'block');
  assert.equal(app.els.viaproxy.textContent, 'プロキシ経由で開く');
  assert.equal(app.els.winhint.style.display === 'block', false);
  const kit = c8Kit(app.env);
  await app.els.viaproxy.handlers.click();
  same(kit.windows, []);
  same(app.created.at(-1), { url: PROXY + art, active: true });
});
test('an incognito tab: the popup shows no window button (the proxy tab with a template), the service worker refuses', async () => {
  const [opts, landing] = C4_FLOWS.stuck;
  const o = Object.assign({ proxy: PROXY }, opts, { setup: opts.setup + `
    if (typeof chrome !== 'undefined' && chrome.tabs && chrome.tabs.query && !chrome.tabs.__incog) {
      const q0 = chrome.tabs.query; chrome.tabs.query = async (x) => (await q0(x)).map(t => Object.assign({}, t, { incognito: true }));
      const g0 = chrome.tabs.get; chrome.tabs.get = async (id) => Object.assign({}, await g0(id), id === 1 ? { incognito: true } : {});
      chrome.tabs.__incog = true;
    }` });
  const { app } = await saveAndRead(o, landing);
  assert.equal(app.record().retry.stage, 'pdf'); // 記録は残る（押せないのは popup と service worker の側）
  assert.equal(app.els.viaproxy.textContent, 'プロキシ経由で開く');
  const kit = c8Kit(app.env);
  const res = await new Promise(r => app.env.deliver({ type: 'openref-retry', id: app.record().id }, r));
  assert.equal(res.ok, false);
  assert.match(res.error, /シークレット/);
  same(kit.windows, []);
});
test('a popup left open hides the window button when the record turns into another tab\'s save', async () => {
  const { app } = await c8Save('stuck');
  assert.equal(app.els.viaproxy.style.display, 'block');
  await app.env.session.area.set({ openref_job: { id: 'OTHER', tabId: 2, tabUrl: 'https://other.example/x', startedAt: 0, state: 'running', line: '…', cls: '' } });
  assert.equal(app.els.viaproxy.style.display, 'none');
  assert.equal(app.els.winhint.style.display, 'none');
});


/* ===== URL の規則は pdfbytes.js の 1 つ（urlIds・cleanDoi・looksLikePdfUrl / hasPdfExtension・samePage・pmcLinkFor）。
   content.js の analyze（メタ・リンクなし = URL 由来の規則だけ）と popup の URL だけの解析に同じ入力を通し、同じ答えを表明する。
   表の入力は URL の規則の調査（2026-11-14）の物。前置きは EZproxy の login?url= 型 ===== */
const X7_PROXY = 'https://login.ezproxy.u.ac.jp/login?url=';
const X7 = 'https://pub.example.org';
function x7Content(url, proxy = null, { links = [], fullText = [] } = {}) {
  let listener;
  runContent({ URL, window: {}, location: new URL(url), document: {
    title: 'T', contentType: 'text/html', body: { innerText: '' }, baseURI: url, querySelector: () => null,
    querySelectorAll: sel => (sel === 'a[href]' ? links : sel === '.full-text-links-list a[href]' ? fullText : []).map(href => ({ href })),
  }, chrome: { runtime: { onMessage: { addListener: f => listener = f } } } });
  let res;
  listener({ type: 'openref-analyze', proxy }, {}, r => res = r);
  assert.equal(res.ok, true, res.error);
  return res.data;
}
function x7UrlOnly(url, proxy = null) {
  const h = popup();
  vm.runInContext('proxyTemplate = ' + JSON.stringify(proxy), h.context);
  return vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, h.context);
}
test('cleanDoi: punctuation, prefixes and balanced parentheses', () => {
  const s = shared();
  for (const [input, want] of [
    ['10.1000/x.', '10.1000/x'], ['10.1000/x,', '10.1000/x'], ['10.1000/x;', '10.1000/x'], ['10.1000/x]', '10.1000/x'], ['10.1000/x).', '10.1000/x'],
    ['10.1000/x).,', '10.1000/x'], ['doi:10.1000/x', '10.1000/x'], ['DOI: 10.1000/x', '10.1000/x'], ['https://doi.org/10.1000/x', '10.1000/x'],
    ['http://dx.doi.org/10.1000/x', '10.1000/x'], [' 10.1000/x ', '10.1000/x'],
    // 括弧の釣り合い: ")" は "(" より多いときだけ外す
    ['10.1000/a(b)', '10.1000/a(b)'], ['10.1000/a(b))', '10.1000/a(b)'], ['10.1000/a)', '10.1000/a'], ['10.1000/(a(b)', '10.1000/(a(b)'],
    ['10.1000/a(b).', '10.1000/a(b)'], ['10.1000/a(b),', '10.1000/a(b)'], ['10.1016/S0140-6736(10)62307-0', '10.1016/S0140-6736(10)62307-0'],
    ['10.1016/S0140-6736(10)62307-0)', '10.1016/S0140-6736(10)62307-0'],
    // 変えない物（今までどおり）
    ['https://www.doi.org/10.1000/x', null], ['doi.org/10.1000/x', null], ['10.1000/x"', '10.1000/x"'], ["10.1000/x'", "10.1000/x'"],
    ['10.1000/x>', '10.1000/x>'], ['10.1000/x:', '10.1000/x:'], ['10.1000/', '10.1000/'], ['10.100/x', null], ['10.1000/x%2E', '10.1000/x%2E'], ['', null],
  ]) assert.equal(s.cleanDoi(input), want, JSON.stringify(input));
});
for (const [url, doi, pmid, proxy = null] of [
  [X7 + '/doi/10.1000/abc.', '10.1000/abc', null], [X7 + '/doi/10.1000/abc,', '10.1000/abc', null], [X7 + '/doi/10.1000/abc)', '10.1000/abc', null],
  [X7 + '/doi/10.1000/abc;', '10.1000/abc', null], [X7 + '/doi/10.1000/abc]', '10.1000/abc', null], [X7 + '/doi/10.1000/abc.)', '10.1000/abc', null],
  [X7 + '/doi/10.1000/abc%2E', '10.1000/abc', null], [X7 + '/doi/abs/10.1000/a%28b%29', '10.1000/a(b)', null], [X7 + '/doi/10.1000/a%2Fb', '10.1000/a/b', null],
  [X7 + '/doi/full/10.1000/ABC.Def', '10.1000/ABC.Def', null], [X7 + '/doi/10.1000/.', null, null],
  [X7 + '/doi/pdfdirect/10.1000/x', '10.1000/x', null], [X7 + '/doi/epdf/10.1000/x', '10.1000/x', null], [X7 + '/doi/pdf/10.1000/x?download=true', '10.1000/x', null],
  [X7 + '/doi/reader/10.1000/x', null, null], [X7 + '/doi/10.1000/x#sec1', '10.1000/x', null],
  [X7 + '/doi/10.1016/S0140-6736(10)62307-0', '10.1016/S0140-6736(10)62307-0', null], [X7 + '/doi/10.1000/x%E0%A4%A', null, null],
  [X7 + '/doi/doi:10.1000/x', null, null], [X7 + '/journal/doi/10.1000/x', '10.1000/x', null],
  ['https://doi.org/10.1000/x', null, null], ['https://dx.doi.org/10.1000/x.', null, null],
  ['https://webview.isho.jp/journal/detail/pdf/10.18885/CI.0000000000', '10.18885/CI.0000000000', null],
  ['https://webview.isho.jp/journal/detail/pdf/10.18885/CI.0000000000.', '10.18885/CI.0000000000', null],
  ['https://webview.isho.jp/journal/detail/abs/10.18885/CI.0000000000)', '10.18885/CI.0000000000', null],
  ['https://webview.isho.jp/journal/detail/abs/10.18885/CI.0000000000?from=x', '10.18885/CI.0000000000', null],
  ['https://webview.isho.jp/journal/detail/pdf/10.18885/CI%2E0000000000', '10.18885/CI.0000000000', null],
  ['https://webview.isho.jp/journal/detail/pdf/10.18885/', null, null],
  // クエリの中の URL から DOI を作らない
  ['https://login.ezproxy.u.ac.jp/login?url=https://pubs.rsna.org/doi/10.1148/radiol.251234', null, null],
  ['https://login.ezproxy.u.ac.jp/login?url=https://pubs.rsna.org/doi/10.1148/radiol.251234', null, null, X7_PROXY],
  ['https://www.google.com/url?q=https://pub.example.org/doi/10.1000/x&sa=D', null, null],
  ['https://pubs-rsna-org.ezproxy.u.ac.jp/doi/10.1148/radiol.251234.', '10.1148/radiol.251234', null],
  ['https://pubs-rsna-org.ezproxy.u.ac.jp/doi/10.1148/radiol.251234.', '10.1148/radiol.251234', null, X7_PROXY],
  ['https://webview-isho-jp.ezproxy.u.ac.jp/journal/detail/pdf/10.18885/CI.0000000000.', null, null],
  ['https://webview-isho-jp.ezproxy.u.ac.jp/journal/detail/pdf/10.18885/CI.0000000000.', '10.18885/CI.0000000000', null, X7_PROXY],
  // PubMed: ホスト = pubmed.ncbi.nlm.nih.gov・パス /<数字>/ だけ（桁の制限なし = core の parse_input と同じ）
  ['https://pubmed.ncbi.nlm.nih.gov/21620466/', null, '21620466'], ['https://pubmed.ncbi.nlm.nih.gov/21620466', null, '21620466'],
  ['https://pubmed.ncbi.nlm.nih.gov/21620466/#affiliations', null, '21620466'], ['https://pubmed.ncbi.nlm.nih.gov/123/', null, '123'],
  ['https://pubmed.ncbi.nlm.nih.gov/1234567890/', null, '1234567890'], ['https://pubmed.ncbi.nlm.nih.gov/?term=21620466', null, null],
  ['https://pubmed.ncbi.nlm.nih.gov/21620466/citedby/', null, null], ['https://www.ncbi.nlm.nih.gov/pubmed/21620466', null, null],
  ['https://example.org/go?u=https://pubmed.ncbi.nlm.nih.gov/21620466/', null, null], ['https://example.org/pubmed.ncbi.nlm.nih.gov/21620466', null, null],
  ['https://pubmed-ncbi-nlm-nih-gov.ezproxy.u.ac.jp/21620466/', null, null],
  ['https://pubmed-ncbi-nlm-nih-gov.ezproxy.u.ac.jp/21620466/', null, '21620466', X7_PROXY],
  ['https://pmc.ncbi.nlm.nih.gov/articles/PMC3062508/', null, null], ['https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/', null, null],
  ['https://www.ncbi.nlm.nih.gov/pmc/articles/PMC3062508/', null, null],
]) test(`URL identifiers are the same in content and URL-only analysis: ${url}${proxy ? ' (proxy)' : ''}`, () => {
  const c = x7Content(url, proxy);
  const u = x7UrlOnly(url, proxy);
  same([c.doi, c.pmid], [doi, pmid]);
  same([u.doi, u.pmid], [doi, pmid]);
});
/* 「PDF の URL か」: [URL, ページ上のリンク拾い（content）, タブの URL だけの解析（popup・pdfft / J-STAGE / 医書.jp のサイト規則を含む）,
   保存の工程の着地（content script を入れられないページ = 拡張子だけ）] */
const X7_PDF = [
  [X7 + '/a/paper.pdf', true, true, true], [X7 + '/a/paper.PDF', true, true, true], [X7 + '/a/paper.pdf?x=1', true, true, true],
  [X7 + '/a/paper.pdf#page=2', true, true, true], [X7 + '/a/paper.pdf?x=1#page=2', true, true, true],
  [X7 + '/doi/pdf/10.1000/x', true, true, false], [X7 + '/doi/epdf/10.1000/x', true, true, false], [X7 + '/doi/pdfdirect/10.1000/x', true, true, false],
  [X7 + '/science/article/pii/S1/pdfft?md5=a&pid=1-s2.0-S1-main.pdf', true, true, true], [X7 + '/science/article/pii/S1/pdfft', false, true, false],
  [X7 + '/a?file=paper.pdf', true, true, true], [X7 + '/a.pdf/view', false, false, false], ['https://x.pdf.example.org/a', false, false, false],
  [X7 + '/a/paper.pdf;jsessionid=1', false, false, false], ['https://pmc.ncbi.nlm.nih.gov/articles/PMC3062508/pdf/', false, false, false],
  ['https://pmc.ncbi.nlm.nih.gov/articles/PMC3062508/pdf/nihms277358.pdf', true, true, true],
  ['https://www.jstage.jst.go.jp/article/zzzz/42/1/42_5/_pdf/-char/ja', false, true, false],
  ['https://webview.isho.jp/journal/detail/pdf/10.18885/CI.0000000000', false, true, false],
];
for (const [url, link, tab] of X7_PDF) test(`PDF-looking URL (page link / URL-only tab): ${url}`, () => {
  const c = x7Content(X7 + '/toc', null, { links: [url] });
  same([c.pdfUrl, c.pdfSource], link ? [url, 'link'] : [null, null]);
  const u = x7UrlOnly(url);
  same([u.isPdf, u.pdfUrl, u.pdfSource], tab ? [true, url, 'self'] : [false, null, null]);
});
for (const [url, , , landing] of X7_PDF) test(`an uninjectable DOI landing is fetched only by its .pdf extension: ${url}`, async () => {
  const h = landingHarness([blank], { landingUrl: url });
  h.context.chrome.scripting.executeScript = async () => { throw new Error('cannot inject'); };
  if (landing) assert.match(await via(h, "'10.1000/x'", 'null'), /\[self\]/);
  else await assert.rejects(via(h, "'10.1000/x'", 'null'), /content script を注入できません/);
  same(h.removed, [7]);
});
test('samePage: origin + path + query, fragments ignored, unreadable values differ', () => {
  const s = shared();
  for (const [a, b, want] of [
    ['https://x.org/a?b=1#c', 'https://x.org/a?b=1', true], ['https://x.org/a?', 'https://x.org/a', true], ['https://x.org/a#', 'https://x.org/a', true],
    ['https://X.org/a', 'https://x.org/a', true], ['https://x.org:443/a', 'https://x.org/a', true], ['https://x.org/a/', 'https://x.org/a', false],
    ['https://x.org/%7Ea', 'https://x.org/~a', false], ['https://x.org/a?b=1', 'https://x.org/a?b=2', false], ['about:blank', 'about:blank', true],
    ['', '', false], ['not a url', 'not a url', false], [undefined, undefined, false],
    // http(s) でない URL は文字列が完全に同じときだけ（標準でないスキームの origin が "null" になる環境でも別のページ）
    ['chrome://newtab/', 'chrome://settings/', false], ['chrome://newtab/', 'chrome://newtab/', true], ['chrome://newtab/#x', 'chrome://newtab/', false],
    ['chrome-extension://aaa/x', 'chrome-extension://bbb/x', false], ['chrome-extension://aaa/x', 'chrome-extension://aaa/x', true],
    ['file:///a/x.pdf', 'file:///b/x.pdf', false], ['about:blank', 'about:srcdoc', false], ['https://x.org/a', 'chrome://x.org/a', false],
  ]) assert.equal(s.samePage(a, b), want, JSON.stringify([a, b]));
});
test('the popup record and the user tab check use the same samePage (an empty "?" is the same page)', async () => {
  const p = popup();
  vm.runInContext("tab = { id: 1, url: 'https://x.org/a?' };", p.context);
  assert.equal(vm.runInContext("isMine({ tabId: 1, tabUrl: 'https://x.org/a#top' })", p.context), true);
  assert.equal(vm.runInContext("isMine({ tabId: 1, tabUrl: 'https://x.org/b' })", p.context), false);
  const h = worker({ tabUrl: 'https://x.org/a' });
  h.userTab.url = 'https://x.org/a?';
  assert.equal(await vm.runInContext('userTabUnchanged(J)', h.context), true);
  h.userTab.url = 'https://x.org/a?b=1';
  assert.equal(await vm.runInContext('userTabUnchanged(J)', h.context), false);
});
for (const [link, want] of [
  [pmcLink, pmcLink], [pmcLink + '?report=x', pmcLink], [pmcLink + '#x', pmcLink],
  [pmcLink.replace('21620466', '12345678'), null], [pmcLink.replace('https:', 'http:'), null], [pmcLink.replace('.gov/', '.gov.example.org/'), null],
]) test(`the PubMed full-text PMC link is kept in its canonical form: ${link}`, () => {
  assert.equal(x7Content('https://pubmed.ncbi.nlm.nih.gov/21620466/', null, { fullText: [link] }).pmcUrl, want);
});
test('the PMC route opens the canonical link and still refuses another PMID', async () => {
  const ok = landingHarness([pmcReal]);
  assert.match(await via(ok, undefined, `{url:'${pmcLink}?report=x',pmid:'21620466'}`), /PMC 本文経由/);
  assert.ok([...ok.created, ...ok.updated.map(u => u[1])].includes(pmcLink), JSON.stringify([ok.created, ok.updated]));
  for (const route of [`{url:'${pmcLink}',pmid:'12345678'}`, `{url:'${pmcLink.replace('21620466', '12345678')}',pmid:'21620466'}`, `{url:'${pmcLink}',pmid:''}`]) {
    const h = landingHarness([pmcReal]);
    await assert.rejects(via(h, undefined, route), /対象 PMID の PMC リンクではありません/);
  }
});
test('an OpenRefShared left by 0.2.10 (no URL rules) is replaced by the new pdfbytes.js', () => {
  // 0.2.10 の pdfbytes.js が出していた物（関数は空・定数は同じ形）+ URL の規則の 5 つのうち 1 つを欠く物。どれが欠けても定義し直す
  // （typeof の列に 1 つ足し忘れると、その 1 つを欠く古い定義が残る）
  const old = {};
  for (const k of ['metaRefreshUrl', 'metaRefreshUrlInDoc', 'safeDecode', 'cleanDoi', 'jstageArticleUrl', 'proxied', 'deproxifyHost', 'proxyCheck',
    'usableProxy', 'plainUrl', 'isLoginUrl', 'isPdfTab', 'shouldIngestPdf', 'doiUrl', 'withTimeout']) old[k] = () => null;
  old.JOB_KEY = 'openref_job';
  for (const missing of ['urlIds', 'looksLikePdfUrl', 'hasPdfExtension', 'samePage', 'pmcLinkFor']) {
    const stale = Object.assign({}, old);
    for (const k of ['urlIds', 'looksLikePdfUrl', 'hasPdfExtension', 'samePage', 'pmcLinkFor']) if (k !== missing) stale[k] = () => null;
    const ctx = vm.createContext({ URL, OpenRefShared: stale });
    vm.runInContext(code('pdfbytes.js'), ctx);
    for (const k of ['urlIds', 'looksLikePdfUrl', 'hasPdfExtension', 'samePage', 'pmcLinkFor']) assert.equal(typeof ctx.OpenRefShared[k], 'function', missing + ' → ' + k);
    // 新しい版がある所へもう一度注入しても、同じ物のまま（最初の 1 回の定義を使う）
    const first = ctx.OpenRefShared;
    vm.runInContext(code('pdfbytes.js'), ctx);
    assert.equal(ctx.OpenRefShared, first);
  }
});

/* ===== 保存の道（attachPdf = 論文ページ → PMC 本文 → OpenRef 側 → DOI ページ）と、確認の窓のやり直し（retryStageOnce = 止まった段 1 つ）の
   差を固定する。段の中身（取得・PDF タブ・背景タブ・connectd）は差し替え、段の順・入る条件・段の間で渡す値・結果の文・進み具合の文・
   送った要求・分類（failClass・retryStage）・成功の印（ok）を記録して、実行器を 1 本にする前の実装の値と比べる（1 本にしても同じであること）。
   sc = 場面: page（J.page に足す）・allSites・doi（savedDoi）・retry（止まった段）・proxy・fetch（fetchPdfBytes の答え）・nav（PDF タブの答え
   か投げる物）・attach（/capture/attachment の答えか投げる物）・oa（/capture/fetch-oa の答えか投げる物）・via（背景タブの答えか投げる物・
   段ごと = pmc / doi） */
const K16_PDF = 'https://pub.example.org/t/paper.pdf';
const K16_PMC = 'https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/';
const k16Err = (msg, extra = {}) => ({ __err: msg, ...extra });
async function k16Run(mode, sc) {
  const h = worker();
  const log = { prog: [], api: [], fetch: [], nav: [], via: [] };
  h.context.k16 = { sc, log };
  vm.runInContext(`
    (() => {
      const { sc, log } = k16;
      const raise = v => { if (v && v.__err) throw Object.assign(new Error(v.__err), v); return v; };
      Object.assign(J.page, { pdfUrl: null, pdfSource: null, pmcUrl: null, pmid: null, doi: null, url: 'https://pub.example.org/t/article' }, sc.page || {});
      J.allSites = !!sc.allSites;
      J.savedDoi = sc.doi === undefined ? '10.1234/k16' : sc.doi;
      J.proxyTemplate = sc.proxy || null;
      J.itemKey = 'KEY';
      if (sc.retry) J.retry = { stage: sc.retry, cls: 'login', key: 'KEY' };
      progress = (job, p) => { log.prog.push(typeof p === 'string' ? p : JSON.stringify(p)); return Promise.resolve(true); };
      fetchPdfBytes = async (job, url) => { log.fetch.push(url); return raise(sc.fetch) || { bytes: null, error: 'x', html: false, kind: null }; };
      fetchPdfViaNavigation = async (job, url, ctx, inj) => { log.nav.push([url, ctx, inj]); return raise(sc.nav); };
      viaDoiTab = async (job, key, doi, pmc) => {
        const stage = pmc ? 'pmc' : 'doi';
        log.via.push([stage, doi, pmc ? pmc.url : null, pmc ? pmc.pmid : null]);
        const r = raise((sc.via || {})[stage]);
        if (r === undefined) throw new Error(stage + ' の背景タブ: 失敗');
        return r;
      };
      api = async (path, opts) => {
        log.api.push(path);
        const p = path.split('?')[0];
        const v = p === '/capture/attachment' ? sc.attach : p === '/capture/fetch-oa' ? sc.oa : null;
        if (v && v.__null) return null;
        const r = raise(v);
        if (r == null) throw new Error(p + ' の失敗（場面に無い）');
        return r;
      };
    })();
  `, h.context);
  let out;
  try {
    out = { line: await vm.runInContext(mode === 'save' ? "attachPdf(J, 'KEY')" : "retryStageOnce(J, 'KEY')", h.context) };
  } catch (e) {
    out = { threw: String(e.message), stale: !!e.stale, connectdTimeout: !!e.connectdTimeout };
  }
  const j = vm.runInContext('({ failClass: J.failClass, retryStage: J.retryStage, ok: J.ok })', h.context);
  return clone({ ...out, ...j, ...log });
}
const k16Bytes = (n = 2048) => ({ bytes: new Uint8Array(n), how: 'ページ内' });
const K16_OA_NOPDF = 'pmc: PMCID なし / unpaywall: OA 版なし / crossref: link[] に PDF なし。paywall の論文は…';
const K16 = [
  // ---- 保存の道（attachPdf）----
  ['save', 'page: attached (DOI unchecked)', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'attached', bytes: 2048, doi_check: 'unknown' } }],
  ['save', 'page: attached_additional', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'attached_additional', bytes: 2048 } }],
  ['save', 'page: skipped', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'skipped' } }],
  ['save', 'page: unknown attach status goes on to the server', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'weird' }, oa: { source: 'unpaywall' } }],
  ['save', 'page: attach 500 goes on; server fails; no all-sites', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: k16Err('HTTP 500', { status: 500 }), oa: k16Err('oa: 失敗') }],
  ['save', 'page: attach refused (409) stops', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: k16Err('別の論文の PDF です', { status: 409 }), oa: { source: 'x' } }],
  ['save', 'page: attach refused (422 unverified) stops', { page: { pdfUrl: K16_PDF, pdfSource: 'link' }, fetch: k16Bytes(), attach: k16Err('DOI を確認できません', { status: 422, body: { status: 'unverified' } }), oa: { source: 'x' } }],
  ['save', 'page: connectd timeout on attach halts', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: k16Err('OpenRef（connectd）の /capture/attachment が 120 秒以内に応答しません', { connectdTimeout: true, path: '/capture/attachment' }) }],
  ['save', 'page: HTML + all-sites + declared → PDF tab (moved → link)', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'PDF ではない応答', html: true, kind: 'login' }, nav: { b64: btoa('%PDF-k16'), how: 'PDF タブ経由・タブ内', moved: true }, attach: { status: 'attached', bytes: 8 } }],
  ['save', 'page: HTML without all-sites → no PDF tab', { page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'PDF ではない応答', html: true, kind: null }, oa: { source: 'pmc' } }],
  ['save', 'page: HTML + all-sites but a page link → no PDF tab', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'link' }, fetch: { bytes: null, error: 'PDF ではない応答', html: true, kind: null }, oa: { source: 'pmc' } }],
  ['save', 'page: PDF tab fails (login) → server → DOI tab fails', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'PDF ではない応答', html: true, kind: null }, nav: k16Err('PDF タブが PDF へ届きませんでした', { kind: 'login' }), oa: k16Err('oa: 失敗'), via: { doi: k16Err('着地ページに PDF の宣言がありません', { kind: 'no_pdf' }) } }],
  ['save', 'page: PDF tab halts', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'html', html: true }, nav: k16Err('この保存は中断されました（以後は OpenRef へ送りません）', { stale: true }) }],
  ['save', 'no PDF link: PMC succeeds', { allSites: true, page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: 'PMC 本文経由で添付 ✓' } }],
  ['save', 'no PDF link: PMC refused stops (pmc kind only)', { allSites: true, page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: k16Err('別の論文の PDF です', { status: 409 }) }, oa: { source: 'x' } }],
  ['save', 'no PDF link: PMC fails → server ok', { allSites: true, page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: k16Err('PMC 本文の識別子を確認できません', { kind: 'challenge' }) }, oa: { source: 'pmc-aws' } }],
  ['save', 'page link fails (login) then PMC refused (422): the class comes from the PMC stage alone', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'link', pmcUrl: K16_PMC, pmid: '21620466' }, fetch: { bytes: null, error: 'HTTP 401', html: false, kind: 'login' }, via: { pmc: k16Err('DOI を確認できません', { status: 422, body: { status: 'unverified' } }) }, oa: { source: 'x' } }],
  // 段の run が想定外の例外を投げたとき: 前の実装と同じ終わり方
  ['save', 'page: fetch throws an unexpected error → the save ends with that error', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Err('Invalid character'), oa: { source: 'x' } }],
  ['save', 'page: connectd answers null to the attachment → 転送失敗 and on to the server and the DOI page', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { __null: true }, oa: k16Err('oa: 失敗') }],
  ['save', 'page: the PDF tab returns broken base64 → a PDF tab line', { allSites: true, page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'html', html: true, kind: null }, nav: { b64: '%%%', how: 'PDF タブ経由・タブ内', moved: false }, oa: k16Err('oa: 失敗') }],
  ['save', 'server: connectd answers null to fetch-oa → an OpenRef line', { oa: { __null: true } }],
  ['save', 'PMC: an unexpected error is a PMC line', { allSites: true, page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: k16Err('Cannot read properties of undefined') }, oa: { source: 'x' } }],
  ['save', 'no PDF link: PMC skipped without all-sites', { page: { pmcUrl: K16_PMC, pmid: '21620466' }, oa: k16Err('oa: 失敗') }],
  ['save', 'no PDF link: DOI tab refused does not stop (it is the last stage)', { allSites: true, oa: k16Err(K16_OA_NOPDF), via: { doi: k16Err('別の論文の PDF です', { status: 409 }) } }],
  ['save', 'no PDF link: DOI tab succeeds', { allSites: true, oa: k16Err('oa: 失敗'), via: { doi: 'DOI ページ経由で添付 ✓' } }],
  ['save', 'no PDF link: every stage says no PDF → no_pdf', { allSites: true, oa: k16Err(K16_OA_NOPDF), via: { doi: k16Err('着地ページに PDF の宣言がありません', { kind: 'no_pdf' }) } }],
  ['save', 'no PDF link: no DOI → DOI tab not tried', { allSites: true, doi: null, oa: k16Err('oa: HTTP 401 — x.example.org が HTTP 401') }],
  ['save', 'no PDF link: server halts', { oa: k16Err('OpenRef（connectd）の /capture/fetch-oa が 300 秒以内に応答しません', { connectdTimeout: true, path: '/capture/fetch-oa' }) }],
  ['save', 'no PDF link: DOI tab halts', { allSites: true, oa: k16Err('oa: 失敗'), via: { doi: k16Err('この保存は中断されました（以後は OpenRef へ送りません）', { stale: true }) } }],
  // ---- 確認の窓のやり直し（retryStageOnce）----
  ['retry', 'pdf: attached (host in the line)', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'attached', bytes: 2048, doi_check: 'unknown' } }],
  ['retry', 'pdf: attached_additional (how in the line)', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'attached_additional', bytes: 2048 } }],
  ['retry', 'pdf: unknown attach status is the result', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: { status: 'weird' } }],
  ['retry', 'pdf: attach 500 ends the retry', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { ...k16Bytes(), kind: 'login' }, attach: k16Err('HTTP 500', { status: 500 }) }],
  ['retry', 'pdf: attach refused', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: k16Err('別の論文の PDF です', { status: 409 }) }],
  ['retry', 'pdf: HTML → PDF tab without the all-sites / declared gate', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'link' }, fetch: { bytes: null, error: 'html', html: true, kind: 'login' }, nav: { b64: btoa('%PDF-k16'), how: 'PDF タブ経由・拡張', moved: false }, attach: { status: 'attached', bytes: 8 } }],
  ['retry', 'pdf: fetch fails (not HTML)', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'HTTP 403', html: false, kind: null } }],
  ['retry', 'pdf: PDF tab fails', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'html', html: true, kind: null }, nav: k16Err('PDF タブが PDF へ届きませんでした', { kind: 'stuck' }) }],
  ['retry', 'pdf: through the proxy prefix', { retry: 'pdf', proxy: 'https://login.ezproxy.u.ac.jp/login?url=', page: { pdfUrl: 'https://www-sciencedirect-com.ezproxy.u.ac.jp/science/article/pii/S1/pdfft', pdfSource: 'sciencedirect' }, fetch: k16Bytes(), attach: { status: 'attached', bytes: 2048 } }],
  ['retry', 'pdf: connectd timeout halts', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Bytes(), attach: k16Err('OpenRef（connectd）の /capture/attachment が 120 秒以内に応答しません', { connectdTimeout: true, path: '/capture/attachment' }) }],
  ['retry', 'pdf: fetch throws an unexpected error → the retry ends with that error', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: k16Err('Invalid character') }],
  ['retry', 'pdf: connectd answers null to the attachment → the raw error ends the retry', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { ...k16Bytes(), kind: 'login' }, attach: { __null: true } }],
  ['retry', 'pdf: the PDF tab returns broken base64', { retry: 'pdf', page: { pdfUrl: K16_PDF, pdfSource: 'meta' }, fetch: { bytes: null, error: 'html', html: true, kind: null }, nav: { b64: '%%%', how: 'PDF タブ経由・タブ内', moved: false } }],
  ['retry', 'pmc: succeeds', { retry: 'pmc', page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: 'PMC 本文経由で添付 ✓' } }],
  ['retry', 'pmc: fails', { retry: 'pmc', page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: k16Err('PMC 本文の識別子を確認できません', { kind: 'challenge' }) } }],
  ['retry', 'pmc: refused', { retry: 'pmc', page: { pmcUrl: K16_PMC, pmid: '21620466' }, via: { pmc: k16Err('別の論文の PDF です', { status: 409 }) } }],
  ['retry', 'doi: refused stops (unlike the save path)', { retry: 'doi', via: { doi: k16Err('別の論文の PDF です', { status: 409 }) } }],
  ['retry', 'doi: fails', { retry: 'doi', via: { doi: k16Err('着地ページ（x）に content script を注入できません') } }],
  ['retry', 'doi: halts', { retry: 'doi', via: { doi: k16Err('この保存は中断されました（以後は OpenRef へ送りません）', { stale: true }) } }],
];
/* 実行器を 1 本にする前の実装（attachPdf・retryStageOnce・postBytes の 3 つの写し）が出した値。差の要点:
   - 結果の文: 保存の道の論文ページの段は「転送中…（大きさ）」「2 本目…（既存添付と別ファイル・大きさ）」・不明な応答と転送失敗は次の段へ /
     やり直しは「転送中…（経路・ホスト）」「2 本目…（既存添付と別ファイル・経路・ホスト・大きさ）」・不明な応答はそのまま結果・転送失敗で終わる
   - PDF タブで開き直す条件: 保存の道 = HTML・全サイト許可・宣言された PDF / やり直し = HTML だけ
   - 拒否で止まる段: 保存の道 = 論文ページ・PMC 本文（DOI ページは最後の段の失敗の行に入る）/ やり直し = どの段も止まる
   - 失敗の行の区切り: 保存の道 = 論文ページ「 / PMC 本文経由: 」・「\nOpenRef 側: 」「\nDOI ページ経由: 」・最後に「誌面で…」/ やり直し = 区切りなし・最後の行なし */
const K16_GOLD = {
  "save|page: fetch throws an unexpected error → the save ends with that error": {"threw": "Invalid character", "stale": false, "connectdTimeout": false, "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: connectd answers null to the attachment → 転送失敗 and on to the server and the DOI page": {"line": "書誌は保存済み。PDF は取得できませんでした — 転送失敗（Cannot read properties of null (reading 'doi_check')・2 KB）\nOpenRef 側: oa: 失敗\nDOI ページ経由: doi の背景タブ: 失敗\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください", "failClass": null, "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（2 KB）", "転送失敗（Cannot read properties of null (reading 'doi_check')・2 KB） → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page", "/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "save|page: the PDF tab returns broken base64 → a PDF tab line": {"line": "書誌は保存済み。PDF は取得できませんでした — ブラウザ経由で取れず（html / PDF タブ: Invalid character）\nOpenRef 側: oa: 失敗\nDOI ページ経由: doi の背景タブ: 失敗\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください", "failClass": null, "retryStage": null, "ok": false, "prog": ["ブラウザ経由で取れず（html / PDF タブ: Invalid character） → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": [["doi", "10.1234/k16", null, null]]},
  "save|server: connectd answers null to fetch-oa → an OpenRef line": {"line": "書誌は保存済み。PDF は取得できませんでした — ページに PDF リンクなし\nOpenRef 側: 応答が不明（null）\nDOI ページ経由: 全サイトへのアクセス許可が無いため未試行（拡張の設定 = アイコンを右クリック → オプション で許可できます）\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください", "failClass": null, "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": []},
  "save|PMC: an unexpected error is a PMC line": {"line": "PDF 添付 ✓（取得元: x）", "failClass": null, "retryStage": null, "ok": true, "prog": ["ページに PDF リンクなし / PMC 本文経由: Cannot read properties of undefined → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "retry|pdf: fetch throws an unexpected error → the retry ends with that error": {"threw": "Invalid character", "stale": false, "connectdTimeout": false, "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: connectd answers null to the attachment → the raw error ends the retry": {"line": "書誌は保存済み。PDF は取得できませんでした — Cannot read properties of null (reading 'doi_check')\n分類: 購読なし・ログインが要る → ブラウザで PDF を開いて（ログインして）から、その PDF タブで Capture", "failClass": "login", "retryStage": "pdf", "ok": false, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: the PDF tab returns broken base64": {"line": "書誌は保存済み。PDF は取得できませんでした — ブラウザ経由で取れず（html / PDF タブ: Invalid character）", "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": []},
  "save|page link fails (login) then PMC refused (422): the class comes from the PMC stage alone": {"line": "書誌は保存済み。PDF は添付しませんでした — DOI を確認できません", "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "save|page: attached (DOI unchecked)": {"line": "PDF 添付 ✓（ページ内 fetch・2 KB・DOI 未確認）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（2 KB）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: attached_additional": {"line": "PDF を 2 本目として添付 ✓（既存添付と別ファイル・2 KB）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（2 KB）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: skipped": {"line": "PDF は既に添付済み（同一ファイル）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（2 KB）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: unknown attach status goes on to the server": {"line": "PDF 添付 ✓（取得元: unpaywall）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（2 KB）", "転送結果 weird → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page", "/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: attach 500 goes on; server fails; no all-sites": {"line": "書誌は保存済み。PDF は取得できませんでした — 転送失敗（HTTP 500・2 KB）\nOpenRef 側: oa: 失敗\nDOI ページ経由: 全サイトへのアクセス許可が無いため未試行（拡張の設定 = アイコンを右クリック → オプション で許可できます）\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください", "failClass": null, "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（2 KB）", "転送失敗（HTTP 500・2 KB） → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page", "/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: attach refused (409) stops": {"line": "書誌は保存済み。PDF は添付しませんでした — 別の論文の PDF です\n分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ", "failClass": "mismatch", "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（2 KB）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: attach refused (422 unverified) stops": {"line": "書誌は保存済み。PDF は添付しませんでした — DOI を確認できません", "failClass": null, "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（2 KB）"], "api": ["/capture/attachment?key=KEY&source=link&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: connectd timeout on attach halts": {"threw": "OpenRef（connectd）の /capture/attachment が 120 秒以内に応答しません", "stale": false, "connectdTimeout": true, "failClass": null, "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（2 KB）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: HTML + all-sites + declared → PDF tab (moved → link)": {"line": "PDF 添付 ✓（PDF タブ経由・タブ内 fetch・0 KB）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（0 KB）"], "api": ["/capture/attachment?key=KEY&source=link&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": []},
  "save|page: HTML without all-sites → no PDF tab": {"line": "PDF 添付 ✓（取得元: pmc）", "failClass": null, "retryStage": null, "ok": true, "prog": ["ブラウザ経由で取れず（PDF ではない応答） → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: HTML + all-sites but a page link → no PDF tab": {"line": "PDF 添付 ✓（取得元: pmc）", "failClass": null, "retryStage": null, "ok": true, "prog": ["ブラウザ経由で取れず（PDF ではない応答） → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "save|page: PDF tab fails (login) → server → DOI tab fails": {"line": "書誌は保存済み。PDF は取得できませんでした — ブラウザ経由で取れず（PDF ではない応答 / PDF タブ: PDF タブが PDF へ届きませんでした）\nOpenRef 側: oa: 失敗\nDOI ページ経由: 着地ページに PDF の宣言がありません\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください\n分類: 購読なし・ログインが要る → ブラウザで PDF を開いて（ログインして）から、その PDF タブで Capture", "failClass": "login", "retryStage": "pdf", "ok": false, "prog": ["ブラウザ経由で取れず（PDF ではない応答 / PDF タブ: PDF タブが PDF へ届きませんでした） → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": [["doi", "10.1234/k16", null, null]]},
  "save|page: PDF tab halts": {"threw": "この保存は中断されました（以後は OpenRef へ送りません）", "stale": true, "connectdTimeout": false, "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": []},
  "save|no PDF link: PMC succeeds": {"line": "PMC 本文経由で添付 ✓", "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "save|no PDF link: PMC refused stops (pmc kind only)": {"line": "書誌は保存済み。PDF は添付しませんでした — 別の論文の PDF です\n分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ", "failClass": "mismatch", "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "save|no PDF link: PMC fails → server ok": {"line": "PDF 添付 ✓（取得元: pmc-aws）", "failClass": null, "retryStage": null, "ok": true, "prog": ["ページに PDF リンクなし / PMC 本文経由: PMC 本文の識別子を確認できません → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "save|no PDF link: PMC skipped without all-sites": {"line": "書誌は保存済み。PDF は取得できませんでした — ページに PDF リンクなし\nOpenRef 側: oa: 失敗\nDOI ページ経由: 全サイトへのアクセス許可が無いため未試行（拡張の設定 = アイコンを右クリック → オプション で許可できます）\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください", "failClass": null, "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": []},
  "save|no PDF link: DOI tab refused does not stop (it is the last stage)": {"line": "書誌は保存済み。PDF は取得できませんでした — ページに PDF リンクなし\nOpenRef 側: pmc: PMCID なし / unpaywall: OA 版なし / crossref: link[] に PDF なし\nDOI ページ経由: 別の論文の PDF です\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください\n分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ", "failClass": "mismatch", "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "save|no PDF link: DOI tab succeeds": {"line": "DOI ページ経由で添付 ✓", "failClass": null, "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "save|no PDF link: every stage says no PDF → no_pdf": {"line": "書誌は保存済み。PDF は取得できませんでした — ページに PDF リンクなし\nOpenRef 側: pmc: PMCID なし / unpaywall: OA 版なし / crossref: link[] に PDF なし\nDOI ページ経由: 着地ページに PDF の宣言がありません\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください\n分類: ページと DOI ページに PDF の宣言が見つからない → ブラウザで PDF を開ければその PDF タブで Capture、無ければ DL して OpenRef へドロップ", "failClass": "no_pdf", "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "save|no PDF link: no DOI → DOI tab not tried": {"line": "書誌は保存済み。PDF は取得できませんでした — ページに PDF リンクなし\nOpenRef 側: oa: HTTP 401 — x.example.org が HTTP 401\nDOI ページ経由: DOI なしのため未試行\n誌面で PDF を開いた状態でもう一度 Capture するか、DL して OpenRef へドロップしてください\n分類: 購読なし・ログインが要る → ブラウザで PDF を開いて（ログインして）から、その PDF タブで Capture", "failClass": "login", "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": []},
  "save|no PDF link: server halts": {"threw": "OpenRef（connectd）の /capture/fetch-oa が 300 秒以内に応答しません", "stale": false, "connectdTimeout": true, "failClass": null, "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": []},
  "save|no PDF link: DOI tab halts": {"threw": "この保存は中断されました（以後は OpenRef へ送りません）", "stale": true, "connectdTimeout": false, "failClass": null, "retryStage": null, "ok": false, "prog": ["ページに PDF リンクなし → OpenRef 側で取得を試行…（PMC / Unpaywall / Crossref / DOI 着地ページ）"], "api": ["/capture/fetch-oa"], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "retry|pdf: attached (host in the line)": {"line": "PDF 添付 ✓（ページ内 fetch・pub.example.org・2 KB・DOI 未確認）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: attached_additional (how in the line)": {"line": "PDF を 2 本目として添付 ✓（既存添付と別ファイル・ページ内 fetch・pub.example.org・2 KB）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: unknown attach status is the result": {"line": "PDF 転送の応答が不明（{\"status\":\"weird\"}）", "failClass": null, "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: attach 500 ends the retry": {"line": "書誌は保存済み。PDF は取得できませんでした — 転送失敗（HTTP 500・2 KB）\n分類: 購読なし・ログインが要る → ブラウザで PDF を開いて（ログインして）から、その PDF タブで Capture", "failClass": "login", "retryStage": "pdf", "ok": false, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: attach refused": {"line": "書誌は保存済み。PDF は添付しませんでした — 別の論文の PDF です\n分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ", "failClass": "mismatch", "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: HTML → PDF tab without the all-sites / declared gate": {"line": "PDF 添付 ✓（PDF タブ経由・拡張 fetch・pub.example.org・0 KB）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（PDF タブ経由・拡張 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=link&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": []},
  "retry|pdf: fetch fails (not HTML)": {"line": "書誌は保存済み。PDF は取得できませんでした — ブラウザ経由で取れず（HTTP 403）", "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pdf: PDF tab fails": {"line": "書誌は保存済み。PDF は取得できませんでした — ブラウザ経由で取れず（html / PDF タブ: PDF タブが PDF へ届きませんでした）\n分類: PDF の URL がログイン・購入・確認などのページで止まった → ブラウザで PDF を開いてから、その PDF タブで Capture", "failClass": "stuck", "retryStage": "pdf", "ok": false, "prog": [], "api": [], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [["https://pub.example.org/t/paper.pdf", 1, true]], "via": []},
  "retry|pdf: through the proxy prefix": {"line": "PDF 添付 ✓（ページ内 fetch・login.ezproxy.u.ac.jp・2 KB）", "failClass": null, "retryStage": null, "ok": true, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・login.ezproxy.u.ac.jp）"], "api": ["/capture/attachment?key=KEY&source=link&via=page"], "fetch": ["https://login.ezproxy.u.ac.jp/login?url=https://www.sciencedirect.com/science/article/pii/S1/pdfft"], "nav": [], "via": []},
  "retry|pdf: connectd timeout halts": {"threw": "OpenRef（connectd）の /capture/attachment が 120 秒以内に応答しません", "stale": false, "connectdTimeout": true, "failClass": null, "retryStage": null, "ok": false, "prog": ["PDF を OpenRef へ転送中…（ページ内 fetch・pub.example.org）"], "api": ["/capture/attachment?key=KEY&source=meta&via=page"], "fetch": ["https://pub.example.org/t/paper.pdf"], "nav": [], "via": []},
  "retry|pmc: succeeds": {"line": "PMC 本文経由で添付 ✓", "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "retry|pmc: fails": {"line": "書誌は保存済み。PDF は取得できませんでした — PMC 本文経由: PMC 本文の識別子を確認できません\n分類: 確認ページ（bot 対策）のまま → ブラウザで一度 PDF を開いて確認を通してから、その PDF タブで Capture", "failClass": "challenge", "retryStage": "pmc", "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "retry|pmc: refused": {"line": "書誌は保存済み。PDF は添付しませんでした — 別の論文の PDF です\n分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ", "failClass": "mismatch", "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["pmc", "10.1234/k16", "https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/", "21620466"]]},
  "retry|doi: refused stops (unlike the save path)": {"line": "書誌は保存済み。PDF は添付しませんでした — 別の論文の PDF です\n分類: 別の論文の PDF だった → 意図した PDF なら DL して OpenRef へドロップ", "failClass": "mismatch", "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "retry|doi: fails": {"line": "書誌は保存済み。PDF は取得できませんでした — DOI ページ経由: 着地ページ（x）に content script を注入できません", "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
  "retry|doi: halts": {"threw": "この保存は中断されました（以後は OpenRef へ送りません）", "stale": true, "connectdTimeout": false, "failClass": null, "retryStage": null, "ok": false, "prog": [], "api": [], "fetch": [], "nav": [], "via": [["doi", "10.1234/k16", null, null]]},
};
for (const [mode, name, sc] of K16) test(`${mode === 'save' ? 'save path' : 'window retry'}: ${name}`, async () => {
  assert.deepEqual(await k16Run(mode, sc), K16_GOLD[mode + '|' + name]);
});
test('every scenario has a recorded value', () => {
  assert.deepEqual(K16.map(([m, n]) => m + '|' + n).sort(), Object.keys(K16_GOLD).sort());
});

test('declaredPdf: a declared source needs the PDF URL itself', () => {
  const s = shared();
  for (const [page, want] of [
    [{ pdfUrl: K16_PDF, pdfSource: 'meta' }, true], [{ pdfUrl: K16_PDF, pdfSource: 'self' }, true], [{ pdfUrl: K16_PDF, pdfSource: 'atypon' }, true],
    [{ pdfUrl: K16_PDF, pdfSource: 'sciencedirect' }, true], [{ pdfUrl: K16_PDF, pdfSource: 'link' }, false], [{ pdfUrl: null, pdfSource: 'meta' }, false],
    [{ pdfUrl: '', pdfSource: 'meta' }, false], [{ pdfUrl: K16_PDF, pdfSource: null }, false],
  ]) assert.equal(s.declaredPdf(page), want, JSON.stringify(page));
});

test('bytesToB64 joins every 32 KB chunk (the page fetch and the extension fetch share it)', () => {
  const ctx = vm.createContext({ URL, btoa }); // service worker と content script には btoa がある
  vm.runInContext(code('pdfbytes.js'), ctx);
  const s = ctx.OpenRefShared;
  for (const n of [0, 1, 2, 3, 0x7fff, 0x8000, 0x8001, 0x10000 + 7, 3 * 0x8000 + 5]) {
    const b = new Uint8Array(n);
    for (let i = 0; i < n; i++) b[i] = (i * 131 + 7) & 0xff;
    assert.equal(s.bytesToB64(b), Buffer.from(b).toString('base64'), 'n=' + n);
  }
});
test('isPdfBytes: %PDF and at least one more byte', () => {
  const s = shared();
  const u = t => new Uint8Array([...t].map(c => c.charCodeAt(0)));
  for (const [t, want] of [['%PDF-1.7', true], ['%PDF-', true], ['%PDF', false], ['%PD', false], ['', false], ['<html>', false], ['%PDX-', false], ['%pdf-', false], [' %PDF-', false]]) {
    assert.equal(s.isPdfBytes(u(t)), want, JSON.stringify(t));
  }
});

/* ===== 結果の 1 行目（書誌 ✓ ・ PDF ✓ / ✗（理由））・色・段ごとの理由を畳む・設定のページの全サイト許可のスイッチ ===== */
/* 結果の欄を組む 2 つ（doneResult・errorResult）を、工程の外から直接呼ぶ。job = 分類と成功の印・itemKey だけを持つ工程 */
function r12(fn, args, jobPatch = {}) {
  const h = worker();
  h.context.r12 = { args, jobPatch };
  return clone(vm.runInContext(`(() => { const j = Object.assign(makeJob({ tabId: 1, page: {} }, false), r12.jobPatch); return ${fn}(j, ...r12.args); })()`, h.context));
}
const PF = '書誌は保存済み。PDF は取得できませんでした — ';
const PR = '書誌は保存済み。PDF は添付しませんでした — ';
for (const c of Object.keys(NEXT)) test(`PDF ✗ with the class ${c}: the reason in line 1, the next step shown, the stage reasons folded (nothing lost)`, () => {
  const pdfMsg = PF + 'ページ内 fetch: x\nOpenRef 側: y\n誌面で…\n' + NEXT[c];
  const r = r12('doneResult', [{ bib: '✓', key: '登録 K1', pdfOk: false, pdfMsg }], { failClass: c });
  same([r.state, r.cls], ['done', 'warn']);
  same(r.head.split('\n'), ['書誌 ✓ ・ PDF ✗（' + WHY[c] + '）', '登録 K1', NEXT[c]]);
  assert.ok(r.line.startsWith(r.head + '\n'));
  const fold = r.line.slice(r.head.length + 1);
  same(fold, PF + 'ページ内 fetch: x\nOpenRef 側: y\n誌面で…');
  same(fold + '\n' + NEXT[c], pdfMsg); // 前の PDF の文が全部ある（分類の行は畳まずに見せる側）
});
test('PDF ✗ without a class: 取得できず / a refusal: 論文を確認できず / an unknown attach answer: PDF ？ / why first', () => {
  const none = r12('doneResult', [{ bib: '✓', key: '登録 K1', pdfOk: false, pdfMsg: PF + 'HTTP 403\n誌面で…' }]);
  same(none.head.split('\n'), ['書誌 ✓ ・ PDF ✗（取得できず）', '登録 K1']);
  same(none.line.slice(none.head.length + 1), PF + 'HTTP 403\n誌面で…');
  const refused = r12('doneResult', [{ bib: '✓', key: '既存 K1', pdfOk: false, pdfMsg: PR + 'DOI を確認できません' }]);
  same([refused.head, refused.line], ['書誌 ✓ ・ PDF ✗（論文を確認できず）\n既存 K1', '書誌 ✓ ・ PDF ✗（論文を確認できず）\n既存 K1\n' + PR + 'DOI を確認できません']);
  const unknown = r12('doneResult', [{ bib: '✓', key: '既存 K1', pdfOk: false, pdfMsg: 'PDF 転送の応答が不明（{"status":"weird"}）' }]);
  same(unknown.head, '書誌 ✓ ・ PDF ？（応答が不明）\n既存 K1');
  // 識別子なし: 理由は分類より先（分類があっても）・書誌は要確認・注記は key の次
  const noId = r12('doneResult', [{ bib: '？（要確認）', key: '登録 K2', pdfOk: false, pdfMsg: 'PDF リンクなし（識別子なしのため OA 経路は対象外）', why: '識別子なし', notes: ['要確認ラベルを付けました: R', '⚠ 撤回論文の可能性: W'] }], { failClass: 'login' });
  same(noId.line.split('\n'), ['書誌 ？（要確認） ・ PDF ✗（識別子なし）', '登録 K2', '要確認ラベルを付けました: R', '⚠ 撤回論文の可能性: W', 'PDF リンクなし（識別子なしのため OA 経路は対象外）']);
  same(noId.head.split('\n').length, 4);
});
test('PDF ✓: nothing folded; the attach line stays on line 2; green only when not under review', () => {
  const ok = r12('doneResult', [{ bib: '✓', key: '登録 K1', pdfOk: true, pdfMsg: 'PDF 添付 ✓（ページ内 fetch・2 KB）', notes: ['⚠ 撤回論文の可能性: W'] }], { ok: true });
  same([ok.line, ok.head, ok.cls], ['書誌 ✓ ・ PDF ✓\n登録 K1 — PDF 添付 ✓（ページ内 fetch・2 KB）\n⚠ 撤回論文の可能性: W', ok.line, 'done']);
  const review = r12('doneResult', [{ bib: '？（要確認）', key: '登録 K2', pdfOk: true, pdfMsg: 'PDF は既に添付済み' }], { ok: false });
  same([review.line, review.cls], ['書誌 ？（要確認） ・ PDF ✓\n登録 K2 — PDF は既に添付済み', 'warn']);
});
test('a failed job: line 1 says how far it got; the old line follows unfolded; red', () => {
  const e = (msg, extra = {}) => Object.assign(new Error(msg), extra);
  const cases = [
    [{}, e('connectd が落ちた'), '✗ connectd が落ちた', '書誌 ✗ ・ PDF —'],
    [{}, e('t', { connectdTimeout: true, path: '/capture/save' }), '✗ t', '書誌 ？（時間切れ） ・ PDF —'],
    [{ pdfPosted: true }, e('t', { connectdTimeout: true, path: '/capture/pdf' }), '✗ t', '書誌 ？（時間切れ） ・ PDF ？'],
    // PDF タブの道で PDF を送った後の失敗は「—」（扱っていない）ではない
    [{ pdfPosted: true }, e('PDF を読めません', { status: 422 }), '✗ PDF を読めません', '書誌 ✗ ・ PDF ✗'],
    [{ pdfPosted: true }, e('Failed to fetch'), '✗ Failed to fetch', '書誌 ？ ・ PDF ？（届いたか不明）'],
    [{ itemKey: 'K1' }, e('Invalid character'), '✗ Invalid character', '書誌 ✓ ・ PDF ✗'],
    [{ itemKey: 'K1' }, e('t', { connectdTimeout: true, path: '/capture/attachment' }), '✗ t', '書誌 ✓ ・ PDF ？（時間切れ）'],
  ];
  for (const [patch, err, text, first] of cases) {
    const h = worker();
    h.context.r12 = { patch, err, text };
    const r = clone(vm.runInContext(`(() => { const j = Object.assign(makeJob({ tabId: 1, page: {} }, false), r12.patch); return errorResult(j, r12.err, r12.text); })()`, h.context));
    same([r.state, r.cls, r.line, r.head], ['error', 'err', first + '\n' + text, first + '\n' + text]);
  }
});
/* PDF タブ（識別子なし）を /capture/pdf へ: 応答の status ごとの結果 */
for (const [status, extra, want, cls, ok] of [
  ['added', {}, ['書誌 ✓ ・ PDF ✓', '登録 K2 — PDF 本文の識別子で書誌を確定して添付（拡張 fetch）'], 'done', true],
  ['attached', {}, ['書誌 ✓ ・ PDF ✓', '既存 K2 に PDF を添付（本文の識別子が既存 item と一致）'], 'done', true],
  ['attached_additional', { reason: 'R' }, ['書誌 ✓ ・ PDF ✓', '既存 K2 に 2 本目として添付（既存添付と別ファイル）'], 'done', true],
  // 同一 PDF の相手で 1 行目を変える（確かな item = ✓・ゴミ箱・要確認 stub・形が分からない = 言い切らない）
  ['duplicate', { has_pdf: true }, ['書誌 ✓ ・ PDF ✓', '同一 PDF が既に添付済み（K2）'], 'done', true],
  ['duplicate', { reason: 'R', trashed: true }, ['書誌 ？（ゴミ箱の中） ・ PDF ✓', '同一 PDF が既に添付済み（K2・ゴミ箱内。完全削除すれば取り込み直せます）', 'R'], 'warn', false],
  ['duplicate', { has_pdf: true, reason: '同一ファイルが要確認 stub に添付済み。その item の詳細 → 編集から DOI/PMID を引き直せます' },
    ['書誌 ？（要確認） ・ PDF ✓', '同一 PDF が既に添付済み（K2）', '同一ファイルが要確認 stub に添付済み。その item の詳細 → 編集から DOI/PMID を引き直せます'], 'warn', false],
  ['duplicate', {}, ['書誌 ？ ・ PDF ✓', '同一 PDF が既に添付済み（K2）'], 'warn', false],
  ['needs_review', { reason: 'R' }, ['書誌 ？（要確認） ・ PDF ✓', '要確認として登録 K2', 'R'], 'warn', false],
  ['stub', { reason: 'R', retraction: 'W' }, ['書誌 ？（要確認） ・ PDF ✓', '要確認として登録 K2（PDF は添付済み。詳細ペインの編集で DOI/PMID を入力すると書誌が入ります）', 'R', '⚠ 撤回論文の可能性: W'], 'warn', false],
  ['weird', {}, ['書誌 ？ ・ PDF ？（応答が不明）', 'weird K2'], 'warn', false],
]) test(`the PDF-tab route (/capture/pdf) answering ${status}${Object.keys(extra).length ? ' ' + Object.keys(extra).join('+') : ''}`, async () => {
  const h = worker();
  h.context.answer = Object.assign({ status, key: 'K2', title: 'T' }, extra);
  vm.runInContext(`J.page = { pdfUrl: 'https://x.example.org/a.pdf', url: 'https://x.example.org/a.pdf', title: 'T' }; J.contentScriptOk = false;
    bgFetchPdf = async () => ({ ok: true, b64: btoa('%PDF-test') }); api = async () => answer;`, h.context);
  const r = clone(await vm.runInContext('ingestPdfFlow(J)', h.context));
  same([r.done, r.line.split('\n'), r.head === r.line, r.cls, vm.runInContext('J.ok', h.context)], [true, want, true, cls, ok]);
});

/* popup: 結果の 1 行目と注記と次の一手は状態行・段ごとの理由は畳んだ詳細（既定は閉じる）・前の版が残した記録はそのまま */
test('popup: a PDF ✗ result shows head in the status line and the stage reasons in a closed detail', async () => {
  const [opts, landing] = C4_FLOWS.login;
  const { app } = await saveAndRead(opts, landing);
  const rec = app.record();
  same([app.els.status.textContent, app.els.status.className], [rec.head, 'warn']);
  same([app.els.detail.style.display, app.els.detail.open, app.els.detailtext.textContent], ['block', false, rec.line.slice(rec.head.length + 1)]);
  assert.match(app.els.detailtext.textContent, /^書誌は保存済み。PDF は取得できませんでした — /);
  assert.match(app.els.detailtext.textContent, /\nDOI ページ経由: タブ内: Error: HTTP 401/);
  same(app.els.status.textContent + '\n' + app.els.detailtext.textContent, rec.line); // 開けば全部
  // 利用者が開いた詳細は、同じ結果を描き直しても閉じない（seen の書き込み・記録の変化）
  app.els.detail.open = true;
  vm.runInContext('showRecord(latest)', app.context);
  same(app.els.detail.open, true);
  // 次の結果（別の保存）は閉じて出す
  await vm.runInContext('init()', app.context);
  await app.els.save.handlers.click();
  same([app.els.detail.style.display, app.els.detail.open], ['block', false]);
});
test('popup: a PDF ✓ result has no detail; a failed job has no detail; a progress line hides an earlier detail', async () => {
  const ok = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', ok.context);
  await ok.els.save.handlers.click();
  same([ok.els.status.textContent, ok.els.status.className, ok.els.detail.style.display], [ok.record().line, 'done', 'none']);
  assert.match(ok.els.status.textContent, /^書誌 ✓ ・ PDF ✓\n登録 K1 — PDF 添付 ✓（/);
  const [opts, landing] = C4_FLOWS.login;
  const { app } = await saveAndRead(opts, landing);
  same(app.els.detail.style.display, 'block');
  vm.runInContext("setStatus('PDF を取得中…', '')", app.context);
  same(app.els.detail.style.display, 'none');
});
for (const [name, rec, shown] of [
  ['an older version\'s result (no head)', { line: '✓ 登録 K1 — 書誌は保存済み。PDF は取得できませんでした — ページに PDF リンクなし\n分類: x', cls: 'done' }, 'line'],
  ['a head that is not the start of the line', { line: '書誌 ✓ ・ PDF ✗\nA\nB', head: '書誌 ✓ ・ PDF ✓', cls: 'warn' }, 'line'],
  ['a head that is not a string', { line: 'L1\nL2', head: 3, cls: 'warn' }, 'line'],
  ['a line that is not a string', { line: null, head: 'H', cls: 'warn' }, ''],
]) test(`popup after an update: ${name} is shown whole, without a detail, and nothing throws`, async () => {
  const env = swEnv({ allSites: false });
  await env.session.area.set({ openref_job: Object.assign({ id: 'old', tabId: 1, tabUrl: art, startedAt: 0, state: 'done', endedAt: 1, seen: false, failClass: null, retry: null }, rec) });
  const app = popupApp(art, {}, { env, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', app.context);
  same([app.els.status.textContent, app.els.status.className, app.els.detail.style.display || 'none'], [shown === 'line' ? rec.line : '', rec.cls, 'none']);
});
test('popup after an update: another tab\'s older result keeps its first line in the one-line note', async () => {
  const env = swEnv({ allSites: false });
  await env.session.area.set({ openref_job: { id: 'old', tabId: 1, tabUrl: art, startedAt: 0, state: 'done', endedAt: 1, seen: false, title: 'T', line: '✓ 登録 K1 — PDF 添付 ✓（x）\n分類: y', cls: 'done' } });
  const other = popupApp('https://www.example-journal.org/article/456', {}, { env, tabId: 2, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', other.context);
  same(other.els.note.textContent, '前の保存（T）: ✓ 登録 K1 — PDF 添付 ✓（x）');
});

/* 全サイト許可を外しても、走っている保存は始めたときの段の並びのまま（開始のときに確かめた許可で段を外す）。次の保存から外れる */
test('removing the all-sites permission during a save: the running save keeps its stages; the next save skips the DOI page', async () => {
  const h = c6Worker();
  let allowed = true, asked = 0;
  h.context.chrome.permissions.contains = async () => { asked++; return allowed; };
  h.context.viaCalls = [];
  vm.runInContext(`
    const bg0 = bgFetchPdf;
    bgFetchPdf = async (url) => { await bg0(url); return { ok: false, error: 'HTTP 403', httpStatus: 403 }; };
    const api0 = api;
    api = async (url, opts) => { if (url === '/capture/fetch-oa') { fakeApi(url); throw Object.assign(new Error('PDF を自動取得できませんでした'), { status: 422 }); } return api0(url, opts); };
    viaDoiTab = async (job, key, doi) => { viaCalls.push(doi); throw new Error('doi の背景タブ: 失敗'); };`, h.context);
  const open = h.gate('a');
  const a = await h.start(c6Input('a'));
  await tick();
  allowed = false; // 設定のページで外した（走っている保存の途中）
  open();
  await a.done;
  same(h.context.viaCalls, [c6Doi('a')]); // 始めたときの許可のまま DOI ページの段を試した
  assert.match(h.record().line, /DOI ページ経由: doi の背景タブ: 失敗/);
  const b = await h.start(c6Input('b'));
  await b.done;
  same([h.context.viaCalls.length, asked], [1, 2]); // 次の保存は開始のときに確かめ直し、DOI ページを開かない
  assert.match(h.record().line, /DOI ページ経由: 全サイトへのアクセス許可が無いため未試行（拡張の設定 = アイコンを右クリック → オプション で許可できます）/);
  same(h.record().head.split('\n')[0], '書誌 ✓ ・ PDF ✗（取得できず）');
});

/* 設定のページの全サイト許可のスイッチ。状態は実際の権限から読む（保存しない）・オン = request・オフ = remove */
function optionsPerms({ granted = false, request = true, remove = true, stored = {} } = {}) {
  const calls = [], added = [], removedL = [];
  const perm = { granted };
  const permissions = {
    contains: async p => { calls.push(['contains', p.origins.join(' ')]); if (perm.containsThrows) throw new Error(perm.containsThrows); return perm.granted; },
    request: async p => { calls.push(['request', p.origins.join(' ')]); if (request instanceof Error) throw request; if (request) perm.granted = true; return !!request; },
    remove: async p => { calls.push(['remove', p.origins.join(' ')]); if (remove instanceof Error) throw remove; if (remove) perm.granted = false; return !!remove; },
    onAdded: { addListener: f => added.push(f) }, onRemoved: { addListener: f => removedL.push(f) },
  };
  const o = optionsPage(stored, permissions);
  Object.assign(o, { perm, calls, added, removedL });
  o.toggle = async on => { o.els.allsites.checked = on; await o.els.allsites.handlers.change(); };
  return o;
}
const ALL = 'https://*/* http://*/*';
for (const granted of [true, false]) test(`options: the switch reads the real permission (${granted ? 'granted' : 'not granted'}) and stores nothing`, async () => {
  const o = optionsPerms({ granted });
  await tick();
  same([o.els.allsites.checked, o.els.allsites.disabled], [granted, false]);
  same(o.calls, [['contains', ALL]]);
  same(o.writes, []);
  same(vm.runInContext('ALL_SITES', o.ctx), inPopup('ALL_SITES')); // popup と同じ範囲（manifest の optional_host_permissions）
  same(vm.runInContext('DECLINED_KEY', o.ctx), inPopup('DECLINED_KEY'));
});
test('options: turning it on requests the permission first (no await before it) and clears the declined record', async () => {
  const o = optionsPerms({ granted: false, stored: { openref_allsites_declined: true } });
  await tick();
  o.calls.length = 0;
  o.els.allsites.checked = true;
  const p = o.els.allsites.handlers.change();
  same(o.calls, [['request', ALL]]); // 同期の区間で request（ユーザー操作の直後）
  await p;
  same([o.els.allsites.checked, o.els.allsites.disabled, o.els['allsites-status'].className], [true, false, 'ok']);
  same(o.writes, [['remove', 'openref_allsites_declined']]);
  same(o.calls.at(-1), ['contains', ALL]); // 終わったら実際の権限から読み直す
  assert.match(o.els['allsites-status'].textContent, /許可しました。次の保存から/);
});
test('options: a refused request leaves the switch off and writes nothing', async () => {
  const o = optionsPerms({ granted: false, request: false });
  await tick();
  await o.toggle(true);
  same([o.els.allsites.checked, o.els['allsites-status'].className, o.writes], [false, 'err', []]);
  assert.match(o.els['allsites-status'].textContent, /許可されませんでした/);
});
test('options: turning it off removes the permission and records the decline (the popup will not ask again)', async () => {
  const o = optionsPerms({ granted: true });
  await tick();
  await o.toggle(false);
  same(o.calls.filter(c => c[0] !== 'contains'), [['remove', ALL]]);
  same([o.els.allsites.checked, o.els['allsites-status'].className], [false, 'ok']);
  same(o.writes, [['set', { openref_allsites_declined: true }]]);
  assert.match(o.els['allsites-status'].textContent, /許可を外しました。次の保存から/);
});
test('options: a remove that fails or throws leaves the switch on the real permission', async () => {
  for (const remove of [false, new Error('Permission is required')]) {
    const o = optionsPerms({ granted: true, remove });
    await tick();
    await o.toggle(false);
    same([o.els.allsites.checked, o.els.allsites.disabled, o.els['allsites-status'].className, o.writes], [true, false, 'err', []]);
  }
});
test('options: a change made elsewhere (chrome://extensions) moves the switch; an unreadable permission disables it', async () => {
  const o = optionsPerms({ granted: false });
  await tick();
  same([o.added.length, o.removedL.length], [1, 1]);
  o.perm.granted = true;
  await o.added[0]();
  same(o.els.allsites.checked, true);
  o.perm.granted = false;
  await o.removedL[0]();
  same(o.els.allsites.checked, false);
  o.perm.containsThrows = 'no';
  await o.removedL[0]();
  same([o.els.allsites.disabled, o.els['allsites-status'].className], [true, 'err']);
});
test('a save without identifiers whose item already has the PDF: 書誌 ？（要確認） ・ PDF ✓, orange, badge !', async () => {
  const data = analyzeFixture(abstractPage);
  const { app, lines, headLines } = await saveAndRead({ setup: articleTab(data, { ok: false, error: 'x' }),
    routes: { '/capture/save': { key: 'K4', status: 'existing', doi: null, has_pdf: true, title: 'T', reason: 'R' } } });
  same(lines, ['書誌 ？（要確認） ・ PDF ✓', '既存 K4 — PDF は既に添付済み', '要確認ラベルを付けました: R']);
  same([headLines.length, app.record().cls, app.env.badges, app.els.detail.style.display], [3, 'warn', ['…', '!', '', ''], 'none']); // バッジは ! の後に popup が消す（描画と seen の 2 回）
});

/* ===== 結果の行の境界: 同一 PDF の相手・応答の形・別のタブの 1 行 ===== */
test('fetch-oa answering null (or without a source) is not a PDF ✓: the save goes on, orange, badge !', async () => {
  for (const answer of ['null', '{ status: "fetched" }', '"ok"']) {
    const h = c6Worker();
    h.context.chrome.permissions.contains = async () => false;
    vm.runInContext(`bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });
      const api0 = api; api = async (url, opts) => { if (url === '/capture/fetch-oa') { fakeApi(url); return ${answer}; } return api0(url, opts); };`, h.context);
    await (await h.start(c6Input('a'))).done;
    const rec = h.record();
    same([rec.head.split('\n')[0], rec.cls, h.badges], ['書誌 ✓ ・ PDF ✗（取得できず）', 'warn', ['…', '!']]);
    assert.match(rec.line, /\nOpenRef 側: 応答が不明（/);
    same(h.posts.map(p => p.split('?')[0]), ['/capture/save', '/capture/fetch-oa']); // 要求の列は前と同じ
  }
});
test('the PDF-tab route posts the PDF, then /capture/pdf fails: PDF ✗ when connectd answered, PDF ？ when it may not have arrived', async () => {
  for (const [err, first] of [[apiError(422, 'PDF を読めません'), '書誌 ✗ ・ PDF ✗'], [new TypeError('Failed to fetch'), '書誌 ？ ・ PDF ？（届いたか不明）']]) {
    const app = await openAndSave(jsPdf + '/-char/ja', { [jsArticle + '/-char/ja']: swap('citation_pdf_url', jsPdf.replace('42_5', '42_6')) }, { routes: { '/capture/pdf': err } });
    await app.els.save.handlers.click();
    const rec = app.record();
    same([rec.state, rec.cls, rec.line], ['error', 'err', first + '\n✗ ' + err.message]);
  }
  // PDF を送る前（/capture/save の道）の失敗は今まで通り「—」= 扱っていない
  const app = popupApp(art, {}, { allSites: false, declined: true, setup: c6Setup, routes: { '/capture/save': new Error('connectd が落ちた') } });
  await vm.runInContext('init()', app.context);
  await app.els.save.handlers.click();
  same(app.record().head.split('\n')[0], '書誌 ✗ ・ PDF —');
});
test('another tab\'s PDF ✗ result: line 1 (with its reason) and line 2 in the note; only line 1 has the colour class', async () => {
  const [opts, landing] = C4_FLOWS.login;
  const { app: first } = await saveAndRead(opts, landing);
  vm.runInContext('showRecord = () => {};', first.context);
  const env = first.env;
  await env.session.area.set({ openref_job: Object.assign(first.record(), { seen: false }) });
  const other = popupApp('https://www.example-journal.org/article/456', {}, { env, tabId: 2, allSites: false, declined: true, setup: c6Setup });
  await vm.runInContext('init()', other.context);
  await tick();
  // 理由は 1 行目の括弧にある = 畳んだ詳細の最初の行は繰り返さない
  same(other.els.note.textContent, '前の保存（T）: 書誌 ✓ ・ PDF ✗（ログインが要る） / 登録 K1');
  const mark = other.doc.made.at(-1);
  same([mark.tag, mark.className, mark.textContent, other.els.note.style.display], ['span', 'mark warn', '書誌 ✓ ・ PDF ✗（ログインが要る）', 'block']);
});
test('a reason is readable in every one-line note; a repeated reason is dropped; a long rest is cut', () => {
  // 1 行目に括弧の理由が無い終わり（PDF ✗ だけ・PDF —）は、2 行目（失敗の文）か畳んだ詳細の最初の行が残りに続く
  const h = popup();
  const long = '✗ ' + 'あ'.repeat(80);
  h.context.cases = [
    { head: '書誌 ✓ ・ PDF ✗\n✗ Invalid character', line: '書誌 ✓ ・ PDF ✗\n✗ Invalid character' },
    { head: '書誌 ✗ ・ PDF —\n✗ connectd が落ちた', line: '書誌 ✗ ・ PDF —\n✗ connectd が落ちた' },
    { head: '書誌 ✓ ・ PDF ✗（確認ページのまま）\n既存 K\n分類: x', line: '書誌 ✓ ・ PDF ✗（確認ページのまま）\n既存 K\n分類: x\n書誌は保存済み。窓で 5 分待ちましたが PDF は届きませんでした（窓を閉じました）' },
    { head: '書誌 ✓ ・ PDF ✗\n既存 K', line: '書誌 ✓ ・ PDF ✗\n既存 K\n書誌は保存済み。PDF は取得できませんでした — x' },
    { head: '書誌 ✗ ・ PDF —\n' + long, line: '書誌 ✗ ・ PDF —\n' + long },
    { line: '✓ 登録 K1 — PDF 添付 ✓（x）\n分類: y' },
  ];
  same(clone(vm.runInContext('cases.map(noteParts)', h.context)), [
    ['書誌 ✓ ・ PDF ✗', '✗ Invalid character'],
    ['書誌 ✗ ・ PDF —', '✗ connectd が落ちた'],
    ['書誌 ✓ ・ PDF ✗（確認ページのまま）', '既存 K'],
    ['書誌 ✓ ・ PDF ✗', '既存 K / PDF は取得できませんでした — x'],
    ['書誌 ✗ ・ PDF —', long.slice(0, 50) + '…'],
    ['✓ 登録 K1 — PDF 添付 ✓（x）', ''],
  ]);
});
test('another tab\'s unseen result arriving while this tab shows its own result does not hide the own detail; the note comes after it', async () => {
  const [opts, landing] = C4_FLOWS.login;
  const { app } = await saveAndRead(opts, landing);
  const own = app.record();
  same(app.els.detail.style.display, 'block');
  app.els.detail.open = true;
  // 別のタブの保存が終わり、まだ誰も見ていない記録が届く
  await app.env.session.area.set({ openref_job: { id: 'other', tabId: 9, tabUrl: 'https://pub.example.org/z', startedAt: 0, state: 'done', endedAt: 5, seen: false,
    title: 'Z', line: '書誌 ✓ ・ PDF ✓\n登録 KZ — PDF 添付 ✓（x）', head: '書誌 ✓ ・ PDF ✓\n登録 KZ — PDF 添付 ✓（x）', cls: 'done' } });
  same([app.els.detail.style.display, app.els.detail.open, app.els.detailtext.textContent], ['block', true, own.line.slice(own.head.length + 1)]);
  same([app.els.status.textContent, app.els.status.className], [own.head, 'warn']); // 自分の結果はそのまま
  same([app.els.note.textContent, app.els.note.style.display], ['前の保存（Z）: 書誌 ✓ ・ PDF ✓ / 登録 KZ — PDF 添付 ✓（x）', 'block']);
  // 自分の次の進み具合・結果が来たら、別のタブの 1 行は消える
  vm.runInContext("setStatus('PDF を取得中…', '')", app.context);
  same([app.els.note.textContent, app.els.note.style.display], ['', 'none']);
});


/* ===== 結果の行の境界（続き）===== */
test('with the all-sites permission, a malformed fetch-oa answer goes on to the DOI page (no PDF ✓ from the answer itself)', async () => {
  for (const answer of ['null', '{}', '{ status: "fetched" }', '[]', '"ok"', '{ source: "" }']) {
    for (const viaOk of [false, true]) {
      const h = c6Worker();
      h.context.viaCalls = [];
      h.context.viaOk = viaOk;
      vm.runInContext(`bgFetchPdf = async () => ({ ok: false, error: 'HTTP 403', httpStatus: 403 });
        const api0 = api; api = async (url, opts) => { if (url === '/capture/fetch-oa') { fakeApi(url); return ${answer}; } return api0(url, opts); };
        viaDoiTab = async (job, key, doi) => { viaCalls.push(doi); if (!viaOk) throw new Error('doi の背景タブ: 失敗'); job.ok = true; return 'PDF 添付 ✓（DOI ページ経由・x）'; };`, h.context);
      await (await h.start(c6Input('a'))).done; // c6Worker の権限は全サイト許可あり
      const rec = h.record();
      same(h.context.viaCalls, [c6Doi('a')], answer); // 次の段（DOI ページ）を 1 回だけ試す
      same([rec.head.split('\n')[0], rec.cls, h.badges.at(-1)], viaOk ? ['書誌 ✓ ・ PDF ✓', 'done', '✓'] : ['書誌 ✓ ・ PDF ✗（取得できず）', 'warn', '!'], answer);
      if (viaOk) assert.match(rec.line, /\n登録 K-a — PDF 添付 ✓（DOI ページ経由・x）$|^書誌 ✓ ・ PDF ✓\n登録 K-a — PDF 添付 ✓（DOI ページ経由・x）$/);
      else assert.match(rec.line, /\nOpenRef 側: 応答が不明（/);
    }
  }
});
test('the stub reason that connectd sends (main.rs) starts with the prefix the extension looks for', async () => {
  const rs = fs.readFileSync(path.join(__dirname, '..', '..', 'crates', 'openref-connectd', 'src', 'main.rs'), 'utf8');
  const found = [...rs.matchAll(/reason = Some\("([^"]*要確認 stub[^"]*)"\.into\(\)\)/g)].map(m => m[1]);
  same(found.length, 1); // duplicate の相手が要確認 stub という案内は 1 つ
  const prefix = inJob('DUP_ON_STUB');
  assert.ok(prefix.length >= 10 && found[0].startsWith(prefix), JSON.stringify([prefix, found[0]]));
  // その文をそのまま返されたとき、1 行目が「書誌 ？（要確認）」になる
  const h = worker();
  h.context.answer = { status: 'duplicate', key: 'K2', title: 'T', has_pdf: true, reason: found[0] };
  vm.runInContext(`J.page = { pdfUrl: 'https://x.example.org/a.pdf', url: 'https://x.example.org/a.pdf', title: 'T' }; J.contentScriptOk = false;
    bgFetchPdf = async () => ({ ok: true, b64: btoa('%PDF-test') }); api = async () => answer;`, h.context);
  const r = clone(await vm.runInContext('ingestPdfFlow(J)', h.context));
  same([r.line.split('\n')[0], r.cls], ['書誌 ？（要確認） ・ PDF ✓', 'warn']);
});
/* PDF タブ（識別子なし）の道の工程を通す: /capture/pdf の答え（値か投げる物）と /capture/save の答え */
async function pdfTabSave({ pdfAnswer, bgOk = true, saveThrows = null }) {
  const h = c6Worker();
  h.context.pdfAnswer = pdfAnswer;
  h.context.bgOk = bgOk;
  h.context.saveThrows = saveThrows;
  vm.runInContext(`bgFetchPdf = async () => (bgOk ? { ok: true, b64: btoa('%PDF-test') } : { ok: false, error: 'HTTP 403', httpStatus: 403 });
    const api0 = api; api = async (url, opts) => {
      if (url.startsWith('/capture/pdf?')) { fakeApi(url); if (pdfAnswer && pdfAnswer.__throw) throw Object.assign(new Error(pdfAnswer.message), pdfAnswer.extra || {}); return pdfAnswer; }
      if (url === '/capture/save' && saveThrows) { fakeApi(url); throw Object.assign(new Error(saveThrows.message), saveThrows.extra || {}); }
      return api0(url, opts); };`, h.context);
  await (await h.start(c6Input('x', { page: { doi: null, pmid: null, pdfUrl: 'https://pub.example.org/x.pdf', pdfSource: 'self', isPdf: true, title: 'X', url: 'https://pub.example.org/x.pdf' } }))).done;
  return h;
}
test('/capture/pdf answering JSON null arrived: 書誌 ？ ・ PDF ？（応答が不明）, not 届いたか不明', async () => {
  const h = await pdfTabSave({ pdfAnswer: null });
  same([h.record().state, h.record().line, h.badges.at(-1)], ['error', '書誌 ？ ・ PDF ？（応答が不明）\n✗ OpenRef の応答が不明（null）', '!']);
  same(h.posts.map(p => p.split('?')[0]), ['/capture/pdf']);
});
test('the PDF counts as posted only from the /capture/pdf request (a PDF-tab fetch that fails, then a failed /capture/save, is PDF —)', async () => {
  const h = await pdfTabSave({ bgOk: false, saveThrows: { message: 'HTTP 500', extra: { status: 500 } } });
  same(h.posts, ['/capture/save']); // PDF は取れず送っていない（題名のみで保存へ）
  same([h.record().state, h.record().line], ['error', '書誌 ✗ ・ PDF —\n✗ HTTP 500']);
  // 送った後の失敗は ✗ / ？（対照）
  const sent = await pdfTabSave({ pdfAnswer: { __throw: true, message: 'PDF を読めません', extra: { status: 422 } } });
  same(sent.record().line, '書誌 ✗ ・ PDF ✗\n✗ PDF を読めません');
});
test('/capture/pdf answering {} shows the answer itself on line 2 (not "undefined undefined")', async () => {
  const h = await pdfTabSave({ pdfAnswer: {} });
  same([h.record().state, h.record().line, h.record().cls], ['done', '書誌 ？ ・ PDF ？（応答が不明）\n応答: {}', 'warn']);
});
