const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const code = name => fs.readFileSync(path.join(__dirname, '..', name), 'utf8');
const article = 'https://www.sciencedirect.com/science/article/pii/S1930043326008514';
const pdf = article + '/pdfft?pid=1-s2.0-S1930043326008514-main.pdf';
const landing = 'https://pdf.sciencedirectassets.com/313380/issue/1-s2.0-S1930043326008514/main.pdf?signature=test';
const pmcPage = 'https://pmc.ncbi.nlm.nih.gov/articles/PMC3062508/';
const pmcPdf = pmcPage + 'pdf/nihms277358.pdf';
const pmcLink = 'https://pmc.ncbi.nlm.nih.gov/articles/pmid/21620466/';
const pmcRoute = `{url:'${pmcLink}',pmid:'21620466'}`;
const htmlError = msg => Object.assign(new Error(msg || 'PDF ではない応答'), { html: true });
const same = (a, b) => assert.equal(JSON.stringify(a), JSON.stringify(b)); // vm の別 realm の配列を比べる
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
  const result = analyze(article, [article.replace('8514', '9999') + '/pdfft?pid=other.pdf', pdf]);
  assert.equal(result.pdfUrl, pdf);
  assert.equal(result.pdfSource, 'sciencedirect');
});
for (const [name, url, links] of [
  ['search page', 'https://www.sciencedirect.com/search', [pdf]],
  ['different PII', article, [article.replace('8514', '9999') + '/pdfft']],
  ['lookalike host', article.replace('.com', '.com.example.org'), [pdf]],
  ['off-site PDF', article, ['https://example.org/science/article/pii/S1930043326008514/pdfft']],
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
const nav = (h, url = pdf) => vm.runInContext(`fetchPdfViaNavigation(${JSON.stringify(url)}, 1, true)`, h.context);

test('HTML relay: the PDF tab moves on, the landing is fetched from the article tab, temporary tab closed', async () => {
  const h = popup();
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
  const h = popup();
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
  const h = popup();
  let calls = 0;
  h.fetcher(async () => { calls++; throw htmlError(); });
  await assert.rejects(nav(h), /中継ページのまま/);
  assert.equal(calls, 1);
  assert.ok(h.now() <= 12000, 'quiet give-up at ' + h.now() + ' ms');
  same(h.removed, [7]);
});
test('a PDF tab that keeps reloading HTML stops at the overall deadline', async () => {
  const h = popup();
  const t = [];
  for (let ms = 0; ms < 40000; ms += 1000) t.push([ms, pdf, 'loading'], [ms + 500, pdf, 'complete']);
  h.timeline(t);
  h.fetcher(async () => { throw htmlError(); });
  await assert.rejects(nav(h), /PDF へ届きませんでした/);
  assert.ok(h.now() <= 21000, 'deadline at ' + h.now() + ' ms');
  same(h.removed, [7]);
});
test('non-HTML failure (HTTP 403) is not retried and the tab is closed', async () => {
  const h = popup();
  let calls = 0;
  h.fetcher(async () => { calls++; throw new Error('HTTP 403'); });
  await assert.rejects(nav(h), /^Error: HTTP 403$/);
  assert.equal(calls, 1);
  assert.ok(h.now() < 2000, 'gave up at ' + h.now() + ' ms');
  same(h.removed, [7]);
});
test('a non-web page in the PDF tab (error page / about:blank) is not fetched', async () => {
  const h = popup();
  h.timeline([[0, 'chrome-error://chromewebdata/', 'complete'], [1500, landing, 'loading'], [2000, landing, 'complete']]);
  const fetched = [];
  h.fetcher(async (id, url) => { fetched.push(url); return { b64: btoa('%PDF-test'), how: 'タブ内' }; });
  await nav(h);
  same(fetched, [landing]);
});
test('the PDF tab is fetched only after it finishes loading', async () => {
  const h = popup();
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
    const h = popup();
    h.context.answers = { tab: { ok: false, error: 'x', html: tabHtml }, bg: { ok: false, error: 'y', html: bgHtml } };
    vm.runInContext("sendToTab = async () => answers.tab; sendToBg = async () => answers.bg;", h.context);
    const err = await vm.runInContext(`fetchPdfViaTabThenBg(1, ${JSON.stringify(pdf)}, true).then(() => null, e => e)`, h.context);
    assert.equal(!!err.html, want);
  }
});
test('fetchPdfBytes (real) reports html when either attempt saw HTML', async () => {
  for (const [tabHtml, bgHtml, want] of [[true, false, true], [false, true, true], [false, false, false]]) {
    const h = popup();
    h.context.answers = { tab: { ok: false, error: 'x', html: tabHtml }, bg: { ok: false, error: 'y', html: bgHtml } };
    vm.runInContext("sendToTab = async () => answers.tab; sendToBg = async () => answers.bg;", h.context);
    const got = await vm.runInContext(`fetchPdfBytes(${JSON.stringify(pdf)})`, h.context);
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
  const res = await new Promise(resolve => listener({ type: 'openref-bg-fetch-pdf', url: pdf }, {}, resolve));
  assert.equal(res.ok, false);
  assert.equal(res.html, true);
});

/* 論文ページでの保存（attachPdf） */
function articleSave(h, pdfSource, { html = true, sites = true, moved = false } = {}) {
  h.context.navCalls = 0;
  vm.runInContext(`
    page = { pdfUrl: ${JSON.stringify(pdf)}, pdfSource: ${JSON.stringify(pdfSource)} };
    allSites = ${sites};
    fetchPdfBytes = async () => ({ bytes: null, error: 'HTML response', html: ${html} });
    fetchPdfViaNavigation = async () => { navCalls++; return { b64: btoa('%PDF-test'), how: 'PDF タブ経由・タブ内', moved: ${moved} }; };
    api = async (url) => {
      fakeApi(url);
      if (url === '/capture/fetch-oa') return { source: 'pmc-aws' };
      if (!url.startsWith('/capture/attachment?')) throw new Error('unexpected route ' + url);
      return { status: 'attached', bytes: 9, doi_check: 'match' };
    };
  `, h.context);
  return vm.runInContext("attachPdf('KEY')", h.context);
}
for (const [source, posted] of [['sciencedirect', 'link'], ['meta', 'meta']]) test(`article page (${source}): HTML falls back to the PDF tab and attaches without an OA round trip`, async () => {
  const h = popup();
  const result = await articleSave(h, source);
  assert.match(result, /PDF 添付 ✓/);
  assert.match(result, /PDF タブ経由/);
  assert.equal(h.context.navCalls, 1);
  same(h.posts.map(u => new URLSearchParams(u.split('?')[1]).get('source')), [posted]);
});
test('article page: a PDF that ended up on another origin is posted for strict DOI matching', async () => {
  const h = popup();
  await articleSave(h, 'meta', { moved: true });
  assert.match(h.posts[0], /source=link/);
});
for (const [name, opts, source] of [
  ['a non-HTML failure', { html: false }, 'meta'],
  ['no all-sites permission', { sites: false }, 'meta'],
  ['a PDF link picked from the page (not declared)', {}, 'link'],
]) test(`article page: ${name} goes to the OA chain without opening a PDF tab`, async () => {
  const h = popup();
  const result = await articleSave(h, source, opts);
  assert.match(result, /取得元: pmc-aws/);
  assert.equal(h.context.navCalls, 0);
  same(h.posts, ['/capture/fetch-oa']);
});
test('DOI mismatch is surfaced and does not fall through to OA', async () => {
  const h = popup();
  vm.runInContext(`
    page = { pdfUrl: ${JSON.stringify(pdf)}, pdfSource: 'sciencedirect' };
    allSites = true;
    fetchPdfBytes = async () => ({ bytes: null, error: 'HTML response', html: true });
    fetchPdfViaNavigation = async () => ({ b64: btoa('%PDF-test'), how: 'PDF タブ経由', moved: true });
    api = async (url) => {
      if (!url.startsWith('/capture/attachment?')) throw new Error('unexpected OA route');
      const error = new Error('別論文'); error.status = 409; throw error;
    };
  `, h.context);
  const result = await vm.runInContext("attachPdf('KEY')", h.context);
  assert.match(result, /PDF は添付しませんでした.*別論文/);
});
test('ScienceDirect attachment keeps strict link DOI verification', async () => {
  const h = popup();
  await vm.runInContext(`postAttachment('KEY', new Uint8Array(), 'sciencedirect', 'page')`, h.context);
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
  const url = 'https://webview.isho.jp/journal/detail/pdf/10.18885/CI.0000000612';
  const result = vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(url)})`, h.context);
  assert.equal(result.doi, '10.18885/CI.0000000612');
  assert.equal(result.pdfUrl, url);
  assert.equal(result.pdfSource, 'self');
  assert.equal(result.isPdf, true);
  assert.equal(analyze(url, []).doi, result.doi);
  const fake = url.replace('isho.jp', 'isho.jp.example.org');
  assert.equal(vm.runInContext(`urlOnlyAnalyze(${JSON.stringify(fake)})`, h.context).doi, null);
});
test('isho abstract identifies the article without pretending to be PDF', () => {
  const h = popup();
  const result = vm.runInContext("urlOnlyAnalyze('https://webview.isho.jp/journal/detail/abs/10.18885/CI.0000000612?from=test')", h.context);
  assert.equal(result.doi, '10.18885/CI.0000000612');
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
  const h = popup();
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
    postPdf = async (key, b64, how, source) => how + ' [' + source + ']';
  `, h.context);
  return h;
}
const page = (data) => ({ ok: true, data: Object.assign({ pmid: null, doi: null, pdfUrl: null, pdfSource: null }, data) });
const pmcReal = page({ pmid: '21620466', doi: '10.1016/S0140-6736(10)62307-0', pdfUrl: pmcPdf, pdfSource: 'meta' });
const blank = page({});
const via = (h, doi = "'10.1016/s0140-6736(10)62307-0'", route = pmcRoute) => vm.runInContext(`viaDoiTab('KEY', ${doi}, ${route})`, h.context);

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
    fetchPdfViaNavigation = async (url, ctx, injected) => { navigated.push([url, ctx, injected]); return { b64: btoa('%PDF-test'), how: 'PDF タブ経由・タブ内', moved: ${moved} }; };
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
  const h = popup();
  vm.runInContext(`
    page = {pmid:'21620466',pmcUrl:'${pmcLink}'};
    allSites = true;
    viaDoiTab = async (key, doi, pmc) => { if (pmc.pmid !== '21620466') throw new Error('wrong PMID'); return 'PMC success'; };
    api = async () => { throw new Error('unexpected OA request'); };
  `, h.context);
  assert.equal(await vm.runInContext("attachPdf('KEY')", h.context), 'PMC success');
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
  return new Promise(resolve => listener({ type: 'openref-bg-fetch-pdf', url, follow: true }, {}, resolve)).then(result => ({ result, urls }));
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
  // 正しいエスケープは従来どおり decode する。末尾の ")" を落とすのは popup の現行の後処理（content の cleanDoi と同じ。
  // URL 規則を共有するときに見直す）をここで固定している
  ['valid escapes still decoded', 'https://example.org/doi/abs/10.1000/a%28b%29', '10.1000/a(b', false],
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
      querySelector: sel => (sel === 'a[href*="doi.org/10."]' && doiLink ? { href: doiLink } : null),
      querySelectorAll: () => [],
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
