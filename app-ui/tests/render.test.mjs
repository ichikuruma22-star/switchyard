import test from 'node:test';
import assert from 'node:assert/strict';
import { html, render } from '../vendor/preact-htm.js';

// A strict DOM fixture runs the real Preact renderer. It deliberately rejects
// invalid element names like a browser, catching HTM errors that JS parsing misses.
class Element {
  constructor(name,type=1){
    if(type===1&&!/^[A-Za-z][A-Za-z0-9:-]*$/.test(name))throw new DOMException(`Invalid element name: ${name}`,'InvalidCharacterError');
    this.localName=name;this.nodeType=type;this.childNodes=[];this.parentNode=null;this.attributes=[];this.style={setProperty(){}};
  }
  get firstChild(){return this.childNodes[0]||null;}
  get nextSibling(){const siblings=this.parentNode?.childNodes||[];return siblings[siblings.indexOf(this)+1]||null;}
  appendChild(node){if(node.parentNode)node.parentNode.removeChild(node);node.parentNode=this;this.childNodes.push(node);return node;}
  insertBefore(node,before){if(node.parentNode)node.parentNode.removeChild(node);node.parentNode=this;const i=this.childNodes.indexOf(before);this.childNodes.splice(i<0?this.childNodes.length:i,0,node);return node;}
  removeChild(node){this.childNodes.splice(this.childNodes.indexOf(node),1);node.parentNode=null;return node;}
  replaceChildren(...nodes){for(const child of [...this.childNodes])this.removeChild(child);for(const node of nodes)this.appendChild(node);}
  setAttribute(name,value){this.removeAttribute(name);this.attributes.push({name,value:String(value)});}
  removeAttribute(name){this.attributes=this.attributes.filter(item=>item.name!==name);}
  addEventListener(){}
  removeEventListener(){}
}
const flatten=node=>[node,...node.childNodes.flatMap(flatten)];
const memoryStorage=()=>{const map=new Map();return{getItem:key=>map.get(key)||null,setItem:(key,value)=>map.set(key,String(value)),removeItem:key=>map.delete(key)};};
const textIn=node=>flatten(node).filter(item=>item.nodeType===3).map(item=>item.data).join('');
// Collapsed <details> content is not what a reviewer reads first.
const visibleText=node=>node.localName==='details'?'':node.nodeType===3?node.data:node.childNodes.map(visibleText).join('');
const attribute=(node,name)=>node?.attributes.find(item=>item.name===name)?.value;
const buttonLabelled=(root,label)=>flatten(root).find(node=>node.localName==='button'&&textIn(node).includes(label));
const taskInput=root=>flatten(root).find(node=>attribute(node,'id')==='task-input');
// Preact keeps each element's listeners in `l`; call one as the browser would.
function fire(node,type,event={}) {
  const key=Object.keys(node?.l||{}).find(name=>name.toLowerCase()===`${type}false`);
  assert.ok(key,`${node?.localName} must handle ${type}`);
  return node.l[key]({type,preventDefault(){},target:node,currentTarget:node,...event});
}
// Hook effects run after a timer in Node, so interactions wait for their rendered outcome.
async function until(check,label) {
  for(let attempt=0;attempt<250;attempt++){if(check())return;await new Promise(resolve=>setTimeout(resolve,20));}
  assert.fail(`Timed out waiting for ${label}`);
}
function installDom(root) {
  globalThis.document={createElement:name=>new Element(name),createElementNS:(_,name)=>new Element(name),createTextNode:text=>Object.assign(new Element('#text',3),{data:text}),getElementById:()=>root,activeElement:null};
  globalThis.location={hash:'#token=render-fixture-token',pathname:'/',search:''};
  globalThis.history={state:null,replaceState(){}};
  globalThis.sessionStorage=memoryStorage();globalThis.localStorage=memoryStorage();
  globalThis.window={addEventListener(){},removeEventListener(){},dispatchEvent(){}};
  globalThis.matchMedia=()=>({matches:false,addEventListener(){}});
}

