// Words that mark a listing as sensitive in safe mode when the vendor did not
// label it. Matching is whole-word and case-insensitive, so "heroin" does not
// match "heroine" and "xxx" does not match "XXXL".
//
// Keep this list short. A term belongs here only if an ordinary listing would
// almost never use it. Words such as "gun" (glue gun), "knife", "wine",
// "casino" (poker chips), "gore" (a skirt panel), "vibrator", or "adult"
// (adult sizes) stay out: a false positive hides a good from a buyer.
// Plural and other forms are listed on their own; there is no stemming.
export const SENSITIVE_KEYWORDS: readonly string[] = [
  // Sexual content
  "porn",
  "porno",
  "pornography",
  "pornographic",
  "xxx",
  "nsfw",
  "hentai",
  "erotica",
  "nudes",
  "sex toy",
  "sex toys",
  "dildo",
  "dildos",
  "fleshlight",
  "onlyfans",
  // Hard drugs
  "cocaine",
  "heroin",
  "methamphetamine",
  "fentanyl",
  "mdma",
  // Firearms
  "firearm",
  "firearms",
  "handgun",
  "handguns",
  "ammunition",
];
