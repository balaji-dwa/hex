// Static Cloudflare Worker serving Hex schema-1 release manifests, immutable
// artifacts with HTTP Range support, dev release notes, and a permanent /download redirect.

const CHANNELS = new Set(["dev", "stable", "beta"]);
const SHA256_RE = /^[a-f0-9]{64}$/;
const FILE_NAME_RE = /^[A-Za-z0-9][A-Za-z0-9._+-]{0,199}$/;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function etagMatches(ifNoneMatch, etag) {
  if (!ifNoneMatch || !etag) return false;
  const stripWeak = (tag) => tag.trim().replace(/^W\//, "");
  const target = stripWeak(etag);
  return ifNoneMatch
    .split(",")
    .map(stripWeak)
    .some((candidate) => candidate === "*" || candidate === target);
}

function parseByteRange(header, totalSize) {
  if (!header || !header.startsWith("bytes=")) return null;
  const spec = header.slice("bytes=".length).trim();
  if (!spec || spec.includes(",")) return null;
  const dash = spec.indexOf("-");
  if (dash === -1) return null;
  const startRaw = spec.slice(0, dash).trim();
  const endRaw = spec.slice(dash + 1).trim();

  if (startRaw === "") {
    if (!/^\d+$/.test(endRaw)) return { invalid: true };
    const suffix = Number(endRaw);
    if (!Number.isSafeInteger(suffix) || suffix <= 0 || totalSize === 0) {
      return { unsatisfiable: true };
    }
    const length = Math.min(suffix, totalSize);
    const start = totalSize - length;
    return { start, end: totalSize - 1, length };
  }

  if (!/^\d+$/.test(startRaw)) return { invalid: true };
  const start = Number(startRaw);
  if (!Number.isSafeInteger(start) || start >= totalSize) {
    return { unsatisfiable: true };
  }

  if (endRaw === "") {
    return { start, end: totalSize - 1, length: totalSize - start };
  }

  if (!/^\d+$/.test(endRaw)) return { invalid: true };
  const end = Math.min(Number(endRaw), totalSize - 1);
  if (!Number.isSafeInteger(end) || end < start) {
    return { unsatisfiable: true };
  }
  return { start, end, length: end - start + 1 };
}

async function serveJson(bucket, key, request) {
  const headOnly = request.method === "HEAD";
  const object = headOnly ? await bucket.head(key) : await bucket.get(key);
  if (!object) {
    return new Response("Not found\n", { status: 404 });
  }
  const headers = new Headers({
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": "no-cache",
  });
  if (object.httpEtag) {
    headers.set("ETag", object.httpEtag);
    if (etagMatches(request.headers.get("If-None-Match"), object.httpEtag)) {
      await object.body?.cancel?.();
      return new Response(null, { status: 304, headers });
    }
  }
  if (typeof object.size === "number") {
    headers.set("Content-Length", String(object.size));
  }
  return new Response(headOnly ? null : object.body, { status: 200, headers });
}

async function serveDownloadRedirect(bucket, channel, request) {
  const object = await bucket.get(`hex/desktop/${channel}.json`);
  if (!object) {
    return new Response(`No ${channel} release published yet.\n`, { status: 404 });
  }
  let manifest;
  try {
    manifest = JSON.parse(await object.text());
  } catch {
    return new Response("Release manifest is unreadable.\n", { status: 500 });
  }
  const target = manifest?.release?.targets?.["macos-arm64-dmg"]?.url;
  if (typeof target !== "string" || !target.startsWith("/hex/files/")) {
    return new Response(`No macOS DMG available on ${channel}.\n`, { status: 404 });
  }
  return new Response(null, {
    status: 302,
    headers: {
      Location: new URL(target, request.url).toString(),
      "Cache-Control": "no-store",
    },
  });
}

async function serveImmutableFile(bucket, sha256, fileName, request) {
  const key = `hex/files/${sha256}/${fileName}`;
  const head = await bucket.head(key);
  if (!head) {
    return new Response("Not found\n", { status: 404 });
  }

  const baseHeaders = new Headers({
    "Accept-Ranges": "bytes",
    "Cache-Control": "public, max-age=31536000, immutable",
    "Content-Disposition": `attachment; filename="${fileName}"`,
    "Content-Type": fileName.endsWith(".zip")
      ? "application/zip"
      : "application/octet-stream",
  });
  if (head.httpEtag) {
    baseHeaders.set("ETag", head.httpEtag);
  }

  const rangeHeader = request.headers.get("Range");
  if (rangeHeader) {
    const parsed = parseByteRange(rangeHeader, head.size);
    if (parsed?.unsatisfiable) {
      baseHeaders.set("Content-Range", `bytes */${head.size}`);
      return new Response(null, { status: 416, headers: baseHeaders });
    }
    if (parsed && !parsed.invalid) {
      baseHeaders.set(
        "Content-Range",
        `bytes ${parsed.start}-${parsed.end}/${head.size}`,
      );
      baseHeaders.set("Content-Length", String(parsed.length));
      if (request.method === "HEAD") {
        return new Response(null, { status: 206, headers: baseHeaders });
      }
      const ranged = await bucket.get(key, {
        range: { offset: parsed.start, length: parsed.length },
      });
      if (!ranged) {
        return new Response("Not found\n", { status: 404 });
      }
      return new Response(ranged.body, { status: 206, headers: baseHeaders });
    }
  }

  baseHeaders.set("Content-Length", String(head.size));
  if (request.method === "HEAD") {
    return new Response(null, { status: 200, headers: baseHeaders });
  }
  const object = await bucket.get(key);
  if (!object) {
    return new Response("Not found\n", { status: 404 });
  }
  return new Response(object.body, { status: 200, headers: baseHeaders });
}

async function serveDevNotesHtml(bucket, request, url) {
  const object = await bucket.get("hex/files/dev-notes.json");
  let builds = [];
  if (object) {
    try {
      const parsed = JSON.parse(await object.text());
      if (Array.isArray(parsed?.builds)) {
        builds = parsed.builds;
      }
    } catch {
      builds = [];
    }
  }
  const since = url.searchParams.get("since");
  const visible = [];
  for (const build of builds) {
    if (since && build?.version === since) break;
    visible.push(build);
  }
  const sections = visible
    .map((build) => {
      const version = escapeHtml(build?.version ?? "unknown");
      const publishedAt = escapeHtml(build?.published_at ?? "");
      const items = Array.isArray(build?.items)
        ? build.items
            .map((item) => `<li>${escapeHtml(item?.title ?? "")}</li>`)
            .join("")
        : "";
      return `<section id="${version}"><h2>${version}</h2><p>${publishedAt}</p><ul>${items}</ul></section>`;
    })
    .join("\n");

  const html = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Hex Releases</title></head>
<body>
<h1>Hex Team Releases</h1>
<p><a href="/download">Download latest macOS DMG</a></p>
${sections || "<p>No release notes published yet.</p>"}
</body>
</html>
`;
  return new Response(request.method === "HEAD" ? null : html, {
    status: 200,
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-cache",
    },
  });
}

export default {
  async fetch(request, env) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed\n", {
        status: 405,
        headers: { Allow: "GET, HEAD" },
      });
    }

    const bucket = env.HEX_RELEASES;
    const url = new URL(request.url);
    const path = url.pathname;

    if (path === "/" || path === "/download") {
      return serveDownloadRedirect(bucket, "dev", request);
    }

    const downloadMatch = path.match(/^\/download\/(dev|stable|beta)$/);
    if (downloadMatch) {
      return serveDownloadRedirect(bucket, downloadMatch[1], request);
    }

    const manifestMatch = path.match(/^\/hex\/desktop\/([a-z-]+)\.json$/);
    if (manifestMatch && CHANNELS.has(manifestMatch[1])) {
      return serveJson(bucket, `hex/desktop/${manifestMatch[1]}.json`, request);
    }

    if (path === "/hex/files/dev-notes.json") {
      return serveJson(bucket, "hex/files/dev-notes.json", request);
    }

    if (path === "/hex/dev") {
      return serveDevNotesHtml(bucket, request, url);
    }

    const fileMatch = path.match(/^\/hex\/files\/([^/]+)\/([^/]+)$/);
    if (fileMatch) {
      const [, sha256, fileName] = fileMatch;
      if (SHA256_RE.test(sha256) && FILE_NAME_RE.test(fileName)) {
        return serveImmutableFile(bucket, sha256, fileName, request);
      }
    }

    return new Response("Not found\n", { status: 404 });
  },
};
