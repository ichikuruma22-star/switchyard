import test from 'node:test';
import assert from 'node:assert/strict';
import { transcript, mergeEvents, reviewOperations, stateTone, canSubmit } from '../presentation.js';
import { acknowledgedTurn, readJournal, writeJournal } from '../turn-journal.js';
const event=(seq,kind,payload={},run_id='r1',session_id='s1')=>({seq,kind,payload,run_id,session_id,at_ms:1000+seq});
test('streamed assistant completion replaces deltas and does not duplicate output',()=>{
  const result=transcript([event(1,'turn.started',{text:'Task'}),event(2,'model.delta',{text:'hel'}),event(3,'model.delta',{text:'lo'}),event(4,'model.completed',{text:'hello'})]);
  assert.equal(result.length,2);assert.equal(result[1].text,'hello');assert.equal(result[1].streaming,false);
});

test('tool-only model turns do not create empty assistant messages',()=>{
  const result=transcript([event(1,'model.delta',{text:''}),event(2,'model.completed',{text:'',finish:'tool_calls'}),event(3,'tool.proposed',{id:'read',name:'read_file'}),event(4,'model.completed',{text:'   ',finish:'tool_calls'})]);
  assert.equal(result.length,1);assert.equal(result[0].type,'tool');assert.equal(result[0].operation.id,'read');
});
test('tool steps separate adjacent assistant messages and track exact results',()=>{
  const operation={id:'o1',name:'run_command',arguments_hash:'abc'};
  const result=transcript([event(1,'model.completed',{text:'Checking'}),event(2,'tool.proposed',operation),event(3,'approval.required',operation),event(4,'approval.decided',{operation_id:'o1',decision:'allow_once'}),event(5,'tool.started',{operation_id:'o1'}),event(6,'tool.completed',{operation_id:'o1',result:{status:'error',exit_code:1}}),event(7,'model.completed',{text:'Check failed'})]);
  assert.equal(result.length,3);assert.equal(result[1].status,'error');assert.equal(result[1].result.exit_code,1);assert.equal(result[2].text,'Check failed');
});
test('unknown events remain visible as untrusted data',()=>{
  const result=transcript([event(1,'future.event',{html:'<script>bad</script>'})]);
  assert.equal(result[0].type,'activity');assert.equal(result[0].payload.html,'<script>bad</script>');
});
test('bounded event merge deduplicates sequences and excludes another session',()=>{
  const old=Array.from({length:610},(_,i)=>event(i+1,'notice'));
  const result=mergeEvents(old,[event(610,'changed'),event(611,'notice',{},'r','other')],'s1');
  assert.equal(result.length,600);assert.equal(result[0].seq,11);assert.equal(result.at(-1).kind,'changed');
});
test('completed is neutral and uncertain recovery cannot submit a turn',()=>{
  assert.equal(stateTone('completed'),'neutral');assert.equal(canSubmit('recovery_required'),false);assert.equal(canSubmit('interrupted'),true);assert.equal(canSubmit('running'),false);
});
test('review retains proposal when a tool result arrives',()=>{
  const result=reviewOperations([event(1,'tool.proposed',{id:'o',name:'apply_patch',preview:{edits:[]}}),event(2,'tool.completed',{operation_id:'o',name:'apply_patch',result:{status:'ok'}})]);
  assert.deepEqual(result[0].preview,{edits:[]});assert.equal(result[0].result.status,'ok');
});
test('journal accepts only complete original turns and acknowledges the exact session and command',()=>{
  let saved='';const storage={getItem:()=>saved,setItem:(_,value)=>{saved=value;}};
  const turn={session_id:'s1',command_id:'4bd6cc30-30ef-4872-b8d7-6e5ef50cfd61',text:'Run focused checks'};
  assert.equal(writeJournal(storage,{s1:turn}),true);assert.deepEqual(readJournal(storage),{s1:turn});
  assert.equal(acknowledgedTurn([event(1,'turn.started',{command_id:turn.command_id})],turn),true);
  assert.equal(acknowledgedTurn([event(1,'turn.started',{command_id:turn.command_id},'r','other')],turn),false);
  saved=JSON.stringify({s1:{...turn,command_id:'invalid'},s2:turn});assert.deepEqual(readJournal(storage),{});
});
test('blocked storage is reported without losing the in-memory request',()=>{
  assert.deepEqual(readJournal({getItem(){throw Error('blocked');}}),{});
  assert.equal(writeJournal({setItem(){throw Error('blocked');}},{s1:{}}),false);
});

