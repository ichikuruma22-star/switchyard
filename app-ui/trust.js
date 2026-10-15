import { html, useEffect, useState } from './vendor/preact-htm.js';
import { Button, Dialog, Notice } from './components.js';

const OFF={edits:false,commands:false};
export const normalPolicy=policy=>({edits:policy?.edits===true,commands:policy?.commands===true});

/** The policy that applies to the next operation: the session's own view, or the
 * project's setting before a session exists. */
export function effectivePolicy(project,view){
  if(view?.auto_approve)return {...normalPolicy(view.auto_approve),source:view.auto_approve.source==='session'?'session':'project'};
  return {...normalPolicy(project?.auto_approve),source:'project'};
}

export function autoApproveLabel(policy){
  const {edits,commands}=normalPolicy(policy);
  if(edits&&commands)return 'Edits and commands run without asking';
  if(edits)return 'Edits apply without asking · commands need approval';
  if(commands)return 'Commands run without asking · edits need approval';
  return 'Writes and commands need approval';
}

export const autoApproveTone=policy=>normalPolicy(policy).commands?'stop':normalPolicy(policy).edits?'caution':'neutral';

function Toggles({value,onChange,disabled,idPrefix}){
  return html`<fieldset class="trust-toggles" disabled=${disabled}><label class="trust-toggle" for=${`${idPrefix}-edits`}><input id=${`${idPrefix}-edits`} type="checkbox" checked=${value.edits} onChange=${event=>onChange({...value,edits:event.currentTarget.checked})}/><span><strong>Apply file edits without asking</strong><span class="field-hint">Edits are still journalled with their diff. Repository metadata, likely secrets, links and short-name aliases stay blocked, and edits to Switchya’s own state or gateway configuration still ask.</span></span></label><label class="trust-toggle" for=${`${idPrefix}-commands`}><input id=${`${idPrefix}-commands`} type="checkbox" checked=${value.commands} onChange=${event=>onChange({...value,commands:event.currentTarget.checked})}/><span><strong>Run shell commands without asking</strong><span class="field-hint">Commands run with your account’s full machine and network access. The project folder is not a sandbox.</span></span></label></fieldset>`;
}

export function TrustDialog({open,onClose,project,view,onSaveProject,onSaveSession}){
  const session=view?.session||null;
  const [projectPolicy,setProjectPolicy]=useState(OFF), [sessionMode,setSessionMode]=useState('inherit'), [sessionPolicy,setSessionPolicy]=useState(OFF);
  const [busy,setBusy]=useState(false), [error,setError]=useState('');
  useEffect(()=>{
    if(!open)return;
    setError('');setProjectPolicy(normalPolicy(project?.auto_approve));
    setSessionMode(session?.auto_approve?'custom':'inherit');setSessionPolicy(normalPolicy(session?.auto_approve||project?.auto_approve));
  },[open,project?.id,session?.id]);
  const next=session&&sessionMode==='custom'?sessionPolicy:projectPolicy;
  async function save(event){
    event.preventDefault();if(busy||!project)return;
    setBusy(true);setError('');
    try{
      const current=normalPolicy(project.auto_approve);
      if(current.edits!==projectPolicy.edits||current.commands!==projectPolicy.commands)await onSaveProject(project.id,projectPolicy);
      if(session){
        const policy=sessionMode==='custom'?sessionPolicy:null, saved=session.auto_approve?normalPolicy(session.auto_approve):null;
        if(JSON.stringify(policy)!==JSON.stringify(saved))await onSaveSession(session.id,policy);
      }
      onClose();
    }catch(error){setError(error?.message||'The approval setting could not be saved.');}
    finally{setBusy(false);}
  }
  return html`<${Dialog} open=${open} onClose=${()=>!busy&&onClose()} title="Approval settings" class="trust-dialog"><form onSubmit=${save}><div class="dialog-body">
    <p class="field-hint">Off by default. When on, the built-in agent proceeds without a per-operation prompt. Every edit and command is still recorded in the session, and you can interrupt at any time. Changes apply to the next proposed operation; one already waiting still needs your decision.</p>
    <section class="trust-section" aria-labelledby="trust-project"><h3 id="trust-project">Project: ${project?.name||'No project'}</h3><${Toggles} idPrefix="trust-project" value=${projectPolicy} onChange=${setProjectPolicy} disabled=${busy||!project}/></section>
    ${session&&html`<section class="trust-section" aria-labelledby="trust-session"><h3 id="trust-session">This session</h3><div class="trust-mode" role="radiogroup" aria-labelledby="trust-session"><label><input type="radio" name="trust-mode" checked=${sessionMode==='inherit'} onChange=${()=>setSessionMode('inherit')} disabled=${busy}/> Use the project setting</label><label><input type="radio" name="trust-mode" checked=${sessionMode==='custom'} onChange=${()=>setSessionMode('custom')} disabled=${busy}/> Set for this session only</label></div>${sessionMode==='custom'&&html`<${Toggles} idPrefix="trust-session" value=${sessionPolicy} onChange=${setSessionPolicy} disabled=${busy}/>`}</section>`}
    ${next.commands?html`<${Notice} title="Commands will run without review" tone="stop">The agent can run any shell command with your permissions, including commands that delete files, change other projects or use the network. Turn this on only for projects and models you trust.<//>`:next.edits?html`<${Notice} title="Edits will apply without review" tone="caution">Review the session’s recorded changes and diffs afterwards. Edits are not rolled back automatically.<//>`:null}
    ${error&&html`<p class="field-error" role="alert">${error}</p>`}
  </div><footer class="form-actions"><${Button} onClick=${onClose} disabled=${busy}>Cancel<//><${Button} type="submit" variant="primary" disabled=${busy||!project} busy=${busy}>${busy?'Saving…':'Save'}<//></footer></form><//>`;
}
