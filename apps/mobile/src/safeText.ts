// Text from other people's signed records, made safe to lay out. A record can
// carry any valid UTF-8, so a hostile seller can send direction overrides that
// flip neighbouring text, invisible characters, newlines, or a tower of
// combining marks that spills over the rows around it.

// C0 and C1 controls, bidi embeddings, overrides and isolates, zero-width and
// other invisible format characters, line and paragraph separators, and BOM.
const HIDDEN = /[\u0000-\u001f\u007f-\u009f\u00ad\u061c\u115f\u1160\u17b4\u17b5\u180e\u200b-\u200f\u2028-\u202e\u2060-\u206f\u3164\ufe00-\ufe0f\ufeff\uffa0\ufff0-\ufffb]|\udb40[\udc00-\udfff]/g;
const MARK = /\p{M}/u;
const MAX_MARKS = 2;

/// `text` with hidden characters removed, at most two combining marks on any
/// letter, whitespace collapsed to single spaces, and cut to `max` characters.
export function displayText(text: string, max: number): string {
  let out = "";
  let marks = 0;
  for (const ch of text.replace(HIDDEN, " ")) {
    if (MARK.test(ch)) {
      if (++marks > MAX_MARKS) continue;
    } else marks = 0;
    out += ch;
  }
  const chars = [...out.replace(/\s+/g, " ").trim()];
  return chars.length > max ? `${chars.slice(0, max - 1).join("").trimEnd()}…` : chars.join("");
}
