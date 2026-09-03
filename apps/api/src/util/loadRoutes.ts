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

function toRoutePath(parentPath: string, fileName: string, initialPath: string) {
  const fullPath = `${parentPath}/${fileName}`;
  const withoutPrefix = fullPath.startsWith(initialPath) ? fullPath.slice(initialPath.length) : fullPath;
  const withoutExtension = withoutPrefix.endsWith('.ts')
    ? withoutPrefix.slice(0, -3)
    : withoutPrefix.endsWith('.js')
      ? withoutPrefix.slice(0, -3)
      : withoutPrefix;
  const withParams = withoutExtension.replaceAll(/\[([^\]]+)\]/g, ':$1');
  if (withParams.endsWith('/index')) return withParams.slice(0, -6);
  return withParams;
}

export async function loadRoutes(path: string, instance: Hono, initialPath: string = path) {
  const files = await readdir(path, { withFileTypes: true });
  for (const file of files) {
    if (file.isDirectory()) {
      await loadRoutes(join(path, file.name), instance, initialPath);
      continue;
    }
    if (file.name.endsWith('.map')) continue;
    if (!file.name.endsWith('.js') && !file.name.endsWith('.ts')) continue;

    const route: { routes: Routes } = await import(`${file.parentPath}/${file.name}`);

    const transformedPath = toRoutePath(file.parentPath, file.name, initialPath);
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
