import { createClient } from 'redis';

type ClientType = ReturnType<typeof createClient>;

let cachedClient: ClientType;
export async function getRedisClient(): Promise<ClientType> {
  if (cachedClient) return cachedClient;

  // Create and configure Redis client
  const redisClient = createClient({ url: 'redis://redis:6379' });
  redisClient.on('error', (err) => console.log('Redis Client Error', err));

  // Connect to Redis
  await redisClient.connect();

  cachedClient = redisClient;

  return redisClient;
}
