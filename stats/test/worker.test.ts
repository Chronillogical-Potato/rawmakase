import { env, exports } from "cloudflare:workers";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { published, receive, secondsUntilMidnight, sweep } from "../src/index";
import { isoWeek, utcDay } from "../src/report";
import { forgetVersions } from "../src/versions";

const fields = {
  schema: 1 as const,
  version: "0.1.10",
  os: "macos" as const,
  arch: "aarch64" as const,
  channel: "macos-dmg" as const,
  os_release: "macos-26",
  distro: "none" as const,
  display: "none" as const,
  gpu: "metal" as const,
};
const body = (overrides = {}) => JSON.stringify({ ...fields, ...overrides });

// git's ref advertisement, as GitHub serves it.
const REFS =
  "001e# service=git-upload-pack\n0000" +
  "003f1111111111111111111111111111111111111111 refs/tags/v0.1.9\n" +
  "00402222222222222222222222222222222222222222 refs/tags/v0.1.10\n" +
  "00433333333333333333333333333333333333333333 refs/tags/v0.1.10^{}\n0000";

/// The app's User-Agent for the report's version.
function agent(body: string): string {
  try {
    return `RAWmakase/${JSON.parse(body).version}`;
  } catch {
    return "RAWmakase";
  }
}

const post = (body: string, headers: Record<string, string> = {}) =>
  exports.default.fetch("https://stats.rawmakase.com/v1/report", {
    method: "POST",
    headers: { "content-type": "application/json", "user-agent": agent(body), ...headers },
    body,
  });

const request = (body: string) =>
  new Request("https://stats.rawmakase.com/v1/report", {
    method: "POST",
    headers: { "user-agent": agent(body) },
    body,
  });

const rows = async (table: string) =>
  (await env.DB.prepare(`SELECT * FROM ${table} ORDER BY 1, 2`).all()).results;

