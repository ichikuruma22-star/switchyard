import { html, useEffect, useRef, useState } from './vendor/preact-htm.js';
import { Button, IconButton, CopyButton, Notice } from './components.js';
import { Icon } from './icons.js';
import { DiffView } from './diff.js';
import { TerminalDock } from './terminal.js';
import { Handoffs } from './handoffs.js';

const enc = encodeURIComponent;
const errorText = error => error?.message || 'The project could not be read.';
export const parentPath = path => path.includes('/') ? path.slice(0,path.lastIndexOf('/')) : '.';
const changeNames = {M:'Modified',A:'Added',D:'Deleted',R:'Renamed',C:'Copied',U:'Conflict',T:'Type changed','?':'Untracked'};
export const changeLabel = change => change.untracked ? 'Untracked' : changeNames[change.unstaged.trim()] || changeNames[change.staged.trim()] || 'Changed';

export function CodePreview({file,loading,error}) {
  const [wrap,setWrap] = useState(false);
  if (loading) return html`<p class="tool-placeholder" role="status">Reading file…</p>`;
  if (error) return html`<div class="tool-placeholder"><${Notice} title="File preview unavailable" tone="neutral">${error}<//></div>`;
  if (!file) return html`<div class="tool-placeholder"><${Icon} name="file" size=${28}/><h2>Open a file</h2><p>Browse a folder or enter a file path to inspect the source.</p></div>`;
  const lines = file.content.split('\n',4001), shortened = lines.length > 4000;
  if(shortened)lines.length=4000;
  return html`<div class="source-view"><div class="source-tools"><span>Read-only preview</span><${Button} icon="wrap" aria-pressed=${wrap} onClick=${()=>setWrap(!wrap)}>Wrap lines<//><${CopyButton} text=${file.content} label="Copy content"/></div><div class="source-scroll" tabindex="0" aria-label=${`Source of ${file.path}`}><pre class="source-lines" data-wrap=${wrap}>${lines.map((line,index)=>html`<span class="source-line" key=${index}><span class="source-number" aria-hidden="true">${index+1}</span><code>${line}</code></span>`)}</pre></div>${(file.truncated||shortened)&&html`<p class="tool-footnote">Preview limited to ${shortened?'4,000 lines':'256 KiB'}. The file on disk is unchanged.</p>`}</div>`;
}

