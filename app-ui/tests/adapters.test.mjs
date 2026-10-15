import test from 'node:test';
import assert from 'node:assert/strict';
import { adapterTranscript, mergeAdapterEvents, groupConversations, continuationTarget, continuationBlocker, codexItemSummary, approvalSummary, lifecycleOnlyItem } from '../adapters.js';
import { readAdapterJournal, writeAdapterJournal } from '../adapter-journal.js';
const event=(seq,kind,payload={},run_id='r')=>({seq,kind,payload,run_id,at_ms:seq});
test('external run keeps the actual human task and final assistant text once',()=>{
  const items=adapterTranscript([event(1,'user_task',{text:'Inspect the project'}),event(2,'assistant_delta',{item_id:'i',text:'Hel'}),event(3,'assistant_delta',{item_id:'i',text:'lo'}),event(4,'item_completed',{item:{id:'i',type:'agentMessage',text:'Hello'}})]);
  assert.equal(items.length,2);assert.equal(items[0].type,'user');assert.equal(items[0].text,'Inspect the project');assert.equal(items[1].text,'Hello');
});
test('empty lifecycle items do not surround the actual bubbles of a turn',()=>{
  const items=adapterTranscript([
    event(1,'user_task',{text:'Inspect the project'}),
    event(2,'item_started',{item:{id:'u1',type:'userMessage',status:'completed'}}),
    event(3,'item_completed',{item:{id:'u1',type:'userMessage',status:'completed'}}),
    event(4,'item_started',{item:{id:'r1',type:'reasoning'}}),
    event(5,'item_completed',{item:{id:'r1',type:'reasoning'}}),
    event(6,'item_started',{item:{id:'a1',type:'agentMessage'}}),
    event(7,'assistant_delta',{item_id:'a1',text:'Hel'}),
    event(8,'item_completed',{item:{id:'a1',type:'agentMessage',text:'Hello'}}),
    event(9,'item_started',{item:{id:'r2',type:'reasoning'}}),
    event(10,'item_completed',{item:{id:'r2',type:'reasoning'}}),
  ]);
  assert.deepEqual(items.map(item=>item.type),['user','assistant']);
  assert.equal(items[1].text,'Hello');
});
test('a completed message without text keeps the text already streamed',()=>{
  const items=adapterTranscript([event(1,'assistant_delta',{item_id:'a',text:'Partial answer'}),event(2,'item_completed',{item:{id:'a',type:'agentMessage',text:''}})]);
  assert.deepEqual(items.map(item=>item.text),['Partial answer']);
  const once=adapterTranscript([event(1,'item_started',{item:{id:'a',type:'agentMessage',text:'Partial'}}),event(2,'item_completed',{item:{id:'a',type:'agentMessage',text:'Partial answer'}})]);
  assert.deepEqual(once.map(item=>item.text),['Partial answer']);
  // A corrected final message replaces streamed text, even when it is shorter.
  const corrected=adapterTranscript([event(1,'assistant_delta',{item_id:'a',text:'Draft that was revised'}),event(2,'item_completed',{item:{id:'a',type:'agentMessage',text:'Final answer'}})]);
  assert.deepEqual(corrected.map(item=>item.text),['Final answer']);
  const shorter=adapterTranscript([event(1,'assistant_delta',{item_id:'a',text:'Hello obsolete suffix'}),event(2,'item_completed',{item:{id:'a',type:'agentMessage',text:'Hello'}})]);
  assert.deepEqual(shorter.map(item=>item.text),['Hello']);
  // A started announcement must not shorten text the deltas already delivered.
  const announced=adapterTranscript([event(1,'assistant_delta',{item_id:'a',text:'Hello there'}),event(2,'item_started',{item:{id:'a',type:'agentMessage',text:'Hello'}}),event(3,'item_completed',{item:{id:'a',type:'agentMessage',text:'Hello there'}})]);
  assert.deepEqual(announced.map(item=>item.text),['Hello there']);
  // Re-reading the same page of saved history must not add a second bubble.
  const repeated=adapterTranscript([event(1,'assistant_delta',{item_id:'a',text:'Hel'}),event(2,'item_completed',{item:{id:'a',type:'agentMessage',text:'Hello'}}),event(3,'item_completed',{item:{id:'a',type:'agentMessage',text:'Hello'}}),event(4,'item_completed',{item:{id:'b',type:'agentMessage',text:'Second message'}})]);
  assert.deepEqual(repeated.map(item=>item.text),['Hello','Second message']);
});
test('reported failures and readable reasoning are never treated as empty noise',()=>{
  assert.equal(lifecycleOnlyItem({id:'i',type:'fileChange',status:'failed',changes:[]}),false);
  assert.equal(lifecycleOnlyItem({id:'i',type:'reasoning',text:'Planning the fix'}),false);
  assert.equal(lifecycleOnlyItem({id:'i',type:'reasoning',status:'completed'}),true);
  const items=adapterTranscript([event(1,'item_completed',{item:{id:'i',type:'reasoning',text:'Planning the fix'}}),event(2,'item_completed',{item:{id:'d',type:'claudeTool',name:'Bash',status:'declined',input:{command:'rm -rf .'}}})]);
  assert.deepEqual(items.map(item=>item.type),['activity','claude_item']);
  assert.equal(items[0].payload.item.text,'Planning the fix');
});
test('external tool and unfamiliar events retain exact untrusted payloads',()=>{
  const payload={item:{type:'future',output:'<img onerror="bad">'}};
  const items=adapterTranscript([event(1,'item_completed',payload)]);
  assert.equal(items[0].type,'activity');assert.deepEqual(items[0].payload,payload);
});
test('external event history is isolated to a run and bounded',()=>{
  const items=mergeAdapterEvents(Array.from({length:605},(_,i)=>event(i+1,'notice')),[event(605,'updated'),event(606,'foreign',{},'another')],'r');
  assert.equal(items.length,600);assert.equal(items[0].seq,6);assert.equal(items.at(-1).kind,'updated');
});
test('uncertain external starts retain their original request across reload within one host',()=>{
  let stored='';const storage={getItem:()=>stored,setItem:(_,value)=>{stored=value;}};
  const request={project_id:'p1',adapter_id:'codex',prompt:'Inspect local files',command_id:'4bd6cc30-30ef-4872-b8d7-6e5ef50cfd61'};
  assert.equal(writeAdapterJournal(storage,'host1',{p1:request}),true);
  assert.deepEqual(readAdapterJournal(storage,'host1'),{p1:request});
  assert.deepEqual(readAdapterJournal(storage,'new-host'),{});
});
test('external pending journal rejects malformed requests and reports blocked persistence',()=>{
  const storage={getItem:()=>JSON.stringify({host_id:'h',requests:{p:{project_id:'p',adapter_id:'codex',prompt:'hello',command_id:'bad'}}})};
  assert.deepEqual(readAdapterJournal(storage,'h'),{});
  assert.equal(writeAdapterJournal({setItem(){throw Error('blocked');}},'h',{}),false);
});

