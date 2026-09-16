import { describe, expect, test } from "bun:test";
import { FILMSTRIP_THUMBS } from "../src/video/model";
import {
  cellsForClip,
  isMediaSrc,
  placeholderCells,
  stripToCells,
} from "../src/video/thumbnails";

const MEDIA = { id: "vid_scene", src: "file:/media/scene.mp4" };
const TAKE = { id: "vid_title", src: "take:vid_title" };

describe("filmstrip thumbnails: real cells, placeholder only when no media", () => {
  test("take: sources render placeholders only, even with a map entry", () => {
    const cells = cellsForClip(TAKE, {
      [TAKE.id]: [{ index: 0, atSeconds: 1, url: "blob:abc", placeholder: false }],
    });
    expect(cells).toHaveLength(FILMSTRIP_THUMBS);
    expect(cells.every((c) => c.placeholder && c.url === null)).toBe(true);
  });

  test("missing strip degrades to a full placeholder lane", () => {
    const cells = cellsForClip(MEDIA, undefined);
    expect(cells).toHaveLength(FILMSTRIP_THUMBS);
    expect(cells.every((c) => c.placeholder)).toBe(true);
  });

  test("real cells pass blob: URLs through; short entries pad, long ones truncate", () => {
    const real = [
      { index: 0, atSeconds: 0.5, url: "blob:one", placeholder: false },
      { index: 1, atSeconds: 1.5, url: "blob:two", placeholder: false },
    ];
    const padded = cellsForClip(MEDIA, { [MEDIA.id]: real });
    expect(padded).toHaveLength(FILMSTRIP_THUMBS);
    expect(padded[0]).toEqual({ index: 0, atSeconds: 0.5, url: "blob:one", placeholder: false });
    expect(padded[1].url).toBe("blob:two");
    expect(padded.slice(2).every((c) => c.placeholder && c.url === null)).toBe(true);

    const long = placeholderCells(FILMSTRIP_THUMBS + 4).map((c, i) => ({
      ...c,
      url: `blob:${i}`,
      placeholder: false,
    }));
    expect(cellsForClip(MEDIA, { [MEDIA.id]: long })).toHaveLength(FILMSTRIP_THUMBS);
  });

  test("cells claiming real without a URL fall back to placeholder", () => {
    const cells = cellsForClip(MEDIA, {
      [MEDIA.id]: [{ index: 0, atSeconds: 0, url: null, placeholder: false }],
    });
    expect(cells[0].placeholder).toBe(true);
    expect(cells[0].url).toBeNull();
  });

  test("stripToCells wires core ThumbStrip paths through the host URL resolver", () => {
    const frames = [
      { index: 0, at_seconds: 4.0, path: "/tmp/thumbs/scene_000.jpg", placeholder: false },
      { index: 1, at_seconds: 12.0, path: null, placeholder: true },
      { index: 2, at_seconds: 20.0, path: "", placeholder: false },
    ];
    const cells = stripToCells(frames, (path) => `blob:${path}`);
    expect(cells[0]).toEqual({
      index: 0,
      atSeconds: 4.0,
      url: "blob:/tmp/thumbs/scene_000.jpg",
      placeholder: false,
    });
    expect(cells[1].placeholder).toBe(true);
    expect(cells[2].placeholder).toBe(true);
  });

  test("stripToCells keeps placeholders when the resolver yields nothing", () => {
    const cells = stripToCells(
      [{ index: 0, at_seconds: 1.0, path: "/tmp/x.jpg", placeholder: false }],
      () => null,
    );
    expect(cells[0].placeholder).toBe(true);
    expect(cells[0].url).toBeNull();
    expect(stripToCells(null, () => "blob:x")).toEqual([]);
  });

  test("isMediaSrc gates the thumbnail path on media-backed sources", () => {
    expect(isMediaSrc("take:vid_scene")).toBe(false);
    expect(isMediaSrc("")).toBe(false);
    expect(isMediaSrc("file:/media/scene.mp4")).toBe(true);
    expect(isMediaSrc("blob:abc")).toBe(true);
  });
});
