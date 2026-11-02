// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/dsh_notify.mjs. Copyright (c) 2026 HS7097. MIT: see ../LICENSE-codex-goal-lifecycle.txt.
// Workflow #432 AGT-T3 (contracts/agent-forward.md): the Runtime embeds this file and runs it as
// `node --input-type=module -e <this file>`. The adapted source's `observe` mode is left out.

import { readFile } from "node:fs/promises";
import { isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const schema = "dsh-native-notification/v1";
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

class NotifyError extends Error {
  constructor(code, message) { super(message); this.code = code; }
}
const fail = (code, message) => { throw new NotifyError(code, message); };

export function validateInput(input) {
  if (!object(input) || !["check", "send"].includes(input.mode)) fail("INPUT", "Expected check or send mode.");
  const target = input.target;
  if (!object(target) || ![target.app_root, target.home_root].every((value) => typeof value === "string" && isAbsolute(value))
      || typeof target.session_id !== "string" || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,255}$/.test(target.session_id)) {
    fail("TARGET", "Expected absolute DSH roots and one exact session identifier.");
  }
  if (!Number.isInteger(input.timeout_ms) || input.timeout_ms < 1000 || input.timeout_ms > 120000) {
    fail("TIMEOUT", "DSH notification timeout must be between 1000 and 120000 ms.");
  }
  if (input.mode === "send" && (!uuid.test(input.request_id) || !uuid.test(input.rpc_id)
      || input.request_id === input.rpc_id || typeof input.text !== "string" || !input.text.trim()
      || Buffer.byteLength(input.text, "utf8") > 1024 * 1024)) {
    fail("MESSAGE", "Expected distinct request identities and a non-empty UTF-8 message of at most 1 MiB.");
  }
  return input;
}

