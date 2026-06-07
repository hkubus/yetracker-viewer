import { readdir } from "node:fs/promises";
import { join } from "node:path";
import type {
	FastifyInstance,
	FastifyPluginOptions,
	FastifyReply,
	FastifyRequest,
	HTTPMethods,
} from "fastify";
export type Route = "get" | "post" | "put" | "delete" | "patch";
export type Routes = Record<
	Route,
	{
		handler: (req: FastifyRequest, res: FastifyReply) => void;
	}
>;
export function routesPlugin(
	instance: FastifyInstance,
	opts: FastifyPluginOptions,
	done,
) {
	loadRoutes(opts.path, instance).then(done);
}
export async function loadRoutes(
	path: string,
	instance: FastifyInstance,
	initialPath: string = path,
) {
	const files = await readdir(path, { withFileTypes: true });
	for (const file of files) {
		if (
			file.isDirectory() ||
			(!file.name.endsWith(".js") && !file.name.endsWith(".ts"))
		) {
			await loadRoutes(join(path, file.name), instance, initialPath || path);
		} else {
			if (file.parentPath.endsWith(".map")) continue;

			const route: { routes: Routes } = await import(
				`${file.parentPath}/${file.name}`
			);

			let transformedPath = `${file.parentPath}/${file.name}`
				.replaceAll(initialPath, "")
				.replaceAll(/\[(.*)\]/gm, ":$1")
				.replaceAll(".ts", "")
				.replaceAll(".js", "");

			if (transformedPath.endsWith("/index"))
				transformedPath = transformedPath.slice(0, -6);
			console.log(transformedPath);
			Object.entries(route.routes).forEach(([method, { handler }]) => {
				if (!handler) return console.log(transformedPath);
				if (method === "post") console.log(`${transformedPath} b`);
				instance.route({
					method: method.toUpperCase() as HTTPMethods,
					handler,
					url: transformedPath,
				});
			});
		}
	}
}
