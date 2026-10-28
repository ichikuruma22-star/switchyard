import { html, useEffect, useLayoutEffect, useRef, useState } from './vendor/preact-htm.js';
import { Icon } from './icons.js';
import { Button, Notice, Dialog, RawValue, TextContent, CopyButton, SignalLight, TurnRule, useFocusRecovery } from './components.js';
import { DiffView } from './diff.js';
import { stateLabel, stateTone, timeLabel, fullTime, pretty, displayPath } from './presentation.js';
import { readAdapterJournal, writeAdapterJournal } from './adapter-journal.js';
import { chosenProfile, canStartProfile, authLabel, agentName } from './account-presentation.js';
import { autoApproveLabel, autoApproveTone } from './trust.js';

const enc=encodeURIComponent;
const active=state=>['starting','running','awaiting_approval','interrupting'].includes(state);
const errorText=error=>error?.message||'The existing agent could not complete this request.';
const availabilityLabel=status=>({available:'Available',not_installed:'Not installed',integration_not_implemented:'No workspace integration',version_check_failed:'Version check failed'}[status]||'Status unavailable');
export function mergeAdapterEvents(previous,incoming,runId) {
  const bySeq=new Map();for(const event of [...previous,...incoming])if(event.run_id===runId&&Number.isSafeInteger(event.seq))bySeq.set(event.seq,event);
  return [...bySeq.values()].sort((a,b)=>a.seq-b.seq).slice(-600);
}
const LIFECYCLE_FIELDS=['id','type','status'];
const emptyValue=value=>value===null||value===undefined||value===''||(Array.isArray(value)&&!value.length)||(typeof value==='object'&&!Array.isArray(value)&&!Object.keys(value).length);
/** True when an item event only announces an item's existence: nothing to read.
 * A reported failure or decline is an outcome, so it is never hidden. */
export function lifecycleOnlyItem(item) {
  if(!item||typeof item!=='object')return false;
  if(item.status&&!['inProgress','completed'].includes(item.status))return false;
  return Object.entries(item).every(([key,value])=>LIFECYCLE_FIELDS.includes(key)||emptyValue(value));
}
// Progress states are shown by the header, the live row and the status region. A run's
// end state is shown once: by its turn report when the agent sent one, else by the state.
const PROGRESS_STATES=['starting','running','awaiting_approval','interrupting'];
export function adapterTranscript(events) {
  const items=[],streams=new Map(),codexItems=new Map();
  const itemEvent=event=>event.kind==='item_started'||event.kind==='item_completed';
  const reported=new Set(events.filter(event=>event.kind==='turn_completed').map(event=>event.payload?.status));
  for(const event of events){
    const p=event.payload||{};
    if(event.kind==='state_changed'){
      if(PROGRESS_STATES.includes(p.state)||reported.has(p.state))continue;
      items.push({...event,type:'activity'});
    }
    else if(event.kind==='user_task')items.push({...event,type:'user',text:typeof p.text==='string'?p.text:''});
    else if(event.kind==='assistant_delta'){
      const key=p.item_id||'assistant';let item=streams.get(key);
      if(!item){item={...event,type:'assistant',text:''};streams.set(key,item);items.push(item);}
      item.text+=typeof p.text==='string'?p.text:'';
    }else if(itemEvent(event)&&p.item?.type==='agentMessage'){
      // One bubble per message: its deltas, started announcement and completion all
      // carry the same item ID. A completion's nonempty text is the final message, even
      // when it corrects streamed text to something shorter; an empty text is lifecycle
      // only and never erases text. A started announcement only adds text the deltas
      // have not delivered yet.
      const id=typeof p.item.id==='string'?p.item.id:'',key=id||'assistant';
      const stream=streams.get(key),text=typeof p.item.text==='string'?p.item.text:'';
      if(stream){if(text&&(event.kind==='item_completed'||text.length>stream.text.length))stream.text=text;}
      else if(text){const started={...event,type:'assistant',text};streams.set(key,started);items.push(started);}
      // A real ID keeps its bubble, so a repeated completion updates it in place.
      if(event.kind==='item_completed'&&!id)streams.delete(key);
    }else if(itemEvent(event)&&['commandExecution','fileChange','claudeTool'].includes(p.item?.type)){
      // One entry per tool: its completion replaces the started view in place.
      const key=typeof p.item.id==='string'?p.item.id:'';const known=key&&codexItems.get(key);
      if(known)Object.assign(known,{kind:event.kind,item:p.item,last_seq:event.seq});
      else{const entry={...event,type:p.item.type==='claudeTool'?'claude_item':'codex_item',item:p.item};items.push(entry);if(key)codexItems.set(key,entry);}
    }else if(event.kind==='approval_resolved')items.push({...event,type:'decision',decision:p.decision,auto:p.decided_by==='auto_approve'});
    // Started/completed pairs that report only an ID and type (for example an
    // empty reasoning step) are duplicate lifecycle noise around real activity.
    else if(!(itemEvent(event)&&lifecycleOnlyItem(p.item)))items.push({...event,type:'activity'});
  }
  return items;
}
/** Each run's transcript, with a repeated agent notice shown only where it first appeared
 * in the conversation. Errors and recovery reviews are outcomes and always stay. */
export function conversationTranscript(blocks) {
  const seen=new Set();
  return blocks.map(block=>({...block,items:block.events===null?null:adapterTranscript(block.events).filter(item=>{
    if(item.kind!=='notice')return true;
    const key=String(item.payload?.message??'');
    if(seen.has(key))return false;
    seen.add(key);return true;
  })}));
}
/** Runs that continue one saved agent session, oldest first; newest conversation first. */
export function groupConversations(runs) {
  const groups=new Map();
  for(const run of runs){const key=run.conversation_id||run.id;if(!groups.has(key))groups.set(key,[]);groups.get(key).push(run);}
  return [...groups.entries()].map(([id,items])=>{
    const ordered=[...items].sort((a,b)=>a.started_at_ms-b.started_at_ms||String(a.id).localeCompare(String(b.id)));
    return {id,runs:ordered,latest:ordered.at(-1),title:String(ordered[0].title||'').trim()};
  }).sort((a,b)=>b.latest.started_at_ms-a.latest.started_at_ms);
}
/** The run a new message continues: the latest run attached to the saved thread. */
export function continuationTarget(conversation) {
  if(!conversation||conversation.runs.some(run=>active(run.state)))return null;
  // Require a confirmed turn before resuming CLI history; the host enforces this too.
  if(!conversation.runs.some(run=>run.turn_id))return null;
  const target=conversation.runs.filter(run=>run.thread_id).at(-1);
  return target&&!target.ephemeral&&target.state!=='recovery_required'?target:null;
}
/** Why a finished conversation cannot be continued, or '' when it can. */
export function continuationBlocker(conversation) {
  if(!conversation||continuationTarget(conversation)||conversation.runs.some(run=>active(run.state)))return '';
  const target=conversation.runs.filter(run=>run.thread_id).at(-1);
  if(target?.state==='recovery_required')return 'review';
  if(!target)return 'The agent did not create a conversation for this task. Start a new conversation.';
  if(target.ephemeral)return 'This conversation was not saved, so it cannot be continued. Start a new conversation.';
  return 'The agent never confirmed a message in this conversation, so it may not have saved it. Start a new conversation.';
}

