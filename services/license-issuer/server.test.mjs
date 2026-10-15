// Hermetic test of the issuer's request contract (CS160-R003): a body whose
// `email` is not a string is a controlled 400, the process stays up and
// serves the next valid request. Runs the real server.mjs on an ephemeral
// port with a throwaway seed.
//
//   node --test services/license-issuer/
//
// Created by Vetcoders (c)2026

import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SERVER = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "server.mjs"
);
// Any 64-hex seed except the RFC 8032 test vector, which the server refuses.
const THROWAWAY_SEED = "11".repeat(32);

let child;
let port;

before(async () => {
  child = spawn(process.execPath, [SERVER], {
    env: {
      ...process.env,
      PORT: "0",
      CODESCRIBE_LICENSE_SIGNER_SEED_HEX: THROWAWAY_SEED,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  port = await new Promise((resolve, reject) => {
    let out = "";
    let errors = "";
    child.stdout.on("data", (chunk) => {
      out += chunk;
      const bound = out.match(/listening on 127\.0\.0\.1:(\d+)/);
      if (bound) resolve(Number(bound[1]));
    });
    child.stderr.on("data", (chunk) => {
      errors += chunk;
    });
    child.on("exit", (code) =>
      reject(new Error(`server exited early (${code}): ${errors}`))
    );
  });
});

after(() => {
  child?.kill();
});

let client = 0;

/// Each request arrives from its own forwarded address, so the per-IP
/// cooldown cannot mask the verdict under test.
async function post(body) {
  const response = await fetch(`http://127.0.0.1:${port}/api/license/issue`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-forwarded-for": `10.0.0.${++client}`,
    },
    body,
  });
  return { status: response.status, json: await response.json() };
}

test("a body whose email is not a string is a 400 and the issuer stays up", async () => {
  const bodies = [
    '{"email":1}',
    '{"email":{}}',
    '{"email":[]}',
    '{"email":null}',
    '{"email":true}',
    "{}",
    "[]",
    "null",
    '"someone@example.com"',
    "1",
    "{",
    "",
  ];
  for (const body of bodies) {
    const { status, json } = await post(body);
    assert.equal(
      status,
      400,
      `body ${JSON.stringify(body)} answered ${status}`
    );
    assert.match(json.error, /JSON/, `body ${JSON.stringify(body)}`);
  }
  assert.equal(child.exitCode, null, "the issuer process is still running");
  const issued = await post('{"email":"someone@example.com"}');
  assert.equal(issued.status, 200);
  assert.match(issued.json.license, /^CSK1\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/);
  assert.equal(issued.json.seat_limit, 3);
});

test("a string that is not an email address is a 400", async () => {
  const { status, json } = await post('{"email":"not an address"}');
  assert.equal(status, 400);
  assert.match(json.error, /email address/);
});

test("an email is normalised before hashing, so case and padding do not mint distinct licences", async () => {
  const [first, second] = await Promise.all([
    post('{"email":"  Someone@Example.com "}'),
    post('{"email":"someone@example.com"}'),
  ]);
  assert.equal(first.status, 200);
  assert.equal(second.status, 200);
  const claims = (reply) =>
    JSON.parse(Buffer.from(reply.json.license.split(".")[1], "base64url"));
  assert.equal(claims(first).email_hash, claims(second).email_hash);
});
