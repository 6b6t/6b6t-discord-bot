import assert from "node:assert/strict";
import test from "node:test";
import { mapWithConcurrency } from "./map-with-concurrency";

test("bounds concurrent work and preserves result order", async () => {
  let active = 0;
  let maximumActive = 0;

  const results = await mapWithConcurrency([1, 2, 3, 4, 5], 2, async (item) => {
    active++;
    maximumActive = Math.max(maximumActive, active);
    await new Promise((resolve) => setTimeout(resolve, 5));
    active--;
    return item * 2;
  });

  assert.equal(maximumActive, 2);
  assert.deepEqual(results, [2, 4, 6, 8, 10]);
});

test("rejects invalid concurrency", async () => {
  await assert.rejects(
    () => mapWithConcurrency([1], 0, async (item) => item),
    RangeError,
  );
});