test('persisted uncertain starts retain their exact account profile',()=>{
  const request={project_id:'p',adapter_id:'codex',profile_id:'original-account',prompt:'Inspect local files',command_id:'4bd6cc30-30ef-4872-b8d7-6e5ef50cfd61'};
  const storage={getItem:()=>JSON.stringify({host_id:'profile-reload',requests:{p:request}})};
  assert.deepEqual(readAdapterJournal(storage,'profile-reload').p,request);
});
test('blocked storage retains uncertain starts in memory across workspace mode changes',()=>{
  const storage={getItem(){throw Error('blocked');},setItem(){throw Error('blocked');}};
  const request={project_id:'p',adapter_id:'codex',prompt:'Inspect local files',command_id:'4bd6cc30-30ef-4872-b8d7-6e5ef50cfd61'};
  assert.equal(writeAdapterJournal(storage,'blocked-host',{p:request}),false);
  assert.deepEqual(readAdapterJournal(storage,'blocked-host'),{p:request});
  writeAdapterJournal(storage,'blocked-host',{});
  assert.deepEqual(readAdapterJournal(storage,'blocked-host'),{});
});

const run=(id,started_at_ms,extra={})=>({id,conversation_id:'c1',started_at_ms,state:'completed',thread_id:'t1',turn_id:'u1',ephemeral:false,title:'',...extra});
test('conversations group continued runs oldest first and list the newest conversation first',()=>{
  const groups=groupConversations([run('b',20),run('x',30,{conversation_id:'c2',title:' Other '}),run('a',10,{title:'First task'}),run('legacy',5,{conversation_id:undefined})]);
  assert.deepEqual(groups.map(group=>group.id),['c2','c1','legacy']);
  assert.deepEqual(groups[1].runs.map(item=>item.id),['a','b']);
  assert.equal(groups[1].title,'First task');assert.equal(groups[1].latest.id,'b');assert.equal(groups[0].title,'Other');
});
test('a finished saved conversation continues from its latest run that attached a thread',()=>{
  const [conversation]=groupConversations([run('a',10),run('b',20),run('c',30,{state:'failed',thread_id:null})]);
  assert.equal(continuationTarget(conversation).id,'b');assert.equal(continuationBlocker(conversation),'');
});
test('active, interrupted-unreviewed and unsaved conversations cannot be continued',()=>{
  const busy=groupConversations([run('a',10),run('b',20,{state:'awaiting_approval'})])[0];
  assert.equal(continuationTarget(busy),null);assert.equal(continuationBlocker(busy),'');
  const review=groupConversations([run('a',10,{state:'recovery_required'})])[0];
  assert.equal(continuationTarget(review),null);assert.equal(continuationBlocker(review),'review');
  const unsaved=groupConversations([run('a',10,{ephemeral:true})])[0];
  assert.equal(continuationTarget(unsaved),null);assert.match(continuationBlocker(unsaved),/not saved/);
  const turnless=groupConversations([run('a',10,{state:'failed',turn_id:null})])[0];
  assert.equal(continuationTarget(turnless),null);assert.match(continuationBlocker(turnless),/never confirmed a message/);
  const laterTurnless=groupConversations([run('a',10),run('b',20,{state:'failed',turn_id:null,continued_from:'a'})])[0];
  assert.equal(continuationTarget(laterTurnless).id,'b');
  const threadless=groupConversations([run('a',10,{state:'failed',thread_id:null,turn_id:null})])[0];
  assert.equal(continuationTarget(threadless),null);assert.match(continuationBlocker(threadless),/did not create a conversation/);
  assert.equal(continuationTarget(null),null);assert.equal(continuationBlocker(null),'');
});
test('pending continuation starts keep their continued run and reject an empty one',()=>{
  const request={project_id:'p',adapter_id:'codex',prompt:'Next step',continue_run_id:'run-1',command_id:'4bd6cc30-30ef-4872-b8d7-6e5ef50cfd61'};
  assert.deepEqual(readAdapterJournal({getItem:()=>JSON.stringify({host_id:'continue-host',requests:{p:request}})},'continue-host').p,request);
  assert.deepEqual(readAdapterJournal({getItem:()=>JSON.stringify({host_id:'continue-empty',requests:{p:{...request,continue_run_id:''}}})},'continue-empty'),{});
});

