/// <reference types="node" />
import assert from "node:assert/strict";
import { test } from "node:test";

import { appendNotice, appendOutput, emptyOutput, filterOutput } from "./util.remote-output.ts";

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
    buffer = appendOutput(buffer, { lines: [{ text: `line ${index}` }], ended: false }, 3);
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
