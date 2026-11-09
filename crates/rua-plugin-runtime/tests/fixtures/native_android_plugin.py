import json
import os
from pathlib import Path
import platform
import subprocess
import sys

assert sys.platform == "linux" and platform.machine() == "aarch64"
assert os.environ.get("PLUGIN_SECRET") == "fixture-secret"
assert "fixture-secret" not in " ".join(sys.argv)
for name in ["PLUGIN_REMOVE", "RUA_PHONE_TOKEN", "RUA_PHONE_SOCKET", "RUA_PHONE_HOST_PID", "RUA_ANDROID_NATIVE_DIR", "LD_LIBRARY_PATH"]:
    assert name not in os.environ, name
background = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(120)"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
Path("plugin-server-pids").write_text(f"{os.getpid()} {background.pid}")
calls = {}

def send(request_id, result):
    print(json.dumps({"jsonrpc":"2.0", "id":request_id, "result":result}), flush=True)

for line in sys.stdin:
    request = json.loads(line)
    method, rid = request.get("method"), request.get("id")
    if method == "notifications/cancelled":
        child = calls.pop(request["params"]["requestId"], None)
        if child:
            child.terminate()
            child.wait(timeout=2)
        continue
    if rid is None:
        continue
    if method == "initialize":
        send(rid, {"protocolVersion":"2024-11-05", "serverInfo":{"name":"native-plugin","version":"1"}, "capabilities":{"tools":{}}})
    elif method == "tools/list":
        send(rid, {"tools":[{"name":name,"description":"Native plugin acceptance", "inputSchema":{"type":"object"}} for name in ["probe","wait"]]})
    elif method == "tools/call" and request["params"]["name"] == "wait":
        code = 'import os,signal,subprocess,sys,time\nfrom pathlib import Path\nchild=subprocess.Popen([sys.executable,"-c","import time;time.sleep(120)"])\ndef stop(*_):\n    child.kill();child.wait();sys.exit(0)\nsignal.signal(signal.SIGTERM,stop)\nPath("plugin-call-pids").write_text(f"{os.getpid()} {child.pid}")\ntime.sleep(120)\n'
        calls[rid] = subprocess.Popen([sys.executable,"-c",code], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    elif method == "tools/call":
        nested = subprocess.check_output([sys.executable,"-c","import platform;print(platform.system())"], text=True).strip()
        output = {"marker":"NATIVE_PLUGIN_OK","cwd":os.getcwd(),"python":sys.executable,"nested":nested}
        Path("plugin-output.json").write_text(json.dumps(output))
        send(rid, {"content":[{"type":"text","text":json.dumps(output)}]})
    elif method == "plugin/invoke_cli":
        send(rid, {"output":"NATIVE_PLUGIN_CLI_OK"})
    else:
        send(rid,{})
