import { BaseGuildTextChannel } from 'discord.js';
import { google } from 'googleapis';
import he from 'he';
import 'dotenv/config';

const youtube = google.youtube({
  version: 'v3',
  auth: process.env.YOUTUBE_API_KEY,
});

export async function getLatestVideo(
  queries: string[],
  ignoreWords: string[],
): Promise<{
  author: string;
  title: string;
  url: string;
} | null> {
  try {
    const query = [
      queries.join(' OR '),
      ...ignoreWords.map((word) => `-` + word),
    ].join(' ');
    const response = await youtube.search.list({
      q: query,
      order: 'date',
      part: ['snippet'],
      maxResults: 1,
      type: ['video'],
    });

    if (response.data.items && response.data.items.length > 0) {
      const video = response.data.items[0];
      const videoId = video.id?.videoId;
      const title = he.decode(video.snippet?.title ?? '');
      const description = video.snippet?.description ?? '';
      const author = video.snippet?.channelTitle ?? '';

      const hasQuery = queries.some(
        (query) =>
          title.toLowerCase().includes(query.toLowerCase()) ||
          description.toLowerCase().includes(query.toLowerCase()),
      );

      if (videoId && hasQuery) {
        return {
          author,
          title,
          url: `https://www.youtube.com/watch?v=${videoId}`,
        };
      }
    }
  } catch (error) {
    console.error('YouTube API Error: ', error);
  }
  return null;
}

async function getLastNotifications(
  channel: BaseGuildTextChannel,
): Promise<string[]> {
  try {
    const messages = await channel.messages.fetch({ limit: 5 });
    return messages.map((msg) => msg.content);
  } catch (error) {
    console.error(`Error while getting last notifications: `, error);
    return [];
  }
}

export async function sendYoutubeNotification(
  channel: BaseGuildTextChannel,
  queries: string[],
  ignoreWords: string[],
) {
  console.log('Checking for new YouTube videos...');
  const video = await getLatestVideo(queries, ignoreWords);
  if (!video) {
    console.log('No new video found.');
    return;
  }

  const lastMessages = await getLastNotifications(channel);
  if (lastMessages.some((msg) => msg.includes(video.url))) {
    console.log('Video already notified.');
    return;
  }

  const message = await channel.send(
    `**${video.title}** - ${video.author}\n${video.url}`,
  );

  setTimeout(
    async () => {
      // won't publish if the bot shuts down
      try {
        await message.crosspost();
      } catch (error) {
        console.error('Failed to crosspost:', error);
      }
    },
    12 * 60 * 60 * 1000,
  );

  console.log('New video notification sent:', video.url);
}