function Files({api,project,onReference}) {
  const [directory,setDirectory] = useState('.'), [listing,setListing] = useState(null), [filter,setFilter] = useState('');
  const [path,setPath] = useState(''), [typedPath,setTypedPath] = useState(''), [file,setFile] = useState(null);
  const [loading,setLoading] = useState(false), [fileLoading,setFileLoading] = useState(false), [error,setError] = useState(''), [fileError,setFileError] = useState(''), [tick,setTick] = useState(0);
  useEffect(()=>{
    const abort = new AbortController(); setLoading(true);setError('');
    api.request(`/projects/${enc(project.id)}/files?path=${enc(directory)}`,{signal:abort.signal}).then(value=>{if(!abort.signal.aborted)setListing(value);}).catch(error=>{if(!abort.signal.aborted)setError(errorText(error));}).finally(()=>{if(!abort.signal.aborted)setLoading(false);});
    return()=>abort.abort();
  },[project.id,directory,tick]);
  useEffect(()=>{
    setFile(null);setFileError('');if(!path)return;
    const abort = new AbortController();setFileLoading(true);
    api.request(`/projects/${enc(project.id)}/file?path=${enc(path)}`,{signal:abort.signal}).then(value=>{if(!abort.signal.aborted)setFile(value);}).catch(error=>{if(!abort.signal.aborted)setFileError(errorText(error));}).finally(()=>{if(!abort.signal.aborted)setFileLoading(false);});
    return()=>abort.abort();
  },[project.id,path,tick]);
  function open(entry){if(entry.kind==='directory'){setDirectory(entry.path);setFilter('');setListing(null);}else{setPath(entry.path);setTypedPath(entry.path);}}
  const entries=(listing?.entries||[]).filter(entry=>entry.name.toLowerCase().includes(filter.toLowerCase()));
  return html`<main class="project-tool" aria-label="Project files"><header class="project-tool-heading"><div><p class="eyebrow">${project.name}</p><h1>Files</h1></div><${IconButton} label="Refresh files" icon="refresh" onClick=${()=>setTick(value=>value+1)} disabled=${loading}/></header><div class="file-workspace"><nav class="file-browser" aria-label="Project folders"><div class="folder-heading"><${IconButton} label="Go to parent folder" icon="chevron-left" disabled=${directory==='.'} onClick=${()=>{setDirectory(parentPath(directory));setFilter('');setListing(null);}}/><button type="button" class="folder-root" onClick=${()=>{setDirectory('.');setFilter('');}} title=${directory}>${directory==='.'?project.name:directory}</button></div><label class="file-filter"><${Icon} name="search"/><input type="search" value=${filter} onInput=${event=>setFilter(event.currentTarget.value)} placeholder="Filter this folder" aria-label="Filter this folder" spellcheck="false" autocapitalize="off"/></label><div class="file-list">${loading&&html`<p class="tool-footnote" role="status">Loading folder…</p>`}${error&&html`<p class="field-error" role="alert">${error}</p>`}${!loading&&!error&&!entries.length&&html`<p class="tool-footnote">${filter?'No matching names.':'This folder has no previewable files.'}</p>`}${entries.map(entry=>html`<button type="button" key=${entry.path} class="file-row" data-active=${entry.path===path} aria-current=${entry.path===path?'true':undefined} onClick=${()=>open(entry)} title=${entry.path}><${Icon} name=${entry.kind==='directory'?'folder':'file'}/><span>${entry.name}</span>${entry.kind==='directory'&&html`<${Icon} name="chevron-right"/>`}</button>`)}</div>${Boolean(listing?.omitted)&&html`<p class="tool-footnote">${listing.omitted} generated, protected or unavailable ${listing.omitted===1?'entry':'entries'} omitted.</p>`}${listing?.truncated&&html`<p class="tool-footnote">Only the first 2,000 folder entries were scanned.</p>`}</nav><section class="file-preview" aria-label="File preview"><form class="file-path-form" onSubmit=${event=>{event.preventDefault();if(typedPath.trim())setPath(typedPath.trim().replaceAll('\\','/'));}}><label class="sr-only" for="file-path">Project-relative file path</label><input id="file-path" value=${typedPath} onInput=${event=>setTypedPath(event.currentTarget.value)} placeholder="Open a file by path, e.g. src/main.rs" spellcheck="false" autocapitalize="off" aria-keyshortcuts="Control+P Meta+P" aria-describedby="file-path-shortcut"/><kbd class="shortcut-hint" id="file-path-shortcut" title="Focus this field from anywhere in the workspace"><span class="sr-only">Shortcut: </span>Ctrl / ⌘ P</kbd><${Button} type="submit" disabled=${!typedPath.trim()}>Open<//></form>${path&&html`<div class="file-preview-heading"><code title=${path}>${path}</code><${CopyButton} text=${path} label="Copy path"/><${Button} icon="send" onClick=${()=>onReference(path)}>Use in task<//></div>`}<${CodePreview} file=${file} loading=${fileLoading} error=${fileError}/></section></div></main>`;
}

