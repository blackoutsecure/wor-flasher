import { createServer } from "node:http";
import { readFile, realpath } from "node:fs/promises";
import { resolve, sep, extname } from "node:path";
import { fileURLToPath } from "node:url";

const root = await realpath(fileURLToPath(new URL("../../release/ui-fixtures", import.meta.url)));
const types = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".json": "application/json", ".png": "image/png" };
const server = createServer(async (request, response) => {
  try {
    if (request.method !== "GET" && request.method !== "HEAD") {
      response.writeHead(405).end("Read-only visual fixture server");
      return;
    }
    const url = new URL(request.url, "http://127.0.0.1:4179");
    const pathname = decodeURIComponent(url.pathname);
    const path = await realpath(resolve(root, `.${pathname === "/" ? "/image-system.html" : pathname}`));
    if (!path.startsWith(`${root}${sep}`) || !types[extname(path)]) {
      response.writeHead(403).end("Not a visual fixture");
      return;
    }
    const data = await readFile(path);
    response.writeHead(200, {
      "Content-Type": types[extname(path)], "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    });
    response.end(request.method === "HEAD" ? undefined : data);
  } catch (error) {
    if (["ENOENT", "ENOTDIR", "URIError"].includes(error.code ?? error.name)) {
      response.writeHead(404).end("Visual fixture not found");
    } else {
      console.error("Visual fixture request failed:", error);
      response.writeHead(500).end("Visual fixture request failed");
    }
  }
});
server.listen(4179, "127.0.0.1", () => console.log("Read-only visual fixture server ready on 127.0.0.1:4179"));
for (const signal of ["SIGTERM", "SIGINT"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
