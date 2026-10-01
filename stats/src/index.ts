// RAWmakase usage stats: counts opt-in weekly reports and publishes coarse
// totals: the current week's running total through yesterday, and breakdowns
// for completed weeks. Reports are added to tallies on arrival and
// never stored; the client address is never read. See README.md.
import { MIN_GROUP, publishWeek, type Group, type Stats, type Week } from "./publish";
import {
  isoWeek,
  MAX_BODY_BYTES,
  parseReport,
  utcDay,
  weekStart,
  type Report,
} from "./report";
import { DIMENSIONS, type DimensionName } from "./dimensions";
import { renderPage } from "./page";
import { releasedVersions } from "./versions";

/// Weeks published on the page and in stats.json.
const PUBLISHED_WEEKS = 26;
/// Tallies older than this are deleted by the daily sweep.
const RETENTION_DAYS = 730;
/// A tally stops growing here, which bounds what a flood of fabricated
/// reports can do to one week.
const MAX_COUNT = 1_000_000;

export default {
  async fetch(request, env, ctx): Promise<Response> {
    const url = new URL(request.url);
    const { pathname } = url;
    // Plain HTTP: pages redirect, reports are refused so the app never
    // learns to send them unencrypted.
    if (url.protocol === "http:") {
      if (request.method === "GET" || request.method === "HEAD") {
        url.protocol = "https:";
        return Response.redirect(url.toString(), 301);
      }
      return text(403, "use HTTPS");
    }
    if (pathname === "/v1/report") {
      if (request.method !== "POST") return text(405, "use POST");
      return receive(request, env, new Date());
    }
    if (request.method !== "GET" && request.method !== "HEAD") {
      return text(405, "use GET");
    }
    if (pathname !== "/" && pathname !== "/stats.json") return text(404, "not found");
    return cachedPage(url, env, ctx, new Date());
  },

  async scheduled(_controller, env): Promise<void> {
    await sweep(env, new Date());
  },
} satisfies ExportedHandler<Env>;


function text(status: number, body: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain", "cache-control": "no-store" },
  });
}

/// The page or stats.json, from Cloudflare's cache when it has them. What
/// they show changes only at midnight UTC, so they are cached until then.
/// The key ignores the query string, so it can't be used to skip the cache,
/// and includes the deployed version, so a deploy shows at once.
async function cachedPage(
  url: URL,
  env: Env,
  ctx: ExecutionContext,
  now: Date,
): Promise<Response> {
  const key = new Request(`${url.origin}${url.pathname}?v=${env.VERSION.id}`);
  const hit = await caches.default.match(key);
  if (hit) return forClients(hit);
  const stats = await published(env, now);
  const headers = {
    "cache-control": `public, max-age=${secondsUntilMidnight(now)}`,
    "strict-transport-security": "max-age=31536000",
  };
  const response =
    url.pathname === "/"
      ? new Response(renderPage(stats), {
          headers: { "content-type": "text/html; charset=utf-8", ...headers },
        })
      : Response.json(stats, { headers });
  ctx.waitUntil(caches.default.put(key, response.clone()));
  return forClients(response);
}

/// Browsers and API clients keep a copy for five minutes at most, so a
/// deploy reaches them soon; only Cloudflare's copy lasts until midnight.
const CLIENT_MAX_AGE = 300;

function forClients(response: Response): Response {
  const copy = new Response(response.body, response);
  copy.headers.set("cache-control", `public, max-age=${CLIENT_MAX_AGE}`);
  return copy;
}

export function secondsUntilMidnight(now: Date): number {
  const midnight = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
  return Math.max(60, Math.ceil((midnight - now.getTime()) / 1000));
}

export async function receive(
  request: Request,
  env: Env,
  now: Date,
): Promise<Response> {
  if (env.REPORTS !== "on") return text(503, "reports are paused");
  const declared = Number(request.headers.get("content-length") ?? 0);
  if (declared > MAX_BODY_BYTES) return text(413, "report too large");
  const body = await readLimited(request, MAX_BODY_BYTES);
  if (body === null) return text(413, "report too large");
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return text(400, "invalid JSON");
  }
  const report = parseReport(value);
  if (typeof report === "string") return text(400, report);
  // The app's own User-Agent, as its update check sends it. Anyone reading
  // the source can copy it; it only keeps out scanners and generic scripts.
  if (request.headers.get("user-agent") !== `RAWmakase/${report.version}`) {
    return text(400, "unexpected user agent");
  }
  const versions = await releasedVersions(env, now.getTime());
  if (!versions.has(report.version)) return text(400, "unknown version");
  if (!(await count(env, now, report, Number(env.DAILY_REPORT_BUDGET)))) {
    return text(503, "daily budget reached");
  }
  return new Response(null, { status: 204, headers: { "cache-control": "no-store" } });
}