// Field names follow the Codex 0.159.3 app-server schema: ThreadItem
// commandExecution/fileChange, FileUpdateChange and the approval params.
const ITEM_STATUS={inProgress:'Running',completed:'Completed',failed:'Failed',declined:'Declined'};
const text=value=>typeof value==='string'?value:'';
const humanize=value=>{const words=text(value).replace(/([a-z0-9])([A-Z])/g,'$1 $2').toLowerCase();return words?words[0].toUpperCase()+words.slice(1):'Activity';};
/** One file change as Codex reported it: its path, kind and exact change text. */
export function fileChangeSummary(change) {
  const kind=change?.kind?.type,move=text(change?.kind?.move_path);
  const label=kind==='add'?'New file':kind==='delete'?'Delete':kind==='update'?(move?'Move and edit':'Edit'):'Change';
  const diff=text(change?.diff);
  return {path:text(change?.path)||'Path not reported',label,move,diff,unified:/^@@ /m.test(diff)};
}
/** Concise, verbatim summary of a Codex command or file-change item. */
export function codexItemSummary(item) {
  const status=text(item?.status),statusLabel=ITEM_STATUS[status]||(status?humanize(status):'Status not reported');
  if(item?.type==='commandExecution'){
    const exitCode=Number.isInteger(item.exitCode)?item.exitCode:null;
    const tone=status==='failed'||(exitCode!==null&&exitCode!==0)?'stop':status==='declined'?'caution':'neutral';
    return {kind:'command',title:'Command',command:text(item.command),cwd:text(item.cwd),status,statusLabel,tone,exitCode,
      output:typeof item.aggregatedOutput==='string'?item.aggregatedOutput:null};
  }
  if(item?.type==='fileChange'){
    const changes=(Array.isArray(item.changes)?item.changes:[]).map(fileChangeSummary);
    return {kind:'files',title:changes.length===1?'Edit 1 file':`Edit ${changes.length} files`,changes,status,statusLabel,
      tone:status==='failed'?'stop':status==='declined'?'caution':'neutral'};
  }
  return {kind:'other',title:humanize(item?.type),status,statusLabel,tone:'neutral'};
}
/** What a pending approval asks for, using only fields the CLI supplied. */
export function approvalSummary(approval) {
  if(approval?.method==='claude/can_use_tool'){
    const request=approval.preview?.request||{},name=text(request.tool_name)||'Tool';
    return {kind:'claude',title:`Allow Claude Code to use ${name}?`,name,input:request.input||{},reason:text(request.description)};
  }
  const params=approval?.preview?.params||{},reason=text(params.reason);
  if(approval?.method==='item/commandExecution/requestApproval'){
    const network=params.networkApprovalContext&&text(params.networkApprovalContext.host)?params.networkApprovalContext:null;
    const stdin=params.kind==='writeStdin';
    const title=network?`Allow network access to ${network.host}?`:stdin?'Send input to a running command?':'Run this command?';
    return {kind:'command',title,command:text(params.command),cwd:text(params.cwd),reason,stdin,
      network:network&&{host:network.host,protocol:text(network.protocol)}};
  }
  if(approval?.method==='item/fileChange/requestApproval'){
    const item=approval.preview?.item;
    const changes=(item?.type==='fileChange'&&Array.isArray(item.changes)?item.changes:[]).map(fileChangeSummary);
    const grantRoot=text(params.grantRoot);
    return {kind:'files',title:changes.length===1?'Apply this file change?':changes.length?`Apply these ${changes.length} file changes?`:'Apply file changes?',
      changes,reason,grantRoot};
  }
  return {kind:'other',title:humanize(String(approval?.method||'').split('/').at(-2)),reason};
}

