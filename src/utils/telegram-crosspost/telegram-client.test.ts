import assert from "node:assert/strict";
import test from "node:test";
import { TelegramClient } from "./telegram-client";

test("sends a Telegram text message to the configured topic", async () => {
  let request:
    | { url: string; method: string | undefined; body: Record<string, unknown> }
    | undefined;
  const fetchImplementation: typeof fetch = async (input, init) => {
    request = {
      url: String(input),
      method: init?.method,
      body: JSON.parse(String(init?.body)) as Record<string, unknown>,
    };
    return new Response(
      JSON.stringify({ ok: true, result: { message_id: 123 } }),
      { status: 200, headers: { "Content-Type": "application/json" } },
    );
  };
  const client = new TelegramClient("test-token", {
    fetchImplementation,
    retryAttempts: 1,
  });

  const delivered = await client.sendPost("-1001234567890", 42, {
    textChunks: ["Hello Telegram"],
    attachments: [],
  });

  assert.deepEqual(delivered, [{ messageId: 123, kind: "text" }]);
  assert.equal(request?.url.endsWith("/sendMessage"), true);
  assert.equal(request?.method, "POST");
  assert.deepEqual(request?.body, {
    chat_id: "-1001234567890",
    text: "Hello Telegram",
    link_preview_options: { is_disabled: false },
    message_thread_id: 42,
  });
});
