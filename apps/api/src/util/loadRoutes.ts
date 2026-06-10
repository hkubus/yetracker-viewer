import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import type { Context, Hono } from 'hono';
import type { HandlerResponse } from 'hono/types';
export type Route = 'get' | 'post' | 'put' | 'delete' | 'patch';
export type Routes = Record<
  Route,
  {
    handler: (c: Context) => HandlerResponse<'json' | 'text'>;
  }
>;

export async function loadRoutes(path: string, instance: Hono, initialPath: string = path) {
  const files = await readdir(path, { withFileTypes: true });
  for (const file of files) {
    if (file.isDirectory() || (!file.name.endsWith('.js') && !file.name.endsWith('.ts'))) {
      await loadRoutes(join(path, file.name), instance, initialPath || path);
    } else {
      if (file.parentPath.endsWith('.map')) continue;

      const route: { routes: Routes } = await import(`${file.parentPath}/${file.name}`);

      let transformedPath = `${file.parentPath}/${file.name}`
        .replaceAll(initialPath, '')
        .replaceAll(/\[(.*)\]/gm, ':$1')
        .replaceAll('.ts', '')
        .replaceAll('.js', '');

      if (transformedPath.endsWith('/index')) transformedPath = transformedPath.slice(0, -6);
      console.log(transformedPath);
      Object.entries(route.routes).forEach(([method, { handler }]) => {
        if (!handler) {
          console.log(transformedPath, 'no handler :(');
          return;
        }
        if (method === 'post') console.log(`${transformedPath} b`);
        instance.on(method, [transformedPath], handler);
      });
    }
  }
}