test('workspace, review and adapter render with the shipped HTM/Preact runtime',async()=>{
  const root=new Element('div');
  const loadingFallback=new Element('main');root.appendChild(loadingFallback);
  installDom(root);
  // First render is synchronous; no backend is simulated or called by this test.
  await import('../app.js');
  assert.equal(loadingFallback.parentNode,null,'the static loading fallback must be removed at mount');
  let tags=flatten(root).map(node=>node.localName);
  assert.ok(tags.includes('main'));assert.equal(tags.filter(tag=>tag==='aside').length,2);
  assert.ok(tags.includes('textarea'));assert.ok(tags.includes('dialog'));
  render(null,root);
  const { AdapterWorkspace }=await import('../adapters.js');
  render(html`<${AdapterWorkspace} api=${{request(){throw Error('Network should not run during synchronous render');}}} project=${null} hostId="render-fixture" storage=${memoryStorage()} onNative=${()=>{}} onOpenProject=${()=>{}} onOpenNavigation=${()=>{}}/>`,root);
  tags=flatten(root).map(node=>node.localName);
  assert.ok(tags.includes('main'));assert.ok(tags.includes('aside'));assert.ok(tags.includes('textarea'));
  render(null,root);
  const { CodexItem, ClaudeItem, ApprovalSummary }=await import('../adapters.js');
  const textOf=()=>flatten(root).filter(node=>node.nodeType===3).map(node=>node.data).join('');
  const changes=[{path:'src/a.js',kind:{type:'update',move_path:null},diff:'@@ -1 +1 @@\n-a\n+b\n'},{path:'src/new.js',kind:{type:'add'},diff:'export const x=1;\n'}];
  render(html`<div><${CodexItem} entry=${{item:{id:'c',type:'commandExecution',command:'npm test',cwd:'/p',status:'completed',exitCode:1,aggregatedOutput:'1 failing'}}}/><${CodexItem} entry=${{item:{id:'f',type:'fileChange',status:'completed',changes}}}/><${ApprovalSummary} approval=${{method:'item/commandExecution/requestApproval',preview:{params:{command:'curl https://example.com',cwd:'/p',reason:'fetch docs',networkApprovalContext:{host:'example.com',protocol:'https'}},item:null}}}/><${ApprovalSummary} approval=${{method:'item/fileChange/requestApproval',preview:{params:{grantRoot:'/'},item:{id:'f',type:'fileChange',changes}}}}/></div>`,root);
  tags=flatten(root).map(node=>node.localName);
  assert.ok(tags.includes('pre'));assert.ok(tags.includes('details'));
  const shown=textOf();
  for(const expected of ['npm test','1 failing','src/a.js','src/new.js','New file','Allow network access to example.com?','curl https://example.com','fetch docs','Broader write access requested'])assert.ok(shown.includes(expected),expected);
  render(null,root);
  render(html`<div><${ClaudeItem} entry=${{item:{id:'claude-tool',type:'claudeTool',name:'Write',input:{file_path:'fixture.txt',content:'<script>literal</script>'},status:'completed',output:'File created'}}}/><${ApprovalSummary} approval=${{method:'claude/can_use_tool',can_allow_once:true,preview:{request:{tool_name:'Edit',description:'Update the fixture',input:{file_path:'fixture.txt',old_string:'old text',new_string:'new text'}}}}}/><${ApprovalSummary} approval=${{method:'claude/can_use_tool',can_allow_once:false,preview:{request:{tool_name:'AskUserQuestion',input:{questions:[]}}}}}/></div>`,root);
  const claudeShown=textOf();
  for(const expected of ['fixture.txt','<script>literal</script>','File created','Allow Claude Code to use Edit?','old text','new text','Exact tool input','ordinary chat'])assert.ok(claudeShown.includes(expected),expected);
  assert.ok(!flatten(root).some(node=>node.localName==='script'),'tool content must remain text');
  render(null,root);
  // Claude Code runs shell work through PowerShell on this host: its command must be
  // readable and copyable in the approval, exactly like Bash.
  render(html`<div><${ApprovalSummary} approval=${{method:'claude/can_use_tool',can_allow_once:true,preview:{request:{tool_name:'PowerShell',description:'List the project',input:{command:'Get-ChildItem -Path E:\\projects'}}}}}/><${ClaudeItem} entry=${{item:{id:'ps',type:'claudeTool',name:'PowerShell',input:{command:'Get-ChildItem -Path E:\\projects'},status:'completed',output:'adapters.js'}}}/></div>`,root);
  const shellShown=textOf();
  for(const expected of ['Allow Claude Code to use PowerShell?','Get-ChildItem -Path E:\\projects','Copy command','Exact tool input','adapters.js'])assert.ok(shellShown.includes(expected),expected);
  render(null,root);
  const { AccountsDialog }=await import('../accounts.js');
  render(html`<${AccountsDialog} api=${{}} open=${false} onRefresh=${()=>{}} profiles=${[
    {id:'c',name:'Codex profile',agent_id:'codex',managed:true,account:{auth_status:'signed_in',email:'fixture@example.test',plan:'Fixture',checked_at_ms:1234,usage:{status:'available',windows:[{id:'daily',name:'Daily',remaining_percent:0,used_percent:100,resets_at:2345}]}}},
    {id:'a',name:'Claude profile',agent_id:'claude',managed:true,account:{auth_status:'signed_out',checked_at_ms:1234,usage:{status:'unavailable'}}},
  ]}/>`,root);
  tags=flatten(root).map(node=>node.localName);assert.equal(tags.filter(tag=>tag==='meter').length,1);assert.ok(tags.includes('dialog'));assert.ok(tags.includes('input'));
  assert.ok(textOf().includes('Remove profile'),'managed profiles offer removal');
  assert.ok(textOf().includes('Anthropic Console'),'signed-out profiles offer sign-in methods');
  render(null,root);
  for(const [agent,name,label] of [['codex','Existing Codex CLI','Codex'],['claude','Existing Claude CLI','Claude Code']]){
    render(html`<${AccountsDialog} api=${{}} open=${false} onRefresh=${()=>{}} profiles=${[
      {id:`system-${agent}`,name,agent_id:agent,managed:false,read_only:true,account:{auth_status:'signed_out',usage:{status:'unavailable'}}},
    ]}/>`,root);
    assert.ok(textOf().includes(name));
    assert.ok(textOf().includes(`Uses the existing ${label} CLI sign-in and configuration.`));
    const buttons=flatten(root).filter(node=>node.localName==='button').map(node=>flatten(node).filter(item=>item.nodeType===3).map(item=>item.data).join(''));
    assert.ok(buttons.includes('Refresh account'));
    assert.ok(buttons.includes('Use as default'));
    assert.ok(!buttons.includes('Sign in'));
    assert.ok(!buttons.includes('Open Claude Code'));
    assert.ok(!buttons.includes('Remove profile'));
    render(null,root);
  }
  assert.throws(()=>render(html`<><span>Unsupported fragment</span></>`,root),{name:'InvalidCharacterError'});
  render(null,root);
});

