-- Weekly tallies, one table per question (src/dimensions.ts). No table
-- combines every field of a report, so a rare combination never preserves
-- one installation's report. `week` is the ISO week of receipt in UTC, e.g.
-- 2026-W40. distro_counts and display_counts count Linux reports only.

CREATE TABLE platform_counts (
  week TEXT NOT NULL,
  os TEXT NOT NULL,
  arch TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, os, arch)
) WITHOUT ROWID;

CREATE TABLE version_counts (
  week TEXT NOT NULL,
  version TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, version)
) WITHOUT ROWID;

CREATE TABLE channel_counts (
  week TEXT NOT NULL,
  channel TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, channel)
) WITHOUT ROWID;

CREATE TABLE os_release_counts (
  week TEXT NOT NULL,
  os_release TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, os_release)
) WITHOUT ROWID;

CREATE TABLE distro_counts (
  week TEXT NOT NULL,
  distro TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, distro)
) WITHOUT ROWID;

CREATE TABLE display_counts (
  week TEXT NOT NULL,
  display TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, display)
) WITHOUT ROWID;

CREATE TABLE gpu_counts (
  week TEXT NOT NULL,
  gpu TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (week, gpu)
) WITHOUT ROWID;

-- Reports received per UTC day, with no other field. Only sums of completed
-- days are published, as the current week's running total.
CREATE TABLE daily_counts (
  day TEXT NOT NULL PRIMARY KEY,
  count INTEGER NOT NULL
) WITHOUT ROWID;

-- The last release list read from GitHub, so version checks keep working
-- while GitHub can't be reached.
CREATE TABLE releases (
  version TEXT NOT NULL PRIMARY KEY
) WITHOUT ROWID;
