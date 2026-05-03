import { randomUUID } from "node:crypto";

export interface PendingBannerRequest {
  id: string;
  submitterId: string;
  submitterTag: string;
  imageUrl: string;
  guildId: string;
  channelId: string;
  messageId: string | null;
  createdAt: number;
}

export const TTL_MS = 60 * 60 * 1000; // 1 hour

const pendingRequests = new Map<string, PendingBannerRequest>();

export function createRequest(data: {
  submitterId: string;
  submitterTag: string;
  imageUrl: string;
  guildId: string;
  channelId: string;
}): string {
  const id = randomUUID();
  pendingRequests.set(id, {
    id,
    ...data,
    messageId: null,
    createdAt: Date.now(),
  });
  return id;
}

export function getRequest(id: string): PendingBannerRequest | null {
  const request = pendingRequests.get(id);
  if (!request) return null;

  if (Date.now() - request.createdAt > TTL_MS) {
    pendingRequests.delete(id);
    return null;
  }

  return request;
}

export function setMessageId(id: string, messageId: string): void {
  const request = pendingRequests.get(id);
  if (request) {
    request.messageId = messageId;
  }
}

export function removeRequest(id: string): void {
  pendingRequests.delete(id);
}

export function cleanupExpired(): void {
  const now = Date.now();
  for (const [id, request] of pendingRequests) {
    if (now - request.createdAt > TTL_MS) {
      pendingRequests.delete(id);
    }
  }
}

// Auto cleanup every 10 minutes
setInterval(cleanupExpired, 10 * 60 * 1000).unref();
