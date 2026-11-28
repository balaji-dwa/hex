import assert from "node:assert/strict";
import crypto from "node:crypto";
import test from "node:test";

import worker from "./worker.js";

function createMockBucket(initial = {}) {
  const store = new Map();
  for (const [key, value] of Object.entries(initial)) {
    const bytes = typeof value === "string" ? Buffer.from(value, "utf8") : Buffer.from(value);
    const etag = `"${crypto.createHash("md5").update(bytes).digest("hex")}"`;
    store.set(key, { bytes, etag });
  }
  return {
    async head(key) {
      const item = store.get(key);
      if (!item) return null;
      return { size: item.bytes.length, httpEtag: item.etag };
    },
    async get(key, options = {}) {
      const item = store.get(key);
      if (!item) return null;
      let slice = item.bytes;
      if (options.range) {
        const offset = options.range.offset ?? 0;
        const length = options.range.length ?? (item.bytes.length - offset);
        slice = item.bytes.subarray(offset, offset + length);
      }
      return {
        size: item.bytes.length,
        httpEtag: item.etag,
        body: slice,
        async text() {
          return slice.toString("utf8");
        },
      };
    },
  };
}

const SAMPLE_SHA = "a".repeat(64);
const SAMPLE_DMG_SHA = "b".repeat(64);
const SAMPLE_MANIFEST = JSON.stringify({
  schema: 1,
  app: "hex",
  artifact: "desktop",
  channel: "dev",
  sequence: 2,
  published_at: "2026-11-28T05:00:00Z",
  release: {
    version: "0.0.0-dev-1795843000",
    rollout_percent: 100,
    required: false,
    targets: {
      "macos-arm64": {
        url: `/hex/files/${SAMPLE_SHA}/Hex.zip`,
        size: 10,
        sha256: SAMPLE_SHA,
      },
      "macos-arm64-dmg": {
        url: `/hex/files/${SAMPLE_DMG_SHA}/Hex.dmg`,
        size: 10,
        sha256: SAMPLE_DMG_SHA,
      },
    },
    distributions: [{ kind: "direct" }],
  },
});

const SAMPLE_NOTES = JSON.stringify({
  schema: 1,
  builds: [
    {
      version: "0.0.0-dev-2",
      commit: "222",
      published_at: "2026-11-28T05:00:00Z",
      items: [{ title: "Fix <script>alert(1)</script>", url: "" }],
    },
    {
      version: "0.0.0-dev-1",
      commit: "111",
      published_at: "2026-11-27T05:00:00Z",
      items: [{ title: "Older change", url: "" }],
    },
  ],
});

function envWithSample() {
  return {
    HEX_RELEASES: createMockBucket({
      "hex/desktop/dev.json": SAMPLE_MANIFEST,
      "hex/files/dev-notes.json": SAMPLE_NOTES,
      [`hex/files/${SAMPLE_SHA}/Hex.zip`]: Buffer.from("0123456789"),
      [`hex/files/${SAMPLE_DMG_SHA}/Hex.dmg`]: Buffer.from("abcdefghij"),
    }),
  };
}

test("/download redirects 302 to latest DMG and 404s when empty", async () => {
  const res = await worker.fetch(
    new Request("https://releases.example.com/download"),
    envWithSample(),
  );
  assert.equal(res.status, 302);
  assert.equal(
    res.headers.get("Location"),
    `https://releases.example.com/hex/files/${SAMPLE_DMG_SHA}/Hex.dmg`,
  );
  assert.equal(res.headers.get("Cache-Control"), "no-store");

  const empty = await worker.fetch(
    new Request("https://releases.example.com/download"),
    { HEX_RELEASES: createMockBucket() },
  );
  assert.equal(empty.status, 404);
});

test("manifest supports ETag and If-None-Match 304", async () => {
  const env = envWithSample();
  const first = await worker.fetch(
    new Request("https://releases.example.com/hex/desktop/dev.json"),
    env,
  );
  assert.equal(first.status, 200);
  const etag = first.headers.get("ETag");
  assert.ok(etag);
  assert.equal(first.headers.get("Cache-Control"), "no-cache");

  const second = await worker.fetch(
    new Request("https://releases.example.com/hex/desktop/dev.json", {
      headers: { "If-None-Match": etag },
    }),
    env,
  );
  assert.equal(second.status, 304);
});

test("immutable files support full GET, HEAD, open-ended Range, bounded Range, and 416", async () => {
  const env = envWithSample();
  const url = `https://releases.example.com/hex/files/${SAMPLE_SHA}/Hex.zip`;

  const full = await worker.fetch(new Request(url), env);
  assert.equal(full.status, 200);
  assert.equal(await full.text(), "0123456789");
  assert.equal(full.headers.get("Accept-Ranges"), "bytes");
  assert.equal(full.headers.get("Content-Disposition"), 'attachment; filename="Hex.zip"');
  assert.match(full.headers.get("Cache-Control"), /immutable/);

  const head = await worker.fetch(new Request(url, { method: "HEAD" }), env);
  assert.equal(head.status, 200);
  assert.equal(head.headers.get("Content-Length"), "10");

  const openEnded = await worker.fetch(
    new Request(url, { headers: { Range: "bytes=4-" } }),
    env,
  );
  assert.equal(openEnded.status, 206);
  assert.equal(openEnded.headers.get("Content-Range"), "bytes 4-9/10");
  assert.equal(await openEnded.text(), "456789");

  const bounded = await worker.fetch(
    new Request(url, { headers: { Range: "bytes=2-5" } }),
    env,
  );
  assert.equal(bounded.status, 206);
  assert.equal(bounded.headers.get("Content-Range"), "bytes 2-5/10");
  assert.equal(await bounded.text(), "2345");

  const badRange = await worker.fetch(
    new Request(url, { headers: { Range: "bytes=99-" } }),
    env,
  );
  assert.equal(badRange.status, 416);
  assert.equal(badRange.headers.get("Content-Range"), "bytes */10");
});

test("rejects bad hashes, traversal names, and non-GET/HEAD methods", async () => {
  const env = envWithSample();
  for (const badPath of [
    "/hex/files/notasha/Hex.zip",
    `/hex/files/${SAMPLE_SHA}/.hidden`,
    `/hex/files/${SAMPLE_SHA}/../secret`,
    "/hex/desktop/nightly.json",
  ]) {
    const res = await worker.fetch(
      new Request(`https://releases.example.com${badPath}`),
      env,
    );
    assert.equal(res.status, 404, badPath);
  }

  const post = await worker.fetch(
    new Request("https://releases.example.com/download", { method: "POST" }),
    env,
  );
  assert.equal(post.status, 405);
});

test("/hex/dev escapes HTML and filters by ?since=", async () => {
  const res = await worker.fetch(
    new Request("https://releases.example.com/hex/dev?since=0.0.0-dev-1"),
    envWithSample(),
  );
  assert.equal(res.status, 200);
  const html = await res.text();
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script>/);
  assert.doesNotMatch(html, /Older change/);
});
