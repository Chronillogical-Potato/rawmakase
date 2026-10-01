// The versions RAWmakase has been released as, from the repository's git
// tags. Read over git's smart HTTP endpoint rather than the GitHub API, which
// rate-limits the shared addresses Workers fetch from. Each list read is
// kept in D1, which answers while GitHub can't be reached.

const REFS = "https://github.com/pch/rawmakase.git/info/refs?service=git-upload-pack";
const TTL_MS = 60 * 60 * 1000;
/// After a failed read, GitHub is tried again this much later.
const RETRY_MS = 5 * 60 * 1000;

let cached: { at: number; ttl: number; versions: Set<string> } | null = null;

/// Every `vX.Y.Z` tag, without the `v`. Annotated tags appear twice in the
/// listing, once peeled (`^{}`); both give the same version.
export function parseTags(refs: string): Set<string> {
  const versions = new Set<string>();
  for (const [, version] of refs.matchAll(/refs\/tags\/v([0-9A-Za-z.-]+?)(?:\^\{\})?\n/g)) {
    versions.add(version!);
  }
  return versions;
}

/// The released versions, refreshed from GitHub at most hourly. When GitHub
/// can't be reached, the last list stored in D1; empty if there is none, so
/// unknown versions are never accepted.
export async function releasedVersions(env: Env, now: number = Date.now()): Promise<Set<string>> {
  if (cached && now - cached.at < cached.ttl) return cached.versions;
  const fetched = await fetchTags();
  if (fetched) {
    // The stored list becomes exactly this one, so a deleted tag isn't
    // accepted during a later outage. Written only when the list changed.
    const known = cached?.versions ?? (await stored(env));
    const same = known.size === fetched.size && [...fetched].every((v) => known.has(v));
    if (!same) {
      await env.DB.batch([
        env.DB.prepare(`DELETE FROM releases`),
        ...[...fetched].map((v) => env.DB.prepare(`INSERT INTO releases (version) VALUES (?1)`).bind(v)),
      ]);
    }
    cached = { at: now, ttl: TTL_MS, versions: fetched };
    return fetched;
  }
  const versions = cached?.versions ?? (await stored(env));
  cached = { at: now, ttl: RETRY_MS, versions };
  return versions;
}

async function fetchTags(): Promise<Set<string> | null> {
  try {
    // Not cached at Cloudflare too: the hour below is the only cache, so
    // a new tag is accepted within an hour.
    const response = await fetch(REFS, { headers: { "user-agent": "rawmakase-stats" } });
    if (!response.ok) return null;
    const refs = await response.text();
    // Anything but git's ref advertisement (an error page, say) is a failed
    // read. A real one with no version tags means there are none.
    if (!refs.startsWith("001e# service=git-upload-pack\n")) return null;
    return parseTags(refs);
  } catch {
    return null;
  }
}

async function stored(env: Env): Promise<Set<string>> {
  const { results } = await env.DB.prepare(`SELECT version FROM releases`).all<{ version: string }>();
  return new Set(results.map((r) => r.version));
}

/// Drops the cached list. For tests.
export function forgetVersions(): void {
  cached = null;
}
