import test from 'node:test';
import assert from 'node:assert/strict';
import { usageWindows, chosenProfile, canStartProfile, validProfileName, foldProfileName, activeLogin, mergeLoginState } from '../account-presentation.js';

test('unknown or failed usage never becomes a zero balance',()=>{
  for(const status of ['unavailable','error',undefined])assert.deepEqual(usageWindows({checked_at_ms:1234,usage:{status,windows:[{remaining_percent:0}]}}),[]);
  assert.deepEqual(usageWindows({checked_at_ms:0,usage:{status:'available',windows:[{remaining_percent:0}]}}),[]);
});
test('authoritative usage preserves zero and missing values separately',()=>{
  const result=usageWindows({checked_at_ms:1234,usage:{status:'available',windows:[{id:'daily',used_percent:100,remaining_percent:0,resets_at:456},{id:'weekly',used_percent:null,remaining_percent:null,resets_at:null},{id:'bad',used_percent:Infinity,remaining_percent:120,resets_at:'soon'}]}});
  assert.equal(result[0].remaining_percent,0);assert.equal(result[0].resets_at,456);assert.equal(result[1].remaining_percent,null);assert.equal(result[2].remaining_percent,null);assert.equal(result[2].resets_at,null);
});
test('future run profile uses exact agent default or explicit selection',()=>{
  const profiles=[{id:'a',agent_id:'codex'},{id:'b',agent_id:'codex'},{id:'c',agent_id:'claude'}];
  assert.equal(chosenProfile(profiles,{codex:'a'},'codex','b').id,'b');assert.equal(chosenProfile(profiles,{codex:'a'},'codex','').id,'a');
  assert.equal(chosenProfile(profiles,{codex:'a'},'codex','c'),null);assert.equal(chosenProfile(profiles,{},'codex',''),null);
});

test('existing CLI profiles can be selected for their own agent without managed sign-in',()=>{
  const profiles=[
    {id:'system-codex',agent_id:'codex',managed:false,account:{auth_status:'unknown'}},
    {id:'system-claude',agent_id:'claude',managed:false,account:{auth_status:'unknown'}},
    {id:'managed-claude',agent_id:'claude',managed:true,account:{auth_status:'signed_in'}},
  ];
  const defaults={codex:'system-codex',claude:'system-claude'};
  assert.equal(chosenProfile(profiles,defaults,'claude','').id,'system-claude');
  assert.equal(chosenProfile(profiles,{...defaults,claude:'managed-claude'},'claude','system-claude').id,'system-claude');
  assert.equal(chosenProfile(profiles,defaults,'codex','system-claude'),null);
  assert.equal(canStartProfile(profiles[1]),true);
});

test('managed run profiles require confirmed sign-in and system unknown preserves CLI integration',()=>{
  assert.equal(canStartProfile(null),false);assert.equal(canStartProfile({managed:true,account:{auth_status:'unknown'}}),false);assert.equal(canStartProfile({managed:true,account:{auth_status:'signed_out'}}),false);assert.equal(canStartProfile({managed:true,account:{auth_status:'signed_in'}}),true);assert.equal(canStartProfile({managed:false,account:{auth_status:'unknown'}}),true);assert.equal(canStartProfile({managed:false,account:{auth_status:'signed_out'}}),false);
});
test('antigravity profiles can start without a verifiable sign-in status',()=>{
  assert.equal(canStartProfile({agent_id:'antigravity',managed:true,account:{auth_status:'unknown'}}),true);
  assert.equal(canStartProfile({agent_id:'antigravity',managed:false,account:{auth_status:'unknown'}}),true);
});
test('profile names accept Unicode limits and reject controls',()=>{
  assert.equal(validProfileName(' Work '),true);assert.equal(validProfileName('😀'.repeat(64)),true);assert.equal(validProfileName('😀'.repeat(65)),false);assert.equal(validProfileName('bad\nname'),false);assert.equal(validProfileName('bad\u0085name'),false);assert.equal(validProfileName('  '),false);
  assert.equal(foldProfileName(' WORK '),'work');assert.notEqual(foldProfileName('É'),foldProfileName('é'));
});
test('only active sign-ins continue polling',()=>{
  assert.equal(activeLogin({status:'starting'}),true);assert.equal(activeLogin({status:'awaiting_user'}),true);for(const status of ['completed','failed','cancelled',undefined])assert.equal(activeLogin({status}),false);
});

test('late cached login responses cannot roll a newer sign-in backwards',()=>{
  const current={profile_id:'a',id:'new',updated_at_ms:20,status:'awaiting_user'};
  assert.equal(mergeLoginState({a:current},[{profile_id:'a',id:'old',updated_at_ms:10,status:'completed'}]).a,current);
  assert.equal(mergeLoginState({a:current},[{...current,updated_at_ms:21,status:'completed'}]).a.status,'completed');
});
