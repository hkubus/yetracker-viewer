import Fastify from "fastify";
import { drizzle } from "drizzle-orm/node-sqlite";
import { relations } from "./db/relations.ts";
import {routesPlugin  } from "./util/loadRoutes.ts";
import { join } from "path";
const app = Fastify();
const db = drizzle(process.env.DB_FILENAME as string, { relations });
const mainDir = import.meta.url
	.replace("file://", "")
	.split("/")
	.slice(0, -1)
	.join("/");
app.register(routesPlugin, { path: join(mainDir, "routes") });

app.listen({
    port:3000,
}).then(()=> console.log('Server running on http://localhost:3000'));