/// The body as text, or null past `limit` bytes, without buffering more.
async function readLimited(request: Request, limit: number): Promise<string | null> {
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(bytes);
}

/// Adds one report to the week's tallies (src/dimensions.ts) and the day's
/// count, in a single transaction.
///
/// The daily budget keeps a flood within D1's free limits. The transaction
/// first reserves a place in the day's budget, and every count after it
/// applies only while that reservation is within budget, so concurrent
/// reports can't overshoot it. The reservation stops growing one past the
/// budget, so later reports write nothing at all; they are lost, like one
/// sent while offline. Returns whether it was counted.
async function count(env: Env, now: Date, report: Report, budget: number): Promise<boolean> {
  const week = isoWeek(now);
  const day = utcDay(now);
  // Bound last, after each statement's own parameters.
  const within = (n: number) => `(SELECT used FROM report_budget WHERE day = ?${n}) <= ?${n + 1}`;
  const tallies = DIMENSIONS.flatMap(({ table, columns, values }) => {
    const fields = values(report);
    if (!fields) return [];
    const n = columns.length + 2;
    return [
      env.DB.prepare(
        `INSERT INTO ${table} (week, ${columns.join(", ")}, count)
         SELECT ?1, ${columns.map((_, i) => `?${i + 2}`).join(", ")}, 1 WHERE ${within(n)}
         ON CONFLICT DO UPDATE SET count = count + 1 WHERE count < ${MAX_COUNT}`,
      ).bind(week, ...fields, day, budget),
    ];
  });
  const [, daily] = await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO report_budget (day, used) VALUES (?1, 1)
       ON CONFLICT DO UPDATE SET used = used + 1 WHERE used <= ?2`,
    ).bind(day, budget),
    env.DB.prepare(
      `INSERT INTO daily_counts (day, count) SELECT ?1, 1 WHERE ${within(1)}
       ON CONFLICT DO UPDATE SET count = count + 1 WHERE count < ${MAX_COUNT}`,
    ).bind(day, budget),
    ...tallies,
  ]);
  return (daily?.meta.changes ?? 0) > 0;
}

/// The current week's running total through yesterday, and completed weeks
/// newest first. Breakdowns wait for the week to end: daily snapshots of
/// them could be subtracted to recover a single day's reports.
export async function published(env: Env, now: Date): Promise<Stats> {
  const [thisWeek, weeks] = await Promise.all([weekSoFar(env, now), completedWeeks(env, now)]);
  return { thisWeek, weeks };
}

async function weekSoFar(env: Env, now: Date): Promise<Stats["thisWeek"]> {
  const start = weekStart(now);
  const today = utcDay(now);
  if (start === today) return null;
  const row = await env.DB.prepare(
    `SELECT coalesce(sum(count), 0) AS total FROM daily_counts WHERE day >= ?1 AND day < ?2`,
  )
    .bind(start, today)
    .first<{ total: number }>();
  const total = row?.total ?? 0;
  return {
    week: isoWeek(now),
    through: utcDay(new Date(now.getTime() - 86_400_000)),
    total: total < MIN_GROUP ? null : total,
  };
}

async function completedWeeks(env: Env, now: Date): Promise<Week[]> {
  const current = isoWeek(now);
  const results = await Promise.all(
    DIMENSIONS.map(({ table, key }) =>
      env.DB.prepare(`SELECT week, ${key} AS key, count FROM ${table} WHERE week < ?1`)
        .bind(current)
        .all<{ week: string; key: string; count: number }>(),
    ),
  );
  // week → dimension → groups
  const weeks = new Map<string, Record<string, Group[]>>();
  DIMENSIONS.forEach(({ name }, i) => {
    for (const { week, key, count } of results[i]!.results) {
      const dimensions = weeks.get(week) ?? {};
      (dimensions[name] ??= []).push({ key, count });
      weeks.set(week, dimensions);
    }
  });
  return [...weeks.keys()]
    .filter((week) => weeks.get(week)!.platform)
    .sort()
    .reverse()
    .slice(0, PUBLISHED_WEEKS)
    .map((week) => publishWeek(week, weeks.get(week)! as Partial<Record<DimensionName, Group[]>>));
}

/// Deletes tallies past the retention period.
export async function sweep(env: Env, now: Date): Promise<void> {
  const cutoff = new Date(now.getTime() - RETENTION_DAYS * 86_400_000);
  const oldest = isoWeek(cutoff);
  await env.DB.batch([
    ...DIMENSIONS.map(({ table }) =>
      env.DB.prepare(`DELETE FROM ${table} WHERE week < ?1`).bind(oldest),
    ),
    env.DB.prepare(`DELETE FROM daily_counts WHERE day < ?1`).bind(utcDay(cutoff)),
    env.DB.prepare(`DELETE FROM report_budget WHERE day < ?1`).bind(
      utcDay(new Date(now.getTime() - 86_400_000)),
    ),
  ]);
}
