import { getMysqlPool, type MysqlPoolConfig } from "./mysql-pool";

const host = process.env.MYSQL_DB_HOST;
const port = parseInt(process.env.MYSQL_DB_PORT ?? "3306", 10);
const user = process.env.MYSQL_DB_USER;
const password = process.env.MYSQL_DB_PASS;
const database = process.env.MYSQL_DB_STATS;

const statsPoolConfig: MysqlPoolConfig = {
  host: host ?? "",
  port,
  user: user ?? "",
  password: password ?? "",
  database: database ?? "",
  label: "Stats MySQL",
};

export function getStatsPool() {
  return getMysqlPool(statsPoolConfig);
}
