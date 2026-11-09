#!/usr/bin/env node
'use strict';
const fs = require('node:fs');
const assert = require('node:assert/strict');
const {spawn, execFileSync} = require('node:child_process');
const readline = require('node:readline');
assert.equal(process.platform, 'linux');
assert.equal(process.arch, 'arm64');
for (const key of ['RUA_PHONE_TOKEN','RUA_PHONE_SOCKET','RUA_PHONE_HOST_PID','RUA_ANDROID_NATIVE_DIR','LD_LIBRARY_PATH']) assert.equal(process.env[key], undefined);
const background = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], {stdio:'ignore'});
fs.writeFileSync('native-acp-server-pids', process.pid+' '+background.pid);
const sid = 'native-acp-'+process.pid;
let prompt, requestChild, permission;
const send = value => process.stdout.write(JSON.stringify({jsonrpc:'2.0',...value})+'\n');
const result = (id, value) => send({id,result:value});
function finish(text) {
  send({method:'session/update',params:{sessionId:sid,update:{sessionUpdate:'agent_message_chunk',content:{type:'text',text}}}});
  result(prompt.id,{stopReason:'end_turn'}); prompt = undefined;
}
readline.createInterface({input:process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  if (request.id === permission && !request.method) {
    assert.equal(request.result.outcome.optionId,'allow_once');
    fs.writeFileSync('native-acp-permission','allow_once');
    finish('NATIVE_ACP_PERMISSION_OK'); return;
  }
  switch(request.method) {
    case 'initialize': result(request.id,{protocolVersion:1,agentCapabilities:{},authMethods:[],agentInfo:{name:'native-acp-fixture',version:'1.0'}}); break;
    case 'session/new':
      assert.equal(request.params.cwd,process.cwd());
      result(request.id,{sessionId:sid}); break;
    case 'session/prompt': {
      prompt = request;
      const text = request.params.prompt.filter(b=>b.type==='text').map(b=>b.text).join('');
      fs.appendFileSync('native-acp-prompts',text+'\n');
      if (text.includes('wait')) {
        const code = `const {spawn}=require('node:child_process');const fs=require('node:fs');const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});fs.writeFileSync('native-acp-call-pids',process.pid+' '+child.pid);process.on('SIGTERM',()=>{child.kill('SIGKILL');child.on('exit',()=>process.exit(0));});setInterval(()=>{},1000);`;
        requestChild = spawn(process.execPath,['-e',code],{stdio:'ignore'});
      } else if (text.includes('permission')) {
        permission = 'permission-'+request.id;
        send({method:'session/update',params:{sessionId:sid,update:{sessionUpdate:'tool_call',toolCallId:permission,title:'fixture command',kind:'execute',status:'pending',rawInput:{command:'fixture'}}}});
        send({id:permission,method:'session/request_permission',params:{sessionId:sid,toolCall:{toolCallId:permission},options:[{optionId:'allow_once',name:'Allow once',kind:'allow_once'},{optionId:'reject_once',name:'Reject',kind:'reject_once'}]}});
      } else {
        const python = execFileSync('python3',['-c',"import platform;print(platform.system()+' '+platform.machine())"],{encoding:'utf8'}).trim();
        const output = {marker:'NATIVE_ACP_OK',cwd:process.cwd(),node:process.version,python};
        fs.writeFileSync('native-acp-output.json',JSON.stringify(output));
        finish(JSON.stringify(output));
      }
      break;
    }
    case 'session/cancel':
      if(requestChild) {requestChild.kill('SIGTERM');requestChild=undefined;}
      fs.writeFileSync('native-acp-cancelled','cancelled');
      if(prompt) {result(prompt.id,{stopReason:'cancelled'});prompt=undefined;}
      break;
    default: if(request.id!==undefined) result(request.id,{});
  }
});
