#!/usr/bin/env python3
import json
import os
import subprocess
import sys
import time


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def success(request_id, result):
    send({"jsonrpc": "2.0", "id": request_id, "result": result})


def prompt_text(params):
    blocks = params.get("prompt", [])
    return "".join(
        block.get("text", "")
        for block in blocks
        if isinstance(block, dict) and block.get("type") == "text"
    )


def emit_agent_text(session_id, text):
    chunks = [text]
    if os.environ.get("ACP_MOCK_SPLIT_REPLY") == "1" and len(text) > 1:
        midpoint = len(text) // 2
        chunks = [text[:midpoint], text[midpoint:]]
    for chunk in chunks:
        send(
            {
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {
                            "type": "text",
                            "text": chunk,
                        },
                    },
                },
            }
        )


def history_update(session_id, history):
    update_kind = os.environ.get("ACP_MOCK_LOAD_HISTORY_KIND", "agent_message_chunk")
    return {
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": update_kind,
                "content": {
                    "type": "text",
                    "text": history,
                },
            },
        },
    }


def emit_history(session_id, history):
    repeat = int(os.environ.get("ACP_MOCK_LOAD_HISTORY_REPEAT", "1"))
    indexed = os.environ.get("ACP_MOCK_LOAD_HISTORY_INDEXED") == "1"
    for index in range(repeat):
        text = f"{index:06}:{history}" if indexed else history
        send(history_update(session_id, text))


def emit_available_commands(session_id):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": [],
                },
            },
        }
    )


def tool_update(session_id, update):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": update,
            },
        }
    )


def emit_tool_scenario(session_id, scenario):
    if scenario != "interleaved":
        return

    tool_update(
        session_id,
        {
            "sessionUpdate": "tool_call",
            "toolCallId": "call-a",
            "title": "bash",
            "kind": "execute",
            "status": "in_progress",
            "rawInput": {
                "command": "printf first",
                "cwd": "/workspace",
            },
        },
    )
    tool_update(
        session_id,
        {
            "sessionUpdate": "tool_call",
            "toolCallId": "call-b",
            "title": "read",
            "kind": "read",
            "status": "in_progress",
            "rawInput": {"path": "README.md"},
        },
    )
    tool_update(
        session_id,
        {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-a",
            "rawInput": {
                "command": "printf final",
                "cwd": "/workspace",
            },
        },
    )
    tool_update(
        session_id,
        {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-b",
            "status": "completed",
            "rawOutput": {"content": "README"},
        },
    )
    tool_update(
        session_id,
        {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-a",
            "status": "completed",
            "rawOutput": {
                "formatted_output": "final",
                "exit_code": 0,
            },
        },
    )


def request_permission(session_id, scenario):
    if scenario != "codex":
        return None

    tool_update(
        session_id,
        {
            "sessionUpdate": "tool_call",
            "toolCallId": "permission-tool",
            "title": "mcp.knowledge.search",
            "kind": "execute",
            "status": "pending",
            "rawInput": {
                "server": "knowledge",
                "tool": "search",
                "arguments": {"query": "ACP"},
            },
        },
    )
    permission_request_id = "permission-request-1"
    send(
        {
            "jsonrpc": "2.0",
            "id": permission_request_id,
            "method": "session/request_permission",
            "params": {
                "sessionId": session_id,
                "toolCall": {"toolCallId": "permission-tool"},
                "options": [
                    {
                        "optionId": "allow_once",
                        "name": "Allow once",
                        "kind": "allow_once",
                    },
                    {
                        "optionId": "allow_session",
                        "name": "Allow for session",
                        "kind": "allow_always",
                    },
                    {
                        "optionId": "allow_always",
                        "name": "Always allow",
                        "kind": "allow_always",
                    },
                    {
                        "optionId": "decline",
                        "name": "Decline",
                        "kind": "reject_once",
                    },
                ],
            },
        }
    )

    while True:
        response_line = sys.stdin.readline()
        if not response_line:
            return "cancelled"
        response = json.loads(response_line)
        if response.get("method") == "session/cancel":
            return "cancelled"
        if response.get("id") != permission_request_id:
            continue
        outcome = response.get("result", {}).get("outcome", {})
        if outcome.get("outcome") != "selected":
            return outcome.get("outcome", "cancelled")
        return outcome.get("optionId", "missing-option")

prompt_count = 0

