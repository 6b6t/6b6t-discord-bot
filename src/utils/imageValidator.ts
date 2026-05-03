import type { Attachment } from "discord.js";

export const ALLOWED_CONTENT_TYPES = [
  "image/png",
  "image/jpeg",
  "image/jpg",
  "image/gif",
  "image/webp",
] as const;

export const MAX_FILE_SIZE = 10 * 1024 * 1024; // 10 MB

export interface ValidationResult {
  valid: boolean;
  imageUrl?: string;
  error?: string;
  isAnimated?: boolean;
}

export function validateBannerImage(
  attachment: Attachment | null,
  url: string | null,
): ValidationResult {
  if (!attachment && !url) {
    return {
      valid: false,
      error:
        "You must provide either an **image attachment** or an **image URL**.",
    };
  }

  if (attachment) {
    const contentType = attachment.contentType?.split(";")[0];
    if (
      contentType &&
      !(ALLOWED_CONTENT_TYPES as readonly string[]).includes(contentType)
    ) {
      return {
        valid: false,
        error: `Invalid file type: \`${attachment.contentType}\`. Allowed types: PNG, JPG, GIF, WebP.`,
      };
    }

    if (attachment.size > MAX_FILE_SIZE) {
      const sizeMB = (attachment.size / (1024 * 1024)).toFixed(1);
      return {
        valid: false,
        error: `File too large: \`${sizeMB} MB\`. Maximum allowed size is **10 MB**.`,
      };
    }

    const isAnimated =
      attachment.contentType?.includes("gif") ||
      attachment.name?.endsWith(".gif") ||
      false;

    return { valid: true, imageUrl: attachment.url, isAnimated };
  }

  if (url) {
    try {
      const parsed = new URL(url);
      if (!["http:", "https:"].includes(parsed.protocol)) {
        return {
          valid: false,
          error: "URL must use `http://` or `https://` protocol.",
        };
      }
    } catch {
      return {
        valid: false,
        error: "Invalid URL format. Please provide a valid image URL.",
      };
    }

    const lowerUrl = url.toLowerCase().split("?")[0];
    const validExtensions = [".png", ".jpg", ".jpeg", ".gif", ".webp"];
    const hasValidExtension = validExtensions.some((ext) =>
      lowerUrl.endsWith(ext),
    );

    if (!hasValidExtension) {
      return {
        valid: false,
        error:
          "URL does not appear to point to a valid image. Supported formats: PNG, JPG, GIF, WebP.",
      };
    }

    const isAnimated = lowerUrl.endsWith(".gif");

    return { valid: true, imageUrl: url, isAnimated };
  }

  return { valid: false, error: "Unexpected error validating image." };
}