test('an open conversation that cannot continue never starts a separate run from the keyboard',async()=>{
  const root=new Element('div');installDom(root);
  const { AdapterWorkspace }=await import('../adapters.js');
  const project={id:'p',name:'Project',root:'/work/project'};
  // Both agents are usable, so a fresh start would otherwise go to the first one (Codex).
  const adapters=[{id:'codex',name:'Codex',installed:true,supported:true,status:'available'},{id:'claude',name:'Claude Code',installed:true,supported:true,status:'available'}];
  const profiles=[{id:'system-codex',name:'Existing Codex CLI',agent_id:'codex',managed:false,account:{auth_status:'unknown'}},{id:'system-claude',name:'Existing Claude CLI',agent_id:'claude',managed:false,account:{auth_status:'unknown'}}];
  const claudeRun=fields=>({id:'claude-run',conversation_id:'claude-run',adapter_id:'claude',profile_id:'system-claude',profile_name:'Existing Claude CLI',project_path:project.root,state:'completed',thread_id:'thread',turn_id:'turn',started_at_ms:1,last_seq:0,pending_approvals:[],...fields});
  async function pressCtrlEnter(run) {
    const posts=[];
    const api={async request(path,{method='GET',body}={}){
      if(method==='POST'){posts.push({path,body});return {run:{...run,id:'next-run',state:'starting'}};}
      if(path==='/adapters')return {adapters};
      if(path==='/adapter-runs')return {runs:[run]};
      if(path===`/adapter-runs/${run.id}`)return {run};
      return {events:[]};
    }};
    render(html`<${AdapterWorkspace} api=${api} project=${project} hostId="keyboard-fixture" storage=${memoryStorage()} profiles=${profiles} accountDefaults=${{codex:'system-codex',claude:'system-claude'}} conversationId=${run.conversation_id} onNative=${()=>{}} onOpenProject=${()=>{}} onOpenNavigation=${()=>{}}/>`,root);
    try{
      await until(()=>/Finish or review|Continues the saved/.test(textIn(root)),'the open conversation');
      fire(taskInput(root),'input',{currentTarget:{value:'Follow up'}});
      await until(()=>attribute(taskInput(root),'value')==='Follow up','the typed follow-up');
      fire(taskInput(root),'keydown',{key:'Enter',ctrlKey:true});
      if(posts.length)await until(()=>attribute(taskInput(root),'value')==='','the confirmed start');
      return posts;
    }finally{render(null,root);}
  }
  for(const [label,run] of [['running',claudeRun({state:'running'})],['awaiting approval',claudeRun({state:'awaiting_approval'})],['recovery review',claudeRun({state:'recovery_required'})],['unsaved',claudeRun({ephemeral:true})]])
    assert.deepEqual(await pressCtrlEnter(run),[],`${label} conversation must not start another run`);
  const continued=await pressCtrlEnter(claudeRun({}));
  assert.equal(continued.length,1);
  assert.equal(continued[0].body.continue_run_id,'claude-run');
  assert.equal(continued[0].body.adapter_id,'claude');
});

