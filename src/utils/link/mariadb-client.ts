import {
  ensureDatabaseExists,
  getMysqlPool,
  type MysqlConfig,
  type MysqlPoolConfig,
} from "../mysql-pool";

const host = process.env.MYSQL_DB_HOST;
const port = parseInt(process.env.MYSQL_DB_PORT ?? "3306", 10);
const user = process.env.MYSQL_DB_USER;
const password = process.env.MYSQL_DB_PASS;
const database =
  process.env.MYSQL_DB_LINK ?? process.env.MYSQL_DB_LINKS ?? "6b6t_link";

const linkPoolConfig: MysqlPoolConfig = {
  host: host ?? "",
  port,
  user: user ?? "",
  password: password ?? "",
  database,
  label: "Link MySQL",
};

const linkDatabaseConfig: MysqlConfig = {
  host: host ?? "",
  port,
  user: user ?? "",
  password: password ?? "",
  database,
  label: "Link MySQL",
};

export function getLinkPool() {
  return getMysqlPool(linkPoolConfig);
}

export async function ensureLinkDatabaseExists() {
  await ensureDatabaseExists(linkDatabaseConfig);
}

export { database as linkDatabaseName };
