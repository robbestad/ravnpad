import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const dist = path.resolve(__dirname, "dist");
const host = process.env.HOST || "127.0.0.1";
const port = Number(process.env.PORT || 3010);

const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".json": "application/json; charset=utf-8",
  ".map": "application/json; charset=utf-8",
  ".xml": "application/xml; charset=utf-8",
  ".txt": "text/plain; charset=utf-8",
  ".webp": "image/webp",
  ".jpg": "image/jpeg",
};

function send(res, status, body, headers = {}) {
  res.writeHead(status, headers);
  res.end(body);
}

function inside(root, file) {
  const rel = path.relative(root, file);
  return Boolean(rel) && !rel.startsWith("..") && !path.isAbsolute(rel);
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`);

  if (url.pathname === "/healthz") {
    return send(res, 200, JSON.stringify({ ok: true }), {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    });
  }

  let rel = decodeURIComponent(url.pathname);
  if (rel === "/") rel = "/index.html";

  const file = path.normalize(path.join(dist, rel));
  if (!inside(dist, file) && path.resolve(file) !== dist) {
    return send(res, 403, "Forbidden");
  }

  fs.stat(file, (err, st) => {
    const ext = path.extname(file);
    const spa = !ext || ext === ".html";
    const target = !err && st.isFile() ? file : spa ? path.join(dist, "index.html") : null;
    if (!target) return send(res, 404, "Not found");

    fs.readFile(target, (readErr, data) => {
      if (readErr) return send(res, 404, "Not found");
      const outExt = path.extname(target);
      const hashed = /-[a-zA-Z0-9_-]{8}\.\w+$/.test(path.basename(target));
      send(res, 200, data, {
        "content-type": types[outExt] || "application/octet-stream",
        "cache-control": hashed
          ? "public, max-age=31536000, immutable"
          : "no-cache",
      });
    });
  });
});

server.listen(port, host, () => {
  console.log(`ravnpad-web listening on http://${host}:${port}`);
});
