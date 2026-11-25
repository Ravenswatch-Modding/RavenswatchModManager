import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { config as loadEnv } from 'dotenv';
import { migrate } from 'drizzle-orm/node-postgres/migrator';
import { getDb } from './client';

const here = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = resolve(here, '..', '..', '..');
loadEnv({ path: resolve(repoRoot, '.env.local') });
loadEnv({ path: resolve(repoRoot, '.env') });
loadEnv({ path: '.env.local' });
loadEnv();

async function main() {
  await migrate(getDb(), { migrationsFolder: './drizzle' });
  console.log('migrations applied');
  process.exit(0);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