function CommandBlock({command,cwd,label='Exact command'}) {
  return html`<pre class="command-preview" tabindex="0" aria-label=${label}><code>${command||'Command not reported'}</code></pre><dl class="command-location"><dt>Working directory</dt><dd><code>${cwd||'Not reported'}</code></dd></dl>`;
}
function FileChanges({changes,verb='Proposed'}) {
  if(!changes.length)return html`<${Notice} title="File changes not reported" tone="caution">Review the exact Codex request details before deciding.<//>`;
  return html`<div class="patch-review">${changes.map((change,index)=>html`<section class="file-review" key=${`${change.path}:${index}`}><h4><${Icon} name="edit"/><code>${change.path}</code><span class="field-hint">${change.label}${change.move?html` → <code>${change.move}</code>`:''}</span></h4>${change.unified?html`<${DiffView} text=${change.diff} label=${`${verb} changes to ${change.path}`}/>`:change.diff?html`<${RawValue} value=${change.diff} label=${`Exact change text for ${change.path}`}/>`:html`<p class="field-hint">Codex did not include change text for this file.</p>`}</section>`)}</div>`;
}
export function ApprovalSummary({approval}) {
  const summary=approvalSummary(approval);
  if(summary.kind==='claude')return html`<h3>${summary.title}</h3>${summary.reason&&html`<p class="field-hint">${summary.reason}</p>`}<${ClaudeToolInput} name=${summary.name} input=${summary.input}/>${!approval.can_allow_once&&html`<p class="field-hint">This interaction needs a structured answer or permission-mode change. Deny it and ask Claude to continue in ordinary chat.</p>`}`;
  return html`<h3>${summary.title}</h3>${summary.reason&&html`<p class="field-hint">Codex's stated reason: ${summary.reason}</p>`}${summary.kind==='command'&&html`<div class="command-review">${summary.network&&html`<${Notice} title="Network access request" tone="caution">Codex asks to reach <code>${summary.network.host}</code>${summary.network.protocol?` over ${summary.network.protocol}`:''}.<//>`}<${CommandBlock} command=${summary.command} cwd=${summary.cwd} label=${summary.stdin?'Command receiving input':'Exact proposed command'}/>${summary.command&&html`<${CopyButton} text=${summary.command} label="Copy command"/>`}</div>`}${summary.kind==='files'&&html`${summary.grantRoot&&html`<${Notice} title="Broader write access requested" tone="stop">Codex also asks to write under <code>${summary.grantRoot}</code> for the rest of the session. Switchya only allows single operations, so this request can only be denied.<//>`}<${FileChanges} changes=${summary.changes}/>`}`;
}
export function CodexItem({entry}) {
  const s=codexItemSummary(entry.item);
  const headline=s.kind==='command'?s.command:s.changes.map(change=>change.path).join(', ');
  return html`<details class="operation-history codex-item"><summary><${Icon} name=${s.kind==='files'?'edit':'command'}/><span><strong>${s.title}</strong> <code class="codex-item-headline">${headline||'Not reported'}</code></span><span class="status-label" data-tone=${s.tone}>${s.statusLabel}</span></summary>${s.kind==='command'?html`<${CommandBlock} command=${s.command} cwd=${s.cwd}/>${s.exitCode!==null?html`<p class="exit-status">Exit code <code>${s.exitCode}</code></p>`:s.status&&s.status!=='inProgress'&&html`<p class="exit-status field-hint">Exit code not reported.</p>`}${s.output?html`<${RawValue} value=${s.output} label="Command output reported by Codex"/>`:html`<p class="field-hint">${s.output===null?'Codex did not report output.':'No output was reported.'}</p>`}`:html`<${FileChanges} changes=${s.changes} verb="Recorded"/>`}<details class="operation-details result-details"><summary>Details (exact Codex item)</summary><${RawValue} value=${entry.item} label="Exact Codex item"/></details></details>`;
}

// Claude Code runs shell work through Bash or PowerShell depending on the host.
const SHELL_TOOLS=['Bash','PowerShell'];
// Antigravity reports its own tool names (view_file) through the same item type.
const TOOL_LABELS={Read:'Read file',view_file:'Read file',Write:'Write file',Edit:'Edit file',MultiEdit:'Edit file',NotebookEdit:'Edit notebook',Bash:'Run command',PowerShell:'Run command',Grep:'Search files',Glob:'Find files',LS:'List folder',WebFetch:'Fetch web page',WebSearch:'Search the web'};
/** A readable label for a transcript row; an unknown tool keeps its exact name. */
export const toolLabel=name=>Object.hasOwn(TOOL_LABELS,name)?TOOL_LABELS[name]:name;
const TOOL_ICONS={'Read file':'file','Write file':'edit','Edit file':'edit','Edit notebook':'edit','Search files':'search','Find files':'search','List folder':'folder','Fetch web page':'external','Search the web':'search'};
// Edit's replace_all turns one replacement into every match, so the summary states it.
function ClaudeToolInput({name,input}) {
  const path=text(input.file_path)||text(input.notebook_path);
  return html`<div class="command-review">${path&&html`<dl class="command-location"><dt>File</dt><dd><code>${path}</code></dd></dl>`}${SHELL_TOOLS.includes(name)&&text(input.command)&&html`<${RawValue} value=${input.command} label=${`Exact Claude Code ${name} command`}/><${CopyButton} text=${input.command} label="Copy command"/>`}${name==='Write'&&typeof input.content==='string'&&html`<${RawValue} value=${input.content} label="Proposed file contents"/>`}${name==='Edit'&&html`${input.replace_all===true&&html`<${Notice} title="Replaces every occurrence" tone="caution">This edit replaces every match of the text below in the file, not just one.<//>`}${typeof input.old_string==='string'&&html`<p class="field-hint">Replace this exact text</p><${RawValue} value=${input.old_string} label="Text to replace"/>`}${typeof input.new_string==='string'&&html`<p class="field-hint">With this text</p><${RawValue} value=${input.new_string} label="Replacement text"/>`}`}<details class="operation-details"><summary>Exact tool input</summary><dl><dt>Tool</dt><dd><code>${name}</code></dd></dl><${RawValue} value=${input} label="Exact Claude Code tool input"/></details></div>`;
}

export function ClaudeItem({entry}) {
  const item=entry.item,status=text(item.status),name=text(item.name)||'Claude Code tool';
  const headline=text(item.input?.file_path)||text(item.input?.command);
  return html`<details class="operation-history codex-item"><summary><${Icon} name=${TOOL_ICONS[toolLabel(name)]||'command'}/><span><strong>${toolLabel(name)}</strong>${headline&&html` <code class="codex-item-headline">${headline}</code>`}</span><span class="status-label" data-tone=${status==='failed'?'stop':'neutral'}>${ITEM_STATUS[status]||'Status not reported'}</span></summary><${ClaudeToolInput} name=${name} input=${item.input||{}}/>${typeof item.output==='string'&&html`<${RawValue} value=${item.output} label="Tool output reported by Claude Code"/>`}</details>`;
}

