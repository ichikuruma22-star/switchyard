/** Serialize preference writes so an older, slower request cannot win a newer
 * selection. Failed navigation saves never submit or retry an agent task. */
export function selectionWriter(api, onError = () => {}) {
  let pending = null, running = false;
  async function drain() {
    if (running) return;
    running = true;
    try {
      while (pending) {
        const selection = pending; pending = null;
        try { await api.request('/workspace', {method:'POST', body:selection}); onError(''); }
        catch (error) { onError(error?.message || 'Workspace navigation could not be saved.'); }
      }
    } finally { running = false; }
  }
  return selection => { pending = {...selection}; void drain(); };
}
