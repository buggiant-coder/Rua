package com.rua.mobile;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.function.Consumer;
import org.json.*;

// Real HTTP + SSE + Android PTY acceptance, intentionally no browser renderer.
final class NativeTerminalApiProbe {
    private final String token;
    private final File home;
    private final Consumer<String> log;
    private final String viewer = "native-terminal-acceptance";
    private String session;
    private String otherSession;
    private HttpURLConnection stream;
    private Thread reader;
    private final StringBuffer output = new StringBuffer();
    private volatile String streamError;
    NativeTerminalApiProbe(String token, File home, Consumer<String> log) {
        this.token=token; this.home=home; this.log=log;
    }
    private HttpURLConnection connection(String path) throws Exception {
        HttpURLConnection c=(HttpURLConnection)new URL("http://127.0.0.1:8090/api/terminal"+path).openConnection();
        c.setRequestProperty("Authorization","Bearer "+token);
        c.setConnectTimeout(5000); c.setReadTimeout(15000); return c;
    }
    private JSONObject request(String method, String path, JSONObject body) throws Exception {
        HttpURLConnection c=connection(path);
        try {
            c.setRequestMethod(method);
            if(body!=null) {
                c.setDoOutput(true); c.setRequestProperty("Content-Type","application/json");
                try(OutputStream out=c.getOutputStream()) {out.write(body.toString().getBytes(StandardCharsets.UTF_8));}
            }
            int status=c.getResponseCode();
            InputStream in=status<400?c.getInputStream():c.getErrorStream();
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();
            if(in!=null) try(InputStream source=in) {
                byte[] buffer=new byte[4096];
                int count;
                while((count=source.read(buffer))!=-1) bytes.write(buffer,0,count);
            }
            String text=new String(bytes.toByteArray(),StandardCharsets.UTF_8);
            if(status>=400) throw new IOException(method+" "+path+" status="+status+" "+text);
            return text.trim().isEmpty()?new JSONObject():new JSONObject(text);
        } finally {c.disconnect();}
    }
    private JSONObject viewerBody() throws Exception {
        return new JSONObject().put("browser_viewer_id",viewer).put("scrollback_lines",2000);
    }
    private String path(String suffix) {return "/sessions/"+session+suffix;}
    private void input(String data) throws Exception {
        request("POST",path("/input"),new JSONObject().put("browser_viewer_id",viewer).put("data",data));
    }
    private JSONObject attach() throws Exception { return request("POST",path("/attach"),viewerBody()); }
    private void connectStream(JSONObject cursor) throws Exception {
        stream=connection(path("/activity?execution_device=local&browser_viewer_id="+viewer+
            "&retain_authority=true&next_activity_sequence="+cursor.getLong("next_activity_sequence")+
            "&next_output_offset="+cursor.getLong("next_output_offset")));
        stream.setReadTimeout(0);
        if(stream.getResponseCode()!=200) throw new IOException("SSE status="+stream.getResponseCode());
        final InputStream bytes=stream.getInputStream(); streamError=null;
        reader=new Thread(() -> {
            try(BufferedReader lines=new BufferedReader(new InputStreamReader(bytes,StandardCharsets.UTF_8))) {
                String line;
                while((line=lines.readLine())!=null) {
                    if(!line.startsWith("data:")) continue;
                    JSONObject event=new JSONObject(line.substring(5).trim());
                    if(event.optString("type").equals("output")) {
                        output.append(new String(android.util.Base64.decode(event.getString("data_base64"),android.util.Base64.DEFAULT),StandardCharsets.UTF_8));
                    } else if(event.optString("type").equals("error")) streamError=event.toString();
                }
            } catch(Exception e) { streamError=e.toString(); }
        },"terminal-sse-probe"); reader.setDaemon(true);reader.start();
    }
    private void disconnectStream() throws Exception {
        if(stream!=null) {stream.disconnect();stream=null;}
        if(reader!=null) {reader.join(2000);reader=null;}
    }
    private void awaitOutput(String marker) throws Exception {
        for(int i=0;i<150;i++) {
            if(output.toString().contains(marker)) return;
            if(streamError!=null) throw new IOException("SSE failed: "+streamError);
            Thread.sleep(100);
        }
        throw new IOException("Missing "+marker+"; PTY="+output);
    }
    private String awaitFile(String name) throws Exception {
        File f=new File(home,name);
        for(int i=0;i<150;i++) {
            if(f.isFile() && f.length()>0) return new String(Files.readAllBytes(f.toPath()),StandardCharsets.UTF_8).trim();
            Thread.sleep(100);
        }
        throw new IOException("Missing guest result "+name+"; PTY="+output);
    }
    private void gone(String pids) throws Exception {
        for(int i=0;i<100;i++) {
            boolean any=false;
            for(String pid:pids.trim().split("\\s+")) {
                if(!pid.matches("[0-9]+")) throw new IOException("Invalid PID "+pid);
                any |= new File("/proc/"+pid).exists();
            }
            if(!any) return;
            Thread.sleep(100);
        }
        throw new IOException("Processes remain after terminal control: "+pids);
    }
    private void forbidden(String suffix, JSONObject body) throws Exception {
        try { request("POST",path(suffix),body); }
        catch(IOException expected) { if(expected.getMessage().contains("status=403")) return; throw expected; }
        throw new IOException("Non-lease holder was allowed: "+suffix);
    }
    static String sessionLeader(String pid) throws Exception {
        String stat=new String(Files.readAllBytes(new File("/proc/"+pid+"/stat").toPath()),StandardCharsets.UTF_8);
        return stat.substring(stat.lastIndexOf(") ")+2).split(" ")[3];
    }
    void run() throws Exception {
        for(String name:new String[]{"terminal-shell.pid","terminal-interrupt.pid","terminal-size","terminal-detached","terminal-close.pids","terminal-other.pid","terminal-other-alive"}) Files.deleteIfExists(new File(home,name).toPath());
        JSONObject opened=request("POST","/sessions",viewerBody().put("execution_device",new JSONObject().put("kind","local"))
            .put("terminal_profile_id","proot").put("cwd",home.toString()));
        session=opened.getJSONObject("session").getString("session_id");
        log.accept("TERMINAL_OPEN "+session);
        try {
            JSONObject attached=attach();
            connectStream(attached.getJSONObject("live_cursor"));
            input("printf '%s' $$ > terminal-shell.pid; printf 'TERM_%s\\n' 'READY'\n");
            awaitOutput("TERM_READY"); String shell=awaitFile("terminal-shell.pid");
            log.accept("TERMINAL_IO_PASS shell_pid="+shell);
            input("python3 -c \"import os,time; open('terminal-interrupt.pid','w').write(str(os.getpid())); time.sleep(120)\"\n");
            String interrupted=awaitFile("terminal-interrupt.pid");
            input("\u0003"); gone(interrupted);
            input("printf 'CTRL_%s\\n' 'RECOVERED'\n"); awaitOutput("CTRL_RECOVERED");
            log.accept("TERMINAL_CTRL_C_PASS python_pid="+interrupted);
            request("POST",path("/adapt-pty-geometry"),viewerBody().put("cols",101).put("rows",37));
            input("stty size > terminal-size\n");
            String size=awaitFile("terminal-size");
            if(!size.equals("37 101")) throw new IOException("Wrong PTY size: "+size);
            log.accept("TERMINAL_RESIZE_PASS "+size);
            JSONObject second=new JSONObject().put("browser_viewer_id","terminal-observer").put("scrollback_lines",2000);
            request("POST",path("/attach"),second);
            forbidden("/input", new JSONObject(second.toString()).put("data","echo invalid\n"));
            forbidden("/adapt-pty-geometry", new JSONObject(second.toString()).put("cols",80).put("rows",20));
            request("POST",path("/take-input-lease"),second);
            forbidden("/input", viewerBody().put("data","echo invalid\n"));
            request("POST",path("/take-input-lease"),viewerBody());
            request("POST",path("/detach"),second);
            log.accept("TERMINAL_LEASE_PASS");
            disconnectStream();
            request("POST",path("/detach"),viewerBody());
            // No viewers: session must remain alive. Reattach then request asynchronous output,
            // disconnect transport, and restore the canonical screen after the command finishes.
            Thread.sleep(300);
            JSONObject retained=attach();
            if(!retained.getJSONObject("session").getString("session_id").equals(session)) throw new IOException("Session changed");
            request("POST",path("/take-input-lease"),viewerBody());
            input("sleep 1; printf 'OFFLINE_%s\\n' 'OUTPUT'; printf '%s' $$ > terminal-detached\n");
            request("POST",path("/detach"),viewerBody());
            String sameShell=awaitFile("terminal-detached");
            if(!sameShell.equals(shell)) throw new IOException("Shell restarted");
            JSONObject restored=attach();
            String screen=restored.getJSONObject("snapshot").getString("data");
            if(!screen.contains("OFFLINE_OUTPUT")) throw new IOException("Snapshot lost offline output: "+screen);
            request("POST",path("/take-input-lease"),viewerBody());
            connectStream(restored.getJSONObject("live_cursor"));
            input("printf 'RECONNECT_%s\\n' 'LIVE'\n");awaitOutput("RECONNECT_LIVE");
            log.accept("TERMINAL_RECONNECT_PASS same shell; snapshot and live SSE restored");
            input("python3 -c \"import os,subprocess,time; p=subprocess.Popen(['python3','-c','import time; time.sleep(120)']); open('terminal-close.pids','w').write(str(os.getpid())+' '+str(p.pid)); time.sleep(120)\"\n");
            String children=awaitFile("terminal-close.pids");
            String leader=sessionLeader(shell);
            otherSession=request("POST","/sessions",viewerBody().put("execution_device",new JSONObject().put("kind","local"))
                .put("terminal_profile_id","proot").put("cwd",home.toString())).getJSONObject("session").getString("session_id");
            request("POST","/sessions/"+otherSession+"/input",viewerBody().put("data","printf '%s' $$ > terminal-other.pid\n"));
            String otherPid=awaitFile("terminal-other.pid");
            request("DELETE",path("?browser_viewer_id="+viewer),null);
            try { request("DELETE",path("?browser_viewer_id="+viewer),null); }
            catch(IOException expected) { if(!expected.getMessage().contains("status=404")) throw expected; }
            session=null;
            gone(leader+" "+shell+" "+children);
            log.accept("TERMINAL_CLOSE_PASS shell_and_children="+shell+" "+children);
            request("POST","/sessions/"+otherSession+"/input",viewerBody().put("data","printf '%s' $$ > terminal-other-alive\n"));
            if(!awaitFile("terminal-other-alive").equals(otherPid)) throw new IOException("Other terminal was disrupted");
            request("DELETE","/sessions/"+otherSession+"?browser_viewer_id="+viewer,null); otherSession=null;
            gone(otherPid);
            log.accept("TERMINAL_SESSION_ISOLATION_PASS");
            log.accept("TERMINAL_ALL_PASS");
        } finally {
            disconnectStream();
            if(otherSession!=null) request("DELETE","/sessions/"+otherSession+"?browser_viewer_id="+viewer,null);
            if(session!=null) request("DELETE",path("?browser_viewer_id="+viewer),null);
        }
    }
}