export function localEndpoint(text) {
  let url;
  try { url = new URL(text.trim()); } catch { fail("ENDPOINT", "DSH .web-url is not an absolute URL."); }
  if (!['http:', 'https:'].includes(url.protocol) || !['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname)
      || url.username || url.password || url.hash) {
    fail("ENDPOINT", "DSH .web-url must identify a local loopback HTTP endpoint.");
  }
  return url;
}

export function exactEvents(events, requestId, text) {
  if (!Array.isArray(events)) fail("READBACK", "DSH durable events are missing.");
  const found = [];
  for (const event of events) {
    const messages = event.type === "user/message" ? [event.data]
      : event.type === "agent/inbox/spliced" ? event.data?.inserted : [];
    for (const message of messages ?? []) {
      if (message?.source?.kind !== "user" || message.source.rpcId !== requestId) continue;
      if (!Array.isArray(message.content) || message.content.length !== 1
          || message.content[0].type !== "text" || message.content[0].text !== text) {
        fail("IDENTITY_CONFLICT", "The DSH request identity already belongs to different content.");
      }
      if (!Number.isSafeInteger(event.seq) || event.seq < 0) fail("READBACK", "DSH readback has an invalid event sequence.");
      found.push({ seq: event.seq, type: event.type, time: event.time });
    }
  }
  return found;
}

async function nativeReader(target) {
  const load = (name) => import(pathToFileURL(join(target.app_root, "node_modules", "@deepseek-ai", name, "lib/index.js")).href);
  const [{ Context }, { default: Persistence }] = await Promise.all([load("cordis"), load("dsh-session-persistence-jsonl")]);
  const persistence = new Persistence(new Context(), { root: join(target.home_root, "sessions"), compression: "zstd" });
  return async () => {
    if (typeof await persistence.resolveCurrentLog(target.session_id) !== "string") {
      fail("SESSION", "The selected DSH session has no native durable log.");
    }
    const handle = await persistence.open(target.session_id, "read");
    try {
      if (handle.id !== target.session_id) fail("SESSION", "DSH native session identity differs.");
      const { events } = await handle.read();
      if (!Array.isArray(events)) fail("SESSION", "DSH native event list is missing.");
      return events;
    } finally { await handle.close(); }
  };
}

function redact(error, secrets) {
  let message = String(error?.message ?? error);
  for (const secret of [...secrets].filter(Boolean).sort((a, b) => b.length - a.length)) {
    message = message.split(secret).join("[REDACTED]");
  }
  return message;
}

export async function execute(raw, dependencies = {}) {
  const secrets = new Set();
  const fetcher = dependencies.fetch ?? fetch;
  const sleep = dependencies.sleep ?? ((ms) => new Promise((done) => setTimeout(done, ms)));
  const now = dependencies.now ?? Date.now;
  const result = { schema, status: "not_submitted", accepted: false, events: [] };
  let posted = false;
  let input;
  let abortTimer;
  try {
    input = validateInput(raw);
    result.session_id = input.target.session_id;
    result.request_id = input.request_id;
    result.rpc_id = input.rpc_id;
    const deadline = now() + input.timeout_ms;
    const abort = new AbortController();
    abortTimer = setTimeout(() => abort.abort(), input.timeout_ms);
    const reader = dependencies.readEvents ?? await nativeReader(input.target);
    const before = await reader();
    if (input.mode === "send") {
      result.events = exactEvents(before, input.request_id, input.text);
      if (result.events.length) {
        return { ...result, status: "recorded", accepted: true, submission: "already_recorded" };
      }
    }
    const url = localEndpoint(await (dependencies.readEndpoint ?? (() => readFile(join(input.target.home_root, ".web-url"), "utf8")))());
    secrets.add(url.href);
    for (const value of url.searchParams.values()) secrets.add(value);
    const bootstrap = await fetcher(url, { redirect: "manual", signal: abort.signal });
    const location = bootstrap.headers.get("location");
    if (location && new URL(location, url).origin !== url.origin) fail("AUTH", "DSH authentication redirected to another origin.");
    const cookies = bootstrap.headers.getSetCookie().map((value) => value.split(";", 1)[0]);
    for (const cookie of cookies) secrets.add(cookie.slice(cookie.indexOf("=") + 1));
    await bootstrap.body?.cancel();
    if (![200, 302, 303, 307, 308].includes(bootstrap.status) || !cookies.length) {
      fail("AUTH", `DSH authentication did not establish a session (HTTP ${bootstrap.status}).`);
    }
    if (input.mode === "check") return { ...result, status: "ready", last_seq: before.at(-1)?.seq ?? null };
    if (now() >= deadline) fail("TIMEOUT", "DSH preparation exceeded the notification deadline; no prompt submitted.");
    const body = {
      type: "client-request", rpcId: input.rpc_id, method: "session/prompt",
      payload: { args: { request: {
        requestId: input.request_id, sessionId: input.target.session_id, mode: "queue",
        content: [{ type: "text", text: input.text }],
      } } },
    };
    // This is the only submission. A lost acknowledgement or readback never repeats it.
    posted = true;
    const response = await fetcher(new URL("/api/session/prompt", url.origin), {
      method: "POST", redirect: "error", signal: abort.signal,
      headers: { "content-type": "application/json", cookie: cookies.join("; ") }, body: JSON.stringify(body),
    });
    const reply = await response.json();
    if (reply?.rpcId !== input.rpc_id) fail("ACK_IDENTITY", "DSH acknowledgement has another RPC identity; delivery is unknown.");
    if (reply.result?.ok === false) {
      result.status = "rejected";
      const code = String(reply.result.error?.code ?? "unknown").replace(/[^a-zA-Z0-9_./:-]/g, "").slice(0, 100);
      fail("REJECTED", `DSH rejected the prompt (${code}).`);
    }
    if (!response.ok || reply.result?.ok !== true || reply.result.value?.accepted !== true) {
      fail("ACK", `DSH did not return a matching accepted acknowledgement (HTTP ${response.status}); delivery is unknown.`);
    }
    result.accepted = true;
    while (now() < deadline) {
      result.events = exactEvents(await reader(), input.request_id, input.text);
      if (result.events.length) return { ...result, status: "recorded", submission: "submitted_once" };
      await sleep(Math.min(500, Math.max(0, deadline - now())));
    }
    fail("READBACK_PENDING", "DSH accepted the prompt, but exact durable readback did not arrive before the deadline; inspect this request without resubmitting.");
  } catch (error) {
    if (result.status !== "rejected") result.status = posted ? "unknown" : "not_submitted";
    result.error = { code: error.code ?? "DSH_NOTIFY", message: redact(error, secrets) };
    return result;
  } finally { clearTimeout(abortTimer); }
}

async function main() {
  let input;
  try {
    const chunks = [];
    let size = 0;
    for await (const chunk of process.stdin) {
      size += chunk.length;
      if (size > 2 * 1024 * 1024) fail("INPUT", "DSH notification input exceeds 2 MiB.");
      chunks.push(chunk);
    }
    input = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks)));
  } catch (error) {
    const result = { schema, status: "not_submitted", error: { code: "INPUT", message: String(error.message) } };
    process.stderr.write(`${JSON.stringify({ level: "ERROR", ...result.error })}\n`);
    process.stdout.write(`${JSON.stringify(result)}\n`);
    process.exitCode = 1;
    return;
  }
  const result = await execute(input);
  if (!["ready", "recorded"].includes(result.status)) {
    process.stderr.write(`${JSON.stringify({ level: "ERROR", ...result.error })}\n`);
    process.exitCode = 1;
  }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

if (process.argv[1] === undefined || import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
