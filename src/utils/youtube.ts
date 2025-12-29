import type { BaseGuildTextChannel } from "discord.js";
import { google } from "googleapis";
import he from "he";
import "dotenv/config";
import {
  isYoutubeVideoPosted,
  markYoutubeVideoPosted,
} from "./youtube-storage";

const youtube = google.youtube({
  version: "v3",
  auth: process.env.YOUTUBE_API_KEY,
});

export async function getLatestVideo(
  queries: string[],
  ignoreWords: string[],
  whitelistedChannels: string[],
): Promise<{
  id: string;
  author: string;
  title: string;
  url: string;
} | null> {
  try {
    const query = queries.join(" OR ");
    const response = await youtube.search.list({
      q: query,
      order: "date",
      part: ["snippet"],
      maxResults: 5,
      type: ["video"],
    });

    if (response.data.items && response.data.items.length > 0) {
      for (const video of response.data.items) {
        const videoId = video.id?.videoId;
        const title = he.decode(video.snippet?.title ?? "");
        const description = video.snippet?.description ?? "";
        const author = video.snippet?.channelTitle ?? "";
        const channelId = video.snippet?.channelId ?? "";

        const hasQuery = queries.some(
          (query) =>
            title.toLowerCase().includes(query.toLowerCase()) ||
            description.toLowerCase().includes(query.toLowerCase()),
        );

        if (!videoId || !hasQuery) continue;

        // Skip if video was already posted (persisted across restarts/deletions)
        if (await isYoutubeVideoPosted(videoId)) continue;

        if (!whitelistedChannels.includes(channelId)) {
          const hasIgnoredWord = ignoreWords.some((word) => {
            const lowerWord = word.toLowerCase();
            return (
              title.toLowerCase().includes(lowerWord) ||
              description.toLowerCase().includes(lowerWord) ||
              author.toLowerCase().includes(lowerWord)
            );
          });
          if (hasIgnoredWord) continue;
        }

        return {
          id: videoId,
          author,
          title,
          url: `https://www.youtube.com/watch?v=${videoId}`,
        };
      }
    }
  } catch (error) {
    console.error("YouTube API Error: ", error);
  }
  return null;
}

export async function sendYoutubeNotification(
  channel: BaseGuildTextChannel,
  queries: string[],
  ignoreWords: string[],
  whitelistedChannels: string[],
) {
  console.log("Checking for new YouTube videos...");
  const video = await getLatestVideo(queries, ignoreWords, whitelistedChannels);
  if (!video) {
    console.log("No new video found.");
    return;
  }

  // Mark video as posted before sending to prevent duplicates
  await markYoutubeVideoPosted(video.id);

  const message = await channel.send(
    `**${video.title}** - ${video.author}\n${video.url}`,
  );

  setTimeout(
    async () => {
      // won't publish if the bot shuts down
      try {
        await message.crosspost();
      } catch (error) {
        console.error("Failed to crosspost:", error);
      }
    },
    12 * 60 * 60 * 1000,
  );

  console.log("New video notification sent:", video.url);
}
