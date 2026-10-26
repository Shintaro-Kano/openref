"""connectd /word/v2/* の模擬（行形式。VBA の文書操作の E2E 用 = 本物の整形はしない）。
  python3 word/dotm/e2e/mock_v2.py 23199 &   →   word/dotm/e2e/run.sh http://127.0.0.1:23199
番号 = 初出順の [n,m]（先頭の数字に上付きの run）・文献リスト = "n. Title of KEY. J Test あ. 2020."（誌名に斜体の run）。"""
import json, re, sys, html
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def esc(s):
    out = []
    for ch in s:
        o = ord(ch)
        if ch == '%': out.append('%%')
        elif ch == '\t': out.append('%t')
        elif ch == '\n': out.append('%n')
        elif ch == '\r': out.append('%r')
        elif 32 <= o < 127 and ch not in '\\~': out.append(ch)
        else:
            b = ch.encode('utf-16-le')
            for k in range(0, len(b), 2):
                out.append('%%u%04X' % int.from_bytes(b[k:k+2], 'little'))
    return ''.join(out)

LOG = []
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, body, ctype='text/plain'):
        b = body.encode('ascii')
        self.send_response(200); self.send_header('content-type', ctype); self.send_header('content-length', str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        if self.path.startswith('/api/status'):
            return self.send(json.dumps({"ok": True, "version": "mock", "items": 3, "db_version": 9}), 'application/json')
        self.send('ERR\tunknown ' + esc(self.path))
    def do_POST(self):
        n = int(self.headers.get('content-length') or 0)
        raw = self.rfile.read(n)
        if self.headers.get('X-OpenRef') != 'word':
            self.send_response(403); self.end_headers(); return
        try:
            req = json.loads(raw.decode('ascii'))
        except Exception as e:
            return self.send('ERR\t' + esc('bad json: %s %r' % (e, raw[:200])))
        LOG.append((self.path, req))
        if self.path == '/word/v2/render':
            order, lines = [], []
            for p in req['cites']:
                for it in p['items']:
                    if it['key'] not in order: order.append(it['key'])
            for i, p in enumerate(req['cites']):
                nums = sorted(order.index(it['key']) + 1 for it in p['items'])
                t = '[' + ','.join(map(str, nums)) + ']'
                lines.append('C\t%s\t%s' % (esc(t), '1:%d:s' % len(str(nums[0]))))
            entries, runs, pos = [], [], 0
            for i, k in enumerate(order):
                e = '%d. Title of %s. J Test あ. 2020.' % (i + 1, k)
                j = e.index('J Test')
                runs.append('%d:%d:i' % (pos + j, len('J Test あ')))
                entries.append(e); pos += len(e) + 1
            lines.append('B\t%s\t%s' % (esc('\n'.join(entries)), ','.join(runs)))
            lines.append('S\t%s\t%d' % (esc(req.get('style', '')), len(order)))
            return self.send('OK\n' + '\n'.join(lines) + '\n')
        if self.path == '/word/v2/prefs':
            return self.send('OK\nP\tvancouver\t0\n')
        if self.path == '/word/v2/legacy':
            out = []
            for name, val in re.findall(r'<we:property name="([^"]+)" value="([^"]*)"/>', req['xml']):
                if not name.startswith('refract-') or name == 'refract-index': continue
                v = json.loads(html.unescape(val))
                if isinstance(v, bool): s = 'true' if v else 'false'
                elif isinstance(v, (dict, list)): s = json.dumps(v, ensure_ascii=True, separators=(',', ':'))
                else: s = str(v)
                out.append('V\t%s\t%s' % (esc(name), esc(s)))
            return self.send('OK\n' + '\n'.join(out) + '\n')
        if self.path == '/word/v2/log':
            print('LOG', req, flush=True); return self.send('OK\n')
        self.send('ERR\t' + esc('unknown ' + self.path))

if __name__ == '__main__':
    ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