test('a Claude Code Edit approval states when it replaces every occurrence',async()=>{
  const root=new Element('div');installDom(root);
  const { ApprovalSummary }=await import('../adapters.js');
  for(const [replaceAll,expected] of [[true,true],[false,false],[undefined,false]]){
    const input={file_path:'src/config.js',old_string:'debug: false',new_string:'debug: true',...(replaceAll===undefined?{}:{replace_all:replaceAll})};
    render(html`<div><${ApprovalSummary} approval=${{method:'claude/can_use_tool',can_allow_once:true,preview:{request:{tool_name:'Edit',input}}}}/></div>`,root);
    const shown=visibleText(root);
    assert.ok(shown.includes('debug: false')&&shown.includes('debug: true'));
    assert.equal(shown.includes('every occurrence'),expected,`replace_all=${replaceAll}`);
    render(null,root);
  }
});

test('a task sent from New session is not offered again by the next New session',async()=>{
  const root=new Element('div');installDom(root);
  const project={id:'P',name:'Project',root:'/work/project'};
  const existing={id:'S1',title:'Existing session',state:'completed',model:'fixture-model',updated_at_ms:2,last_seq:0,revision:1};
  const created={id:'S2',title:'Refactor X',state:'idle',model:'fixture-model',updated_at_ms:3,last_seq:0,revision:1};
  const turns=[],originalFetch=globalThis.fetch;
  globalThis.fetch=async(url,{method,body})=>{
    const path=url.replace(/^\/api/,''),reply=value=>({ok:true,status:200,json:async()=>value});
    if(method==='POST'&&path==='/sessions')return reply({session:created});
    if(method==='POST'&&path.endsWith('/turns')){turns.push(JSON.parse(body));return reply({});}
    if(method==='POST')return reply({});
    if(path==='/status')return reply({version:'fixture'});
    if(path==='/models')return reply({models:[{id:'fixture-model',owned_by:'fixture'}]});
    if(path==='/projects')return reply({projects:[project]});
    if(path==='/workspace')return reply({selection:{project_id:'P',session_id:'',mode:'native',tool:'conversation'}});
    if(path.startsWith('/sessions?'))return reply({sessions:turns.length?[created,existing]:[existing]});
    if(path.startsWith('/sessions/'))return reply({session:path.startsWith('/sessions/S2')?created:existing,project,pending_operation:null,events:[]});
    return reply({});
  };
  const sessionButton=title=>flatten(root).find(node=>attribute(node,'class')==='session-button'&&textIn(node).includes(title));
  const current=title=>attribute(sessionButton(title),'aria-current')==='page';
  const newSession=async()=>{fire(buttonLabelled(root,'New session'),'click');await until(()=>!current('Existing session')&&!current('Refactor X'),'the new-session composer');};
  try{
    // A separate module instance mounts its own App against this fixture backend.
    await import('../app.js?draft-fixture');
    await until(()=>sessionButton('Existing session')&&taskInput(root)&&!attribute(taskInput(root),'disabled'),'the native workspace');
    fire(taskInput(root),'input',{currentTarget:{value:'Refactor X'}});
    await until(()=>attribute(taskInput(root),'value')==='Refactor X','the typed draft');
    fire(sessionButton('Existing session'),'click');
    await until(()=>current('Existing session'),'the existing session');
    await newSession();
    assert.equal(attribute(taskInput(root),'value'),'Refactor X','an unsent new-session draft survives navigation');
    fire(flatten(root).find(node=>node.localName==='form'&&attribute(node,'class')==='composer'),'submit');
    await until(()=>turns.length===1&&buttonLabelled(root,'Send task')&&!attribute(buttonLabelled(root,'New session'),'disabled'),'the confirmed task');
    assert.equal(turns[0].text,'Refactor X');
    fire(sessionButton('Existing session'),'click');
    await until(()=>current('Existing session'),'the existing session again');
    await newSession();
    assert.equal(attribute(taskInput(root),'value'),'','the sent task must not return to the composer');
  }finally{render(null,root);globalThis.fetch=originalFetch;}
});

