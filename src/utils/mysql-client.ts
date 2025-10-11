import mysql from 'mysql2/promise';

declare let global: { statsPool: mysql.Pool };

function createMysqlClient() {
  console.log('[MySQL] Creating connection pool');
  return mysql.createPool({
    host: process.env.MYSQL_DB_HOST,
    port: parseInt(process.env.MYSQL_DB_PORT ?? '3306', 10),
    user: process.env.MYSQL_DB_USER,
    password: process.env.MYSQL_DB_PASS,
    database: process.env.MYSQL_DB_STATS,
    waitForConnections: true,
    connectionLimit: 10,
    maxIdle: 10,
    idleTimeout: 60000,
    queueLimit: 0,
    enableKeepAlive: true,
    keepAliveInitialDelay: 0,
  });
}

let statsPool: mysql.Pool;

export function getStatsPool(): mysql.Pool {
  if (statsPool) {
    // console.log('[MySQL] Reusing in-memory pool');
    return statsPool;
  }

  if (process.env.NODE_ENV === 'production') {
    console.log('[MySQL] Initializing pool in production mode');
    statsPool = createMysqlClient();
  } else {
    if (!global.statsPool) {
      console.log('[MySQL] Creating global pool for development');
      global.statsPool = createMysqlClient();
    }
    console.log('[MySQL] Reusing global pool for development');
    statsPool = global.statsPool;
  }

  console.log('[MySQL] Pool ready');
  return statsPool;
}
