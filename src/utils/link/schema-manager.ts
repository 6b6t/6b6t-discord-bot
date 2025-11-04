import { ensureLinkDatabaseExists, getLinkPool } from "./mariadb-client";
import { LINK_SCHEMA_STATEMENTS } from "./schema";

let schemaInitialized = false;

export async function ensureLinkSchema() {
  if (schemaInitialized) {
    return;
  }

  await ensureLinkDatabaseExists();
  const pool = getLinkPool();

  for (const statement of LINK_SCHEMA_STATEMENTS) {
    await pool.query(statement);
  }

  schemaInitialized = true;
}