beforeEach(async () => {
  forgetVersions();
  vi.spyOn(globalThis, "fetch").mockImplementation(async () => new Response(REFS));
  await env.DB.batch(
    [
      "platform_counts",
      "version_counts",
      "channel_counts",
      "os_release_counts",
      "distro_counts",
      "display_counts",
      "gpu_counts",
      "daily_counts",
      "releases",
      "report_budget",
    ].map((t) => env.DB.prepare(`DELETE FROM ${t}`)),
  );
  for (const path of ["/", "/stats.json"]) {
    await caches.default.delete(`https://stats.rawmakase.com${path}?v=${env.VERSION.id}`);
  }
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("POST /v1/report", () => {
  it("adds a report to the current week's three tallies and today's count", async () => {
    const week = isoWeek(new Date());
    const day = utcDay(new Date());
    expect((await post(body())).status).toBe(204);
    expect((await post(body())).status).toBe(204);
    expect(await rows("platform_counts")).toEqual([
      { week, os: "macos", arch: "aarch64", count: 2 },
    ]);
    expect(await rows("version_counts")).toEqual([{ week, version: "0.1.10", count: 2 }]);
    expect(await rows("channel_counts")).toEqual([{ week, channel: "macos-dmg", count: 2 }]);
    expect(await rows("daily_counts")).toEqual([{ day, count: 2 }]);
  });

  it("counts Linux-only fields for Linux reports only", async () => {
    const week = isoWeek(new Date());
    await post(body());
    await post(
      body({ os: "linux", arch: "x86_64", channel: "arch-package", os_release: "linux", distro: "arch", display: "wayland", gpu: "vulkan" }),
    );
    expect(await rows("os_release_counts")).toEqual([
      { week, os_release: "linux", count: 1 },
      { week, os_release: "macos-26", count: 1 },
    ]);
    expect(await rows("distro_counts")).toEqual([{ week, distro: "arch", count: 1 }]);
    expect(await rows("display_counts")).toEqual([{ week, display: "wayland", count: 1 }]);
    expect(await rows("gpu_counts")).toEqual([
      { week, gpu: "metal", count: 1 },
      { week, gpu: "vulkan", count: 1 },
    ]);
  });

  it("refuses versions that were never tagged", async () => {
    const response = await post(body({ version: "9.9.9" }));
    expect(response.status).toBe(400);
    expect(await response.text()).toBe("unknown version");
    expect((await post(body({ version: "0.1.9" }))).status).toBe(204);
  });

  it("keeps the release list it read", async () => {
    await post(body());
    const { results } = await env.DB.prepare(`SELECT version FROM releases ORDER BY 1`).all();
    expect(results).toEqual([{ version: "0.1.10" }, { version: "0.1.9" }]);
  });

  it("drops deleted tags from the stored list", async () => {
    await env.DB.prepare(`INSERT INTO releases VALUES ('0.1.8'), ('0.1.9')`).run();
    await post(body());
    const { results } = await env.DB.prepare(`SELECT version FROM releases ORDER BY 1`).all();
    expect(results).toEqual([{ version: "0.1.10" }, { version: "0.1.9" }]);
  });

  it("accepts no version once every tag is deleted", async () => {
    await env.DB.prepare(`INSERT INTO releases VALUES ('0.1.10')`).run();
    vi.mocked(fetch).mockImplementation(async () => new Response("001e# service=git-upload-pack\n0000"));
    expect((await post(body())).status).toBe(400);
    const { results } = await env.DB.prepare(`SELECT version FROM releases`).all();
    expect(results).toEqual([]);
  });

  it("treats a page that isn't a ref listing as a failed read", async () => {
    await env.DB.prepare(`INSERT INTO releases VALUES ('0.1.10')`).run();
    vi.mocked(fetch).mockImplementation(async () => new Response("<html>rate limited</html>"));
    expect((await post(body())).status).toBe(204);
  });

  it("checks against the stored list when GitHub can't be reached", async () => {
    await env.DB.prepare(`INSERT INTO releases VALUES ('0.1.10')`).run();
    vi.mocked(fetch).mockRejectedValue(new Error("offline"));
    expect((await post(body())).status).toBe(204);
    expect((await post(body({ version: "9.9.9" }))).status).toBe(400);
  });

  it("refuses every version when no list was ever read", async () => {
    vi.mocked(fetch).mockResolvedValue(new Response("unavailable", { status: 503 }));
    expect((await post(body())).status).toBe(400);
  });

  it("stops counting at the daily budget", async () => {
    const tight = { ...env, DAILY_REPORT_BUDGET: "2" } as unknown as Env;
    const now = new Date();
    expect((await receive(request(body()), tight, now)).status).toBe(204);
    expect((await receive(request(body()), tight, now)).status).toBe(204);
    const third = await receive(request(body()), tight, now);
    expect(third.status).toBe(503);
    expect(await third.text()).toBe("daily budget reached");
    expect(await rows("daily_counts")).toEqual([{ day: utcDay(now), count: 2 }]);
    expect(await rows("version_counts")).toEqual([
      { week: isoWeek(now), version: "0.1.10", count: 2 },
    ]);
  });

  it("holds the budget when reports arrive together", async () => {
    const tight = { ...env, DAILY_REPORT_BUDGET: "3" } as unknown as Env;
    const now = new Date();
    const responses = await Promise.all(
      Array.from({ length: 8 }, () => receive(request(body()), tight, now)),
    );
    expect(responses.filter((r) => r.status === 204)).toHaveLength(3);
    expect(responses.filter((r) => r.status === 503)).toHaveLength(5);
    expect(await rows("daily_counts")).toEqual([{ day: utcDay(now), count: 3 }]);
    expect(await rows("platform_counts")).toEqual([
      { week: isoWeek(now), os: "macos", arch: "aarch64", count: 3 },
    ]);
    // It stops one past the budget: later reports write nothing.
    expect(await rows("report_budget")).toEqual([{ day: utcDay(now), used: 4 }]);
  });

  it("refuses reports while switched off", async () => {
    const off = { ...env, REPORTS: "off" } as unknown as Env;
    const response = await receive(request(body()), off, new Date());
    expect(response.status).toBe(503);
    expect(await response.text()).toBe("reports are paused");
    expect(await rows("platform_counts")).toEqual([]);
  });

  it("counts the week the server receives it in", async () => {
    const now = new Date("2027-01-01T00:00:00Z");
    await receive(request(body()), env, now);
    expect(await rows("version_counts")).toEqual([
      { week: "2026-W53", version: "0.1.10", count: 1 },
    ]);
  });

  it("refuses invalid reports and stores nothing", async () => {
    const refused = [
      await post("{"),
      await post(body({ hostname: "studio" })),
      await post(body({ os: "haiku" })),
    ];
    expect(refused.map((r) => r.status)).toEqual([400, 400, 400]);
    expect(await refused[1]!.text()).toBe("unknown field hostname");
    expect(await rows("platform_counts")).toEqual([]);
  });

  it("refuses oversized bodies, declared or not", async () => {
    const padded = body() + " ".repeat(600);
    expect((await post(padded)).status).toBe(413);
    const undeclared = new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode(padded));
        controller.close();
      },
    });
    const response = await exports.default.fetch("https://stats.rawmakase.com/v1/report", {
      method: "POST",
      body: undeclared,
    });
    expect(response.status).toBe(413);
    expect(await rows("platform_counts")).toEqual([]);
  });

  it("refuses reports over plain HTTP", async () => {
    const response = await exports.default.fetch("http://stats.rawmakase.com/v1/report", {
      method: "POST",
      body: body(),
    });
    expect(response.status).toBe(403);
    expect(await rows("platform_counts")).toEqual([]);
  });

  it("requires the app's User-Agent for the report's version", async () => {
    for (const ua of ["curl/8.7.1", "RAWmakase/0.1.9", "rawmakase/0.1.10"]) {
      const response = await post(body(), { "user-agent": ua });
      expect(response.status).toBe(400);
      expect(await response.text()).toBe("unexpected user agent");
    }
    expect(await rows("platform_counts")).toEqual([]);
  });

  it("only accepts POST", async () => {
    const response = await exports.default.fetch("https://stats.rawmakase.com/v1/report");
    expect(response.status).toBe(405);
  });
});

