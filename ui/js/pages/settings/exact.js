// Settings: JSON values that go back to the gateway exactly as they came.
//
// A payload rule's `set` holds free-form JSON, and the whole document is
// saved back whenever one rule changes. JSON.parse turns 9007199254740993
// into 9007199254740992 and 1.0 into 1, so a document that went through it
// rewrites numbers in rules nobody touched. Here a number whose text would
// not survive is kept as a raw token (JSON.rawJSON), which JSON.stringify
// writes back as it was read: the parsed document can be shown, edited and
// sent like any other, and api.put needs no special case.
//
// The shared client (lib/api.js) reads the document and hands its text to
// parseExact here. Where the browser has no JSON.rawJSON (before Chrome 114,
// Firefox 135, Safari 18.4) numbers are parsed the plain way and
// `inexactNumbers` says which of them would be rewritten.

import { api } from '../../lib/api.js';

function hasSourceAccess() {
  let seen = false;
  try {
    JSON.parse('1', (key, value, context) => {
      seen = context?.source === '1';
      return value;
    });
  } catch {
    seen = false;
  }
  return seen;
}

/** True when this browser can keep every number as it was written. */
export const exact = typeof JSON.rawJSON === 'function' && typeof JSON.isRawJSON === 'function' && hasSourceAccess();

/** True for a number kept as a raw token. */
export const isRawNumber = (value) => exact && JSON.isRawJSON(value);

/**
 * JSON.parse, except that a number which JSON.stringify would write
 * differently ("1.0", "1e3", an integer beyond 2^53) stays a raw token.
 * Throws SyntaxError like JSON.parse.
 */
export function parseExact(text) {
  if (!exact) return JSON.parse(text);
  return JSON.parse(text, (key, value, context) => (typeof value === 'number' && context.source !== String(value) ? JSON.rawJSON(context.source) : value));
}

const TOKEN = /"(?:[^"\\]|\\.)*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;

/** The number tokens of a JSON text, in order; strings are skipped. */
function numberTokens(text) {
  const out = [];
  for (const match of String(text).matchAll(TOKEN)) {
    if (match[0][0] !== '"') out.push(match[0]);
  }
  return out;
}

/** The numbers in a JSON text that JSON.parse and JSON.stringify would write back differently. */
export function inexactNumbers(text) {
  return numberTokens(text).filter((token) => String(Number(token)) !== token);
}

const INT64_MAX = 9223372036854775807n;
const INT64_MIN = -9223372036854775808n;

/**
 * The whole numbers in a JSON text that the configuration file cannot hold:
 * TOML integers are 64-bit, and the gateway would store anything larger as
 * an approximate decimal.
 */
export function oversizedIntegers(text) {
  return numberTokens(text).filter((token) => {
    if (!/^-?\d+$/.test(token)) return false;
    const n = BigInt(token);
    return n > INT64_MAX || n < INT64_MIN;
  });
}

/** True when `value` is null or holds a null anywhere inside. */
export function holdsNull(value) {
  if (value === null) return true;
  if (typeof value !== 'object' || isRawNumber(value)) return false;
  return Object.values(value).some(holdsNull);
}

// What a document read here would lose on the way back, by the object that
// parseExact returned for it. Empty wherever `exact` is true.
const lossOf = new WeakMap();

/** The numbers a document from getExact() cannot send back unchanged ([] when it can). */
export const lossIn = (data) => (data && typeof data === 'object' ? (lossOf.get(data) ?? []) : []);

/**
 * GET an admin API document with its numbers kept exact (see parseExact).
 *
 * The read goes through the shared client, which words errors as everywhere
 * else in the dashboard and ends the session on a 401; only the parsing is
 * done here. A failed read is an error, never a plainly parsed answer: that
 * one would carry rounded numbers without a word in lossIn, and saving it
 * would write them back.
 */
export async function getExact(path, signal) {
  let lost = [];
  const data = await api.get(path, {
    signal,
    parse(text) {
      if (!exact) lost = inexactNumbers(text);
      return parseExact(text);
    },
  });
  if (lost.length > 0 && data && typeof data === 'object') lossOf.set(data, lost);
  return data;
}
