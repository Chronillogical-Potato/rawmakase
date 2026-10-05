// What the public page shows. Only completed weeks are published, and each
// breakdown merges groups of fewer than MIN_GROUP installations into "other".
// This reduces exposure; it is not an anonymity guarantee.

import { DIMENSIONS, type DimensionName } from "./dimensions";

export const MIN_GROUP = 5;

export interface Group {
  key: string;
  count: number;
}

export interface Breakdown {
  shown: Group[];
  /// Every group not shown, merged; null when all are shown.
  other: number | null;
}

/// The current week's reports up to the end of yesterday (UTC), so the
/// number changes once a day.
export interface WeekSoFar {
  week: string;
  /// The last completed day counted.
  through: string;
  /// Null when fewer than MIN_GROUP installations have reported.
  total: number | null;
}

export interface Stats {
  /// Null on Mondays, before the week has a completed day.
  thisWeek: WeekSoFar | null;
  /// Completed weeks, newest first.
  weeks: Week[];
}

export interface Week {
  week: string;
  /// Null when fewer than MIN_GROUP installations reported that week.
  total: number | null;
  /// One per dimension (src/dimensions.ts); null when the week's total, or
  /// that dimension's own total, is below MIN_GROUP.
  breakdowns: Record<DimensionName, Breakdown | null> | null;
}

/// Groups below MIN_GROUP go to "other", which must merge at least two
/// groups: the set of platforms is public, so a lone merged group could be
/// named by elimination. Until it does, the smallest shown group joins it.
export function suppress(groups: Group[]): Breakdown {
  const sorted = [...groups].sort(
    (a, b) => b.count - a.count || a.key.localeCompare(b.key),
  );
  const shown = sorted.filter((g) => g.count >= MIN_GROUP);
  const hidden = sorted.filter((g) => g.count < MIN_GROUP);
  if (hidden.length === 1) {
    const smallest = shown.pop();
    if (smallest) hidden.push(smallest);
  }
  const other = hidden.reduce((n, g) => n + g.count, 0);
  return { shown, other: hidden.length > 0 ? other : null };
}

/// One published week from its tallies. A week whose total is below
/// MIN_GROUP publishes no numbers at all. A dimension counted for only some
/// reports (the Linux-only ones) is left out when its own total is below
/// MIN_GROUP.
export function publishWeek(
  week: string,
  tallies: Partial<Record<DimensionName, Group[]>>,
): Week {
  const sum = (groups: Group[]) => groups.reduce((n, g) => n + g.count, 0);
  const total = sum(tallies.platform ?? []);
  if (total < MIN_GROUP) return { week, total: null, breakdowns: null };
  const breakdowns = Object.fromEntries(
    DIMENSIONS.map(({ name }) => {
      const groups = tallies[name] ?? [];
      return [name, sum(groups) < MIN_GROUP ? null : suppress(groups)];
    }),
  ) as Record<DimensionName, Breakdown | null>;
  return { week, total, breakdowns };
}
