/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  appendNotice,
  appendOutput,
  emptyOutput,
  filterOutput,
  MAX_OUTPUT_CHARS,
  MAX_PARTIAL_CHARS,
} from "./util.remote-output.ts";

const texts = (buffer: ReturnType<typeof emptyOutput>) => buffer.entries.map((e) => e.text);

test("a line sent in pieces is joined once its last piece arrives", () => {
  let buffer = appendOutput(emptyOutput(), {
    lines: [{ text: "one" }, { text: "very ", cut: true }],
    ended: false,
  });
  assert.deepEqual(texts(buffer), ["one"]);
  assert.equal(buffer.partial, "very ");
  buffer = appendOutput(buffer, {
    lines: [{ text: "long", cut: true }, { text: " line" }],
    ended: false,
  });
  assert.deepEqual(texts(buffer), ["one", "very long line"]);
  assert.equal(buffer.partial, null);
});

test("the end of a run keeps its unfinished line", () => {
  const buffer = appendOutput(emptyOutput(), { lines: [{ text: "last", cut: true }], ended: true });
  assert.deepEqual(texts(buffer), ["last"]);
});

test("dropped output and notices are said between lines", () => {
  let buffer = appendOutput(emptyOutput(), {
    lines: [{ text: "later" }],
    rotated: { firstOffset: 4_194_304 },
    ended: false,
  });
  buffer = appendNotice(buffer, "Restarted at 10:42");
  assert.deepEqual(
    buffer.entries.map((entry) => [entry.kind, entry.text]),
    [
      ["notice", "Earlier output was dropped."],
      ["line", "later"],
      ["notice", "Restarted at 10:42"],
    ],
  );
});

test("only the newest lines are kept, with keys that stay unique", () => {
  let buffer = emptyOutput();
  for (let index = 0; index < 5; index += 1) {
    buffer = appendOutput(
      buffer,
      { lines: [{ text: `line ${index}` }], ended: false },
      { lines: 3 },
    );
  }
  assert.deepEqual(texts(buffer), ["line 2", "line 3", "line 4"]);
  assert.deepEqual(
    buffer.entries.map((entry) => entry.key),
    [2, 3, 4],
  );
});

test("search keeps the matching lines, ignoring case", () => {
  const buffer = appendOutput(emptyOutput(), {
    lines: [{ text: "GET /api 200" }, { text: "POST /login 500" }, { text: "get /health 200" }],
    ended: false,
  });
  assert.deepEqual(
    filterOutput(buffer.entries, "get").map((entry) => entry.text),
    ["GET /api 200", "get /health 200"],
  );
  assert.equal(filterOutput(buffer.entries, "  ").length, 3);
});

test("a line that never ends stays within the panel's bounds", () => {
  const piece = "x".repeat(8 * 1024);
  let buffer = emptyOutput();
  for (let index = 0; index < 2048; index += 1) {
    buffer = appendOutput(buffer, { lines: [{ text: piece, cut: true }], ended: false });
    assert.ok((buffer.partial?.length ?? 0) <= MAX_PARTIAL_CHARS);
    assert.ok(buffer.chars <= MAX_OUTPUT_CHARS);
  }
  const kept = buffer.entries.reduce((sum, entry) => sum + entry.text.length, 0);
  assert.equal(kept + (buffer.partial?.length ?? 0), buffer.chars);
  assert.ok(buffer.chars <= MAX_OUTPUT_CHARS);
  assert.ok(buffer.entries.length > 0);
  assert.ok(buffer.entries.every((entry) => entry.cut && entry.text.length === MAX_PARTIAL_CHARS));
});

test("the text kept is bounded across many lines too", () => {
  const line = { text: "y".repeat(1000) };
  let buffer = emptyOutput();
  for (let index = 0; index < 100; index += 1) {
    buffer = appendOutput(buffer, { lines: Array.from({ length: 100 }, () => line), ended: false });
  }
  assert.ok(buffer.chars <= MAX_OUTPUT_CHARS);
  assert.equal(buffer.entries.length, Math.floor(MAX_OUTPUT_CHARS / 1000));
});