for line in sys.stdin:
    if not line.strip():
        continue

    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")

    if method == "initialize":
        child_marker = os.environ.get("ACP_MOCK_CHILD_MARKER")
        if child_marker:
            subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "import time; time.sleep(60)",
                    child_marker,
                ]
            )
        success(
            request_id,
            {
                "protocolVersion": 1,
                "agentCapabilities": {},
                "authMethods": [],
                "agentInfo": {
                    "name": "mock-acp-agent",
                    "title": "Mock ACP Agent",
                    "version": "0.1.0",
                },
            },
        )
    elif method == "session/list":
        success(request_id, {"sessions": [{
            "sessionId": "fixture-listed-session",
            "cwd": request.get("params", {}).get("cwd", os.getcwd()),
            "title": "Fixture session",
        }]})
    elif method == "session/new":
        startup_marker = os.environ.get("ACP_MOCK_STARTUP_MARKER")
        if startup_marker:
            with open(startup_marker, "w", encoding="utf-8") as marker:
                marker.write("session/new")
            time.sleep(30)
        if os.environ.get("ACP_MOCK_FAIL_ON_TURN") == "1":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32000,
                        "message": "session/new should not be called",
                    },
                }
            )
            sys.exit(2)
        success(request_id, {"sessionId": "mock-session"})
    elif method == "session/load":


        params = request.get("params", {})
        incoming_sid = params.get("sessionId", "mock-session")
        history = os.environ.get("ACP_MOCK_LOAD_HISTORY")
        if os.environ.get("ACP_MOCK_LOAD_AVAILABLE_COMMANDS_BEFORE_HISTORY") == "1":
            emit_available_commands(incoming_sid)
        history_before_response = os.environ.get("ACP_MOCK_LOAD_HISTORY_BEFORE_RESPONSE") == "1"
        if history and history_before_response:
            emit_history(incoming_sid, history)
        success(request_id, {"sessionId": incoming_sid})
        if history and not history_before_response:
            delay_ms = int(os.environ.get("ACP_MOCK_LOAD_HISTORY_DELAY_MS", "0"))
            if delay_ms > 0:
                time.sleep(delay_ms / 1000)
            emit_history(incoming_sid, history)
        emit_tool_scenario(
            incoming_sid,
            os.environ.get("ACP_MOCK_LOAD_TOOL_SCENARIO"),
        )
    elif method == "session/prompt":
        prompt_count += 1
        fail_once_marker = os.environ.get("ACP_MOCK_FAIL_ONCE_MARKER")
        if fail_once_marker and not os.path.exists(fail_once_marker):
            with open(fail_once_marker, "w", encoding="utf-8") as marker_file:
                marker_file.write("failed")
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32000,
                        "message": "intentional first-attempt failure",
                    },
                }
            )
            sys.exit(2)
        if (
            os.environ.get("ACP_MOCK_FAIL_ON_TURN") == "1"
            or os.environ.get("ACP_MOCK_FAIL_ON_PROMPT") == "1"
        ):
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32000,
                        "message": "session/prompt should not be called",
                    },
                }
            )
            sys.exit(2)
        send_available_sid = request.get("params", {}).get("sessionId", "mock-session")
        emit_available_commands(send_available_sid)



        params = request.get("params", {})
        incoming_sid = params.get("sessionId", "mock-session")
        incoming_prompt = prompt_text(params)
        prompt_log_file = os.environ.get("ACP_MOCK_PROMPT_LOG_FILE")
        if prompt_log_file:
            with open(prompt_log_file, "a", encoding="utf-8") as prompt_log:
                prompt_log.write(
                    json.dumps(
                        {
                            "prompt_index": prompt_count,
                            "session_id": incoming_sid,
                            "pid": os.getpid(),
                            "prompt": incoming_prompt,
                        },
                        separators=(",", ":"),
                    )
                    + "\n"
                )
        emit_tool_scenario(
            incoming_sid,
            os.environ.get("ACP_MOCK_TOOL_SCENARIO"),
        )
        selected_permission = request_permission(
            incoming_sid,
            os.environ.get("ACP_MOCK_PERMISSION_SCENARIO"),
        )
        if os.environ.get("ACP_MOCK_ECHO_PID") == "1":
            reply_text = f"pid={os.getpid()}"
        elif os.environ.get("ACP_MOCK_ECHO_SESSION_ID") == "1":
            reply_text = f"sid={incoming_sid}"
        elif os.environ.get("ACP_MOCK_ECHO_ENV_KEY"):
            env_key = os.environ.get("ACP_MOCK_ECHO_ENV_KEY")
            reply_text = f"{env_key}={os.environ.get(env_key, '')}"
        elif selected_permission is not None:
            reply_text = f"permission={selected_permission}"
        else:
            reply_text = "mock-acp-ok"
        wait_for_cancel_count = int(
            os.environ.get("ACP_MOCK_WAIT_FOR_CANCEL_COUNT", "0")
        )
        wait_for_cancel = (
            os.environ.get("ACP_MOCK_WAIT_FOR_CANCEL") == "1"
            or prompt_count <= wait_for_cancel_count
        )
        if os.environ.get("ACP_MOCK_ECHO_PROMPT_STATE") == "1":
            reply_text = (
                f"sid={incoming_sid} pid={os.getpid()} "
                f"prompt_index={prompt_count} prompt={incoming_prompt}"
            )
        if wait_for_cancel:
            if os.environ.get("ACP_MOCK_ECHO_PROMPT_STATE") == "1":
                reply_text = f"mock-acp-started {reply_text}"
            else:
                reply_text = "mock-acp-started"
        emit_agent_text(incoming_sid, reply_text)
        if wait_for_cancel:
            while True:
                cancel_line = sys.stdin.readline()
                if not cancel_line:
                    sys.exit(0)
                cancel_request = json.loads(cancel_line)
                if cancel_request.get("method") == "session/cancel":
                    cancel_log_file = os.environ.get("ACP_MOCK_CANCEL_LOG_FILE")
                    if cancel_log_file:
                        with open(cancel_log_file, "a", encoding="utf-8") as cancel_log:
                            cancel_log.write(f"{prompt_count}\n")
                    cancel_delay_ms = int(
                        os.environ.get("ACP_MOCK_CANCEL_DELAY_MS", "0")
                    )
                    if cancel_delay_ms > 0:
                        time.sleep(cancel_delay_ms / 1000)
                    success(request_id, {"stopReason": "cancelled"})
                    break
        else:
            success(request_id, {"stopReason": "end_turn"})
    else:
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": f"unknown method: {method}"},
            }
        )
