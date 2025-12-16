import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const STORAGE_PATH = join(join(process.cwd(), "data"), "youtube-posted.json");

async function getPostedVideoIds(): Promise<string[]> {
  try {
    const data = await readFile(STORAGE_PATH, "utf-8");
    return JSON.parse(data);
  } catch {
    return [];
  }
}

async function savePostedVideoIds(videoIds: string[]): Promise<void> {
  await writeFile(STORAGE_PATH, JSON.stringify(videoIds, null, 2));
}

export async function isYoutubeVideoPosted(videoId: string): Promise<boolean> {
  const postedIds = await getPostedVideoIds();
  return postedIds.includes(videoId);
}

export async function markYoutubeVideoPosted(videoId: string): Promise<void> {
  const postedIds = await getPostedVideoIds();
  if (!postedIds.includes(videoId)) {
    postedIds.push(videoId);
    await savePostedVideoIds(postedIds);
  }
}