// Fixtures follow the Codex 0.159.3 app-server schema shapes.
const commandItem=(status,extra={})=>({id:'cmd1',type:'commandExecution',command:'npm test -- --grep "a b"',cwd:'C:\\work\\p',status,...extra});
test('a Codex command item becomes one transcript entry updated in place by its completion',()=>{
  const items=adapterTranscript([event(1,'item_started',{item:commandItem('inProgress')}),event(2,'notice',{message:'x'}),event(3,'item_completed',{item:commandItem('completed',{exitCode:0,aggregatedOutput:'ok\n'})})]);
  assert.deepEqual(items.map(item=>item.type),['codex_item','activity']);
  assert.equal(items[0].item.status,'completed');assert.equal(items[0].seq,1);
  const summary=codexItemSummary(items[0].item);
  assert.equal(summary.command,'npm test -- --grep "a b"');assert.equal(summary.cwd,'C:\\work\\p');
  assert.equal(summary.statusLabel,'Completed');assert.equal(summary.tone,'neutral');assert.equal(summary.exitCode,0);assert.equal(summary.output,'ok\n');
});
test('command outcomes keep failure, declined and unreported fields distinct',()=>{
  assert.equal(codexItemSummary(commandItem('completed',{exitCode:2})).tone,'stop');
  assert.equal(codexItemSummary(commandItem('failed')).tone,'stop');
  const declined=codexItemSummary(commandItem('declined'));assert.equal(declined.statusLabel,'Declined');assert.equal(declined.tone,'caution');
  const bare=codexItemSummary({id:'c',type:'commandExecution',status:'completed'});
  assert.equal(bare.exitCode,null);assert.equal(bare.output,null);assert.equal(bare.command,'');
  assert.equal(codexItemSummary({type:'commandExecution',status:'somethingNew'}).statusLabel,'Something new');
});
test('file changes keep exact paths, kinds and change text',()=>{
  const item={id:'f1',type:'fileChange',status:'completed',changes:[
    {path:'src/a.js',kind:{type:'update',move_path:null},diff:'@@ -1 +1 @@\n-a\n+b\n'},
    {path:'src/new.js',kind:{type:'add'},diff:'export const x=1;\n'},
    {path:'old.txt',kind:{type:'delete'},diff:''},
    {path:'b.js',kind:{type:'update',move_path:'c.js'},diff:'@@ -1 +1 @@\n-x\n+y\n'}]};
  const summary=codexItemSummary(item);
  assert.equal(summary.title,'Edit 4 files');
  assert.deepEqual(summary.changes.map(change=>[change.path,change.label,change.unified]),[['src/a.js','Edit',true],['src/new.js','New file',false],['old.txt','Delete',false],['b.js','Move and edit',true]]);
  assert.equal(summary.changes[1].diff,'export const x=1;\n');assert.equal(summary.changes[3].move,'c.js');
  assert.equal(codexItemSummary({type:'fileChange',changes:[{path:'one'}]}).title,'Edit 1 file');
});
test('unfamiliar Codex items stay raw activity with a readable label',()=>{
  // An item reporting only its ID and type is lifecycle noise; one with content stays.
  const items=adapterTranscript([event(1,'item_completed',{item:{id:'r',type:'reasoning',status:'completed'}}),event(2,'item_completed',{item:{id:'w',type:'webSearch',status:'completed',query:'switchya'}})]);
  assert.equal(items.length,1);
  assert.equal(items[0].type,'activity');assert.equal(items[0].payload.item.query,'switchya');assert.equal(codexItemSummary({type:'webSearch'}).title,'Web search');
});
test('approval summaries show the exact proposed command, network host and stdin kind',()=>{
  const approval=(params,method='item/commandExecution/requestApproval',item=null)=>({method,preview:{params:{threadId:'t',turnId:'u',itemId:'i',startedAtMs:1,...params},item}});
  const plain=approvalSummary(approval({command:'rm -rf build',cwd:'/p',reason:'clean'}));
  assert.equal(plain.title,'Run this command?');assert.equal(plain.command,'rm -rf build');assert.equal(plain.cwd,'/p');assert.equal(plain.reason,'clean');
  const network=approvalSummary(approval({command:'curl x',cwd:'/p',networkApprovalContext:{host:'example.com',protocol:'https'}}));
  assert.equal(network.title,'Allow network access to example.com?');assert.deepEqual(network.network,{host:'example.com',protocol:'https'});
  assert.equal(approvalSummary(approval({kind:'writeStdin'})).title,'Send input to a running command?');
  const files=approvalSummary(approval({grantRoot:'/'},'item/fileChange/requestApproval',{id:'i',type:'fileChange',changes:[{path:'a',kind:{type:'add'},diff:'x'}]}));
  assert.equal(files.title,'Apply this file change?');assert.equal(files.grantRoot,'/');assert.equal(files.changes[0].label,'New file');
  assert.equal(approvalSummary(approval({},'item/fileChange/requestApproval',null)).title,'Apply file changes?');
});
test('recorded decisions and turn outcomes become readable entries',()=>{
  const items=adapterTranscript([event(1,'approval_resolved',{approval_id:'a',decision:'deny'}),event(2,'turn_completed',{status:'completed'})]);
  assert.equal(items[0].type,'decision');assert.equal(items[0].decision,'deny');assert.equal(items[1].kind,'turn_completed');
});

