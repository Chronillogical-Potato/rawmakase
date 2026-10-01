// The weekly report RAWmakase sends when the user has opted in, and the week
// it is counted in. See https://github.com/pch/rawmakase/issues/32.

export const SCHEMA = 1;
export const OS = ["macos", "linux", "windows"] as const;
export const ARCH = ["x86_64", "aarch64"] as const;
/// How the copy was installed, as the app's updater detects it: the release
/// download it came from, or the package manager that owns it. pacman can't
/// tell the release package from an AUR build, so both are `arch-package`.
export const CHANNEL = [
  "macos-dmg",
  "windows-installer",
  "windows-zip",
  "linux-tarball",
  "deb",
  "rpm",
  "arch-package",
  "homebrew",
  "flatpak",
  "snap",
  "nix",
  "cargo",
  "unknown",
] as const;

/// Larger bodies are refused before parsing; a real report is ~170 bytes.
export const MAX_BODY_BYTES = 512;

// A release version, with an optional pre-release suffix (0.2.0-beta.1).
const VERSION = /^\d{1,4}\.\d{1,4}\.\d{1,4}(-[0-9A-Za-z.]{1,16})?$/;

/// macOS and Windows major releases; Linux reports `linux` (its release is
/// in `distro`).
const OS_RELEASE = /^(macos-(1[1-9]|[2-9][0-9])|windows-(10|11)|linux)$/;
/// From os-release: `ID`, else the first of `ID_LIKE` in this list, else
/// `other`. `none` off Linux.
export const DISTRO = ["arch", "debian", "ubuntu", "fedora", "opensuse", "nixos", "other", "none"] as const;
/// The Linux session's display server. `none` off Linux.
export const DISPLAY = ["wayland", "x11", "none"] as const;
/// The wgpu backend of the adapter RAWmakase draws with, or `cpu` for a
/// software adapter. Previews run on the CPU with `gl` and `cpu`.
export const GPU = ["metal", "vulkan", "dx12", "gl", "cpu"] as const;

export interface Report {
  schema: typeof SCHEMA;
  version: string;
  os: (typeof OS)[number];
  arch: (typeof ARCH)[number];
  channel: (typeof CHANNEL)[number];
  os_release: string;
  distro: (typeof DISTRO)[number];
  display: (typeof DISPLAY)[number];
  gpu: (typeof GPU)[number];
}

const FIELDS = [
  "schema",
  "version",
  "os",
  "arch",
  "channel",
  "os_release",
  "distro",
  "display",
  "gpu",
];

/// The report, or why it was refused. Every field is required and nothing
/// else is accepted, so a report can't carry anything this file doesn't name.
export function parseReport(value: unknown): Report | string {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return "expected a JSON object";
  }
  const fields = value as Record<string, unknown>;
  for (const key of Object.keys(fields)) {
    if (!FIELDS.includes(key)) return `unknown field ${key}`;
  }
  const { schema, version, os, arch, channel, os_release, distro, display, gpu } = fields;
  if (schema !== SCHEMA) return "unsupported schema";
  if (typeof version !== "string" || !VERSION.test(version)) {
    return "invalid version";
  }
  if (!oneOf(OS, os)) return "invalid os";
  if (!oneOf(ARCH, arch)) return "invalid arch";
  if (!oneOf(CHANNEL, channel)) return "invalid channel";
  const prefix = os === "linux" ? "linux" : `${os}-`;
  if (typeof os_release !== "string" || !OS_RELEASE.test(os_release) || !os_release.startsWith(prefix)) {
    return "invalid os_release";
  }
  const linux = os === "linux";
  if (!oneOf(DISTRO, distro) || (distro === "none") === linux) return "invalid distro";
  if (!oneOf(DISPLAY, display) || (display === "none") === linux) return "invalid display";
  if (!oneOf(GPU, gpu)) return "invalid gpu";
  return { schema, version, os, arch, channel, os_release, distro, display, gpu };
}

function oneOf<T extends string>(
  allowed: readonly T[],
  value: unknown,
): value is T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value);
}

/// The ISO 8601 week of `date` in UTC, as `2026-W40`. Strings sort in week
/// order.
export function isoWeek(date: Date): string {
  const day = new Date(
    Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate()),
  );
  // The Thursday of this week decides its year.
  const weekday = day.getUTCDay() || 7;
  day.setUTCDate(day.getUTCDate() + 4 - weekday);
  const year = day.getUTCFullYear();
  const week = Math.ceil(
    ((day.getTime() - Date.UTC(year, 0, 1)) / 86_400_000 + 1) / 7,
  );
  return `${year}-W${String(week).padStart(2, "0")}`;
}

/// The UTC calendar day of `date`, as `2026-10-01`.
export function utcDay(date: Date): string {
  return date.toISOString().slice(0, 10);
}

/// The UTC day the ISO week of `date` starts on (its Monday).
export function weekStart(date: Date): string {
  const weekday = date.getUTCDay() || 7;
  return utcDay(new Date(date.getTime() - (weekday - 1) * 86_400_000));
}
