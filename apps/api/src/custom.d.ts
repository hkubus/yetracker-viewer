import type { drizzle } from 'drizzle-orm/node-sqlite';

declare module 'fastify' {
  interface FastifyRequest extends FastifyRequest {
    db: ReturnType<typeof drizzle>;
  }
}
