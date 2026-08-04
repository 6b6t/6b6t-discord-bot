import { randomUUID } from "node:crypto";

export type MiniTerminatorAction = "add" | "remove";

export interface PendingMiniTerminatorRequest {
  id: string;
  action: MiniTerminatorAction;
  submitterId: string;
  submitterTag: string;
  targetId: string;
  targetTag: string;
  guildId: string;
  channelId: string;
  messageId: string | null;
  createdAt: number;
}

export const MINI_TERMINATOR_TTL_MS = 60 * 60 * 1000; // 1 hour

const pendingMiniTerminators = new Map<string, PendingMiniTerminatorRequest>();

export function createMiniTerminatorRequest(data: {
  action: MiniTerminatorAction;
  submitterId: string;
  submitterTag: string;
  targetId: string;
  targetTag: string;
  guildId: string;
  channelId: string;
}): string {
  const id = randomUUID();
  pendingMiniTerminators.set(id, {
    id,
    ...data,
    messageId: null,
    createdAt: Date.now(),
  });
  return id;
}

export function getMiniTerminatorRequest(
  id: string,
): PendingMiniTerminatorRequest | null {
  const request = pendingMiniTerminators.get(id);
  if (!request) return null;

  if (Date.now() - request.createdAt > MINI_TERMINATOR_TTL_MS) {
    pendingMiniTerminators.delete(id);
    return null;
  }

  return request;
}

export function setMiniTerminatorMessageId(
  id: string,
  messageId: string,
): void {
  const request = pendingMiniTerminators.get(id);
  if (request) {
    request.messageId = messageId;
  }
}

export function removeMiniTerminatorRequest(id: string): void {
  pendingMiniTerminators.delete(id);
}

export function cleanupExpiredMiniTerminators(): void {
  const now = Date.now();
  for (const [id, request] of pendingMiniTerminators) {
    if (now - request.createdAt > MINI_TERMINATOR_TTL_MS) {
      pendingMiniTerminators.delete(id);
    }
  }
}

// Auto cleanup every 10 minutes
setInterval(cleanupExpiredMiniTerminators, 10 * 60 * 1000).unref();
