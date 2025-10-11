import { createClient } from 'redis';

type ClientType = ReturnType<typeof createClient>;

let cachedClient: ClientType;

export async function getRedisClient(): Promise<ClientType> {
  if (cachedClient) {
    console.log('[Redis] Reusing cached client');
    return cachedClient;
  }

  // Create and configure Redis client
  console.log('[Redis] Creating client instance');
  const redisClient = createClient({
    url: process.env.REDIS_URL,
    name: '6b6t-discord-bot',
    pingInterval: 30000,
    socket: {
      keepAlive: true,
      noDelay: true,
      reconnectStrategy: (retries) => Math.min(1000 * retries, 15000),
    },
  });
  redisClient.on('error', (err) => console.log('Redis Client Error', err));
  redisClient.on('reconnecting', () => {
    console.log('Redis reconnecting…');
  });
  redisClient.on('end', () => {
    console.log('Redis connection ended');
  });

  // Connect to Redis
  console.log('[Redis] Connecting…');
  await redisClient.connect();
  console.log('[Redis] Connection established');

  cachedClient = redisClient;

  return redisClient;
}
