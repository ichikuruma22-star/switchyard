# Switchya website

Static product website for `https://switchya.com`, with no build step, framework, remote fonts, analytics beacon, or form backend. The live site links to the Windows x64 portable preview `0.1.0-preview.4`, containing the native desktop shell and CLI, and to the separate public Gateway release `v0.1.1`.

The current deployment at [switchya.com](https://switchya.com/) and [www.switchya.com](https://www.switchya.com/) is version `b28762db-c871-4342-af71-bdf96816dd4b`, published on 15 October 2026. It adds the interface polish (a 1200x630 social preview card, an Apple touch icon, download requirements with the ZIP size and SHA-256, and accessibility fixes) on top of version `532e2035-69f2-4fa7-9cc3-3edf19bb5e57`, which first linked these releases. The download links point to Switchya `0.1.0-preview.4` (a bug-fix release from a full audit) and Gateway `v0.1.1` (Windows, Linux and macOS archives). The ZIP was downloaded anonymously and matched the locally verified package byte-for-byte (SHA-256 `574fc84df733e45734deccd297fd13049d0ebfe0502926c44a99bca8f5fc5529`). This deployment also added a `Strict-Transport-Security` header. The earlier releases remain available. See [deployment-readback-20261015-polish.json](./deployment-readback-20261015-polish.json) for the current asset, header, 404 and redirect checks and [deployment-readback-20261015-preview4.json](./deployment-readback-20261015-preview4.json) for the release-link deployment; [VERIFICATION.md](./VERIFICATION.md) retains the original preview publication evidence.

The design refinement was approved after desktop and phone visual review and published. The preview update kept that design and changed availability copy and links. See [DESIGN-REVIEW.md](./DESIGN-REVIEW.md) for the original review. `asset-manifest.json` describes the published public files, and the checks fail when it no longer matches `public/`. `deployment-readback-20261015.json` records the current read-back; the 11 October read-backs (`deployment-readback-20261011.json` and `deployment-readback-preview2-20261011.json`) and `deployment-readback.json`, the original publication record, are kept as history.

The portable preview targets Windows 10/11 x64 and requires the [Microsoft Visual C++ v14 x64 runtime](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170). Its desktop shell also requires Microsoft Edge WebView2 Runtime. These prerequisites are installed separately; the archive has no installer or bundled runtime DLLs. The download row states these requirements and points readers to the linked release notes for setup.

## Preview and checks

Node.js 22 or newer:

```sh
node preview.mjs
# http://127.0.0.1:18742

npm run check
```

`npm run check` also confirms that the FAQ structured data in `index.html` repeats the visible questions and that its download URL is the page's download link. After changing anything in `public/`, run `npm run manifest` to regenerate `asset-manifest.json`. After a deployment, `npm run readback -- VERSION_ID "what changed" > deployment-readback-DATE-LABEL.json` reads every published file back from switchya.com, checks www, the HTTPS redirects, the custom 404 and the security headers, and exits non-zero on any mismatch.

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

The page sections are the hero, the workspace, model access, "From download to first task" (four steps taken from the user guide), Switchyard Gateway, downloads and known limits, and a Questions section of eight `<details>` answers that work without JavaScript. Each answer states only what the docs and code support: no analytics or telemetry (no such code in the app or desktop shell), data stored under `%LOCALAPPDATA%\Switchya`, local Ollama models through the provider setup dialog, the MIT License, and the unsigned-ZIP SmartScreen warning with a `Get-FileHash` check. The head carries SoftwareApplication and FAQPage structured data; JSON-LD is a data block, so the `script-src 'self'` policy does not block it. The site follows the visitor's light or dark system theme; the product illustration stays light in both.

The hero contains labeled, illustrative interface concepts, not screenshots or recorded successful tasks. Its tabs switch explanatory views. All three views remain readable without JavaScript. The only clipboard action copies the existing `switchyard` Gateway startup command. The PowerShell CLI examples match the current native CLI parser and assume the CLI is on PATH, an existing project folder and a configured model.

The live Switchya download targets the Windows portable ZIP at tag `switchya-v0.1.0-preview.4`; other platforms are source-only and there is no macOS binary. Named Codex, Claude Code and Antigravity accounts are described without promising verified live authentication or usage. Claude Code runs in the workspace (an external terminal remains optional for managed profiles) and has no quota display; Antigravity sign-in status cannot be checked. There is no mailing-list signup, pricing page, unsupported competitor claim, or fabricated adoption data. Before future publication, check the release and asset anonymously. The original design passed desktop, tablet, phone and narrow-screen layout checks and mouse/ArrowRight/Home/End preview-tab navigation. On 28 October 2026 an axe-core 4.10.3 audit (WCAG 2.2 AA plus best-practice rules) found no violations in the light and dark themes at 1440 and 390 CSS pixels, on every preview tab and with every FAQ answer open. All 42 keyboard stops showed a visible focus ring in order, and at the equivalent of 200% zoom (640 CSS pixels) the page reflowed with no horizontal scrolling or clipped text; the main navigation stays on one row from 360 pixels and wraps at 320. Screen-reader testing remains unverified.
