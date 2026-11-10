import { describe, expect, it } from "vitest";
import { focusPaneTarget, type Tab } from "./useTabs";

const tabs = [
  {
    id: 1,
    kind: "terminal",
    spaceId: "a",
    title: "shell",
    paneTree: { kind: "leaf", id: 10 },
    activeLeafId: 10,
  },
  {
    id: 2,
    kind: "terminal",
    spaceId: "b",
    title: "split",
    paneTree: {
      kind: "split",
      id: 20,
      dir: "row",
      children: [
        { kind: "leaf", id: 21 },
        { kind: "leaf", id: 22 },
      ],
    },
    activeLeafId: 21,
  },
  { id: 3, kind: "editor", spaceId: "a", title: "x.ts", path: "/x.ts" },
] as Tab[];

describe("focusPaneTarget", () => {
  it("finds the tab that owns a split leaf", () => {
    expect(focusPaneTarget(tabs, { paneId: 22 })).toEqual({
      tabId: 2,
      leafId: 22,
    });
  });

  it("finds a single-pane tab", () => {
    expect(focusPaneTarget(tabs, { paneId: 10 })).toEqual({
      tabId: 1,
      leafId: 10,
    });
  });

  it("returns null for a leaf that has closed", () => {
    expect(focusPaneTarget(tabs, { paneId: 99 })).toBeNull();
  });

  it("does not match a split node id as a leaf", () => {
    expect(focusPaneTarget(tabs, { paneId: 20 })).toBeNull();
  });

  it("rejects malformed payloads", () => {
    for (const payload of [
      null,
      undefined,
      {},
      { paneId: "22" },
      { paneId: 22.5 },
      { paneId: Number.NaN },
      22,
    ]) {
      expect(focusPaneTarget(tabs, payload)).toBeNull();
    }
  });
});
