# Local protocol fixture. It does not measure real model quality.
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json, re, sys, time
from pathlib import Path
log = Path(sys.argv[1])
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def send(self, value):
        raw=json.dumps(value,ensure_ascii=False).encode()
        self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
    def do_GET(self):
        if len(sys.argv) > 2: time.sleep(float(sys.argv[2]))
        self.send({'models':[{'name':'QA-ollama','model':'QA-ollama','modified_at':'2026-09-12T00:00:00Z','size':1}], 'data':[{'id':'QA-api'}]})
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        prompt=body['messages'][-1]['content']; retry='上一轮' in prompt
        rows=re.findall(r'^\[(\d+)\] \([^)]*\) (.*)$',prompt,re.M)
        out=[]
        for index,text in rows:
            index=int(index)
            if index==7 and not retry: out.append({'segment':index});continue
            edits=[{'original':'报复','suggested':'暴富','reason':'homophone','confidence':'high'}] if '报复' in text else []
            out.append({'segment':index,'verdict':'suspect' if edits else 'ok','edits':edits})
        with log.open('a',encoding='utf-8') as f:f.write(json.dumps({'path':self.path,'model':body['model'],'authorization_present':bool(self.headers.get('Authorization')),'indices':[int(i) for i,_ in rows],'retry':retry})+'\n')
        self.send({'choices':[{'message':{'role':'assistant','content':json.dumps({'segments':out},ensure_ascii=False)}}]})
ThreadingHTTPServer(('127.0.0.1',3130),Handler).serve_forever()
