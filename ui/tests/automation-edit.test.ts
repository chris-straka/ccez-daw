import { describe, expect, test } from "bun:test";
import {
  beatAtX,
  laneIdFor,
  lanePath,
  pointSetOp,
  valueAtY,
} from "../src/automation/edit";

describe("automation editing helpers", () => {
  test("lane ids read as the address they automate", () => {
    expect(laneIdFor("t1", "volume")).toBe("t1:volume");
  });

  test("new lanes carry the node:param address, existing lanes just beat+value", () => {
    const create = pointSetOp({ laneId: "t1:volume", node: "t1", param: "volume" }, 4, 0.5, false);
    expect(create.kind).toBe("AutomationPointSet");
    expect(create.target).toBe("t1:volume");
    expect(JSON.parse(create.value_json)).toEqual({ beat: 4, value: 0.5, node: "t1", param: "volume" });
    const set = pointSetOp({ laneId: "t1:volume", node: "t1", param: "volume" }, 8, 0.9, true);
    expect(JSON.parse(set.value_json)).toEqual({ beat: 8, value: 0.9 });
  });

  test("bad beats, values, and missing addresses refuse", () => {
    const t = { laneId: "t1:volume", node: "t1", param: "volume" };
    expect(() => pointSetOp(t, -1, 0.5, true)).toThrow();
    expect(() => pointSetOp(t, NaN, 0.5, true)).toThrow();
    expect(() => pointSetOp(t, 4, Infinity, true)).toThrow();
    expect(() => pointSetOp({ laneId: "x", node: "", param: "" }, 4, 0.5, false)).toThrow();
  });

  test("clicks map to beats and values with clamping", () => {
    expect(beatAtX(0, 300, 32)).toBe(0);
    expect(beatAtX(150, 300, 32)).toBe(16);
    expect(beatAtX(999, 300, 32)).toBe(32);
    expect(() => beatAtX(1, 0, 32)).toThrow();
    expect(valueAtY(0, 48, 0, 1.5)).toBe(1.5);
    expect(valueAtY(48, 48, 0, 1.5)).toBe(0);
    expect(valueAtY(-5, 48, -1, 1)).toBe(1);
    expect(() => valueAtY(1, 48, 1, 1)).toThrow();
  });

  test("lane paths hold endpoints and sort points", () => {
    const path = lanePath(
      [{ beat: 16, value: 1 }, { beat: 4, value: 0 }],
      300, 48, 32, 0, 1.5,
    );
    const pts = path.split(" ");
    expect(pts.length).toBe(4);
    expect(pts[0].startsWith("0.0,")).toBe(true);
    expect(pts[3].startsWith("300.0,")).toBe(true);
    const empty = lanePath([], 300, 48, 32, 0, 1.5);
    expect(empty.split(" ").length).toBe(2);
  });
});