function Changes({api,project,onReference}) {
  const [snapshot,setSnapshot]=useState(null),[selected,setSelected]=useState(''),[scope,setScope]=useState('unstaged'),[detail,setDetail]=useState(null);
  const [loading,setLoading]=useState(true),[detailLoading,setDetailLoading]=useState(false),[error,setError]=useState(''),[detailError,setDetailError]=useState(''),[tick,setTick]=useState(0);
  const detailKey=useRef('');
  const file=snapshot?.changes.find(item=>item.path===selected);
  const refresh=()=>setTick(value=>value+1);
  useEffect(()=>{const timer=setInterval(refresh,5000);window.addEventListener('focus',refresh);return()=>{clearInterval(timer);window.removeEventListener('focus',refresh);};},[project.id]);
  useEffect(()=>{
    const abort=new AbortController();setError('');
    api.request(`/projects/${enc(project.id)}/changes`,{signal:abort.signal}).then(value=>{if(abort.signal.aborted)return;setSnapshot(value);setSelected(old=>value.changes.some(item=>item.path===old)?old:value.changes.find(item=>!item.protected)?.path||'');}).catch(error=>{if(!abort.signal.aborted)setError(errorText(error));}).finally(()=>{if(!abort.signal.aborted)setLoading(false);});
    return()=>abort.abort();
  },[project.id,tick]);
  const fileVersion=file?`${file.path}:${file.staged}:${file.unstaged}:${file.untracked}`:'';
  useEffect(()=>{
    const key=`${project.id}:${fileVersion}:${scope}`;
    if(detailKey.current!==key){setDetail(null);setDetailError('');setDetailLoading(Boolean(file&&!file.protected));detailKey.current=key;}
    if(!file||file.protected)return;
    const abort=new AbortController();
    const endpoint=file.untracked?`file?path=${enc(file.path)}`:`diff?path=${enc(file.path)}&scope=${scope}`;
    // A refresh of the same file replaces an earlier transient error once it succeeds.
    api.request(`/projects/${enc(project.id)}/${endpoint}`,{signal:abort.signal}).then(value=>{if(!abort.signal.aborted){setDetail(value);setDetailError('');}}).catch(error=>{if(!abort.signal.aborted)setDetailError(errorText(error));}).finally(()=>{if(!abort.signal.aborted)setDetailLoading(false);});
    return()=>abort.abort();
  },[project.id,fileVersion,scope,tick]);
  function choose(change){setSelected(change.path);setScope(change.unstaged.trim()&&change.unstaged!=='?'?'unstaged':'staged');}
  return html`<main class="project-tool" aria-label="Git changes"><header class="project-tool-heading"><div><p class="eyebrow">${project.name}${snapshot?.branch?` / ${snapshot.branch}`:''}</p><h1>Changes${snapshot?.available?html`<span class="change-count">${snapshot.changes.length}</span>`:''}</h1></div><${IconButton} label="Refresh changes" icon="refresh" onClick=${refresh}/></header><p class="changes-boundary">Current files on disk, including edits made outside this conversation.</p>${loading?html`<p class="tool-placeholder" role="status">Reading Git changes…</p>`:error?html`<div class="tool-placeholder"><${Notice} title="Changes could not be refreshed" tone="neutral">${error}<//></div>`:!snapshot?.available?html`<div class="tool-placeholder"><${Icon} name="junction" size=${28}/><h2>No Git repository here</h2><p>File browsing and the terminal still work for this project.</p></div>`:!snapshot.changes.length?html`<div class="tool-placeholder"><${Icon} name="check" size=${28}/><h2>Working tree is clean</h2><p>No staged, unstaged or untracked files were reported by Git.</p></div>`:html`<div class="change-workspace"><nav class="change-list" aria-label="Changed files">${snapshot.changes.map(change=>html`<button key=${change.path} type="button" class="changed-file" data-active=${change.path===selected} onClick=${()=>choose(change)} aria-current=${change.path===selected?'true':undefined} title=${change.path}><span class="change-code" aria-hidden="true">${change.untracked?'?':change.unstaged.trim()||change.staged.trim()}</span><span><strong>${change.path}</strong><small>${change.protected?'Protected path':changeLabel(change)}${!change.untracked&&change.staged.trim()?' · staged':''}${!change.untracked&&change.unstaged.trim()?' · unstaged':''}</small></span></button>`)}</nav><section class="change-detail" aria-label="Selected file changes">${file&&html`<div class="file-preview-heading"><code>${file.path}</code><${CopyButton} text=${file.path} label="Copy path"/><${Button} icon="send" onClick=${()=>onReference(file.path)}>Use in task<//></div>${file.original_path&&html`<p class="tool-footnote">Renamed from ${file.original_path}</p>`}${!file.untracked&&!file.protected&&html`<div class="change-scopes" aria-label="Diff scope"><button type="button" aria-pressed=${scope==='unstaged'} onClick=${()=>setScope('unstaged')}>Unstaged</button><button type="button" aria-pressed=${scope==='staged'} onClick=${()=>setScope('staged')}>Staged</button></div>`}${file.protected?html`<div class="tool-placeholder"><p>Contents of protected files are not previewed here.</p></div>`:file.untracked?html`<${CodePreview} file=${detail} loading=${detailLoading} error=${detailError}/>`:detailLoading?html`<p class="tool-placeholder" role="status">Reading diff…</p>`:detailError?html`<p class="tool-placeholder field-error" role="alert">${detailError}</p>`:detail?.diff?html`<div class="working-diff"><${DiffView} text=${detail.diff}/>${detail.truncated&&html`<p class="tool-footnote">This diff exceeds 256 KiB; only its beginning is shown.</p>`}</div>`:html`<p class="tool-placeholder">No ${scope} diff for this file.</p>`}`}</section></div>`}</main>`;
}

