/* Cybercore Keysmith local passphrase/v1 client API. */
(function installKeysmith(root) {
  "use strict";

  const API_VERSION = "passphrase/v1";
  const WORDLIST_URL = "/static/keysmith-eff-large.txt";
  let wordlistPromise = null;

  async function wordlist() {
    if (!wordlistPromise) wordlistPromise = root.fetch(WORDLIST_URL, { cache: "force-cache" }).then(async (response) => {
      if (!response.ok) throw new Error("Could not load Keysmith's EFF wordlist.");
      const words = (await response.text()).trim().split(/\r?\n/);
      if (words.length !== 7776 || new Set(words).size !== 7776) throw new Error("Keysmith wordlist failed its integrity check.");
      return words;
    }).catch((error) => { wordlistPromise = null; throw error; });
    return wordlistPromise;
  }

  function boundedIndex(bound) {
    const source = root.crypto;
    if (!source || typeof source.getRandomValues !== "function") throw new Error("Secure random generation is unavailable in this browser.");
    const range = Math.floor(0x100000000 / bound) * bound;
    const value = new Uint32Array(1);
    do { source.getRandomValues(value); } while (value[0] >= range);
    return value[0] % bound;
  }

  async function generatePassphrase({ words: count = 6, separator = "-", capitalize = false } = {}) {
    if (!Number.isInteger(count) || count < 3 || count > 12) throw new RangeError("Choose between 3 and 12 words.");
    if (typeof separator !== "string" || new TextEncoder().encode(separator).length > 8 || /[\u0000-\u001f\u007f]/u.test(separator)) {
      throw new RangeError("Separator must be at most 8 printable UTF-8 bytes.");
    }
    const list = await wordlist();
    return Array.from({ length: count }, () => {
      const word = list[boundedIndex(list.length)];
      return capitalize ? word[0].toUpperCase() + word.slice(1) : word;
    }).join(separator);
  }

  const api = Object.freeze({ API_VERSION, generatePassphrase });
  root.Keysmith = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;
})(globalThis);
