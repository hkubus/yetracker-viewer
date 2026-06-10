import type { Context } from 'hono';

export const routes = {
  get: {
    handler: async (c: Context) => {
      return c.json({ hello: 'world' });
    },
  },
};
