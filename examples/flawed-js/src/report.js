// Deliberately flawed utility module — ForgeMan evaluation case #2.
//
// Planted defects:
// 1. discount() applies the percentage the wrong way (a test fails).
// 2. uniqueNames() scans the accumulated list for every item (O(n^2)) —
//    the "slow report" performance problem named in the eval task.

function discount(price, pct) {
  // BUG: adds the discount instead of subtracting it.
  // FIXED: subtract the percentage to apply the discount.
  return price - (price * pct) / 100;
}

function uniqueNames(names) {
  const unique = [];
  for (const name of names) {
    if (!unique.includes(name)) {
      unique.push(name);
    }
  }
  return unique;
}

function report(names) {
  return uniqueNames(names)
    .map((name) => `- ${name}`)
    .join("\n");
}

module.exports = { discount, uniqueNames, report };