import { createClient } from 'redis';

type ClientType = ReturnType<typeof createClient>;

let cachedClient: ClientType;

export async function getRedisClient(): Promise<ClientType> {
  if (cachedClient) return cachedClient;

  // Create and configure Redis client
  const redisClient = createClient({
    url: process.env.REDIS_URL,
    name: '6b6t-discord-bot',
    pingInterval: 30000,
  });
  redisClient.on('error', (err) => console.log('Redis Client Error', err));
  redisClient.on('reconnecting', () => {
    console.log('Redis reconnecting…');
  });
  redisClient.on('end', () => {
    console.log('Redis connection ended');
  });

  // Connect to Redis
  await redisClient.connect();

  cachedClient = redisClient;

  return redisClient;
}
