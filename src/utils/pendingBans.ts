import { randomUUID } from "node:crypto";

export interface PendingBanRequest {
  id: string;
  submitterId: string;
  submitterTag: string;
  targetId: string;
  targetTag: string;
  reason: string;
  deleteMessageDays: number;
  guildId: string;
  channelId: string;
  messageId: string | null;
  createdAt: number;
}

export const BAN_TTL_MS = 60 * 60 * 1000; // 1 hour

const pendingBans = new Map<string, PendingBanRequest>();

export function createBanRequest(data: {
  submitterId: string;
  submitterTag: string;
  targetId: string;
  targetTag: string;
  reason: string;
  deleteMessageDays: number;
  guildId: string;
  channelId: string;
}): string {
  const id = randomUUID();
  pendingBans.set(id, {
    id,
    ...data,
    messageId: null,
    createdAt: Date.now(),
  });
  return id;
}

export function getBanRequest(id: string): PendingBanRequest | null {
  const request = pendingBans.get(id);
  if (!request) return null;

  if (Date.now() - request.createdAt > BAN_TTL_MS) {
    pendingBans.delete(id);
    return null;
  }

  return request;
}

export function setBanMessageId(id: string, messageId: string): void {
  const request = pendingBans.get(id);
  if (request) {
    request.messageId = messageId;
  }
}

export function removeBanRequest(id: string): void {
  pendingBans.delete(id);
}

export function cleanupExpiredBans(): void {
  const now = Date.now();
  for (const [id, request] of pendingBans) {
    if (now - request.createdAt > BAN_TTL_MS) {
      pendingBans.delete(id);
    }
  }
}

// Auto cleanup every 10 minutes
setInterval(cleanupExpiredBans, 10 * 60 * 1000).unref();
