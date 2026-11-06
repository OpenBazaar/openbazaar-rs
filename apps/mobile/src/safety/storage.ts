import { DEFAULT_SAFETY, parseSafety, type SafetySettings } from "./filter.ts";

// Kept in this device's storage only. It is never sent to the node.
const KEY = "ob-content-safety";

export function readSafety(): SafetySettings {
  try {
    return typeof localStorage === "undefined" ? DEFAULT_SAFETY : parseSafety(localStorage.getItem(KEY));
  } catch {
    return DEFAULT_SAFETY;
  }
}

export function writeSafety(settings: SafetySettings) {
  try {
    localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // The page still keeps the setting for this visit.
  }
}
