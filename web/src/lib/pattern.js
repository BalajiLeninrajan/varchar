// Tokenizer for displaying a generated pattern: structure, escapes, character
// classes and quantifiers each get their own colour.

const QUANTIFIERS = "*+?";
const STRUCTURE = "()|^$.";

export function tokenizePattern(pattern) {
  const tokens = [];
  let literal = "";
  const flush = () => {
    if (literal) tokens.push({ kind: "lit", text: literal });
    literal = "";
  };
  const emit = (text, kind) => {
    flush();
    tokens.push({ kind, text });
  };

  for (let index = 0; index < pattern.length; index += 1) {
    const character = pattern[index];
    if (character === "\\" && index + 1 < pattern.length) {
      emit(pattern.slice(index, index + 2), "esc");
      index += 1;
    } else if (character === "[") {
      let end = index + 1;
      while (end < pattern.length && pattern[end] !== "]") {
        if (pattern[end] === "\\") end += 1;
        end += 1;
      }
      emit(pattern.slice(index, Math.min(end + 1, pattern.length)), "class");
      index = end;
    } else if (character === "{") {
      const end = pattern.indexOf("}", index);
      if (end === -1) {
        literal += character;
      } else {
        emit(pattern.slice(index, end + 1), "quant");
        index = end;
      }
    } else if (QUANTIFIERS.includes(character)) {
      emit(character, "quant");
    } else if (STRUCTURE.includes(character)) {
      emit(character, "meta");
    } else {
      literal += character;
    }
  }
  flush();
  return tokens;
}
