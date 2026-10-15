import { html, useEffect, useRef, useState } from './vendor/preact-htm.js';
import { Button, IconButton } from './components.js';
import { displayPath } from './presentation.js';

const enc=encodeURIComponent;
const errorText=error=>error?.message||'The terminal could not be reached.';
const wait=milliseconds=>new Promise(resolve=>setTimeout(resolve,milliseconds));

/** Split paste input at Unicode boundaries while respecting the host byte cap. */
export function inputChunks(text, limit=4096) {
  const chunks=[];let current='',bytes=0;
  for(const character of text){const size=new TextEncoder().encode(character).length;if(bytes+size>limit&&current){chunks.push(current);current='';bytes=0;}current+=character;bytes+=size;}
  if(current)chunks.push(current);return chunks;
}

function TerminalScreen({api,id,visible,onStatus}) {
  const container=useRef(null),terminal=useRef(null),fit=useRef(null);
  const [error,setError]=useState('');
  useEffect(()=>{
    if(!container.current||!globalThis.Terminal||!globalThis.FitAddon){setError('The terminal renderer did not load. Reload the app to try again.');return;}
    const abort=new AbortController();let stopped=false,timer,resizeTimer,cursor=0,initialized=false,sequence=0,sending=false,inputPaused=false,pending=[];
    const term=new globalThis.Terminal({fontFamily:'"JetBrains Mono", monospace',fontSize:12,lineHeight:1.2,scrollback:2000,cursorBlink:true,screenReaderMode:true,disableStdin:true,theme:{background:'#101820',foreground:'#e8eef5',cursor:'#76c9f8',selectionBackground:'#35546b'},linkHandler:{activate(_event,text){try{const url=new URL(text);if(['http:','https:'].includes(url.protocol))window.open(url.href,'_blank','noopener,noreferrer');}catch{}}}});
    const fitter=new globalThis.FitAddon.FitAddon();term.loadAddon(fitter);term.open(container.current);terminal.current=term;fit.current=fitter;
    async function send(){
      if(sending||inputPaused||stopped||!initialized)return;sending=true;
      try{
        while(pending.length&&!stopped){
          const data=pending.shift(),number=sequence+1;let failure;
          for(let attempt=0;attempt<3&&!stopped;attempt++){
            try{const ack=await api.request(`/terminals/${enc(id)}/input`,{method:'POST',body:{sequence:number,data},signal:abort.signal});if(ack.sequence!==number)throw new Error('Terminal input acknowledgement did not match.');failure=null;break;}
            catch(error){failure=error;if(!error.uncertain&&error.status!==409)break;await wait(100*(attempt+1));}
          }
          if(stopped)return;
          if(failure){inputPaused=true;pending=[];term.options.disableStdin=true;setError(`${errorText(failure)} Input is paused. Reconnect to check the shell before typing again.`);return;}
          sequence=number;
        }
      }finally{sending=false;}
    }
    const dataListener=term.onData(data=>{
      if(!initialized||inputPaused||stopped)return;
      const chunks=inputChunks(data);
      if(chunks.length+pending.length>64){setError('The paste is too large for the terminal input queue. Paste smaller portions.');return;}
      pending.push(...chunks);void send();
    });
    function resize(){
      clearTimeout(resizeTimer);resizeTimer=setTimeout(()=>{
        if(stopped||!container.current?.offsetWidth||!container.current?.offsetHeight)return;
        fitter.fit();if(!initialized)return;
        api.request(`/terminals/${enc(id)}/resize`,{method:'POST',body:{cols:Math.max(2,Math.min(500,term.cols)),rows:Math.max(1,Math.min(300,term.rows))},signal:abort.signal}).catch(error=>{if(!stopped&&error.code!=='terminal_conflict')setError(errorText(error));});
      },80);
    }
    const observer=new ResizeObserver(resize);observer.observe(container.current);resize();
    async function poll(){
      try{
        const {terminal:state}=await api.request(`/terminals/${enc(id)}?after_seq=${cursor}`,{signal:abort.signal});if(stopped)return;
        onStatus(state);
        if(!initialized){sequence=state.input_sequence||0;initialized=true;term.options.disableStdin=state.state!=='running';resize();}
        if(state.first_seq>cursor+1){term.reset();term.writeln('[Earlier output exceeded the retained terminal history.]');cursor=state.first_seq-1;}
        for(const chunk of state.chunks||[]){if(chunk.seq<=cursor)continue;const bytes=Uint8Array.from(atob(chunk.data),character=>character.charCodeAt(0));term.write(bytes);cursor=chunk.seq;}
        if(state.error)setError(state.error);
        if(state.state==='exited'&&state.output_done&&cursor>=state.last_seq){term.options.disableStdin=true;return;}
        timer=setTimeout(poll,cursor<state.last_seq?0:100);
      }catch(error){if(!stopped){setError(errorText(error));timer=setTimeout(poll,1000);}}
    }
    poll();
    return()=>{stopped=true;abort.abort();clearTimeout(timer);clearTimeout(resizeTimer);observer.disconnect();dataListener.dispose();terminal.current=null;fit.current=null;term.dispose();};
  },[api,id]);
  useEffect(()=>{if(visible)requestAnimationFrame(()=>{fit.current?.fit();terminal.current?.focus();});},[visible,id]);
  return html`<div class="terminal-screen"><div class="terminal-emulator" ref=${container} aria-label="Interactive terminal"/>${error&&html`<p class="terminal-error" role="alert">${error}</p>`}</div>`;
}

