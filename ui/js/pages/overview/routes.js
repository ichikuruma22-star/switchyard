// Overview: the routes yard. Where each model's requests go right now, drawn
// with the TrackDiagram: models on the left, providers on the right, the
// rails that take requests lit and those held back by a cooldown in amber.
//
// The routes come from /models; their state from /providers, which the
// credential frames keep current, so a cooldown shows on its rail as soon as
// it starts and clears when it ends, without a reload (see routeAvailability).

import { html, useMemo } from '../../../vendor/preact-htm.js';
import { ErrorState, Panel, Skeleton, TrackDiagram } from '../../components/index.js';
import { formatCountdownWords, plural } from '../../lib/format.js';
import { useIsPhone } from '../../lib/hooks.js';
import { href } from '../../lib/router.js';
import { kindInfo } from '../providers/model.js';
import { useServerNow } from './data.js';
import { litRouteIndexes, routeAvailability, summarizeProvider } from './model.js';
import { described } from './stale.js';

// More models than this and the yard stops being readable at a glance; the
// rest are a link away. A phone draws the yard as a list, a model and its
// routes per entry, so it shows fewer.
export const ROUTES_SHOWN = 8;
const ROUTES_SHOWN_PHONE = 4;

const MODEL_NOTE = { stop: 'no credential can serve it', off: 'switched off' };

/**
 * The diagram's models, providers and routes. Exported for tests.
 *
 * entries   routeAvailability(...)
 * providers GET /providers (may be undefined)
 * requests  Map(model name -> requests in the last 24 hours)
 * now       the gateway's clock
 * limit     how many models to draw at most
 */
export function routeYard(entries, providers, requests, now, limit = ROUTES_SHOWN) {
  const routable = (entries ?? []).filter((entry) => !entry.alias && entry.routes.length > 0);
  // Trouble first, then the busiest, then the order of GET /models.
  const ranked = routable
    .map((entry, index) => ({ entry, index, trouble: entry.routes.some((r) => r.state !== 'clear'), requests: requests.get(entry.name) ?? 0 }))
    .sort((a, b) => Number(b.trouble) - Number(a.trouble) || b.requests - a.requests || a.index - b.index)
    .slice(0, limit);

  const models = [];
  const routes = [];
  const providerNames = [];
  for (const { entry } of ranked) {
    const lit = litRouteIndexes(entry.routes);
    // Tiers by priority, highest first: 1 is where requests go first.
    const tiers = [...new Set(entry.routes.map((r) => r.priority))].sort((a, b) => b - a);
    // One rail per provider: two upstream ids on one provider share it.
    const byProvider = new Map();
    entry.routes.forEach((route, index) => {
      const rail = { model: entry.name, provider: route.provider, order: tiers.indexOf(route.priority) + 1, state: route.state, lit: lit.has(index), until: route.until };
      const seen = byProvider.get(route.provider);
      if (!seen || (rail.lit && !seen.lit) || rail.order < seen.order) byProvider.set(route.provider, rail);
    });
    const rails = [...byProvider.values()];
    for (const rail of rails) {
      routes.push(rail);
      if (!providerNames.includes(rail.provider)) providerNames.push(rail.provider);
    }

    let note;
    if (!rails.some((r) => r.state === 'clear')) {
      const held = rails.filter((r) => r.state === 'caution' && r.until != null);
      if (held.length > 0) note = `back in ${formatCountdownWords((Math.min(...held.map((r) => r.until)) - now) / 1000)}`;
      else if (rails.some((r) => r.state === 'caution')) note = 'cooling down';
      else note = MODEL_NOTE[rails[0]?.state] ?? undefined;
    }
    models.push({ id: entry.name, label: entry.name, note });
  }

  const byName = new Map((providers ?? []).map((p) => [p.name, p]));
  const providerNodes = providerNames.map((name) => {
    const provider = byName.get(name);
    if (!provider) {
      // Not in the provider list: judged by its routes in GET /models alone.
      const states = routes.filter((r) => r.provider === name).map((r) => r.state);
      return { id: name, label: name, state: states.includes('clear') ? 'clear' : 'caution', note: 'Details not loaded' };
    }
    const summary = summarizeProvider(provider, now);
    return { id: name, label: name, state: summary.tone, note: `${kindInfo(provider.kind).label} · ${summary.counts.ready} of ${summary.counts.total} ready` };
  });

  return { models, providers: providerNodes, routes, total: routable.length, aliases: (entries ?? []).filter((entry) => entry.alias).length };
}

/**
 * models     /models (useResource)
 * providers  /providers (useResource), patched by credential frames
 * top        /usage/summary?range=24h (useResource): orders the models by use
 */
export default function Routes({ models, providers, top }) {
  const now = useServerNow(1000);
  const limit = useIsPhone() ? ROUTES_SHOWN_PHONE : ROUTES_SHOWN;
  const requests = useMemo(() => new Map((top.data?.by_model ?? []).map((m) => [m.name, m.requests ?? 0])), [top.data]);
  const yard = useMemo(() => routeYard(routeAvailability(models.data, providers.data, now), providers.data, requests, now, limit), [models.data, providers.data, requests, now, limit]);

  // Nothing can be routed yet: the setup steps say how to change that.
  if (models.data && yard.total === 0) return null;

  const capped = yard.total > yard.models.length;
  const footer = html`
    <span>${capped ? `Showing ${yard.models.length} of ${plural(yard.total, 'model')}` : yard.aliases > 0 ? `${plural(yard.total, 'model')}; aliases are on the Models page` : plural(yard.total, 'model')}</span>
    <a href=${href('/models')}>Models</a>
  `;

  return html`
    <${Panel}
      class="overview-routes"
      title="Routes"
      description=${described("Where each model's requests go now. Lit rails take requests; amber rails are cooling down.", models, providers)}
      footer=${models.data ? footer : null}
    >
      ${models.loading && !models.data
        ? html`<div class="overview-routes-skeleton" aria-hidden="true"><${Skeleton} lines=${4} /></div>`
        : models.error && !models.data
          ? html`<${ErrorState} compact title="Could not load the models" error=${models.error} onRetry=${models.refresh} />`
          : html`<${TrackDiagram} models=${yard.models} providers=${yard.providers} routes=${yard.routes} label="Routes from models to providers, right now" />`}
    <//>
  `;
}
