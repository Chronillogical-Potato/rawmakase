import { describe, expect, it } from "vitest";
import { parseTags } from "../src/versions";

describe("parseTags", () => {
  it("reads version tags from git's ref advertisement", () => {
    const refs =
      "001e# service=git-upload-pack\n0000" +
      "00a1abcd HEAD\0multi_ack thin-pack\n" +
      "003fabcd refs/heads/main\n" +
      "003fabcd refs/tags/v0.1.9\n" +
      "0040abcd refs/tags/v0.1.10\n" +
      "0043abcd refs/tags/v0.1.10^{}\n" +
      "0043abcd refs/tags/v0.2.0-beta.1\n" +
      "0043abcd refs/tags/nightly\n0000";
    expect([...parseTags(refs)]).toEqual(["0.1.9", "0.1.10", "0.2.0-beta.1"]);
  });
});
