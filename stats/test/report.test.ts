import { describe, expect, it } from "vitest";
import { isoWeek, parseReport, utcDay, weekStart } from "../src/report";

const valid = {
  schema: 1,
  version: "0.1.10",
  os: "linux",
  arch: "x86_64",
  channel: "arch-package",
  os_release: "linux",
  distro: "arch",
  display: "wayland",
  gpu: "vulkan",
};
const mac = {
  ...valid,
  os: "macos",
  arch: "aarch64",
  channel: "macos-dmg",
  os_release: "macos-26",
  distro: "none",
  display: "none",
  gpu: "metal",
};

describe("parseReport", () => {
  it("accepts a complete report", () => {
    expect(parseReport(valid)).toEqual(valid);
    expect(parseReport({ ...valid, version: "0.2.0-beta.1" })).not.toBeTypeOf("string");
  });

  it("rejects unknown fields, so nothing else can ride along", () => {
    expect(parseReport({ ...valid, path: "/home/me/photos" })).toBe("unknown field path");
    expect(parseReport({ ...valid, id: "abc" })).toBe("unknown field id");
  });

  it("requires every field", () => {
    for (const key of Object.keys(valid)) {
      const report: Record<string, unknown> = { ...valid };
      delete report[key];
      expect(parseReport(report)).toBeTypeOf("string");
    }
  });

  it("accepts only the listed values", () => {
    expect(parseReport({ ...valid, schema: 2 })).toBe("unsupported schema");
    expect(parseReport({ ...valid, os: "freebsd" })).toBe("invalid os");
    expect(parseReport({ ...valid, arch: "riscv64" })).toBe("invalid arch");
    expect(parseReport({ ...valid, channel: "aur" })).toBe("invalid channel");
    expect(parseReport({ ...valid, os: ["linux"] })).toBe("invalid os");
  });

  it("accepts only release-shaped versions", () => {
    for (const version of ["1.0", "v0.1.10", "0.1.10 ", "0.1.10-<b>", "0.1.10-averyveryverylongsuffix", 10]) {
      expect(parseReport({ ...valid, version })).toBe("invalid version");
    }
  });

  it("accepts the platform fields that match the OS", () => {
    expect(parseReport(mac)).toEqual(mac);
    expect(parseReport({ ...mac, os: "windows", arch: "x86_64", os_release: "windows-11", gpu: "dx12" })).not.toBeTypeOf("string");
  });

  it("rejects platform fields that don't match the OS", () => {
    expect(parseReport({ ...mac, os_release: "windows-11" })).toBe("invalid os_release");
    expect(parseReport({ ...mac, os_release: "macos-9" })).toBe("invalid os_release");
    expect(parseReport({ ...valid, os_release: "linux-6.8" })).toBe("invalid os_release");
    expect(parseReport({ ...mac, distro: "arch" })).toBe("invalid distro");
    expect(parseReport({ ...valid, distro: "none" })).toBe("invalid distro");
    expect(parseReport({ ...valid, distro: "gentoo" })).toBe("invalid distro");
    expect(parseReport({ ...valid, display: "none" })).toBe("invalid display");
    expect(parseReport({ ...mac, display: "x11" })).toBe("invalid display");
    expect(parseReport({ ...valid, gpu: "cuda" })).toBe("invalid gpu");
  });

  it("rejects anything but an object", () => {
    for (const value of [null, [], "report", 1]) {
      expect(parseReport(value)).toBe("expected a JSON object");
    }
  });
});

describe("isoWeek", () => {
  it.each([
    ["2026-10-01T12:00:00Z", "2026-W40"],
    // Monday starts the week; Sunday ends it.
    ["2026-09-28T00:00:00Z", "2026-W40"],
    ["2026-10-04T23:59:59Z", "2026-W40"],
    ["2026-10-05T00:00:00Z", "2026-W41"],
    // Years whose first or last days belong to a neighbouring year's week.
    ["2024-12-30T00:00:00Z", "2025-W01"],
    ["2027-01-01T00:00:00Z", "2026-W53"],
    ["2021-01-03T00:00:00Z", "2020-W53"],
    ["2026-01-01T00:00:00Z", "2026-W01"],
  ])("%s is %s", (date, week) => {
    expect(isoWeek(new Date(date))).toBe(week);
  });

  it("uses UTC, not the local day", () => {
    // Monday morning in Tokyo is still Sunday in UTC.
    expect(isoWeek(new Date("2026-10-05T08:00:00+09:00"))).toBe("2026-W40");
  });
});

describe("utcDay and weekStart", () => {
  it("use UTC days and Monday-started weeks", () => {
    expect(utcDay(new Date("2026-10-05T08:00:00+09:00"))).toBe("2026-10-04");
    expect(weekStart(new Date("2026-10-04T23:59:59Z"))).toBe("2026-09-28");
    expect(weekStart(new Date("2026-09-28T00:00:00Z"))).toBe("2026-09-28");
    expect(weekStart(new Date("2027-01-01T12:00:00Z"))).toBe("2026-12-28");
  });
});
