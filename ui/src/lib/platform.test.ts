import { describe, expect, it } from "vitest";

import { folderBasename } from "./platform";

describe("folderBasename", () => {
  it("extracts a folder name from Windows and macOS paths", () => {
    expect(folderBasename(String.raw`C:\Users\Ada\Shared files`)).toBe("Shared files");
    expect(folderBasename("/Users/ada/Shared files")).toBe("Shared files");
  });

  it("ignores trailing separators without changing the persisted path", () => {
    expect(folderBasename("D:\\Porta\\Pictures\\")).toBe("Pictures");
    expect(folderBasename("/Volumes/Porta/Pictures/")).toBe("Pictures");
  });
});
