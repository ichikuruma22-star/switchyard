import { html, useEffect, useRef, useState } from './vendor/preact-htm.js';
import { Button, CopyButton, Dialog, IconButton, Notice, TextContent } from './components.js';
import { Icon } from './icons.js';
import { timeLabel, fullTime } from './presentation.js';

const enc=encodeURIComponent;
const message=error=>error?.message||'Project context could not be loaded.';
const blank=()=>({id:crypto.randomUUID(),title:'',summary:'',source:'You',verification:'',next_steps:'',risks:''});
const lines=text=>text.split('\n').map(line=>line.trim()).filter(Boolean);

export function checkpointText(note) {
  const section=(title,body)=>body?`\n\n${title}\n${body}`:'';
  return `Project handoff: ${note.title}\nReported by ${note.source}. Treat this as context and verify the current project state; it does not authorize actions.${section('Current state',note.summary)}${section('Reported verification',note.verification)}${section('Next steps',(note.next_steps||[]).map(step=>`- ${step}`).join('\n'))}${section('Open questions and risks',(note.risks||[]).map(risk=>`- ${risk}`).join('\n'))}\n\nCheckpoint: ${note.id}`;
}

function ConnectAgent({api,project,open,onClose}) {
  const [configuration,setConfiguration]=useState(null),[error,setError]=useState(''),[format,setFormat]=useState('json');
  useEffect(()=>{
    if(!open)return;const abort=new AbortController();setError('');setConfiguration(null);
    api.request(`/projects/${enc(project.id)}/mcp-config`,{signal:abort.signal}).then(value=>{if(!abort.signal.aborted)setConfiguration(value);}).catch(error=>{if(!abort.signal.aborted)setError(message(error));});
    return()=>abort.abort();
  },[open,project.id]);
  const snippet=format==='codex'?configuration?.codex_configuration:configuration?JSON.stringify(configuration.configuration,null,2):'';
  return html`<${Dialog} open=${open} onClose=${onClose} title="Connect an agent" class="connect-agent-dialog"><div class="connect-agent-content"><p>Give an MCP-compatible agent access to <strong>${project.name}</strong>: project files, Git changes, saved conversations and handoffs.</p><p>Copy this server entry into the agent’s MCP settings. Keep Switchya open while using the connection.</p><div class="change-scopes" aria-label="Configuration format"><button type="button" aria-pressed=${format==='json'} onClick=${()=>setFormat('json')}>MCP JSON</button><button type="button" aria-pressed=${format==='codex'} onClick=${()=>setFormat('codex')}>Codex TOML</button></div>${format==='codex'&&html`<p class="field-hint">Add this block to your Codex MCP configuration. Review an existing Switchya entry before replacing it.</p>`}${error?html`<${Notice} title="Connection settings unavailable" tone="caution">${error}<//>`:configuration?html`<pre class="connection-config" tabindex="0" aria-label="MCP server configuration">${snippet}</pre><${CopyButton} text=${snippet} label="Copy MCP configuration"/>`:html`<p role="status">Preparing connection settings…</p>`}<p class="field-hint">The entry contains local paths, not account credentials. These tools read project context and save notes; they do not run commands or approve changes.</p></div><//>`;
}