export function TerminalDock({api,project,visible,onHide}) {
  const [terminals,setTerminals]=useState([]),[selected,setSelected]=useState(''),[busy,setBusy]=useState(false),[error,setError]=useState(''),[expanded,setExpanded]=useState(false),[tick,setTick]=useState(0),[reconnect,setReconnect]=useState(0);
  const choices=useRef(new Map()),pendingStart=useRef(null),currentProject=useRef(project?.id);currentProject.current=project?.id;
  const active=terminals.find(item=>item.id===selected);
  useEffect(()=>{
    setTerminals([]);setSelected(choices.current.get(project?.id)||'');setError('');pendingStart.current=null;
  },[project?.id]);
  useEffect(()=>{
    if(!project)return;const abort=new AbortController();
    api.request(`/terminals?project_id=${enc(project.id)}`,{signal:abort.signal}).then(result=>{
      if(abort.signal.aborted)return;setTerminals(result.terminals);
      setSelected(old=>result.terminals.some(item=>item.id===old)?old:result.terminals.find(item=>item.state==='running')?.id||result.terminals.at(-1)?.id||'');
    }).catch(error=>{if(!abort.signal.aborted)setError(errorText(error));});
    return()=>abort.abort();
  },[project?.id,tick]);
  useEffect(()=>{if(project&&selected)choices.current.set(project.id,selected);},[project?.id,selected]);
  async function start(){
    if(!project||busy)return;setBusy(true);setError('');const projectId=project.id;
    const id=pendingStart.current||crypto.randomUUID();pendingStart.current=id;
    try{
      const {terminal}=await api.request('/terminals',{method:'POST',body:{id,project_id:projectId,cols:100,rows:18}});
      if(currentProject.current!==projectId)return;
      pendingStart.current=null;setTerminals(old=>[...old.filter(item=>item.id!==id),terminal]);setSelected(id);
    }catch(error){if(currentProject.current===projectId){setError(errorText(error));if(!error.uncertain)pendingStart.current=null;}}
    finally{setBusy(false);}
  }
  async function close(){
    if(!selected||busy)return;setBusy(true);setError('');
    try{await api.request(`/terminals/${enc(selected)}/close`,{method:'POST'});setTick(value=>value+1);}
    catch(error){setError(errorText(error));}finally{setBusy(false);}
  }
  function status(value){setTerminals(old=>{const existing=old.find(item=>item.id===value.id);return existing&&(existing.state!==value.state||existing.exit_code!==value.exit_code)?old.map(item=>item.id===value.id?{...item,...value,chunks:[]}:item):old;});}
  return html`<section class="terminal-dock" data-expanded=${expanded} hidden=${!visible} aria-label="Project terminal"><header class="terminal-toolbar"><strong>Terminal</strong>${terminals.length>0&&html`<select aria-label="Selected terminal" value=${selected} onChange=${event=>setSelected(event.currentTarget.value)}>${terminals.map((item,index)=>html`<option key=${item.id} value=${item.id}>${index+1}: ${item.shell}${item.state==='exited'?` · exited ${item.exit_code??''}`:''}</option>`)}</select>`}<span class="terminal-status">${active?.state==='running'?'Running':active?.state==='closing'?'Stopping…':active?.state==='exited'?`Exited${active.exit_code===null?'':` (${active.exit_code})`}`:''}</span><${IconButton} label=${pendingStart.current?'Retry same terminal start':'New terminal'} icon="plus" onClick=${start} disabled=${!project||busy}/>${active&&html`<${IconButton} label="Reconnect terminal view" icon="refresh" onClick=${()=>{setReconnect(value=>value+1);setTick(value=>value+1);}}/><${IconButton} label="Stop terminal and its commands" icon="stop" onClick=${close} disabled=${active.state!=='running'||busy}/>`}<${IconButton} label=${expanded?'Reduce terminal height':'Expand terminal'} icon=${expanded?'chevron-down':'chevron-up'} onClick=${()=>setExpanded(!expanded)}/><${IconButton} label="Hide terminal" icon="x" onClick=${onHide}/></header>${error&&html`<p class="terminal-error" role="alert">${error}</p>`}${active?html`<${TerminalScreen} key=${`${project?.id}:${selected}:${reconnect}`} api=${api} id=${selected} visible=${visible} onStatus=${status}/>`:html`<div class="terminal-empty"><p>${project?'Run commands alongside your conversation.':'Open a project to start a terminal.'}</p>${project&&html`<code title=${project.root}>${displayPath(project.root)}</code><p class="field-hint">Commands run with your local permissions. Terminals stop when Switchya closes.</p><${Button} icon="command" onClick=${start} disabled=${busy}>${pendingStart.current?'Retry same terminal start':'Start terminal'}<//>`}</div>`}</section>`;
}
