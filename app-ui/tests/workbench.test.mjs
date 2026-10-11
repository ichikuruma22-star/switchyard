import test from 'node:test';
import assert from 'node:assert/strict';
import { inputChunks } from '../terminal.js';
import { parentPath, changeLabel } from '../workbench.js';
import { selectionWriter } from '../workspace-state.js';

test('terminal paste preserves Unicode and control input within byte caps',()=>{
  const input='echo 漢字🙂\r\n'+ 'a'.repeat(12000)+'\x03';
  const chunks=inputChunks(input);
  assert.equal(chunks.join(''),input);
  for(const chunk of chunks)assert.ok(new TextEncoder().encode(chunk).length<=4096);
  assert.equal(inputChunks('').length,0);
});

test('workspace preference writes coalesce changes without racing old requests',async()=>{
  let release;const calls=[];const failures=[];
  const writer=selectionWriter({request:async(_path,{body})=>{calls.push(body);if(calls.length===1)await new Promise(resolve=>{release=resolve;});}},message=>failures.push(message));
  writer({project_id:'first'});writer({project_id:'second'});writer({project_id:'last'});
  assert.equal(calls.length,1);release();await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(calls.map(call=>call.project_id),['first','last']);
  assert.deepEqual(failures,['','']);
});

test('workspace preference failure does not block later selections',async()=>{
  let calls=0;const warnings=[];const writer=selectionWriter({request:async()=>{if(++calls===1)throw new Error('offline');}},message=>warnings.push(message));
  writer({project_id:'one'});await new Promise(resolve=>setTimeout(resolve,0));
  writer({project_id:'two'});await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(warnings,['offline','']);
});

test('file navigation and Git descriptions preserve actual status',()=>{
  assert.equal(parentPath('src/lib/test.rs'),'src/lib');assert.equal(parentPath('src'),'.');
  assert.equal(changeLabel({staged:'M',unstaged:'D'}),'Deleted');
  assert.equal(changeLabel({staged:'A',unstaged:' ',untracked:false}),'Added');
  assert.equal(changeLabel({staged:'?',unstaged:'?',untracked:true}),'Untracked');
});