function AdapterReview({run,onDecision,busy,error,connected}) {
  const approvals=run?.pending_approvals||[],agent=agentName(run?.adapter_id)||'Agent';
  const heading=useRef(null);
  const trackFocus=useFocusRecovery(approvals.map(approval=>approval.id).join(','),heading);
  const scope=run?.adapter_id==='claude'
    ?"Allow once approves only the tool input shown. Existing CLI permission rules still apply. Tools run with your OS permissions."
    :run?.adapter_id==='antigravity'
    ?"Antigravity runs in plan mode: it inspects and answers but cannot apply edits, so there are no approval requests. Tools run with your OS permissions."
    :"Allow once permits the displayed command or file change with your OS permissions. The CLI enforces its own sandbox.";
  return html`<header class="review-header"><div><p class="eyebrow">REVIEW</p><h2 ref=${heading} tabindex="-1">Agent requests</h2></div><${SignalLight} count=${approvals.length}/></header><div class="review-content" onfocusin=${trackFocus}>
    ${error&&html`<${Notice} title="Decision could not be confirmed" tone="stop">${error}<//>`}
    ${approvals.map(approval=>html`<section class="approval" key=${approval.id}>
      <p class="eyebrow">${agent} REQUEST</p><${ApprovalSummary} approval=${approval}/>
      <p class="field-hint">${scope}</p>
      ${!approval.can_allow_once&&html`<p class="field-hint">This request cannot be approved once. Deny it to let the agent respond.</p>`}
      <div class="approval-actions"><${Button} onClick=${()=>onDecision(approval,'deny')} disabled=${busy||!connected}>Deny<//><${Button} variant="primary" onClick=${()=>onDecision(approval,'allow_once')} disabled=${busy||!connected||!approval.can_allow_once}>Allow once<//></div>
      <details class="operation-details"><summary>Exact ${agent} request details</summary><${RawValue} value=${approval.preview} label=${`Exact ${agent} approval request`}/><dl><dt>Method</dt><dd><code>${approval.method}</code></dd><dt>Request</dt><dd><code>${approval.id}</code></dd><dt>Expected hash</dt><dd><code>${approval.expected_hash}</code></dd></dl><${CopyButton} text=${pretty(approval.preview)} label="Copy full request"/></details>
      <details class="operation-details"><summary>Agent permissions</summary><p>${approval.permission_boundary}</p></details>
    </section>`)}
    ${!approvals.length&&html`<div class="empty-review"><${Icon} name="lock"/><h3>Requests appear here.</h3><p>Review the exact tool input, command, or change supplied by the agent.</p></div>`}
    ${run&&html`<details class="operation-details"><summary>Run details and permissions</summary><dl><dt>Run</dt><dd><code>${run.id}</code></dd><dt>Model</dt><dd><code>${run.model||'Reported after the agent starts'}</code></dd><dt>Account</dt><dd>${run.profile_name||'Not reported'}</dd>${run.profile_id&&html`<dt>Account profile ID</dt><dd><code>${run.profile_id}</code></dd>`}<dt>Working folder</dt><dd><code title=${run.project_path}>${displayPath(run.project_path)}</code></dd>${run.thread_id&&html`<dt>Agent conversation</dt><dd><code>${run.thread_id}</code></dd>`}<dt>Approval setting</dt><dd>${autoApproveLabel(run.auto_approve)}${(run.auto_approve?.edits||run.auto_approve?.commands)?'. Switchya answers matching complete requests itself; changes outside ordinary project files and requests it cannot allow once still wait for you':''}. Follows the project setting, including changes made while the run is active.</dd></dl><p>${run.permission_boundary}</p><p class="field-hint">${run.ephemeral?'This run exists only while the local host is running. It is separate from built-in agent sessions.':'Saved on this computer together with its agent conversation. It is separate from built-in agent sessions.'}</p></details>`}
  </div>`;
}

const NOTICE_TITLES={adapter_error:'Agent error',recovery_acknowledged:'Recovery reviewed',notice:'Agent notice',auto_approve_changed:'Approval setting changed'};
const END_STATES={failed:['stop','Run failed'],interrupted:['caution','Run interrupted'],recovery_required:['stop','Recovery required'],completed:['neutral','Run completed']};
function TranscriptItem({item,onReview,agent='Agent',pendingIds}) {
  if(item.type==='assistant'||item.type==='user')return html`<article class="message" data-role=${item.type}><div class="message-header"><span class="message-avatar" aria-hidden="true">${item.type==='user'?'Y':html`<${Icon} name="command"/>`}</span><strong>${item.type==='user'?'You':agent}</strong><time title=${fullTime(item.at_ms)}>${timeLabel(item.at_ms)}</time></div><${TextContent} text=${item.text}/></article>`;
  if(Object.hasOwn(NOTICE_TITLES,item.kind))return html`<${Notice} title=${NOTICE_TITLES[item.kind]} tone=${item.kind==='adapter_error'?'stop':'neutral'}>${item.payload?.message}<//>`;
  if(item.kind==='state_changed'){
    const [tone,label]=END_STATES[item.payload?.state]||['neutral',stateLabel(item.payload?.state)];
    return html`<div class="state-event" data-tone=${tone}><${Icon} name=${tone==='neutral'?'info':'alert'}/><span>${label}</span></div>`;
  }
  if(item.kind==='approval_requested'){
    // Amber only while this exact request still waits for a decision.
    const hold=pendingIds?.has(item.payload?.approval?.id);
    return html`<div class="tool-event approval-event" data-hold=${hold?'true':undefined}><${Icon} name=${hold?'signal':'lock'}/><span class="approval-event-text"><strong>${approvalSummary(item.payload?.approval).title}</strong>${hold&&html`<span class="field-hint">Needs your approval</span>`}</span>${hold&&html`<${Button} icon="sidebar" onClick=${onReview}>Review agent request<//>`}</div>`;
  }
  if(item.type==='codex_item')return html`<${CodexItem} entry=${item}/>`;
  if(item.type==='claude_item')return html`<${ClaudeItem} entry=${item}/>`;
  if(item.type==='decision')return html`<div class="state-event"><${Icon} name="lock"/><span>${item.auto&&item.decision==='allow_once'?'Approved automatically by this project’s approval setting.':item.decision==='allow_once'?'You allowed this once.':item.decision==='deny'?'You denied this request.':'Decision recorded.'}</span></div>`;
  if(item.kind==='turn_completed'){
    const status=item.payload?.status;
    const outcome=status==='completed'?'completed':status==='interrupted'?'interrupted':status==='failed'?'failed':'ended with an unknown status';
    return html`<${TurnRule} tone=${status==='failed'?'stop':status==='interrupted'?'caution':'neutral'} label=${`${agent} reported the turn ${outcome}`} time=${timeLabel(item.at_ms)} fullTime=${fullTime(item.at_ms)}/>`;
  }
  const label=(item.kind==='item_started'||item.kind==='item_completed')&&item.payload?.item?.type?`${codexItemSummary(item.payload.item).title}${item.payload.item.status?` · ${codexItemSummary(item.payload.item).statusLabel}`:''}`:item.kind.replaceAll('_',' ');
  return html`<details class="activity-details"><summary>${label}</summary><${RawValue} value=${item.payload}/></details>`;
}