describe("publishing", () => {
  const seed = async (week: string, count: number) => {
    await env.DB.batch([
      env.DB.prepare(
        `INSERT INTO platform_counts VALUES (?1, 'linux', 'x86_64', ?2), (?1, 'macos', 'aarch64', 3)`,
      ).bind(week, count),
      env.DB.prepare(`INSERT INTO version_counts VALUES (?1, '0.1.10', ?2 + 3)`).bind(week, count),
      env.DB.prepare(`INSERT INTO channel_counts VALUES (?1, 'unknown', ?2 + 3)`).bind(week, count),
    ]);
  };

  it("publishes completed weeks only, newest first", async () => {
    const now = new Date("2026-10-01T12:00:00Z");
    await seed("2026-W40", 50);
    await seed("2026-W39", 20);
    await seed("2026-W38", 4);
    const { weeks } = await published(env, now);
    expect(weeks.map((w) => [w.week, w.total])).toEqual([
      ["2026-W39", 23],
      ["2026-W38", null],
    ]);
    expect(weeks[0]!.breakdowns?.platform).toEqual({ shown: [], other: 23 });
  });

  it("totals the current week through yesterday", async () => {
    await env.DB.prepare(
      `INSERT INTO daily_counts VALUES ('2026-09-27', 50), ('2026-09-28', 7), ('2026-09-30', 6), ('2026-10-01', 9)`,
    ).run();
    // Thursday: Monday to Wednesday count, today doesn't, last Sunday doesn't.
    expect((await published(env, new Date("2026-10-01T12:00:00Z"))).thisWeek).toEqual({
      week: "2026-W40",
      through: "2026-09-30",
      total: 13,
    });
    // Tuesday: only Monday so far, below the minimum.
    expect((await published(env, new Date("2026-09-29T08:00:00Z"))).thisWeek).toEqual({
      week: "2026-W40",
      through: "2026-09-28",
      total: null,
    });
    // Monday: no completed day yet.
    expect((await published(env, new Date("2026-09-28T23:00:00Z"))).thisWeek).toBeNull();
  });

  it("serves the page and the JSON", async () => {
    await seed("2020-W01", 20);
    const page = await exports.default.fetch("https://stats.rawmakase.com/");
    expect(page.headers.get("content-type")).toContain("text/html");
    const html = await page.text();
    expect(html).toContain("Week 2020-W01: 23 installations");
    expect(html).not.toMatch(/<(script|link)\b/);
    const json = await exports.default.fetch("https://stats.rawmakase.com/stats.json");
    expect(await json.json()).toEqual(await published(env, new Date()));
  });

  it("redirects pages to HTTPS", async () => {
    const response = await exports.default.fetch("http://stats.rawmakase.com/stats.json?x=1", {
      redirect: "manual",
    });
    expect(response.status).toBe(301);
    expect(response.headers.get("location")).toBe("https://stats.rawmakase.com/stats.json?x=1");
    const page = await exports.default.fetch("https://stats.rawmakase.com/");
    expect(page.headers.get("strict-transport-security")).toBe("max-age=31536000");
  });

  it("serves the page from the cache until midnight UTC", async () => {
    const first = await exports.default.fetch("https://stats.rawmakase.com/");
    expect(await first.text()).toContain("No completed weeks yet.");
    // Clients keep it briefly; Cloudflare's copy lasts until midnight.
    expect(first.headers.get("cache-control")).toBe("public, max-age=300");
    await env.DB.prepare(`INSERT INTO platform_counts VALUES ('2020-W01', 'linux', 'x86_64', 20)`).run();
    // A query string doesn't skip the cache.
    const again = await exports.default.fetch("https://stats.rawmakase.com/?fresh=1");
    expect(await again.text()).toContain("No completed weeks yet.");
  });

  it("renders an empty page before any data", async () => {
    const page = await exports.default.fetch("https://stats.rawmakase.com/");
    expect(await page.text()).toContain("No completed weeks yet.");
  });
});

