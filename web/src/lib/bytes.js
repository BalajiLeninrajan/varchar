// Match spans and SQL error spans are UTF-8 byte offsets, so anything that
// slices them has to work on the encoded bytes rather than the JS string.

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export const encode = (text) => encoder.encode(text);
export const byteLength = (text) => encoder.encode(text).length;
export const decodeRange = (bytes, from, to) => decoder.decode(bytes.subarray(from, to));

/**
 * Splits an encoded blob into alternating plain and matched segments.
 * `limit` caps how many matches get their own segment; the rest stay plain.
 */
export function segmentMatches(bytes, matches, limit) {
  const drawn = matches.slice(0, limit);
  const segments = [];
  let cursor = 0;
  drawn.forEach((match, index) => {
    if (match.start > cursor) {
      segments.push({ text: decodeRange(bytes, cursor, match.start), match: -1 });
    }
    segments.push({ text: decodeRange(bytes, match.start, match.end), match: index });
    cursor = match.end;
  });
  if (cursor < bytes.length) {
    segments.push({ text: decodeRange(bytes, cursor, bytes.length), match: -1 });
  }
  return { segments, drawn: drawn.length };
}