test('a changed file shows its current diff after a transient refresh failure',async()=>{
  const root=new Element('div');installDom(root);
  const { Workbench }=await import('../workbench.js');
  const { ApiError }=await import('../api.js');
  for(const [change,endpoint,detail,expected] of [
    [{path:'src/a.js',staged:' ',unstaged:'M',untracked:false,protected:false},'/diff?',{diff:'@@ -1 +1 @@\n-old line\n+new line\n'},'new line'],
    [{path:'notes.txt',staged:'?',unstaged:'?',untracked:true,protected:false},'/file?',{path:'notes.txt',content:'fresh preview',truncated:false},'fresh preview'],
  ]){
    let detailCalls=0;
    const api={async request(path){
      if(path.endsWith('/changes'))return {available:true,branch:'main',changes:[change]};
      if(!path.includes(endpoint))throw new Error(`Unexpected request ${path}`);
      if(++detailCalls===1)throw new ApiError('busy','Project inspection is busy. Try again shortly.',{status:429});
      return detail;
    }};
    render(html`<${Workbench} api=${api} project=${{id:'p',name:'Project'}} tool="changes" onToolChange=${()=>{}}/>`,root);
    try{
      await until(()=>textIn(root).includes('Project inspection is busy'),'the transient failure');
      // The same file and scope are refreshed, as the five-second poll does.
      fire(buttonLabelled(root,'Refresh changes'),'click');
      await until(()=>detailCalls===2&&textIn(root).includes(expected),`the refreshed ${change.path}`);
      assert.ok(!textIn(root).includes('Project inspection is busy'),'a successful refresh replaces the earlier error');
    }finally{render(null,root);}
  }
});
