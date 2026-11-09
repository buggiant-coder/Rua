'use strict';
const fs = require('node:fs');
const assert = require('node:assert/strict');
const {spawn, execFileSync} = require('node:child_process');
const readline = require('node:readline');
assert.equal(process.platform, 'linux');
assert.equal(process.arch, 'arm64');
assert.equal(process.env.MCP_FIXTURE_SECRET, 'fixture-secret');
assert(!process.argv.join(' ').includes('fixture-secret'));
for (const key of ['RUA_PHONE_TOKEN','RUA_PHONE_SOCKET','RUA_PHONE_HOST_PID','RUA_ANDROID_NATIVE_DIR','LD_LIBRARY_PATH']) assert.equal(process.env[key], undefined);
const background = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], {stdio:'ignore'});
fs.writeFileSync('native-mcp-server-pids', process.pid+' '+background.pid);
const calls = new Map();
function send(id, result) { process.stdout.write(JSON.stringify({jsonrpc:'2.0',id,result})+'\n'); }
readline.createInterface({input:process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'notifications/cancelled') {
    const child = calls.get(request.params.requestId);
    if (child) { child.kill('SIGTERM'); calls.delete(request.params.requestId); }
    return;
  }
  if (request.id === undefined) return;
  if (request.method === 'initialize') {
    send(request.id,{protocolVersion:request.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'native-runtime-mcp',version:'1.0'}});
  } else if (request.method === 'tools/list') {
    send(request.id,{tools:['probe','wait'].map(name=>({name,description:'Native guest acceptance '+name,inputSchema:{type:'object',properties:{}}}))});
  } else if (request.method === 'tools/call' && request.params.name === 'probe') {
    const input = fs.readFileSync('native-mcp-input','utf8');
    const nested = execFileSync(process.execPath,['-e',"process.stdout.write(process.platform+' '+process.arch)"],{encoding:'utf8'});
    const result = {marker:'NATIVE_MCP_OK',cwd:process.cwd(),input,nested};
    fs.writeFileSync('native-mcp-output.json',JSON.stringify(result));
    send(request.id,{content:[{type:'text',text:JSON.stringify(result)}]});
  } else if (request.method === 'tools/call' && request.params.name === 'wait') {
    fs.appendFileSync('native-mcp-calls','wait\n');
    const code = `const {spawn}=require('node:child_process');const fs=require('node:fs');const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});fs.writeFileSync('native-mcp-call-pids',process.pid+' '+child.pid);process.on('SIGTERM',()=>{child.kill('SIGKILL');child.on('exit',()=>process.exit(0));});setInterval(()=>{},1000);`;
    const child = spawn(process.execPath,['-e',code],{stdio:'ignore'});
    calls.set(request.id,child);
    child.on('exit',()=>calls.delete(request.id));
  } else { send(request.id,{}); }
});
