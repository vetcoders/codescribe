# Mac App Store Readiness

Status as of `0.12.2` (source) on branch `fix/make-stop-process-match`.

> **Verdict: NO-GO for the Mac App Store as a single SKU covering the current
> product.** The shipping lane today is **Developer ID + notarization** (outside
> the App Store), and that is the correct lane for the product as it exists. A
> Mac App Store build is feasible only as a **separate, sandbox-clean "Basic"
> SKU** with a materially reduced feature set — and even that carries real App
> Review risk (see Accessibility, below). This document is the plan; it does not
> change the build.

Run the read-only check any time:

```bash
./scripts/appstore-preflight.sh   # exit 0 = no P0 blockers, 1 = blockers present
```

Last run: **3 P0 blockers, 6 P1 warnings**.

---

## Why this matters

CodeScribe's product direction is a **dictation-driven orchestration agent**, not
a dictation toy. The Agentic mode spawns MCP servers, shells out to external
binaries (Vibecrafted), reads broad file context, controls other apps, and uses
global hotkeys. Those capabilities are the product — and they are exactly what
the App Sandbox forbids. So "ship to the App Store" is not a packaging task; it
is a question of **which product** goes to the store.

---

## The hard constraints (Apple, official)

| # | Constraint | Source |
|---|------------|--------|
| 1 | **App Sandbox is required** for Mac App Store apps. Builds without `com.apple.security.app-sandbox = true` are rejected at submission. | [App Sandbox](https://developer.apple.com/documentation/security/app-sandbox); rejection text confirmed on [Apple Developer Forums](https://developer.apple.com/forums/thread/41400) |
| 2 | **Developer ID + notarization** is the *outside-the-store* lane; the store lane needs an **Apple Distribution / "3rd Party Mac Developer"** signing identity, a `.pkg` (`productbuild`), and an App Store Connect upload (Transporter). | [Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) |
| 3 | **Privacy manifest (`PrivacyInfo.xcprivacy`)** with declared **required-reason API** usage is mandatory for App Store Connect submissions since **2024-05-01**. | [Privacy updates for App Store submissions](https://developer.apple.com/news/?id=3d8a9yyh), [Reminder: starts May 1](https://developer.apple.com/news/?id=pvszzano) |
| 4 | **App Privacy Details** ("nutrition labels") must be completed in App Store Connect before publishing — including microphone data. | [App Privacy Details](https://developer.apple.com/app-store/app-privacy-details/) |
| 5 | **Apple Events to other apps** under sandbox need a `scripting-targets` entitlement or an `apple-events` **temporary exception** — which is "carefully reviewed, and **most often rejected**" by App Review. | [App Sandbox Temporary Exception Entitlements](https://developer.apple.com/library/archive/documentation/Miscellaneous/Reference/EntitlementKeyReference/Chapters/AppSandboxTemporaryExceptionEntitlements.html), [QA1888](https://developer.apple.com/library/archive/qa/qa1888/_index.html) |
| 6 | **Accessibility used for non-accessibility purposes** (pasting into / driving other apps) is rejected under **Guideline 2.4.5**. | Apple Developer Forums review reports; App Store Review Guidelines 2.4.5 |
| 7 | **Input Monitoring** (CGEventTap *listen-only*, via `CGPreflightListenEventAccess` / `CGRequestListenEventAccess`) **is available to sandboxed Mac App Store apps**. Global-hotkey *detection* can survive sandbox; *controlling other apps* cannot. | [Xojo: Sandboxing to Notarization](https://blog.xojo.com/2024/08/22/macos-apps-from-sandboxing-to-notarization-the-basics/), [Beyond App Sandbox](https://www.appcoda.com/mac-app-sandbox/) |
| 8 | `allow-unsigned-executable-memory` and `allow-jit` are, per Apple's own entitlement docs, **compatible with both the Mac App Store and Developer ID**. The harder conflict is `disable-library-validation`, which fights the sandbox rule that nested code be team-signed. **Mark as uncertain until validated against a real `productbuild` + App Store Connect upload.** | [allow-unsigned-executable-memory](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.allow-unsigned-executable-memory), [disable-library-validation](https://developer.apple.com/documentation/BundleResources/Entitlements/com.apple.security.cs.disable-library-validation) |

---

## Current state vs. Mac App Store requirement

| Surface | Today (verified in repo) | MAS requirement | Gap |
|---------|--------------------------|-----------------|-----|
| **Sandbox** | `scripts/entitlements.plist` explicitly **disables** App Sandbox (documented as "outside Mac App Store") | `app-sandbox = true` mandatory | **P0** |
| **Entitlements** | `disable-library-validation`, `allow-unsigned-executable-memory`, `allow-dyld-environment-variables` — all required by embedded Whisper/MiniLM dylibs | Sandboxed apps must team-sign nested code; `disable-library-validation` conflicts | **P0/uncertain** |
| **Privacy manifest** | none (`PrivacyInfo.xcprivacy` absent); app reads file mtimes via `std::fs` `metadata().modified()` in `core/state/history.rs`, `core/hf_cache.rs`, `core/attachment.rs` → **FileTimestamp** required-reason category, reason code **C617.1** (metadata of files in the app's own containers) | `PrivacyInfo.xcprivacy` declaring `NSPrivacyAccessedAPICategoryFileTimestamp` / `C617.1` | **P0** (draft template: `scripts/PrivacyInfo.xcprivacy.template`) |
| **App Privacy Details** | only a written `docs/guide/privacy.md`; no App Store Connect record | Nutrition-label questionnaire completed | **P1** (process, blocked on having an app record) |
| **Purpose strings** | Mic, Accessibility, Input Monitoring, Screen Capture, Apple Events — generated in `Makefile` bundle target (lines 127–131) | Mic + Input Monitoring OK; Accessibility/Apple Events review-risky | **P1** |
| **Basic vs Agentic** | Onboarding has Basic (safe default) + Agentic lanes; Agentic probes MCP readiness | Agentic capabilities are sandbox-incompatible | **architecture** |
| **MCP / Vibecrafted** | Agentic mode shells out, spawns MCP, reads broad files | Forbidden under sandbox | **P0 for Agentic SKU** |
| **Signing/upload** | Developer ID → notarytool → stapler → Gatekeeper (DMG); `.github/workflows/release.yml` | Apple Distribution cert → `.pkg` → App Store Connect | **P0** |
| **Release gates / PRView** | PR35 (release-forward) not yet main-ready; signing secrets unset; live release `v0.8.0` ≪ source `0.12.2` | Green release pipeline | **P1** |
| **Cold install smoke** | DMG drag-install + Gatekeeper drill documented in `PUBLIC_RELEASE_CHECKLIST.md` | TestFlight / store install | **P1** |

---

## Recommended path: two SKUs

Marked uncertain where Apple's real truth needs a live submission to confirm.

1. **Developer ID SKU (primary, ships today).** The full product — Agentic
   orchestration, MCP, Vibecrafted, global hotkeys, paste-into-other-apps,
   embedded ML. Stays exactly as it is. This is where the product lives.

2. **Mac App Store SKU (Basic, new build profile).** A sandbox-clean dictation
   app: microphone → transcript → its own window / clipboard copy. **Drops**:
   Accessibility paste, Apple Events focus-restore, Agentic/MCP/Vibecrafted,
   broad file access, Full Disk Access probing. **Keeps** (pending validation):
   Input Monitoring listen-only hotkeys, embedded Whisper/MiniLM (subject to the
   `disable-library-validation` question). Even this SKU risks rejection if the
   transcript is delivered by driving another app via Accessibility — keep
   delivery in-app or via standard clipboard.

A **single SKU covering both is not viable**: the sandbox cannot be both on (for
the store) and off (for the agent). Do not attempt one binary for both.

---

## Blockers, ordered

**P0 — must change before any MAS submission is even possible**

1. **No App Sandbox** — would require a separate sandboxed build profile;
   incompatible with the current feature set.
2. **No App Store distribution path** — no Apple Distribution identity, no
   `productbuild`/`.pkg`, no App Store Connect upload step anywhere in the repo.
3. **No privacy manifest** — `PrivacyInfo.xcprivacy` is required and absent;
   FileTimestamp required-reason usage is confirmed in code.

**P1 — required but not the gate**

4. **Bundle-id split** — `com.codescribe.app` (Makefile/Info.plist) vs
   `com.vetcoders.codescribe` (`core/config/keychain.rs:15`, `release.yml:73`).
   A store app record needs one canonical id; this split also fragments TCC and
   keychain identity today. **Fix is out-of-scope here** (touches the dirty
   `Makefile` on this branch) — see follow-up prompt.
5. **Accessibility / Apple Events purpose strings** — review-risky for a store
   build; fine for Developer ID.
6. **Release pipeline not green** — PR35 unresolved; signing secrets unset; live
   release lags source. (Tracked in `PUBLIC_RELEASE_CHECKLIST.md`.)

**P2 — only after a sandboxed build exists**

7. App Privacy Details questionnaire, App Store screenshots/metadata, TestFlight
   cold-install smoke.

---

## Research verification — live Apple sources (2026-06-27)

The constraints above were re-checked against live Apple/official sources during
an ERi (Examine → Research → Implement) pass. Each core claim held; precision was
added where the first draft was coarse.

| Claim | Verdict (live) | Source |
|-------|----------------|--------|
| App Sandbox (`com.apple.security.app-sandbox = true`) is required for every Mac App Store app; builds without it are rejected at submission | **Confirmed** | [App Sandbox Entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.app-sandbox); rejection text on [Apple Developer Forums 41400](https://developer.apple.com/forums/thread/41400) |
| MAS lane needs an **Apple Distribution** signing identity + a **3rd Party Mac Developer Installer** `.pkg` via `productbuild`, uploaded with **Transporter** to App Store Connect — distinct from Developer ID + `notarytool` (outside-store lane; `altool` retired 2023-11-01) | **Confirmed** | [Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution), [Distributing software on macOS](https://developer.apple.com/macos/distribution/), [Uploading macOS Builds to App Store Connect (Xojo, 2025)](https://blog.xojo.com/2025/01/14/uploading-macos-builds-to-app-store-connect/) |
| **App Privacy Details** ("nutrition labels") are mandatory to submit new apps/updates, apply to macOS, and require declaring **Audio Data** with purpose + linkage + tracking answers | **Confirmed** | [App Privacy Details](https://developer.apple.com/app-store/app-privacy-details/) |
| **Privacy manifest** (`PrivacyInfo.xcprivacy`) with **required-reason API** declarations is enforced since **2024-05-01**; apps without it are rejected | **Confirmed** | [Privacy updates for App Store submissions](https://developer.apple.com/news/?id=3d8a9yyh), [Reminder: starts May 1](https://developer.apple.com/news/?id=pvszzano) |
| CodeScribe's `metadata().modified()` usage maps to **FileTimestamp** category, reason code **C617.1** (metadata of files in the app's own containers); `DDA9.1` is the alternate (show timestamps to the user, no off-device send) | **Confirmed + made precise** | [NSPrivacyAccessedAPIType](https://developer.apple.com/documentation/bundleresources/app-privacy-configuration/nsprivacyaccessedapitypes/nsprivacyaccessedapitype) |

**Honest nuance:** the hardest-edged 2024-05-01 gate is scoped most strictly to
*third-party SDKs on Apple's commonly-used list*, but the required-reason
*declaration* obligation also covers an app's own first-party usage of those
APIs. CodeScribe uses a FileTimestamp API directly, so the manifest is required
regardless of SDKs.

**Still uncertain (needs a real `productbuild` + App Store Connect upload to
settle):** whether `disable-library-validation` (required today by the embedded
Whisper/MiniLM dylibs) can coexist with a sandboxed MAS build, and whether
Input-Monitoring listen-only hotkeys survive App Review in practice. Do not
assert either way from documentation alone.

## What this repo change does and does NOT do

- **Adds** this document, `scripts/appstore-preflight.sh` (read-only check), and
  `scripts/PrivacyInfo.xcprivacy.template` (a clearly-marked DRAFT manifest, not
  wired into any build — a guardrail/starting point for the future Basic SKU).
- **Does not** enable the sandbox, change entitlements, alter signing, touch the
  `Makefile`, or modify any PR36 work. Those are deliberate follow-ups, gated on
  the operator's decision to actually stand up a second SKU.
