/** Clipboard control name from a field label: "Receive address" → "Copy receive address". */
export function copyValueLabel(label: string) {
  return `Copy ${label.charAt(0).toLowerCase()}${label.slice(1)}`
}