/** What a screen reader hears as an existing-agent run moves; empty when nothing is open. */
export function runAnnouncement(run,agent='Agent') {
  return ({starting:`${agent} is starting`,running:`${agent} is working`,awaiting_approval:`${agent} needs your decision`,interrupting:'Interrupting the run',completed:`${agent} reported the turn completed`,failed:'Run failed',interrupted:'Run interrupted',recovery_required:'Review the interrupted run before continuing'})[run?.state]||'';
}

async function allEvents(api,runId,signal) {
  let collected=[],cursor=0;
  for(let page=0;page<20;page++){
    const response=await api.request(`/adapter-runs/${enc(runId)}/events?after_seq=${cursor}&limit=200`,{signal});
    const events=response.events||[];collected=mergeAdapterEvents(collected,events,runId);
    if(events.length<200)break;cursor=events.at(-1).seq;
  }
  return collected;
}

export function AdapterWorkspace({api,project,hostId,storage,onBusyChange,onOpenNavigation,onNative,onOpenProject,onOpenTrust,profiles=[],accountDefaults={},accountsLoading=false,accountsError='',onOpenAccounts,conversationId='',onConversationChange=()=>{},onConversationsChange,reference,onReferenceHandled,drafts=new Map()}) {
  const [adapters,setAdapters]=useState([]),[runs,setRuns]=useState([]),[selectedId,setSelectedId]=useState('');
  const setConversationId=onConversationChange;
  const [run,setRun]=useState(null),[events,setEvents]=useState([]),[history,setHistory]=useState({}),[prompt,setPrompt]=useState(''),[adapterId,setAdapterId]=useState('');
  const [error,setError]=useState(''),[decisionError,setDecisionError]=useState(''),[connected,setConnected]=useState(false),[loading,setLoading]=useState(true);
  const [busy,setBusy]=useState(''),[journal,setJournal]=useState(()=>readAdapterJournal(storage,hostId)),[tick,setTick]=useState(0),[reviewOpen,setReviewOpen]=useState(false),[storageWarning,setStorageWarning]=useState('');
  const eventsRef=useRef([]),lock=useRef(false),scroll=useRef(null),follow=useRef(true),projectRef=useRef(project?.id),selectionRef=useRef(selectedId),journalRef=useRef(journal),mounted=useRef(true);
  const discoveryCache=useRef({value:null,until:0}),historyCache=useRef(new Map()),pane=useRef(null);
  const draftKey=`${project?.id||''}:${conversationId||'new'}`,draftContext=useRef(draftKey),promptRef=useRef(prompt);promptRef.current=prompt;
  useEffect(()=>{
    if(draftContext.current!==draftKey)drafts.set(draftContext.current,promptRef.current);
    draftContext.current=draftKey;setPrompt(drafts.get(draftKey)||'');
    selectionRef.current='';setSelectedId('');setRun(null);setEvents([]);eventsRef.current=[];
  },[draftKey]);
  useEffect(()=>()=>{drafts.set(draftContext.current,promptRef.current);},[]);
  const [profileId,setProfileId]=useState('');
  const conversations=groupConversations(runs);
  const conversation=conversations.find(item=>item.id===conversationId)||null;
  const target=continuationTarget(conversation),blocker=continuationBlocker(conversation);
  const continuing=Boolean(target);
  const reviewRun=conversation?.runs.filter(item=>item.state==='recovery_required').at(-1)||null;
  const runProfile=chosenProfile(profiles,accountDefaults,adapterId,profileId);
  const targetProfile=target&&profiles.find(profile=>profile.id===target.profile_id);
  // Continuations keep the conversation's account; the host enforces it again.
  const startAllowed=continuing?(!targetProfile||canStartProfile(targetProfile)):canStartProfile(runProfile);
  const availableProfiles=profiles.filter(profile=>profile.agent_id===adapterId);
  useEffect(()=>setProfileId(''),[adapterId]);
  projectRef.current=project?.id;journalRef.current=journal;
  selectionRef.current=selectedId;
  const pendingStart=journal[project?.id];
  function changeBusy(value){setBusy(value);onBusyChange?.(Boolean(value));}
  function savePending(request){const next={...readAdapterJournal(storage,hostId),...journalRef.current,[request.project_id]:request};journalRef.current=next;setJournal(next);if(!writeAdapterJournal(storage,hostId,next))setStorageWarning('This browser cannot retain pending starts after reload. Keep this tab open until the run is confirmed.');}
  function clearPending(request){const next={...journalRef.current,...readAdapterJournal(storage,hostId)};if(next[request.project_id]?.command_id===request.command_id)delete next[request.project_id];journalRef.current=next;writeAdapterJournal(storage,hostId,next);if(mounted.current)setJournal(next);}
  useEffect(()=>()=>{mounted.current=false;onBusyChange?.(false);},[]);
  eventsRef.current=events;
  const refresh=()=>setTick(value=>value+1);
  const available=adapters.filter(adapter=>adapter.installed&&adapter.supported);
  // Discovery answered and found nothing this workspace can run: say so instead of
  // leaving a disabled agent picker as the only clue.
  const noAgents=!loading&&connected&&!available.length;
  const checkAgents=()=>{discoveryCache.current={value:null,until:0};refresh();};
  const conversationAgent=conversation?.latest.adapter_id||adapterId;
  const selectedAdapter=adapters.find(adapter=>adapter.id===conversationAgent);
  useEffect(()=>{setRuns([]);setRun(null);setEvents([]);eventsRef.current=[];setSelectedId('');setHistory({});historyCache.current=new Map();setDecisionError('');},[project?.id]);
  useEffect(()=>{onConversationsChange?.(groupConversations(runs));},[runs]);
  useEffect(()=>{if((reference?.path||reference?.text)&&reference.project_id===project?.id){const text=reference.text||`\`${reference.path}\``;setPrompt(old=>`${old}${old?'\n\n':''}${text}`);onReferenceHandled?.();}},[reference?.id]);
  useEffect(()=>{
    const abort=new AbortController();let stopped=false,timer;
    async function poll(){
      const discovery=discoveryCache.current.value&&Date.now()<discoveryCache.current.until?Promise.resolve(discoveryCache.current.value):api.request('/adapters',{signal:abort.signal}).then(value=>{discoveryCache.current={value,until:Date.now()+60000};return value;});
      const results=await Promise.allSettled([discovery,api.request('/adapter-runs',{signal:abort.signal})]);
      if(stopped||projectRef.current!==project?.id)return;
      if(results.every(result=>result.status==='fulfilled')){
        const next=results[0].value.adapters||[];setAdapters(next);setAdapterId(old=>next.some(item=>item.id===old&&item.installed&&item.supported)?old:next.find(item=>item.installed&&item.supported)?.id||'');
        const current=(results[1].value.runs||[]).filter(item=>!project||item.project_path===project.root);setRuns(current);setConnected(true);setError('');
        if(pendingStart){const confirmed=current.find(item=>item.command_id===pendingStart.command_id);if(confirmed){openConversation(confirmed.conversation_id||confirmed.id,confirmed);clearPending(pendingStart);setPrompt('');}}
      }else{setConnected(false);setError(errorText(results.find(result=>result.status==='rejected').reason));}
      setLoading(false);timer=setTimeout(poll,3500);
    }
    poll();return()=>{stopped=true;abort.abort();clearTimeout(timer);};
  },[project?.id,tick,pendingStart?.command_id]);
  // Follow the newest run of the open conversation, including continuations
  // started from another window or the CLI.
  useEffect(()=>{if(conversation&&conversation.latest.id!==selectionRef.current)select(conversation.latest.id,conversation.latest);},[conversation?.latest.id,conversationId]);
  useEffect(()=>{
    if(!selectedId)return;
    const abort=new AbortController();let stopped=false,timer;
    const runProjectId=project?.id;
    const outdated=()=>stopped||projectRef.current!==runProjectId||selectionRef.current!==selectedId;
    async function poll(){
      try{
        const result=await api.request(`/adapter-runs/${enc(selectedId)}`,{signal:abort.signal});if(outdated())return;
        const next=result.run;let cursor=eventsRef.current.at(-1)?.seq||0;
        cursor=Math.max(cursor,(next.first_retained_seq||1)-1,next.last_seq-600);
        let collected=eventsRef.current;
        for(let page=0;page<3&&cursor<next.last_seq;page++){
          const response=await api.request(`/adapter-runs/${enc(selectedId)}/events?after_seq=${cursor}&limit=200`,{signal:abort.signal});if(outdated())return;
          collected=mergeAdapterEvents(collected,response.events||[],selectedId);if(!response.events?.length)break;cursor=response.events.at(-1).seq;
        }
        setRun(next);setEvents(collected);setConnected(true);setError('');timer=setTimeout(poll,active(next.state)?900:3000);
      }catch(error){if(!outdated()&&error.code!=='aborted'){setError(errorText(error));setConnected(false);timer=setTimeout(poll,3000);}}
    }
    poll();return()=>{stopped=true;abort.abort();clearTimeout(timer);};
  },[selectedId,project?.id,tick]);
  // Earlier turns of the open conversation are finished; load each once, and
  // again only if its saved history changed (for example, a recovery review).
  const earlier=conversation?conversation.runs.filter(item=>item.id!==selectedId):[];
  const earlierKey=earlier.map(item=>`${item.id}:${item.last_seq}`).join(',');
  useEffect(()=>{
    if(!earlier.length)return;
    const abort=new AbortController();let stopped=false;
    (async()=>{
      for(const item of earlier){
        const cached=historyCache.current.get(item.id);if(cached?.last_seq===item.last_seq)continue;
        try{const loaded=await allEvents(api,item.id,abort.signal);if(stopped)return;historyCache.current.set(item.id,{last_seq:item.last_seq,events:loaded});setHistory(old=>({...old,[item.id]:loaded}));}
        catch(error){if(!stopped&&error.code!=='aborted')setError(errorText(error));return;}
      }
    })();
    return()=>{stopped=true;abort.abort();};
  },[earlierKey]);
  useLayoutEffect(()=>{if(follow.current&&scroll.current)scroll.current.scrollTop=scroll.current.scrollHeight;},[events,history,error,run?.state]);
  useEffect(()=>{
    const node=scroll.current;if(!node||typeof ResizeObserver==='undefined')return;
    const observer=new ResizeObserver(()=>{if(follow.current)node.scrollTop=node.scrollHeight;});
    observer.observe(node);return()=>observer.disconnect();
  },[]);
  function select(id,selectedRun=runs.find(item=>item.id===id)||null){selectionRef.current=id;setSelectedId(id);setRun(selectedRun);setEvents([]);eventsRef.current=[];setError('');setDecisionError('');follow.current=true;}
  function openConversation(id,latest){setConversationId(id);const group=groupConversations(runs).find(item=>item.id===id);const newest=latest||group?.latest;if(newest)select(newest.id,newest);else select('');}
  function newConversation(){if(lock.current)return;setConversationId('');select('');}
  async function start(event,retry=false){
    // An open conversation only accepts a continuation; it never starts a separate run.
    event?.preventDefault();if(lock.current||!project||!connected||(!retry&&(!prompt.trim()||pendingStart||!startAllowed||(conversation&&!continuing)||(!continuing&&!adapterId))))return;
    lock.current=true;changeBusy('start');setError('');
    const request=retry?pendingStart:continuing
      ?{adapter_id:target.adapter_id,project_id:project.id,...(target.profile_id?{profile_id:target.profile_id}:{}),prompt:prompt.trim(),command_id:crypto.randomUUID(),continue_run_id:target.id}
      :{adapter_id:adapterId,project_id:project.id,profile_id:runProfile.id,prompt:prompt.trim(),command_id:crypto.randomUUID()};
    if(!request){lock.current=false;changeBusy('');return;}
    savePending(request);
    try{const result=await api.request('/adapter-runs',{method:'POST',body:request});clearPending(request);if(mounted.current&&projectRef.current===request.project_id){setRuns(old=>[result.run,...old.filter(item=>item.id!==result.run.id)]);setConversationId(result.run.conversation_id||result.run.id);select(result.run.id,result.run);setPrompt('');refresh();}}
    catch(error){if(!error.uncertain)clearPending(request);if(mounted.current&&projectRef.current===request.project_id){setError(errorText(error));refresh();}}
    finally{changeBusy('');lock.current=false;}
  }
  async function decide(approval,decision){
    if(lock.current||!run)return;lock.current=true;changeBusy('decision');setDecisionError('');
    try{await api.request(`/adapter-runs/${enc(run.id)}/decisions`,{method:'POST',body:{approval_id:approval.id,expected_hash:approval.expected_hash,decision}});refresh();}
    catch(error){setDecisionError(errorText(error));refresh();}
    finally{changeBusy('');lock.current=false;}
  }
  async function interrupt(){
    if(lock.current||!run)return;lock.current=true;changeBusy('interrupt');
    try{await api.request(`/adapter-runs/${enc(run.id)}/interrupt`,{method:'POST',body:{}});refresh();}
    catch(error){setError(errorText(error));refresh();}
    finally{changeBusy('');lock.current=false;}
  }
  async function acknowledge(reviewed){
    if(lock.current||!reviewed)return;lock.current=true;changeBusy('recovery');setError('');
    try{const result=await api.request(`/adapter-runs/${enc(reviewed.id)}/recovery`,{method:'POST',body:{}});if(mounted.current)setRuns(old=>old.map(item=>item.id===result.run.id?result.run:item));refresh();}
    catch(error){setError(errorText(error));refresh();}
    finally{changeBusy('');lock.current=false;}
  }
  const review=html`<${AdapterReview} run=${run} onDecision=${decide} busy=${Boolean(busy)} error=${decisionError} connected=${connected}/>`;
  // The review pane is already on screen at desktop widths; the drawer is for narrow windows.
  const openReview=()=>{if(matchMedia('(max-width: 1179px)').matches)setReviewOpen(true);else pane.current?.focus();};
  const blocks=conversationTranscript([...earlier.map(item=>({run:item,events:history[item.id]||null})),...(run&&conversation?[{run,events}]:[])]);
  // The pinned account stays in the header for every open conversation, also while its run loads.
  const headerRun=conversation?(run||conversation.latest):null;
  const accountName=headerRun?(headerRun.profile_name||headerRun.profile_id||'Not reported'):null;
  const liveRun=run&&conversation&&active(run.state)?run:null;
  const pendingIds=new Set((run?.pending_approvals||[]).map(approval=>approval.id));
  const announcement=conversation&&run?runAnnouncement(run,agentName(run.adapter_id)||'Agent'):'';
  const liveText=state=>state==='awaiting_approval'?'Waiting for your decision':state==='interrupting'?'Interrupting the run':state==='starting'?`${agentName(liveRun.adapter_id)} is starting`:`${agentName(liveRun.adapter_id)} is working`;
  const submitDisabled=!connected||!project||(!continuing&&!adapterId)||!prompt.trim()||Boolean(busy)||Boolean(pendingStart)||!startAllowed||Boolean(conversation&&!continuing);
  return html`<main class="workspace-body"><header class="workspace-header"><${Button} class="rail-toggle" icon="menu" aria-label="Open projects and sessions" onClick=${onOpenNavigation}/><div class="header-project"><span class="eyebrow" title=${project?displayPath(project.root):undefined}>${project?.name||'Local workspace'}</span><h1 title=${conversation?.title||undefined}>${conversation?.title||'New conversation'}</h1>${headerRun&&html`<p class="run-account">${agentName(headerRun.adapter_id)} · Account: <strong>${accountName}</strong></p>`}</div><div class="header-actions">${run&&conversation&&html`<span class="status-label" data-tone=${stateTone(run.state)}><span class="status-dot" aria-hidden="true"></span>${stateLabel(run.state)}</span>`}<${Button} class="review-toggle" icon="sidebar" onClick=${openReview}>Review${run?.pending_approvals?.length?' request':''}<//></div></header><section class="conversation" aria-label="Existing agent conversation"><div class="conversation-scroll" ref=${scroll} onScroll=${()=>{const node=scroll.current;follow.current=node.scrollHeight-node.scrollTop-node.clientHeight<110;}}>
    ${!conversation&&html`<div class="adapter-intro"><p class="eyebrow">CONNECTED THROUGH YOUR CLI</p><p>Run an installed agent in this project using its existing sign-in and configuration. Conversations are saved on this computer and can be continued after a restart; continuing sends only your new message.</p>${noAgents&&html`<${Notice} title="No agent CLI found" tone="info">Switchya found no Codex, Claude Code or Antigravity CLI on this computer. Install one, then choose Check installed agents. To work without one, use the built-in agent with a model provider you connect.<//>`}<div class="adapter-intro-actions"><${Button} onClick=${()=>{if(!lock.current)onNative();}} disabled=${Boolean(busy)} icon="switch">Use the built-in agent<//><${Button} icon="refresh" disabled=${Boolean(busy)} onClick=${checkAgents}>Check installed agents<//></div></div>`}
    ${loading&&html`<p class="loading-state" role="status">Checking installed agents…</p>`}${!conversation&&adapters.length>0&&html`<details class="agent-setup-details" open=${noAgents||undefined}><summary>Installed agents and setup</summary><div class="adapter-list">${adapters.map(adapter=>html`<div class="adapter-status" key=${adapter.id}><div><strong>${adapter.name}</strong><span class="field-hint">${adapter.version||''}</span></div><span>${availabilityLabel(adapter.status)}</span>${!adapter.supported&&html`<p class="field-hint">${adapter.id==='claude'?'Use Accounts to sign in and open Claude Code in its own terminal.':'Runs are not available in this version.'}</p>`}</div>`)}</div></details>`}
    ${blocks.map((block,index)=>html`<div class="adapter-turn" key=${block.run.id}>${index>0&&html`<div class="state-event run-divider"><${Icon} name="switch"/><span>Continued ${timeLabel(block.run.started_at_ms)}</span></div>`}${block.run.first_retained_seq>1&&html`<p class="history-note">Earlier output from this turn has left the bounded event window.</p>`}${block.items===null?html`<p class="loading-state" role="status">Loading saved turn…</p>`:block.items.map(item=>html`<${TranscriptItem} key=${`${block.run.id}:${item.seq}`} item=${item} onReview=${openReview} agent=${agentName(block.run.adapter_id)} pendingIds=${block.run.id===run?.id?pendingIds:null}/>`)}${liveRun&&block.run.id===liveRun.id&&html`<div class="state-event live-state" data-tone=${liveRun.state==='awaiting_approval'?'caution':'info'}><${Icon} name=${liveRun.state==='awaiting_approval'?'signal':'switch'}/><span>${liveText(liveRun.state)}</span></div>`}</div>`)}
    </div></section><section class="composer-area" aria-label="Existing agent task">${error&&html`<${Notice} title="Existing agent status" tone="stop" action=${html`<${Button} icon="refresh" onClick=${refresh}>Refresh state<//>`}>${error}<//>`}${!project&&html`<${Notice} title="Open a project first" action=${html`<${Button} onClick=${onOpenProject}>Open project<//>`}>Choose the local folder this agent should work in.<//>`}${pendingStart&&html`<${Notice} title="Start request awaiting confirmation" tone="info" action=${html`<${Button} onClick=${event=>start(event,true)} disabled=${Boolean(busy)||!connected}>Retry same request<//>`}>Account for this pending request: <strong>${profiles.find(profile=>profile.id===pendingStart.profile_id)?.name||pendingStart.profile_id||'Not reported'}</strong>. Refresh checks whether the run was created. Retrying retains this account and the original request ID.<//>`}${reviewRun&&html`<${Notice} title="Review the interrupted run" tone="stop" action=${html`<${Button} onClick=${()=>acknowledge(reviewRun)} disabled=${Boolean(busy)||!connected} busy=${busy==='recovery'}>I reviewed the project<//>`}>${agentName(reviewRun.adapter_id)} may have finished a command or file change before this run stopped. Check its output and your project, then confirm to continue the conversation. Nothing is replayed.<//>`}${blocker&&blocker!=='review'&&html`<p class="field-hint" role="status">${blocker}</p>`}${run?.history_error&&html`<${Notice} title="History not saved" tone="caution">${run.history_error}<//>`}${storageWarning&&html`<p class="field-hint" role="status">${storageWarning}</p>`}${accountsError&&html`<p class="field-error" role="alert">Account list: ${accountsError}</p>`}${!conversation&&adapterId&&!accountsLoading&&!availableProfiles.length&&html`<${Notice} title="Add an account for this agent" action=${html`<${Button} onClick=${onOpenAccounts}>Open Accounts<//>`}>Create a profile and sign in to ${agentName(adapterId)} before starting a conversation.<//>`}${!conversation&&runProfile&&!canStartProfile(runProfile)&&html`<${Notice} title="Check the selected account" action=${html`<${Button} onClick=${onOpenAccounts}>Open Accounts<//>`}>${runProfile.name}${runProfile.account?.auth_status==='signed_out'?' needs a sign-in before starting a run.':' needs a confirmed sign-in status. Use Refresh account in Accounts to check sign-in status before starting a run.'}<//>`}${continuing&&targetProfile&&!canStartProfile(targetProfile)&&html`<${Notice} title="Check this conversation's account" action=${html`<${Button} onClick=${onOpenAccounts}>Open Accounts<//>`}>${targetProfile.name} must be signed in to continue this conversation. Conversations stay with the account that started them.<//>`}<form class="composer adapter-composer" onSubmit=${start}>${!conversation&&html`<div class="composer-top"><label class="model-select"><span>Agent</span><select value=${conversationAgent} onChange=${event=>setAdapterId(event.currentTarget.value)} disabled=${!available.length||Boolean(busy)} aria-label="Existing agent"><option value="" disabled>${available.length?'Choose agent':'No supported agent available'}</option>${available.map(adapter=>html`<option key=${adapter.id} value=${adapter.id}>${adapter.name}</option>`)}</select></label><label class="model-select account-select"><span>Account</span><select value=${runProfile?.id||''} onChange=${event=>setProfileId(event.currentTarget.value)} disabled=${Boolean(busy)||Boolean(pendingStart)||!availableProfiles.length} aria-label="Account for new run"><option value="" disabled>${accountsLoading?'Loading accounts…':'Choose account'}</option>${availableProfiles.map(profile=>html`<option key=${profile.id} value=${profile.id}>${profile.name} · ${authLabel(profile.account?.auth_status)}</option>`)}</select></label><${Button} class="manage-accounts" onClick=${onOpenAccounts} disabled=${Boolean(busy)}>Accounts<//></div>`}<div class="composer-text"><textarea id="task-input" rows="1" value=${prompt} aria-label=${conversation?'Message for this conversation':'Task for existing agent'} onInput=${event=>setPrompt(event.currentTarget.value)} placeholder=${conversation?'Continue this conversation…':'Describe one task for the existing agent…'} disabled=${!project||Boolean(busy)} onKeyDown=${event=>{if((event.ctrlKey||event.metaKey)&&event.key==='Enter'){event.preventDefault();if(!submitDisabled)start(event);}}}/></div><div class="composer-bottom">${project&&onOpenTrust&&html`<button type="button" class="composer-boundary trust-open" data-tone=${autoApproveTone(project.auto_approve)} onClick=${onOpenTrust} disabled=${!connected} aria-label=${`${autoApproveLabel(project.auto_approve)}, for new and running requests in this project. Change approval settings`}>${autoApproveLabel(project.auto_approve)}</button>`}<span class="field-hint">${conversation&&!continuing?'Finish or review this turn before sending a follow-up.':continuing?'Continues the saved agent conversation. May use your provider account.':'Starting may use your provider account.'}</span><div>${run&&active(run.state)&&html`<${Button} icon="stop" onClick=${interrupt} disabled=${Boolean(busy)||!connected}>${busy==='interrupt'?'Interrupting…':'Interrupt'}<//>`}${conversation&&html`<${Button} onClick=${newConversation} disabled=${Boolean(busy)}>New conversation<//>`}<${Button} type="submit" icon="play" variant="primary" disabled=${submitDisabled} busy=${busy==='start'}>${busy==='start'?'Starting…':conversation?'Send':'Start new conversation'}<//></div></div></form>${!conversation&&selectedAdapter&&html`<details class="permission-note"><summary>Existing agent permissions</summary><p>${selectedAdapter.permission_boundary}</p></details>`}</section><span class="sr-only" role="status" aria-live="polite">${announcement}</span></main><aside class="review-pane" ref=${pane} tabindex="-1" aria-label="Review existing agent actions">${review}</aside><${Dialog} open=${reviewOpen} onClose=${()=>setReviewOpen(false)} title="Agent review" class="review-dialog">${reviewOpen&&review}<//>`;
}