test('display paths hide Windows verbatim prefixes and leave other paths unchanged',async()=>{
  const { displayPath }=await import('../presentation.js');
  const raw=String.raw;
  assert.equal(displayPath(raw`\\?\E:\projects\app`),raw`E:\projects\app`);
  assert.equal(displayPath(raw`\\?\UNC\server\share\app`),raw`\\server\share\app`);
  assert.equal(displayPath(raw`\\?\unc\server\share`),raw`\\server\share`);
  assert.equal(displayPath(raw`E:\projects\app`),raw`E:\projects\app`);
  assert.equal(displayPath(raw`\\server\share`),raw`\\server\share`);
  assert.equal(displayPath('/home/user/app'),'/home/user/app');
  assert.equal(displayPath(undefined),'');
});
test('auto-approved operations stay distinguishable from human decisions',async()=>{
  const { autoApproveChange }=await import('../presentation.js');
  const operation={id:'o1',name:'apply_patch',arguments_hash:'abc'};
  const events=[event(1,'tool.proposed',operation),event(2,'approval.decided',{operation_id:'o1',decision:'allow_once',decided_by:'auto_approve',policy_source:'session'}),event(3,'tool.started',{operation_id:'o1'}),event(4,'tool.completed',{operation_id:'o1',name:'apply_patch',result:{status:'completed'}})];
  const [row]=transcript(events.slice(0,2));
  assert.equal(row.status,'auto_approved');assert.equal(row.autoApproved,true);
  const done=transcript(events)[0];
  assert.equal(done.autoApproved,true);assert.equal(done.status,'completed');
  const [reviewed]=reviewOperations(events);
  assert.deepEqual(reviewed.approval,{decision:'allow_once',auto:true,source:'session'});
  const human=reviewOperations([event(1,'tool.proposed',operation),event(2,'approval.decided',{operation_id:'o1',decision:'allow_once'})])[0];
  assert.equal(human.state,'approved');assert.equal(human.approval.auto,false);
  const change=transcript([event(5,'auto_approve.changed',{session_override:{edits:false,commands:false},effective:{edits:false,commands:false,source:'session'}})])[0];
  assert.equal(change.type,'state');assert.match(change.text,/needs your approval \(this session only\)/);
  assert.match(autoApproveChange({session_override:null,effective:{edits:true,commands:true,source:'project'}}),/edits and commands now run without asking \(following the project setting\)/);
  // A project change recorded in a running session reads as following the project.
  const projectChange=transcript([event(6,'auto_approve.changed',{scope:'project',project_policy:{edits:true,commands:false},effective:{edits:true,commands:false,source:'project'}})])[0];
  assert.match(projectChange.text,/following the project setting/);
});
test('approval labels name what runs without asking',async()=>{
  const { autoApproveLabel, autoApproveTone, effectivePolicy }=await import('../trust.js');
  assert.equal(autoApproveLabel({edits:false,commands:false}),'Writes and commands need approval');
  assert.equal(autoApproveLabel(undefined),'Writes and commands need approval');
  assert.match(autoApproveLabel({edits:true,commands:false}),/^Edits apply without asking/);
  assert.match(autoApproveLabel({edits:false,commands:true}),/^Commands run without asking/);
  assert.equal(autoApproveTone({edits:true,commands:false}),'caution');
  assert.equal(autoApproveTone({edits:true,commands:true}),'stop');
  assert.equal(autoApproveTone({}),'neutral');
  // Before a session exists the project's setting applies; a session view wins once present.
  assert.deepEqual(effectivePolicy({auto_approve:{edits:true,commands:false}},null),{edits:true,commands:false,source:'project'});
  assert.deepEqual(effectivePolicy({auto_approve:{edits:true,commands:true}},{auto_approve:{edits:false,commands:false,source:'session'}}),{edits:false,commands:false,source:'session'});
  assert.deepEqual(effectivePolicy({},null),{edits:false,commands:false,source:'project'});
});
test('the approval dialog warns about project trust even when this session overrides it',async()=>{
  const { trustWarning }=await import('../trust.js');
  assert.equal(trustWarning({edits:false,commands:false},null),null);
  assert.equal(trustWarning({edits:true,commands:false},null),'edits');
  // Project commands reach other sessions and existing-agent runs.
  assert.equal(trustWarning({edits:false,commands:true},{edits:false,commands:false}),'commands');
  assert.equal(trustWarning({edits:false,commands:false},{edits:true,commands:true}),'commands');
  assert.equal(trustWarning(undefined,{edits:true,commands:false}),'edits');
});
