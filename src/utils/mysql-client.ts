import mysql from 'mysql2/promise';

declare let global: { statsPool: mysql.Pool };

function createMysqlClient() {
  return mysql.createPool({
    host: process.env.MYSQL_DB_HOST,
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
  if (statsPool) return statsPool;

  if (process.env.NODE_ENV === 'production') {
    statsPool = createMysqlClient();
  } else {
    if (!global.statsPool) {
      global.statsPool = createMysqlClient();
    }
    statsPool = global.statsPool;
  }

  return statsPool;
}
