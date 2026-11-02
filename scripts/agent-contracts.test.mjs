import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { contract, preflight, modelRequest, searchRequest, parseModel, parseSearch,
  toolResult, outcome, publicIp, validateUrl, validatePageTrace } from "./agent-contracts.mjs";

const fixture = (name) => JSON.parse(readFileSync(new URL(`../docs/agent-integrations/fixtures/${name}.json`, import.meta.url)));
const model = fixture("model");
const search = fixture("search");
const copy = structuredClone;
const configured = { provider: "anthropic", model: contract.model.id, endpoint: contract.model.endpoint,
  modelKey: true, searchEnabled: true, searchProvider: "exa", searchKey: true, persistenceBasis: "fixture-only-agreement" };

describe("agent integration contract v1 (offline specification, not live certification)", () => {
  it("gates optional paid search and unsupported configurations without changing existing AI settings", () => {
    assert.equal(preflight(configured), "ready");
    for (const [patch, expected] of [
      [{ searchEnabled: false }, "search_not_configured"],
      [{ searchProvider: "unknown" }, "unsupported_search"],
      [{ searchKey: false }, "missing_credentials"],
      [{ modelKey: false }, "missing_credentials"],
      [{ persistenceBasis: null }, "persistence_rights_required"],
      [{ provider: "custom" }, "unsupported"],
      [{ endpoint: "https://other.example/v1/messages" }, "unsupported"],
      [{ model: "invented-latest" }, "unsupported"],
    ]) assert.equal(preflight({ ...configured, ...patch }), expected);
    assert.equal(contract.search.enabledByDefault, false);
    assert.notEqual(contract.keychain.modelAccount, contract.keychain.searchAccount);
  });

  it("matches frozen model and Exa requests, including disabled parallel calls and bounded search", () => {
    assert.deepEqual(modelRequest(model.request.body.messages, model.request.body.tools), model.request.body);
    assert.deepEqual(searchRequest("Example Summit official speakers"), search.request);
    for (const n of [contract.limits.queryCharacters - 1, contract.limits.queryCharacters]) {
      assert.doesNotThrow(() => searchRequest("😀".repeat(n)));
    }
    assert.throws(() => searchRequest("x".repeat(contract.limits.queryCharacters + 1)));
    assert.throws(() => searchRequest("  "));
  });

  it("preserves tool IDs across model response and adjacent tool-result continuation", () => {
    const parsed = parseModel(model.response);
    assert.deepEqual(parsed, model.normalized);
    assert.deepEqual(toolResult(parsed, { results: [] }), model.continuation);
    assert.deepEqual(parseSearch(search.response), search.normalized);
    assert.deepEqual(parseSearch({ results: [] }), []);
    assert.equal(parseSearch({ results: [{ title: "Title", url: "https://example.com" }] })[0].excerpt, "");
  });

  it("rejects malformed, multiple, unknown, and incomplete model actions before execution", () => {
    const changes = [
      (b) => { b.content[0].name = "send_mail"; },
      (b) => { b.content[0].input = { query: "Hi", accountId: "other" }; },
      (b) => { b.content[0].input.query = null; },
      (b) => { b.content.push(copy(b.content[0])); },
      (b) => { b.content[0].id = ""; },
      (b) => { b.stop_reason = "end_turn"; },
      (b) => { b.stop_reason = "future_unknown_reason"; },
      (b) => { b.usage.input_tokens = -1; },
      (b) => { b.content[0] = { type: "server_tool_use", name: "web_search" }; },
    ];
    for (const change of changes) {
      const body = copy(model.response); change(body);
      assert.throws(() => parseModel(body));
    }
    const truncated = { ...copy(model.response), stop_reason: "max_tokens" };
    assert.deepEqual(parseModel(truncated), { kind: "truncated", calls: [] });
    const answer = { ...copy(model.response), stop_reason: "end_turn", content: [{ type: "text", text: "Unverified audience." }] };
    assert.deepEqual(parseModel(answer), { kind: "answer", text: "Unverified audience." });
    // An answer is not a published report: publication requires native schema/citation validation.
    for (const b of [null, {}, "not JSON", { ...search.response, results: null }]) {
      assert.throws(() => parseSearch(b));
    }
    assert.throws(() => parseSearch({ results: [{ title: "X", url: "javascript:alert(1)", highlights: [] }] }));
    assert.throws(() => parseSearch({ results: [{ title: "X", url: "https://example.com", highlights: [42] }] }));
  });

  for (const f of fixture("outcomes")) it(f.id, () => {
    assert.deepEqual(outcome(f.service, f.response), f.expected);
  });

  for (const f of fixture("malformed")) it(f.id, () => {
    const parse = f.adapter === "model" ? parseModel : parseSearch;
    assert.throws(() => parse(JSON.parse(f.rawBody)));
  });

  it("bounds retries and preserves usage for ambiguous dispatches", () => {
    assert.equal(outcome("search", { status: 429 }, 1).afterSeconds, 4);
    assert.equal(outcome("search", { status: 429 }, 2).kind, "retry_exhausted");
    assert.equal(outcome("search", { status: 429 }, 3).kind, "retry_exhausted");
    for (const n of [299, 300]) assert.equal(outcome("search", { status: 429, headers: { "retry-after": String(n) } }).kind, "retry");
    for (const raw of ["301", "garbage", "-1", "Wed, 21 Oct 2026 07:28:00 GMT"]) {
      assert.equal(outcome("model", { status: 429, headers: { "retry-after": raw } }).kind, "paused_retry_review");
    }
    assert.equal(outcome("model", { status: 200 }).kind, "parse");
  });

  it("requires public IPs, including IPv6, without trusting a hostname-only check", () => {
    for (const ip of ["8.8.8.8", "93.184.215.14", "2606:4700:4700::1111"]) assert.equal(publicIp(ip), true, ip);
    for (const ip of ["127.0.0.1", "10.0.0.1", "100.64.0.1", "169.254.169.254", "198.18.0.1",
      "192.0.2.1", "224.0.0.1", "::1", "fe80::1", "fc00::1", "::ffff:8.8.8.8", "::ffff:127.0.0.1", "2001:db8::1", "2002:0808:0808::", "64:ff9b::a00:1"]) {
      assert.equal(publicIp(ip), false, ip);
    }
    for (const url of ["http://example.com", "https://user:secret@example.com", "https://example.com:444",
      "https://127.1", "https://[::1]", "https://[::ffff:127.0.0.1]", "https://localhost", "https://printer.local", "https://intranet"]) {
      assert.throws(() => validateUrl(url), url);
    }
  });

  it("checks DNS, pinned peers, each redirect, content type, encoding, and byte limits", () => {
    const f = fixture("page");
    assert.equal(validatePageTrace(f.hops), f.normalized);
    const page = f.hops[0];
    for (const patch of [
      { resolved: ["93.184.215.14", "10.0.0.1"] },
      { peer: "8.8.8.8" }, { peer: "127.0.0.1" }, { resolved: [] },
      { contentType: "application/pdf" }, { contentType: "text/html; charset=shift_jis" },
      { contentEncoding: "gzip" }, { bytes: contract.limits.responseBytes + 1 },
    ]) assert.throws(() => validatePageTrace([{ ...page, ...patch }]));
    for (const bytes of [contract.limits.responseBytes - 1, contract.limits.responseBytes]) {
      assert.equal(validatePageTrace([{ ...page, bytes }]), "accepted");
    }
    const redirect = { ...page, status: 302, location: "/report" };
    assert.equal(validatePageTrace([{ ...redirect, url: "https://example.com/start" }, page]), "accepted");
    assert.throws(() => validatePageTrace([{ ...redirect, location: "https://127.0.0.1/report" }, { ...page, url: "https://127.0.0.1/report" }]));
    assert.throws(() => validatePageTrace([redirect, { ...page, resolved: ["10.0.0.1"], peer: "10.0.0.1" }]));
    for (const n of [contract.limits.redirects - 1, contract.limits.redirects]) {
      assert.equal(validatePageTrace([...Array.from({ length: n }, () => copy(redirect)), page]), "accepted");
    }
    assert.throws(() => validatePageTrace([...Array.from({ length: contract.limits.redirects + 1 }, () => copy(redirect)), page]));
  });

  it("keeps routing explicitly unavailable without invented distance or duration", () => {
    assert.equal(contract.routing.provider, null);
    assert.deepEqual(fixture("routing").normalized, { kind: "unavailable", reason: "not_configured", distanceMeters: null, durationSeconds: null });
  });
});