export function Workbench({api,project,tool='conversation',onToolChange,onReference,onContext,onOpenNavigation,children}) {
  const [terminalOpen,setTerminalOpen]=useState(false),[terminalMounted,setTerminalMounted]=useState(false);
  const toolRef=useRef(null);
  const handoffDrafts=useRef(new Map());
  useEffect(()=>{
    function shortcut(event){
      if(event.ctrlKey&&event.key==='`'){event.preventDefault();setTerminalMounted(true);setTerminalOpen(old=>!old);}
      if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='p'){event.preventDefault();onToolChange('files');requestAnimationFrame(()=>document.getElementById('file-path')?.focus());}
    }
    window.addEventListener('keydown',shortcut);return()=>window.removeEventListener('keydown',shortcut);
  },[onToolChange]);
  return html`<section class="workbench" aria-label="Project workspace"><header class="workbench-toolbar"><${IconButton} label="Open projects and sessions" icon="menu" class="workbench-nav-toggle" onClick=${onOpenNavigation}/><nav class="workspace-tools" aria-label="Workspace views">${[['conversation','Conversation','switch'],['files','Files','folder'],['changes','Changes','junction'],['handoffs','Handoffs','logs']].map(([value,label,icon])=>html`<button type="button" key=${value} aria-pressed=${tool===value} data-active=${tool===value} onClick=${()=>onToolChange(value)}><${Icon} name=${icon}/><span>${label}</span></button>`)}</nav><${Button} class="terminal-toggle" icon="command" aria-expanded=${terminalOpen} title="Toggle terminal (Ctrl + backtick)" aria-keyshortcuts=${"Control+`"} onClick=${()=>{setTerminalMounted(true);setTerminalOpen(old=>!old);}}><span class="terminal-label">Terminal</span><//></header><div class="workbench-content" ref=${toolRef}><div class="agent-surface" hidden=${tool!=='conversation'}>${children}</div>${tool!=='conversation'&&(!project?html`<main class="tool-placeholder"><h1>Open a project first</h1><p>Choose a local folder to browse files and review Git changes.</p></main>`:tool==='files'?html`<${Files} key=${project.id} api=${api} project=${project} onReference=${onReference}/>`:tool==='handoffs'?html`<${Handoffs} key=${project.id} api=${api} project=${project} onUse=${onContext} drafts=${handoffDrafts.current}/>`:html`<${Changes} key=${project.id} api=${api} project=${project} onReference=${onReference}/>` )}</div>${terminalMounted&&html`<${TerminalDock} api=${api} project=${project} visible=${terminalOpen} onHide=${()=>setTerminalOpen(false)}/>`}</section>`;
}
