// Server lokal untuk uji end-to-end.
import { spawn } from "node:child_process";
import { createServer } from "node:http";

/** Server git smart-HTTP minimal: meneruskan permintaan ke `git http-backend` (CGI) setelah memeriksa Basic auth. */
export async function gitHttpServer(projectRoot, accept) {
  const attempts = [];
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    const header = req.headers.authorization ?? "";
    const [user, password] = header.startsWith("Basic ") ? Buffer.from(header.slice(6), "base64").toString().split(/:(.*)/s) : [null, null];
    if (user !== null) attempts.push({ user, password });
    if (user === null || !accept(user, password)) {
      res.writeHead(401, { "WWW-Authenticate": 'Basic realm="uji"', "content-type": "text/plain" });
      res.end("authentication required\n");
      return;
    }
    const cgi = spawn("git", ["http-backend"], {
      env: {
        ...process.env,
        GIT_PROJECT_ROOT: projectRoot,
        GIT_HTTP_EXPORT_ALL: "1",
        REQUEST_METHOD: req.method,
        PATH_INFO: decodeURIComponent(url.pathname),
        QUERY_STRING: url.search.slice(1),
        CONTENT_TYPE: req.headers["content-type"] ?? "",
        HTTP_CONTENT_ENCODING: req.headers["content-encoding"] ?? "",
        REMOTE_USER: user,
        REMOTE_ADDR: "127.0.0.1",
        GIT_CONFIG_GLOBAL: "/dev/null",
        GIT_CONFIG_NOSYSTEM: "1",
      },
    });
    req.pipe(cgi.stdin);
    let buffer = Buffer.alloc(0);
    let headersSent = false;
    cgi.stdout.on("data", (chunk) => {
      if (headersSent) return void res.write(chunk);
      buffer = Buffer.concat([buffer, chunk]);
      const end = buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      let status = 200;
      const headers = {};
      for (const line of buffer.subarray(0, end).toString().split("\r\n")) {
        const colon = line.indexOf(":");
        const name = line.slice(0, colon).trim();
        const value = line.slice(colon + 1).trim();
        if (name.toLowerCase() === "status") status = Number(value.split(" ")[0]);
        else headers[name] = value;
      }
      res.writeHead(status, headers);
      res.write(buffer.subarray(end + 4));
      headersSent = true;
    });
    cgi.on("close", () => res.end());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { origin: `http://127.0.0.1:${server.address().port}`, attempts, close: () => new Promise((resolve) => server.close(resolve)) };
}

