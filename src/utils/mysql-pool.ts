import mysql from "mysql2/promise";

export interface MysqlConfig {
  host: string;
  port?: number;
  user: string;
  password: string;
  database: string;
  ssl?: mysql.SslOptions | string;
  label?: string;
}

export interface MysqlPoolConfig extends MysqlConfig {
  poolOptions?: Omit<
    mysql.PoolOptions,
    "host" | "port" | "user" | "password" | "database" | "ssl"
  >;
}

export interface EnsureDatabaseOptions {
  charset?: string;
  collate?: string;
  label?: string;
}

declare global {
  // eslint-disable-next-line no-var
  var __mysqlPoolMap: Map<string, mysql.Pool> | undefined;
}

const localPools = new Map<string, mysql.Pool>();

function getPoolStore() {
  if (process.env.NODE_ENV === "production") {
    return localPools;
  }

  if (!global.__mysqlPoolMap) {
    global.__mysqlPoolMap = new Map();
  }

  return global.__mysqlPoolMap;
}

function createPool(config: MysqlPoolConfig): mysql.Pool {
  const { host, port, user, password, database, ssl, label, poolOptions } =
    config;

  if (!host || !user || !password || !database) {
    throw new Error("Missing required MySQL configuration values");
  }

  const logLabel = label ?? `MySQL:${database}`;
  console.log(`[${logLabel}] Creating connection pool`);

  const poolConfig: mysql.PoolOptions = {
    waitForConnections: true,
    connectionLimit: 10,
    maxIdle: 10,
    idleTimeout: 60000,
    queueLimit: 0,
    enableKeepAlive: true,
    keepAliveInitialDelay: 0,
    host,
    user,
    password,
    database,
    port: port ?? 3306,
    ...poolOptions,
  };

  if (ssl) {
    poolConfig.ssl = ssl;
  }

  return mysql.createPool(poolConfig);
}

export function getMysqlPool(config: MysqlPoolConfig): mysql.Pool {
  const poolStore = getPoolStore();
  const key = config.database;

  let pool = poolStore.get(key);
  if (!pool) {
    pool = createPool(config);
    poolStore.set(key, pool);
  }

  return pool;
}

export async function ensureDatabaseExists(
  config: MysqlConfig,
  options?: EnsureDatabaseOptions,
) {
  const { host, port, user, password, database, ssl } = config;
  const charset = options?.charset ?? "utf8mb4";
  const collate = options?.collate ?? "utf8mb4_unicode_ci";
  const logLabel = options?.label ?? config.label ?? `MySQL:${database}`;

  if (!host || !user || !password || !database) {
    throw new Error("Missing required MySQL configuration values");
  }

  console.log(`[${logLabel}] Ensuring database exists`);
  const connection = await mysql.createConnection({
    host,
    port: port ?? 3306,
    user,
    password,
    ssl,
  });

  try {
    await connection.query(
      `CREATE DATABASE IF NOT EXISTS \`${database}\` DEFAULT CHARACTER SET ${charset} COLLATE ${collate}`,
    );
  } finally {
    await connection.end();
  }
}

export async function withMysqlPool<T>(
  config: MysqlPoolConfig,
  callback: (pool: mysql.Pool) => Promise<T>,
): Promise<T> {
  const pool = getMysqlPool(config);
  return callback(pool);
}

export async function closeAllMysqlPools() {
  const poolStore = getPoolStore();
  const closePromises: Promise<void>[] = [];

  for (const [key, pool] of poolStore.entries()) {
    closePromises.push(
      pool.end().catch((error) => {
        console.error(`Failed to close MySQL pool ${key}`, error);
      }),
    );
    poolStore.delete(key);
  }

  await Promise.all(closePromises);
}
