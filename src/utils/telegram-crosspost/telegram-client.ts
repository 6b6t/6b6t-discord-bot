import type {
  CrosspostAttachment,
  TelegramCrosspostPayload,
} from "./formatter";

export type TelegramMessageKind = "text" | "photo" | "video" | "document";

export type TelegramMessageReference = {
  messageId: number;
  kind: TelegramMessageKind;
};

type TelegramApiResponse<T> =
  | { ok: true; result: T }
  | {
      ok: false;
      error_code?: number;
      description?: string;
      parameters?: { retry_after?: number };
    };

type TelegramMessageResult = { message_id: number };

export class TelegramApiError extends Error {
  readonly statusCode: number | null;
  readonly retryAfterSeconds: number | null;

  constructor(
    message: string,
    options?: { statusCode?: number | null; retryAfterSeconds?: number | null },
  ) {
    super(message);
    this.name = "TelegramApiError";
    this.statusCode = options?.statusCode ?? null;
    this.retryAfterSeconds = options?.retryAfterSeconds ?? null;
  }
}

export class TelegramDeliveryError extends Error {
  readonly deliveredMessages: TelegramMessageReference[];

  constructor(message: string, deliveredMessages: TelegramMessageReference[]) {
    super(message);
    this.name = "TelegramDeliveryError";
    this.deliveredMessages = deliveredMessages;
  }
}

type FetchImplementation = typeof fetch;

function sleep(milliseconds: number) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function getAttachmentMethod(attachment: CrosspostAttachment) {
  const contentType = attachment.contentType?.toLowerCase() ?? "";
  const filename = attachment.filename.toLowerCase();
  if (contentType.startsWith("image/") || /\.(?:jpe?g|png)$/.test(filename)) {
    return { method: "sendPhoto", field: "photo", kind: "photo" as const };
  }
  if (contentType.startsWith("video/") || /\.(?:mp4|mov)$/.test(filename)) {
    return { method: "sendVideo", field: "video", kind: "video" as const };
  }
  return {
    method: "sendDocument",
    field: "document",
    kind: "document" as const,
  };
}

export class TelegramClient {
  private readonly baseUrl: string;
  private readonly fetchImplementation: FetchImplementation;
  private readonly retryAttempts: number;
  private readonly lastRequestByChat = new Map<string, number>();

  constructor(
    token: string,
    options?: {
      fetchImplementation?: FetchImplementation;
      retryAttempts?: number;
    },
  ) {
    this.baseUrl = `https://api.telegram.org/bot${token}`;
    this.fetchImplementation = options?.fetchImplementation ?? fetch;
    this.retryAttempts = options?.retryAttempts ?? 6;
  }

  private async throttle(chatId: string) {
    const lastRequest = this.lastRequestByChat.get(chatId) ?? 0;
    const waitMilliseconds = Math.max(0, 1_050 - (Date.now() - lastRequest));
    if (waitMilliseconds > 0) await sleep(waitMilliseconds);
    this.lastRequestByChat.set(chatId, Date.now());
  }

  private async request<T>(method: string, payload: Record<string, unknown>) {
    const response = await this.fetchImplementation(
      `${this.baseUrl}/${method}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      },
    );

    let body: TelegramApiResponse<T>;
    try {
      body = (await response.json()) as TelegramApiResponse<T>;
    } catch {
      throw new TelegramApiError(
        `Telegram ${method} returned an invalid response`,
        { statusCode: response.status },
      );
    }

    if (!response.ok || !body.ok) {
      const failure = body.ok ? null : body;
      throw new TelegramApiError(
        failure?.description ?? `Telegram ${method} failed`,
        {
          statusCode: failure?.error_code ?? response.status,
          retryAfterSeconds: failure?.parameters?.retry_after ?? null,
        },
      );
    }

    return body.result;
  }

  private async requestWithRetry<T>(
    method: string,
    chatId: string,
    payload: Record<string, unknown>,
  ) {
    let lastError: unknown;
    for (let attempt = 1; attempt <= this.retryAttempts; attempt += 1) {
      try {
        await this.throttle(chatId);
        return await this.request<T>(method, payload);
      } catch (error) {
        lastError = error;
        const isPermanentClientError =
          error instanceof TelegramApiError &&
          error.statusCode !== null &&
          error.statusCode >= 400 &&
          error.statusCode < 500 &&
          error.statusCode !== 429;
        if (attempt === this.retryAttempts || isPermanentClientError) break;

        const retryAfter =
          error instanceof TelegramApiError ? error.retryAfterSeconds : null;
        const delay = retryAfter
          ? retryAfter * 1_000 + 250
          : Math.min(30_000, 1_000 * 2 ** (attempt - 1)) +
            Math.floor(Math.random() * 250);
        await sleep(delay);
      }
    }
    throw lastError;
  }

  async verifyConnection() {
    const result = await this.request<{ id: number; username?: string }>(
      "getMe",
      {},
    );
    return { id: result.id, username: result.username ?? null };
  }

  async sendPost(
    chatId: string,
    threadId: number | null,
    payload: TelegramCrosspostPayload,
  ) {
    const delivered: TelegramMessageReference[] = [];
    const threadPayload = threadId ? { message_thread_id: threadId } : {};

    try {
      for (const text of payload.textChunks) {
        const result = await this.requestWithRetry<TelegramMessageResult>(
          "sendMessage",
          chatId,
          {
            chat_id: chatId,
            text,
            link_preview_options: { is_disabled: false },
            ...threadPayload,
          },
        );
        delivered.push({ messageId: result.message_id, kind: "text" });
      }

      for (const attachment of payload.attachments) {
        const attachmentMethod = getAttachmentMethod(attachment);
        const result = await this.requestWithRetry<TelegramMessageResult>(
          attachmentMethod.method,
          chatId,
          {
            chat_id: chatId,
            [attachmentMethod.field]: attachment.url,
            ...threadPayload,
          },
        );
        delivered.push({
          messageId: result.message_id,
          kind: attachmentMethod.kind,
        });
      }
    } catch (error) {
      throw new TelegramDeliveryError(
        error instanceof Error ? error.message : "Telegram delivery failed",
        delivered,
      );
    }

    return delivered;
  }

  async deleteMessages(chatId: string, messages: TelegramMessageReference[]) {
    for (const message of messages) {
      try {
        await this.requestWithRetry<boolean>("deleteMessage", chatId, {
          chat_id: chatId,
          message_id: message.messageId,
        });
      } catch (error) {
        if (
          error instanceof TelegramApiError &&
          error.statusCode === 400 &&
          error.message.toLowerCase().includes("message to delete not found")
        ) {
          continue;
        }
        throw error;
      }
    }
  }
}
