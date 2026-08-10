#!/usr/bin/env python3
# Load verify.html in headless Chrome, read window.__RT2_RESULT__. No node/puppeteer:
# a static http server + Chrome's --dump-dom won't run JS, so use the DevTools
# remote-debugging endpoint to evaluate the result object after load.
import http.server, socketserver, threading, subprocess, time, json, urllib.request, os, sys, functools

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # crate root (serves pkg/ + e2e/)
PORT = 0

class Q(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
    def end_headers(self):
        # wasm needs the right mime; SimpleHTTPRequestHandler already maps .wasm on 3.9+? force it.
        if self.path.endswith(".wasm"):
            self.send_header("Content-Type", "application/wasm")
        super().end_headers()

handler = functools.partial(Q, directory=ROOT)
httpd = socketserver.TCPServer(("127.0.0.1", 0), handler)
PORT = httpd.server_address[1]
threading.Thread(target=httpd.serve_forever, daemon=True).start()
url = f"http://127.0.0.1:{PORT}/e2e/verify.html"

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
prof = "/tmp/rt2-chrome-profile"; os.system(f"rm -rf {prof}")
DBG = 9333
proc = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={DBG}",
    "--no-first-run","--no-default-browser-check",f"--user-data-dir={prof}", url],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    # find the page target
    result = None
    for _ in range(60):
        time.sleep(0.5)
        try:
            tabs = json.load(urllib.request.urlopen(f"http://127.0.0.1:{DBG}/json"))
        except Exception:
            continue
        page = next((t for t in tabs if t.get("type")=="page" and "verify.html" in t.get("url","")), None)
        if not page: continue
        # Use the JSON HTTP eval is not available; open ws. Minimal ws client:
        import base64, hashlib, socket, struct
        ws = page["webSocketDebuggerUrl"]
        host, port = ws.split("/devtools/")[0].replace("ws://","").split(":")
        path = "/devtools/"+ws.split("/devtools/")[1]
        s = socket.create_connection((host, int(port)))
        k = base64.b64encode(os.urandom(16)).decode()
        s.send(f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {k}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode())
        s.recv(4096)
        def send(obj):
            data = json.dumps(obj).encode(); hdr = bytearray([0x81]); n=len(data)
            m = os.urandom(4)
            if n<126: hdr.append(0x80|n)
            else: hdr += bytes([0x80|126]) + struct.pack(">H", n)
            hdr += m; s.send(bytes(hdr)+bytes(b^m[i%4] for i,b in enumerate(data)))
        def recv():
            b=s.recv(2); ln=b[1]&0x7f
            if ln==126: ln=struct.unpack(">H",s.recv(2))[0]
            data=b""; 
            while len(data)<ln: data+=s.recv(ln-len(data))
            return json.loads(data)
        send({"id":0,"method":"Runtime.enable"}); recv()
        for _ in range(30):
            r=recv()
            if r.get("method")=="Runtime.consoleAPICalled":
                args=r["params"]["args"]; print("CONSOLE:", " ".join(str(a.get("value","")) for a in args)[:200])
            if r.get("method")=="Runtime.exceptionThrown":
                print("EXCEPTION:", str(r["params"]["exceptionDetails"].get("text",""))[:200])
        send({"id":1,"method":"Runtime.evaluate","params":{"expression":"JSON.stringify(window.__RT2_RESULT__||null)","returnByValue":True}})
        for _ in range(20):
            r = recv()
            if r.get("id")==1:
                val = r["result"]["result"]["value"]
                result = json.loads(val) if val else None
                break
        if result is not None: break
    print(json.dumps(result, indent=2) if result else "NO_RESULT")
    sys.exit(0 if result and result.get("ok") else 1)
finally:
    proc.terminate(); httpd.shutdown()