test('Claude tool completions replace their started card and preserve exact input and output',()=>{
  const started={id:'claude-tool',type:'claudeTool',name:'Write',input:{file_path:'a.txt',content:'<script>literal</script>'},status:'inProgress'};
  const finished={...started,status:'completed',output:'Created a.txt'};
  const items=adapterTranscript([event(1,'item_started',{item:started}),event(2,'item_completed',{item:finished})]);
  assert.equal(items.length,1);assert.equal(items[0].type,'claude_item');
  assert.deepEqual(items[0].item,finished);assert.equal(items[0].seq,1);
});

test('Claude permission summaries retain the exact tool input without inventing a command or diff',()=>{
  const input={file_path:'src/a.js',old_string:'before',new_string:'after',replace_all:true};
  const summary=approvalSummary({method:'claude/can_use_tool',preview:{request:{tool_name:'Edit',input,description:'Update this file'}}});
  assert.equal(summary.kind,'claude');assert.equal(summary.title,'Allow Claude Code to use Edit?');
  assert.deepEqual(summary.input,input);assert.equal(summary.reason,'Update this file');
  assert.equal(summary.command,undefined);assert.equal(summary.changes,undefined);
});

test('Claude streaming text is visible before completion and is shown once after completion',()=>{
  const fragments=[event(1,'assistant_delta',{item_id:'claude-root-msg-0',text:'Live '}),event(2,'assistant_delta',{item_id:'claude-root-msg-0',text:'reply'})];
  assert.equal(adapterTranscript(fragments)[0].text,'Live reply');
  const items=adapterTranscript([...fragments,event(3,'item_completed',{item:{type:'agentMessage',id:'claude-root-msg-0',text:'Live reply'}})]);
  assert.equal(items.length,1);assert.equal(items[0].text,'Live reply');
});

