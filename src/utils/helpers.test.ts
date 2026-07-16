import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { getTopRank } from "./helpers";

const ENV_KEYS = [
  "HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL",
  "HTTP_SLAVE1_COMMAND_SERVICE_ACCESS_TOKEN",
  "HTTP_PROXY_COMMAND_SERVICE_BASE_URL",
  "HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN",
] as const;

const originalEnv = Object.fromEntries(
  ENV_KEYS.map((key) => [key, process.env[key]]),
);
const originalFetch = globalThis.fetch;

afterEach(() => {
  for (const key of ENV_KEYS) {
    const value = originalEnv[key];
    if (value === undefined) {
      delete process.env[key];
    } else {
      process.env[key] = value;
    }
  }
  globalThis.fetch = originalFetch;
});

function successfulRankResponse(ranks: string[]) {
  return new Response(JSON.stringify({ success: true, ranks }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

test("falls back to proxy command-service credentials for rank lookup", async () => {
  delete process.env.HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL;
  delete process.env.HTTP_SLAVE1_COMMAND_SERVICE_ACCESS_TOKEN;
  process.env.HTTP_PROXY_COMMAND_SERVICE_BASE_URL = "https://proxy.example";
  process.env.HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN = "proxy-token";

  let request: { input: string; authorization: string | null } | undefined;
  globalThis.fetch = async (input, init) => {
    request = {
      input: String(input),
      authorization: new Headers(init?.headers).get("Authorization"),
    };
    return successfulRankResponse(["apex"]);
  };

  assert.equal(await getTopRank("TestPlayer"), "apex");
  assert.deepEqual(request, {
    input: "https://proxy.example/get-ranks",
    authorization: "proxy-token",
  });
});

test("prefers Slave1 command-service credentials when configured", async () => {
  process.env.HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL = "https://slave.example";
  process.env.HTTP_SLAVE1_COMMAND_SERVICE_ACCESS_TOKEN = "slave-token";
  process.env.HTTP_PROXY_COMMAND_SERVICE_BASE_URL = "https://proxy.example";
  process.env.HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN = "proxy-token";

  let request: { input: string; authorization: string | null } | undefined;
  globalThis.fetch = async (input, init) => {
    request = {
      input: String(input),
      authorization: new Headers(init?.headers).get("Authorization"),
    };
    return successfulRankResponse(["legend"]);
  };

  assert.equal(await getTopRank("TestPlayer"), "legend");
  assert.deepEqual(request, {
    input: "https://slave.example/get-ranks",
    authorization: "slave-token",
  });
});
