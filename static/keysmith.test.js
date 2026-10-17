"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const crypto = require("node:crypto");

const asset = fs.readFileSync(`${__dirname}/vendor/keysmith/eff-large.txt`, "utf8").trim();
const words = asset.split(/\r?\n/);
assert.equal(crypto.createHash("sha256").update(asset + "\n").digest("hex"), "6d557f0693958fb5e650b68b5bee585eb82cf4da32965505c789e924743bc522");
globalThis.fetch = async () => ({ ok: true, text: async () => asset });

const { API_VERSION, generatePassphrase } = require("./keysmith.js");
assert.equal(API_VERSION, "passphrase/v1");

(async () => {
  const phrase = await generatePassphrase({ words: 6 });
  const selected = phrase.split("-");
  assert.equal(selected.length, 6);
  assert(selected.every((word) => words.includes(word)));
  const capped = await generatePassphrase({ words: 3, capitalize: true });
  assert(capped.split("-").every((word) => word[0] === word[0].toUpperCase()));
  await assert.rejects(generatePassphrase({ words: 2 }), RangeError);
  await assert.rejects(generatePassphrase({ separator: "\n" }), RangeError);
  process.stdout.write("Keysmith passphrase/v1 checks passed.\n");
})().catch((error) => { console.error(error); process.exitCode = 1; });
