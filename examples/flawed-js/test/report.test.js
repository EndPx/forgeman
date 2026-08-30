const test = require("node:test");
const assert = require("node:assert");
const { discount, uniqueNames, report } = require("../src/report.js");

test("discount subtracts the percentage", () => {
  assert.strictEqual(discount(100, 25), 75);
});

test("uniqueNames keeps first occurrences and order", () => {
  assert.deepStrictEqual(uniqueNames(["ana", "bob", "ana"]), ["ana", "bob"]);
});

test("report deduplicates a large list", () => {
  const big = [];
  for (let i = 0; i < 5000; i++) big.push("user" + (i % 200));
  const lines = report(big).split("\n");
  assert.strictEqual(lines.length, 200);
});
