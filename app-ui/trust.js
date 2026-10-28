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
  return 'Edits and commands need approval';
}

export const autoApproveTone=policy=>normalPolicy(policy).commands?'stop':normalPolicy(policy).edits?'caution':'neutral';

/** The strongest trust the dialog's choices turn on anywhere. The project setting
 * reaches its other sessions and existing-agent runs even when this session
 * overrides it, so it counts whatever the session chooses. */
export function trustWarning(projectPolicy,sessionPolicy){
  const project=normalPolicy(projectPolicy),session=normalPolicy(sessionPolicy);
  if(project.commands||session.commands)return 'commands';
  if(project.edits||session.edits)return 'edits';
  return null;
}

/** One toggle. Its name is the short title; the explanation is its description, so a
 * screen reader does not read the whole paragraph as the checkbox's name. */
function Toggle({id,checked,onChange,title,hint}){
  return html`<label class="trust-toggle" for=${id}><input id=${id} type="checkbox" checked=${checked} aria-labelledby=${`${id}-title`} aria-describedby=${`${id}-hint`} onChange=${event=>onChange(event.currentTarget.checked)}/><span><strong id=${`${id}-title`}>${title}</strong><span class="field-hint" id=${`${id}-hint`}>${hint}</span></span></label>`;
}

/** The project and the session each have a set; the group takes its section's heading
 * as its name so the two identical sets stay distinguishable. */
function Toggles({value,onChange,disabled,idPrefix,labelledBy}){
  return html`<fieldset class="trust-toggles" disabled=${disabled} aria-labelledby=${labelledBy}><${Toggle} id=${`${idPrefix}-edits`} checked=${value.edits} onChange=${edits=>onChange({...value,edits})} title="Apply file edits without asking" hint="Each edit is still recorded with its diff. Repository metadata (.git), likely secrets, links and Windows short-name aliases stay blocked. These still ask: edits outside the project; edits to Switchya’s own data or gateway configuration, or to an agent account’s settings; edits to files other programs load by themselves (shell profiles, Git, editor and agent settings); and every edit in a project that contains your home folder."/><${Toggle} id=${`${idPrefix}-commands`} checked=${value.commands} onChange=${commands=>onChange({...value,commands})} title="Run shell commands without asking" hint="Commands run with your account’s full machine and network access. The project folder is not a sandbox."/></fieldset>`;
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
  const warning=trustWarning(projectPolicy,session&&sessionMode==='custom'?sessionPolicy:null);
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
    <p class="field-hint">Off by default. When on, the built-in agent and existing agents go ahead without asking you about each edit or command. Every edit and command is still recorded, and you can interrupt at any time. A change applies at once, also to work in progress: operations that have not started follow the new setting, and a request already waiting still needs your decision.</p>
    <section class="trust-section" aria-labelledby="trust-project"><h3 id="trust-project">Project: ${project?.name||'No project'}</h3><${Toggles} idPrefix="trust-project" labelledBy="trust-project" value=${projectPolicy} onChange=${setProjectPolicy} disabled=${busy||!project}/></section>
    ${session&&html`<section class="trust-section" aria-labelledby="trust-session"><h3 id="trust-session">This session</h3><div class="trust-mode" role="radiogroup" aria-labelledby="trust-session"><label><input type="radio" name="trust-mode" checked=${sessionMode==='inherit'} onChange=${()=>setSessionMode('inherit')} disabled=${busy}/> Use the project setting</label><label><input type="radio" name="trust-mode" checked=${sessionMode==='custom'} onChange=${()=>setSessionMode('custom')} disabled=${busy}/> Set for this session only</label></div>${sessionMode==='custom'&&html`<${Toggles} idPrefix="trust-session" labelledBy="trust-session" value=${sessionPolicy} onChange=${setSessionPolicy} disabled=${busy}/>`}</section>`}
    ${warning==='commands'?html`<${Notice} title="Commands will run without review" tone="stop">The agent can run any shell command with your permissions, including commands that delete files, change other projects or use the network. The project setting also applies to its other sessions and to existing-agent runs. Turn this on only for projects and models you trust.<//>`:warning==='edits'?html`<${Notice} title="Edits will apply without review" tone="caution">Review the recorded changes and diffs afterwards. Edits are not rolled back automatically. The project setting also applies to its other sessions and to existing-agent runs.<//>`:null}
    ${error&&html`<p class="field-error" role="alert">${error}</p>`}
  </div><footer class="form-actions"><${Button} onClick=${onClose} disabled=${busy}>Cancel<//><${Button} type="submit" variant="primary" disabled=${busy||!project} busy=${busy}>${busy?'Saving…':'Save'}<//></footer></form><//>`;
}
