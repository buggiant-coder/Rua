# Protocol fixture only; never shipped with Rua.
import json, sys, os, time
from pathlib import Path
root = Path(__file__).parent
text_buffer = ""
session_state = root/"ended-sessions.json"

def tool(name, props, required=[]):
    return {"name":name, "description":name, "inputSchema":{"type":"object", "properties":props,"required":required,"additionalProperties":False}}
number={"type":"integer"}
window={"pid":number,"window_id":number,"include_screenshot":{"type":"boolean"},"include_accessibility_tree":{"type":"boolean"},"session":{"type":"string"},"tree_format":{"type":"string"},"screenshot_out_file":{"type":"string"}}
tools=[tool("list_apps",{}), tool("list_windows",{"pid":number}),tool("get_window_state",window,["pid","window_id"]), tool("get_desktop_state",{"session":{"type":"string"},"screenshot_out_file":{"type":"string"}}),tool("click",{"pid":number,"window_id":number,"x":number,"y":number}),tool("check_permissions",{}),tool("end_session",{"session":{"type":"string"}})]
tools.append(tool("start_session",{"session":{"type":"string"}}))
for name, fields in [("type_text",{"text":{"type":"string"}}),("press_key",{"key":{"type":"string"},"modifiers":{"type":"array"}}),("scroll",{"direction":{"type":"string"},"amount":number})]:
    tools.append(tool(name,{"pid":number,"window_id":number,"session":{"type":"string"},"element_token":{"type":"string"},**fields}))
for line in sys.stdin:
    msg=json.loads(line)
    if "id" not in msg: continue
    method=msg["method"]
    if method=="initialize": result={"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"cua-driver","version":"0.38.0"}}
    elif method=="tools/list": result={"tools":tools}
    elif method=="tools/call":
        args=msg["params"]["arguments"];name=msg["params"]["name"]
        with (root/"calls.jsonl").open("a") as f: f.write(json.dumps({"name":name,"args":args,"proxy":os.getpid()})+"\n")
        ended = set(json.loads(session_state.read_text())) if session_state.exists() else set()
        if name in ["start_session", "end_session"]:
            if name == "start_session": ended.discard(args["session"])
            else: ended.add(args["session"])
            temporary = root/"ended-sessions.tmp"
            temporary.write_text(json.dumps(sorted(ended)))
            temporary.replace(session_state)
            result={"content":[{"type":"text","text":name}]}
        elif args.get("session") in ended:
            result={"isError":True,"content":[{"type":"text","text":"session has ended; call start_session"}]}
        elif name=="check_permissions":
            result={"content":[{"type":"text","text":"Human-readable permission summary"}],"structuredContent":json.loads((root/"permissions.json").read_text())}
        elif name=="list_windows":
            windows=[] if (root/"window-gone").exists() else [{"pid":100,"window_id":200,"app_name":"fixture","title":"target","bounds":{"x":0,"y":0,"width":800,"height":600}}]
            result={"content":[{"type":"text","text":"windows"}],"structuredContent":{"windows":windows}}
        else:
            if name in ["click","type_text","press_key","scroll"]:
                if (root/"pause-input").exists():
                    (root/"input-entered").touch()
                    deadline=time.monotonic()+5
                    while not (root/"resume-input").exists() and time.monotonic()<deadline: time.sleep(0.01)
                if name == "type_text": text_buffer += args["text"]
                if name == "press_key" and args["key"] == "return": text_buffer += "\n"
                if (root/"drop-after-input").exists(): sys.exit(0)
            content=[{"type":"text","text":"controlled fixture"}]
            if name in ["get_window_state","get_desktop_state"] and args.get("include_screenshot",True) and not (root/"omit-image").exists():
                content.append({"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZXsAAAAASUVORK5CYII="})
            result={"content":content,"structuredContent":{"pid":100,"window_id":200,"snapshot_id":"snapshot-731","text":text_buffer,"elements":[],"screenshot_width":800,"screenshot_height":600}}
            if (root/"markdown-elements").exists() and name == "get_window_state":
                result["structuredContent"]["snapshot_id"] = "s00000001"
                result["structuredContent"]["tree_markdown"] = '- [7] AXButton "Test" [actions=[press]]'
                if args.get("tree_format") == "both":
                    result["structuredContent"]["elements"] = [{"element_token":"s00000001:7","element_index":7}]
    else: result={}
    print(json.dumps({"jsonrpc":"2.0","id":msg["id"],"result":result}),flush=True)
