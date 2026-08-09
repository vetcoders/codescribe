// license-issuer — signs CSK1 licence codes for codescribe.vetcoders.io.
//
// WHY A SERVICE
// -------------
// The site is static files under Caddy; a licence is an Ed25519 signature over
// the claims JSON, and the private seed must never reach a browser (a seed
// shipped in page JS makes every licence forgeable, forever). This service is
// the one place outside the operator's machine that holds the seed — via a
// root-owned systemd EnvironmentFile, never in the repository.
//
// Runs on the ops VPS behind Caddy:
//   codescribe.vetcoders.io/api/license/issue → 127.0.0.1:$PORT (same origin,
//   so the site page needs no CORS at all). Node stdlib only — node 18+.
//
// TOKEN CONTRACT (must match core/licensing/mod.rs — validate_license_key)
// ------------------------------------------------------------------------
//   CSK1.<base64url_nopad(claims_json)>.<base64url_nopad(ed25519_sig(claims_json))>
//   claims: {"v":1,"sku","email_hash","issued","updates_until","seat_limit"}
//   email_hash = sha256 hex of trim().toLowerCase() email — 64 lowercase hex
//   dates are naive YYYY-MM-DD; validation pins v==1, non-empty sku,
//   seat_limit > 0, updates_until >= issued. deny_unknown_fields is part of
//   the contract: emit EXACTLY these six fields.
//
// GATE SEAM
// ---------
// `eligibility()` is the future licence gate. Today it accepts any
// syntactically plausible email (open-beta issuance, operator decision
// 2026-08-09); later it consults an allowlist / payment record without the
// rest of this file changing. A small per-IP cooldown keeps a script from
// minting thousands of codes per second; real abuse control belongs in Caddy.
//
// CONFIG (systemd EnvironmentFile, root 0600)
//   CODESCRIBE_LICENSE_SIGNER_SEED_HEX   64-hex Ed25519 seed (required)
//   LICENSE_SKU        default "agentic-lifetime"
//   UPDATES_MONTHS     default "12"
//   SEAT_LIMIT         default "3"
//   PORT               default "8787"
//
// Created by Vetcoders (c)2026

import { createServer } from "node:http";
import { createHash, createPrivateKey, sign as edSign } from "node:crypto";

// RFC 8032 test-vector seed — the checked-in development signer. Publicly
// known, therefore forgeable; refuse it the same way license_signer.rs does.
const DEV_SEED_HEX = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

// PKCS#8 wrapper for a raw Ed25519 seed (RFC 8410): fixed 16-byte prefix + seed.
const PKCS8_ED25519_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

const PORT = parseInt(process.env.PORT ?? "8787", 10);

const seedHex = (process.env.CODESCRIBE_LICENSE_SIGNER_SEED_HEX ?? "").trim().toLowerCase();
if (!/^[0-9a-f]{64}$/.test(seedHex)) {
  console.error("CODESCRIBE_LICENSE_SIGNER_SEED_HEX must be exactly 64 hex characters");
  process.exit(1);
}
if (seedHex === DEV_SEED_HEX) {
  // A licence anyone can forge must not be issuable by the production tool.
  console.error("refusing the development seed (RFC 8032 test vector — publicly known)");
  process.exit(1);
}
const signingKey = createPrivateKey({
  key: Buffer.concat([PKCS8_ED25519_PREFIX, Buffer.from(seedHex, "hex")]),
  format: "der",
  type: "pkcs8",
});

/// The future licence gate. Open issuance today; swap the body for an
/// allowlist / payment lookup without touching the signing path.
async function eligibility(_email) {
  return { ok: true };
}

// Per-IP cooldown: one issue per IP per COOLDOWN_MS. In-memory on purpose —
// a restart forgets it, and that is fine for a brake (not a gate).
const COOLDOWN_MS = 15_000;
const lastIssue = new Map();
function throttled(ip) {
  const now = Date.now();
  const previous = lastIssue.get(ip) ?? 0;
  if (now - previous < COOLDOWN_MS) return true;
  lastIssue.set(ip, now);
  if (lastIssue.size > 10_000) lastIssue.clear();
  return false;
}

function respond(res, status, body) {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

/// Calendar month add with end-of-month clamping — matches chrono's
/// checked_add_months (Jan 31 + 1 month = Feb 28/29, never Mar 3), so a
/// site-issued licence and a CLI-issued licence never disagree on dates.
function addMonthsClamped(date, months) {
  const target = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + months, 1));
  const lastDay = new Date(
    Date.UTC(target.getUTCFullYear(), target.getUTCMonth() + 1, 0),
  ).getUTCDate();
  target.setUTCDate(Math.min(date.getUTCDate(), lastDay));
  return target;
}

const isoDate = (date) => date.toISOString().slice(0, 10);
const b64url = (buf) => buf.toString("base64url");

const server = createServer((req, res) => {
  if (req.method !== "POST" || !req.url.startsWith("/api/license/issue")) {
    respond(res, 404, { error: "POST /api/license/issue" });
    return;
  }
  const ip = req.headers["x-forwarded-for"]?.split(",")[0]?.trim() || req.socket.remoteAddress;
  if (throttled(ip)) {
    respond(res, 429, { error: "one licence per 15 seconds — try again in a moment" });
    return;
  }

  let raw = "";
  req.setEncoding("utf8");
  req.on("data", (chunk) => {
    raw += chunk;
    if (raw.length > 4096) req.destroy();
  });
  req.on("end", async () => {
    let email;
    try {
      ({ email } = JSON.parse(raw));
    } catch {
      respond(res, 400, { error: 'body must be JSON: {"email":"..."}' });
      return;
    }
    email = (email ?? "").trim().toLowerCase();
    // Syntactic sanity only — the real gate is eligibility() above.
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(email)) {
      respond(res, 400, { error: "that does not look like an email address" });
      return;
    }
    const verdict = await eligibility(email);
    if (!verdict.ok) {
      respond(res, 403, { error: verdict.reason });
      return;
    }

    const today = new Date();
    const claims = {
      v: 1,
      sku: process.env.LICENSE_SKU || "agentic-lifetime",
      email_hash: createHash("sha256").update(email).digest("hex"),
      issued: isoDate(today),
      updates_until: isoDate(addMonthsClamped(today, parseInt(process.env.UPDATES_MONTHS ?? "12", 10))),
      seat_limit: parseInt(process.env.SEAT_LIMIT ?? "3", 10),
    };
    const payload = Buffer.from(JSON.stringify(claims), "utf8");
    const signature = edSign(null, payload, signingKey);

    console.log(
      JSON.stringify({ at: new Date().toISOString(), issued_for_hash: claims.email_hash, ip }),
    );
    respond(res, 200, {
      license: `CSK1.${b64url(payload)}.${b64url(signature)}`,
      sku: claims.sku,
      issued: claims.issued,
      updates_until: claims.updates_until,
      seat_limit: claims.seat_limit,
    });
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`license-issuer listening on 127.0.0.1:${PORT}`);
});