test('progress states leave the transcript; an end state shows once, by the turn report when one exists',async()=>{
  const { adapterTranscript }=await import('../adapters.js');
  const kinds=items=>items.map(item=>item.kind==='state_changed'?`state:${item.payload.state}`:item.kind||item.type);
  const turn=(status,...states)=>adapterTranscript([
    event(1,'state_changed',{state:'starting'}),event(2,'user_task',{text:'Task'}),event(3,'state_changed',{state:'running'}),
    event(4,'state_changed',{state:'awaiting_approval'}),event(5,'state_changed',{state:'interrupting'}),
    ...(status?[event(6,'turn_completed',{status})]:[]),...states.map((state,index)=>event(7+index,'state_changed',{state})),
  ]);
  assert.deepEqual(kinds(turn('completed','completed')),['user_task','turn_completed']);
  // Without a turn report, the end state itself is the outcome row.
  assert.deepEqual(kinds(turn(null,'completed')),['user_task','state:completed']);
  assert.deepEqual(kinds(turn(null,'failed')),['user_task','state:failed']);
  assert.deepEqual(kinds(turn('failed','failed')),['user_task','turn_completed']);
  // A turn that completed but a run that then needed review keeps both facts.
  assert.deepEqual(kinds(turn('completed','recovery_required')),['user_task','turn_completed','state:recovery_required']);
  assert.deepEqual(kinds(turn('interrupted','interrupted')),['user_task','turn_completed']);
});

test('a repeated agent notice is shown once per conversation; errors and recovery reviews always stay',async()=>{
  const { conversationTranscript }=await import('../adapters.js');
  const plan='Antigravity runs in plan mode.';
  const runEvents=(id,extra=[])=>[event(1,'user_task',{text:`Task ${id}`},id),event(2,'notice',{message:plan},id),...extra];
  const blocks=conversationTranscript([
    {run:{id:'a'},events:runEvents('a',[event(3,'adapter_error',{message:'CLI exited'},'a')])},
    {run:{id:'b'},events:runEvents('b',[event(3,'adapter_error',{message:'CLI exited'},'b'),event(4,'recovery_acknowledged',{message:'Reviewed'},'b'),event(5,'notice',{message:'Different notice'},'b')])},
    {run:{id:'c'},events:null},
  ]);
  const kinds=block=>block.items.map(item=>item.kind||item.type);
  assert.deepEqual(kinds(blocks[0]),['user_task','notice','adapter_error']);
  assert.deepEqual(kinds(blocks[1]),['user_task','adapter_error','recovery_acknowledged','notice']);
  assert.equal(blocks[1].items.at(-1).payload.message,'Different notice');
  assert.equal(blocks[2].items,null,'an unloaded turn stays unloaded');
});

test('transcript tool rows use readable names and keep unknown names exactly',async()=>{
  const { toolLabel, approvalSummary }=await import('../adapters.js');
  assert.equal(toolLabel('view_file'),'Read file');assert.equal(toolLabel('Read'),'Read file');
  assert.equal(toolLabel('Bash'),'Run command');assert.equal(toolLabel('PowerShell'),'Run command');
  assert.equal(toolLabel('mcp__fixture__custom_tool'),'mcp__fixture__custom_tool');
  assert.equal(toolLabel('toString'),'toString','object keys are not tool labels');
  // Approval titles always keep the exact tool name.
  assert.equal(approvalSummary({method:'claude/can_use_tool',preview:{request:{tool_name:'Bash',input:{command:'ls'}}}}).title,'Allow Claude Code to use Bash?');
});