describe("retention", () => {
  it("deletes tallies older than 24 months", async () => {
    await env.DB.batch([
      env.DB.prepare(`INSERT INTO version_counts VALUES ('2024-W39', '0.1.0', 5)`),
      env.DB.prepare(`INSERT INTO version_counts VALUES ('2024-W41', '0.1.0', 5)`),
      env.DB.prepare(`INSERT INTO platform_counts VALUES ('2024-W39', 'linux', 'x86_64', 5)`),
      env.DB.prepare(`INSERT INTO daily_counts VALUES ('2024-09-30', 5), ('2024-10-02', 5)`),
      env.DB.prepare(`INSERT INTO report_budget VALUES ('2026-09-29', 5), ('2026-09-30', 5), ('2026-10-01', 5)`),
    ]);
    await sweep(env, new Date("2026-10-01T12:00:00Z"));
    expect(await rows("version_counts")).toEqual([
      { week: "2024-W41", version: "0.1.0", count: 5 },
    ]);
    expect(await rows("platform_counts")).toEqual([]);
    expect(await rows("daily_counts")).toEqual([{ day: "2024-10-02", count: 5 }]);
    // Budgets are kept for today and yesterday only.
    expect(await rows("report_budget")).toEqual([
      { day: "2026-09-30", used: 5 },
      { day: "2026-10-01", used: 5 },
    ]);
  });
});

describe("secondsUntilMidnight", () => {
  it("counts to the next UTC midnight, at least a minute", () => {
    expect(secondsUntilMidnight(new Date("2026-10-01T23:00:00Z"))).toBe(3600);
    expect(secondsUntilMidnight(new Date("2026-10-01T00:00:00Z"))).toBe(86_400);
    expect(secondsUntilMidnight(new Date("2026-10-01T23:59:59Z"))).toBe(60);
  });
});
