# Switchya website

Static product website for `https://switchya.com`, with no build step, framework, remote fonts, analytics beacon, or form backend. The live site links to the Windows x64 portable preview `0.1.0-preview.3`, containing the native desktop shell and CLI. Existing Gateway downloads still link to the separate public Gateway release.

The current deployment at [switchya.com](https://switchya.com/) and [www.switchya.com](https://www.switchya.com/) is version `3206b649-2467-4cde-90ea-b8c40a8ed1f7`, published on 15 October 2026. The download links point to Switchya `0.1.0-preview.3` (sign-in methods, profile removal, CLI account switching, Antigravity agent). The ZIP was downloaded anonymously and matched the locally verified package byte-for-byte (SHA-256 `5b32faced261321728b04d2b14d1e481cf1725f2c7dc8a1832cbd42a73fff503`). The earlier releases remain available. See [deployment-readback-20261015.json](./deployment-readback-20261015.json) for the current asset and endpoint checks; [VERIFICATION.md](./VERIFICATION.md) retains the original preview publication evidence.

The design refinement was approved after desktop and phone visual review and published. The preview update kept that design and changed availability copy and links. See [DESIGN-REVIEW.md](./DESIGN-REVIEW.md) for the original review. `asset-manifest.json` describes the published public files; `deployment-readback-preview2-20261011.json` records the current read-back; `deployment-readback.json` preserves the original publication record.

The portable preview targets Windows 10/11 x64 and requires the [Microsoft Visual C++ v14 x64 runtime](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170). Its desktop shell also requires Microsoft Edge WebView2 Runtime. These prerequisites are installed separately; the archive has no installer or bundled runtime DLLs. The download row states these requirements and points readers to the linked release notes for setup.

## Preview and checks

Node.js 22 or newer:

```sh
node preview.mjs
# http://127.0.0.1:18742

npm run check
```

The preview server binds only to loopback and applies the site's global response headers. It does not emulate every Cloudflare routing or caching behavior. Set `SWITCHYA_SITE_PORT` to change its port. No package installation is required for this preview or the checks.

For Cloudflare's local static-asset behavior, install the pinned development dependency and use `npm run dev:cloudflare`. Stop the lightweight server first because both use port 18742.

## Hosting configuration

`wrangler.jsonc` uses Workers Static Assets with `public/` as the asset directory and `404-page` handling. Custom-domain routes attach switchya.com and www.switchya.com; workers_dev is disabled. No Worker code, account ID in source, storage binding, or backend is configured. `public/_headers` supplies the security policy. Static files and fonts are served from the same origin.

Wrangler is pinned to **4.149.0**, verified against the npm registry on 11 October 2026. Its Miniflare dependency uses patched `sharp 0.35.5`, addressing [GHSA-wq5f-xc86-pv6w](https://github.com/advisories/GHSA-wq5f-xc86-pv6w) in the development tooling. Its declared Node requirement is `>=22.0.0`. The compatibility date is `2026-10-03`.

For an authorized update after visual review, verify the account with `npx wrangler whoami`, run the checks and a deployment dry run, then use `npm run deploy`. Read back both domains, assets, custom 404, HTTPS redirects, and headers afterward. Regenerate asset-manifest.json when public files change. The global cache policy includes no-transform; at the last deployment read-back, live HTML matched its deployed source byte for byte, with no injected analytics beacon. The font rule replaces the global Cache-Control value with a 30-day cache policy.

Official configuration references checked for this implementation:

- [Static asset configuration](https://developers.cloudflare.com/workers/static-assets/binding/)
- [Custom 404 handling](https://developers.cloudflare.com/workers/static-assets/routing/static-site-generation/)
- [Static response headers](https://developers.cloudflare.com/workers/static-assets/headers/)
- [Wrangler 4.149.0 release](https://github.com/cloudflare/workers-sdk/releases/tag/wrangler%404.149.0)

## Content and assets

Brand rules live in `../docs/BRAND.md`. The SVG mark is copied unchanged from `../app-ui/assets/switchya-mark.svg`. Archivo and JetBrains Mono are copied from the app's self-hosted font files, with their OFL notices.

The hero contains labeled, illustrative interface concepts, not screenshots or recorded successful tasks. Its tabs switch explanatory views. All three views remain readable without JavaScript. The only clipboard action copies the existing `switchyard` Gateway startup command. The PowerShell CLI examples match the current native CLI parser and assume the CLI is on PATH, an existing project folder and a configured model.

The live Switchya download targets the Windows portable ZIP at tag `switchya-v0.1.0-preview.3`; other platforms are source-only and there is no macOS binary. Named Codex, Claude Code and Antigravity accounts are described without promising verified live authentication or usage. Claude Code runs in the workspace (an external terminal remains optional for managed profiles) and has no quota display; Antigravity sign-in status cannot be checked. There is no mailing-list signup, pricing page, unsupported competitor claim, or fabricated adoption data. Before future publication, check the release and asset anonymously. The original design passed desktop, tablet, phone and narrow-screen layout checks and mouse/ArrowRight/Home/End preview-tab navigation. Zoom, a full keyboard audit, and a full accessibility audit remain unverified.
