# license-issuer

Signs CSK1 licence codes for the form at <https://codescribe.vetcoders.io/license/>.
Node stdlib only (node 18+), Ed25519 via `node:crypto`; the token contract is
`core/licensing/mod.rs` — see the header of `server.mjs`.

## Deployment (ops VPS, same box as pensieve.vetcoders.io)

| Piece        | Location                                                                                                      |
| ------------ | ------------------------------------------------------------------------------------------------------------- |
| Service code | `/srv/codescribe-license/server.mjs`                                                                          |
| systemd unit | `/etc/systemd/system/codescribe-license.service`                                                              |
| Signing seed | `/etc/codescribe/license-signer.env` — root:root **0600**, read by systemd before dropping to `User=libraxis` |
| Caddy route  | `codescribe.vetcoders.io` vhost: `handle /api/* → reverse_proxy 127.0.0.1:8787`                               |
| Site root    | `/srv/codescribe-landing` (Astro `site/dist/`)                                                                |

Update flow after editing `server.mjs`:

```bash
scp services/license-issuer/server.mjs libraxis-vm:/tmp/server.mjs
ssh libraxis-vm 'sudo mv /tmp/server.mjs /srv/codescribe-license/server.mjs \
  && sudo systemctl restart codescribe-license'
```

Site redeploy:

```bash
cd site && npm run build
rsync -az --delete dist/ libraxis-vm:/tmp/codescribe-landing-stage/
ssh libraxis-vm 'sudo rsync -a --delete /tmp/codescribe-landing-stage/ /srv/codescribe-landing/ \
  && rm -rf /tmp/codescribe-landing-stage'
```

## Open-beta posture (operator decision, 2026-08-09)

`eligibility()` accepts any syntactically plausible email — the form is an
open-beta distribution channel, not a paywall. When licensing becomes a gate,
swap that one function for an allowlist / payment-record lookup; the signing
path does not change. A 15 s per-IP cooldown brakes scripted minting; real
abuse control belongs in a Caddy rate-limit rule on `/api/license/*`.

The issuer logs only `sha256(email)` + IP + timestamp — the same hash the
licence itself carries, so the log can answer "was this licence issued here?"
without storing addresses.

## Verifying end to end

```bash
curl -s -X POST https://codescribe.vetcoders.io/api/license/issue \
  -H 'content-type: application/json' -d '{"email":"you@example.com"}'
# then validate the returned token against the production public key:
CODESCRIBE_LICENSE_PUBLIC_KEY_HEX=$(cat ~/.vibecrafted/secrets/codescribe/license-public.hex) \
  cargo run -q --release -p codescribe-core --example check_license -- 'CSK1.…'
```

Both must agree on fingerprint `b90538f0…` (the key baked into release builds).