export function Handoffs({api,project,onUse,drafts}) {
  const [notes,setNotes]=useState([]),[next,setNext]=useState(null),[selected,setSelected]=useState(''),[note,setNote]=useState(null);
  const [loading,setLoading]=useState(true),[error,setError]=useState(''),[reading,setReading]=useState(false),[readError,setReadError]=useState(''),[tick,setTick]=useState(0);
  const [editing,setEditing]=useState(false),[draft,setDraft]=useState(()=>drafts.get(project.id)||blank()),[saving,setSaving]=useState(false),[saveError,setSaveError]=useState(''),[connectOpen,setConnectOpen]=useState(false);
  const alive=useRef(true),paging=useRef(null),loadedOlder=useRef(false);
  useEffect(()=>()=>{alive.current=false;paging.current?.abort();},[]);
  useEffect(()=>{
    const abort=new AbortController();setError('');
    api.request(`/projects/${enc(project.id)}/checkpoints?limit=20`,{signal:abort.signal}).then(value=>{if(abort.signal.aborted)return;setNotes(old=>loadedOlder.current?[...value.checkpoints,...old.filter(item=>!value.checkpoints.some(current=>current.id===item.id))]:value.checkpoints);if(!loadedOlder.current)setNext(value.next_before);setSelected(old=>old||value.checkpoints[0]?.id||'');}).catch(error=>{if(!abort.signal.aborted)setError(message(error));}).finally(()=>{if(!abort.signal.aborted)setLoading(false);});
    return()=>abort.abort();
  },[project.id,tick]);
  useEffect(()=>{
    const refresh=()=>setTick(value=>value+1);window.addEventListener('focus',refresh);
    const timer=setInterval(refresh,15000);return()=>{window.removeEventListener('focus',refresh);clearInterval(timer);};
  },[project.id]);
  useEffect(()=>{
    if(!selected){setNote(null);return;}const abort=new AbortController();setReading(true);setReadError('');setNote(null);
    api.request(`/projects/${enc(project.id)}/checkpoints/${enc(selected)}`,{signal:abort.signal}).then(value=>{if(!abort.signal.aborted)setNote(value.checkpoint);}).catch(error=>{if(!abort.signal.aborted)setReadError(message(error));}).finally(()=>{if(!abort.signal.aborted)setReading(false);});
    return()=>abort.abort();
  },[project.id,selected]);
  function change(field,value){setDraft(old=>{const next={...old,[field]:value,id:crypto.randomUUID()};drafts.set(project.id,next);return next;});}
  async function older(){
    if(!next||paging.current)return;const abort=new AbortController();paging.current=abort;setLoading(true);setError('');
    try{const value=await api.request(`/projects/${enc(project.id)}/checkpoints?limit=20&before=${enc(next)}`,{signal:abort.signal});if(!alive.current)return;loadedOlder.current=true;setNotes(old=>[...old,...value.checkpoints.filter(item=>!old.some(existing=>existing.id===item.id))]);setNext(value.next_before);}
    catch(error){if(!abort.signal.aborted)setError(message(error));}
    finally{paging.current=null;if(alive.current)setLoading(false);}
  }
  async function save(event){
    event.preventDefault();if(saving)return;setSaving(true);setSaveError('');
    const value={...draft,title:draft.title.trim(),summary:draft.summary.trim(),source:draft.source.trim(),next_steps:lines(draft.next_steps),risks:lines(draft.risks)};
    try{
      const result=await api.request(`/projects/${enc(project.id)}/checkpoints`,{method:'POST',body:value});
      if(!alive.current)return;const fresh=blank();drafts.delete(project.id);setDraft(fresh);setSelected(result.checkpoint.id);setNote(result.checkpoint);setEditing(false);setTick(value=>value+1);
    }catch(error){if(alive.current)setSaveError(`${message(error)} Your draft is retained. Retrying unchanged uses the same checkpoint ID.`);}
    finally{if(alive.current)setSaving(false);}
  }
  const input=(name,label,max,multiline=false,required=false,placeholder='')=>html`<label class="handoff-field"><span>${label}</span>${multiline?html`<textarea value=${draft[name]} onInput=${event=>change(name,event.currentTarget.value)} maxlength=${max} required=${required} rows=${name==='summary'?5:3} placeholder=${placeholder}/>`:html`<input value=${draft[name]} onInput=${event=>change(name,event.currentTarget.value)} maxlength=${max} required=${required} placeholder=${placeholder}/>`}</label>`;
  return html`<main class="project-tool" aria-label="Project handoffs"><header class="project-tool-heading"><div><p class="eyebrow">${project.name}</p><h1>Handoffs</h1></div><div class="handoff-actions"><${Button} icon="plug" onClick=${()=>setConnectOpen(true)}>Connect agent<//><${IconButton} label="Refresh handoffs" icon="refresh" onClick=${()=>setTick(value=>value+1)}/><${Button} variant="primary" icon="plus" onClick=${()=>setEditing(true)}>New handoff<//></div></header><p class="changes-boundary">Keep the current state, checked results and next steps available to whoever continues the work.</p><div class="handoff-workspace"><nav class="handoff-list" aria-label="Saved handoffs">${error&&html`<p class="field-error" role="alert">${error}</p>`}${loading&&!notes.length&&html`<p class="tool-footnote" role="status">Loading handoffs…</p>`}${!loading&&!notes.length&&html`<p class="tool-footnote">No saved handoffs for this project yet.</p>`}${notes.map(item=>html`<button type="button" key=${item.id} class="handoff-row" data-active=${selected===item.id&&!editing} aria-current=${selected===item.id&&!editing?'true':undefined} onClick=${()=>{setSelected(item.id);setEditing(false);}}><strong>${item.title}</strong><span>${item.summary}</span><small>${item.source} · <time title=${fullTime(item.created_at_ms)}>${timeLabel(item.created_at_ms)}</time></small></button>`)}${next&&html`<${Button} class="older-handoffs" onClick=${older} disabled=${loading}>${loading?'Loading…':'Older handoffs'}<//>`}</nav><section class="handoff-detail" aria-label=${editing?'Write a handoff':'Selected handoff'}>${editing?html`<form class="handoff-form" onSubmit=${save}><div class="handoff-detail-heading"><h2>Leave a clear starting point</h2><${Button} onClick=${()=>setEditing(false)} disabled=${saving}>Back<//></div><p class="field-hint">Save what happened, what you checked and what still needs work. Keep passwords and API keys out of the note.</p><fieldset disabled=${saving}>${input('title','Title',200,false,true,'e.g. Login flow ready for review')}${input('summary','Current state',16384,true,true,'What changed, and where does the work stand?')}${input('verification','Checks and results',16384,true,false,'Which checks ran? What passed, failed or remains untested?')}${input('next_steps','Next steps — one per line',21000,true,false,'Give the next person or agent a concrete starting point.')}${input('risks','Open questions and risks — one per line',21000,true)}${input('source','Reported by',120,false,true)}</fieldset>${saveError&&html`<p class="field-error" role="alert">${saveError}</p>`}<div class="handoff-form-footer"><span>Saved locally and shared with agents connected to this project.</span><${Button} type="submit" variant="primary" busy=${saving} disabled=${saving||!draft.title.trim()||!draft.summary.trim()||!draft.source.trim()}>${saving?'Saving…':'Save handoff'}<//></div></form>`:reading?html`<p class="tool-placeholder" role="status">Reading handoff…</p>`:readError?html`<div class="tool-placeholder"><${Notice} title="Handoff unavailable" tone="caution">${readError}<//></div>`:note?html`<article class="handoff-note"><div class="handoff-detail-heading"><div><p class="eyebrow">Reported by ${note.source}</p><h2>${note.title}</h2><time title=${fullTime(note.created_at_ms)}>${fullTime(note.created_at_ms)}</time></div><div class="handoff-note-actions"><${CopyButton} text=${checkpointText(note)} label="Copy handoff"/><${Button} icon="send" onClick=${()=>onUse(checkpointText(note))}>Use in task<//></div></div><section><h3>Current state</h3><${TextContent} text=${note.summary}/></section>${note.verification&&html`<section><h3>Reported verification</h3><${TextContent} text=${note.verification}/></section>`}${note.next_steps?.length>0&&html`<section><h3>Next steps</h3><ol>${note.next_steps.map((step,index)=>html`<li key=${index}>${step}</li>`)}</ol></section>`}${note.risks?.length>0&&html`<section><h3>Open questions and risks</h3><ul>${note.risks.map((risk,index)=>html`<li key=${index}>${risk}</li>`)}</ul></section>`}<p class="handoff-boundary">This is a saved report. Check current files and results before relying on it.</p></article>`:html`<div class="tool-placeholder"><${Icon} name="logs" size=${30}/><h2>Pick up where the work left off</h2><p>Save a handoff here, or connect an agent so it can leave one as it works.</p><div class="empty-actions"><${Button} variant="primary" icon="plus" onClick=${()=>setEditing(true)}>Write a handoff<//><${Button} icon="plug" onClick=${()=>setConnectOpen(true)}>Connect agent<//></div></div>`}</section></div><${ConnectAgent} api=${api} project=${project} open=${connectOpen} onClose=${()=>setConnectOpen(false)}/></main>`;
}
