// The questions the stats answer, one tally table each. No table combines
// every field, so a rare combination never preserves one installation's
// report.
import type { Report } from "./report";

export const DIMENSIONS = [
  {
    name: "platform",
    title: "Platform",
    table: "platform_counts",
    columns: ["os", "arch"],
    key: "os || ' ' || arch",
    values: (r: Report) => [r.os, r.arch],
  },
  {
    name: "version",
    title: "Version",
    table: "version_counts",
    columns: ["version"],
    key: "version",
    values: (r: Report) => [r.version],
  },
  {
    name: "channel",
    title: "Build channel",
    table: "channel_counts",
    columns: ["channel"],
    key: "channel",
    values: (r: Report) => [r.channel],
  },
  {
    name: "os_release",
    title: "OS release",
    table: "os_release_counts",
    columns: ["os_release"],
    key: "os_release",
    values: (r: Report) => [r.os_release],
  },
  {
    name: "distro",
    title: "Linux distribution",
    table: "distro_counts",
    columns: ["distro"],
    key: "distro",
    values: (r: Report) => (r.os === "linux" ? [r.distro] : null),
  },
  {
    name: "display",
    title: "Linux display",
    table: "display_counts",
    columns: ["display"],
    key: "display",
    values: (r: Report) => (r.os === "linux" ? [r.display] : null),
  },
  {
    name: "gpu",
    title: "Graphics",
    table: "gpu_counts",
    columns: ["gpu"],
    key: "gpu",
    values: (r: Report) => [r.gpu],
  },
] as const;

export type DimensionName = (typeof DIMENSIONS)[number]["name"];
