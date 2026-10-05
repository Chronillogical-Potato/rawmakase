import { describe, expect, it } from "vitest";
import { MIN_GROUP, publishWeek, suppress, type Group } from "../src/publish";

const groups = (counts: Record<string, number>): Group[] =>
  Object.entries(counts).map(([key, count]) => ({ key, count }));

describe("suppress", () => {
  it("shows every group when all are large enough", () => {
    expect(suppress(groups({ a: 40, b: 12 }))).toEqual({
      shown: groups({ a: 40, b: 12 }),
      other: null,
    });
  });

  it("merges small groups into other", () => {
    expect(suppress(groups({ a: 40, b: 4, c: 3 }))).toEqual({
      shown: groups({ a: 40 }),
      other: 7,
    });
  });

  it("never lets other stand for a single group", () => {
    // b alone in "other" would be b's exact count, since the set of keys is
    // known; the smallest shown group joins it.
    expect(suppress(groups({ a: 40, c: 20, b: 3 }))).toEqual({
      shown: groups({ a: 40 }),
      other: 23,
    });
  });

  it("lets other fall below the minimum when it merges two groups", () => {
    expect(suppress(groups({ a: 40, d: 15, b: 3, c: 1 }))).toEqual({
      shown: groups({ a: 40, d: 15 }),
      other: 4,
    });
  });

  it("can merge everything", () => {
    expect(suppress(groups({ a: 4, b: 4, c: 4 }))).toEqual({ shown: [], other: 12 });
  });

  it("never publishes a group below the minimum", () => {
    for (let seed = 0; seed < 500; seed++) {
      const counts: Group[] = [];
      for (let i = 0; i < 1 + (seed % 7); i++) {
        counts.push({ key: `k${i}`, count: 1 + ((seed * 31 + i * 17) % 40) });
      }
      const total = counts.reduce((n, g) => n + g.count, 0);
      if (total < MIN_GROUP) continue;
      const { shown, other } = suppress(counts);
      for (const g of shown) expect(g.count).toBeGreaterThanOrEqual(MIN_GROUP);
      expect(shown.reduce((n, g) => n + g.count, other ?? 0)).toBe(total);
    }
  });
});

describe("publishWeek", () => {
  it("publishes nothing for a week below the minimum", () => {
    expect(
      publishWeek("2026-W40", {
        platform: groups({ "linux x86_64": 4 }),
        version: groups({ "0.1.10": 4 }),
      }),
    ).toEqual({ week: "2026-W40", total: null, breakdowns: null });
  });

  it("suppresses each breakdown separately", () => {
    const week = publishWeek("2026-W40", {
      platform: groups({ "linux x86_64": 30, "macos aarch64": 25 }),
      version: groups({ "0.1.10": 52, "0.1.9": 3 }),
      channel: groups({ "macos-dmg": 25, aur: 30 }),
    });
    expect(week.total).toBe(55);
    expect(week.breakdowns?.version).toEqual({ shown: [], other: 55 });
    expect(week.breakdowns?.platform?.other).toBeNull();
  });

  it("leaves out a Linux-only breakdown with too few Linux reports", () => {
    const week = publishWeek("2026-W40", {
      platform: groups({ "linux x86_64": 4, "macos aarch64": 40 }),
      distro: groups({ arch: 3, fedora: 1 }),
      gpu: groups({ metal: 40, vulkan: 4 }),
    });
    expect(week.breakdowns?.distro).toBeNull();
    expect(week.breakdowns?.display).toBeNull();
    expect(week.breakdowns?.gpu).toEqual({ shown: [], other: 44 });
  });
});
