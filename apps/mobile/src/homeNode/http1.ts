import { concatBytes, utf8ToBytes } from "@noble/hashes/utils.js";

// HTTP/1.1 on raw bytes, for the Tor stream and the LAN channel's inner
// request: one request per connection, `Connection: close`.

export type RawResponse = { status: number; headers: Record<string, string>; body: Uint8Array };

export function encodeRequest(method: string, path: string, host: string, headers: Record<string, string>, body: Uint8Array): Uint8Array {
  const lines = [`${method} ${path} HTTP/1.1`, `Host: ${host}`, "Connection: close", `Content-Length: ${body.length}`];
  for (const [name, value] of Object.entries(headers)) {
    if (/[\r\n]/.test(name + value)) throw new Error("A header may not contain a line break.");
    lines.push(`${name}: ${value}`);
  }
  return concatBytes(utf8ToBytes(lines.join("\r\n") + "\r\n\r\n"), body);
}

function headerEnd(bytes: Uint8Array): number {
  for (let i = 0; i + 3 < bytes.length; i++) {
    if (bytes[i] === 13 && bytes[i + 1] === 10 && bytes[i + 2] === 13 && bytes[i + 3] === 10) return i;
  }
  return -1;
}

function latin1(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += String.fromCharCode(byte);
  return out;
}

function dechunk(bytes: Uint8Array): Uint8Array {
  const parts: Uint8Array[] = [];
  let at = 0;
  for (;;) {
    let end = at;
    while (end + 1 < bytes.length && !(bytes[end] === 13 && bytes[end + 1] === 10)) end++;
    if (end + 1 >= bytes.length) throw new Error("The node's answer was cut off.");
    const size = parseInt(latin1(bytes.subarray(at, end)).split(";")[0], 16);
    if (!Number.isFinite(size)) throw new Error("The node sent an unreadable answer.");
    if (size === 0) return concatBytes(...parts);
    if (end + 2 + size > bytes.length) throw new Error("The node's answer was cut off.");
    parts.push(bytes.subarray(end + 2, end + 2 + size));
    at = end + 2 + size + 2;
  }
}

export function decodeResponse(bytes: Uint8Array): RawResponse {
  const end = headerEnd(bytes);
  if (end < 0) throw new Error("The node's answer was cut off.");
  const [statusLine, ...lines] = latin1(bytes.subarray(0, end)).split("\r\n");
  const status = /^HTTP\/1\.[01] (\d{3})/.exec(statusLine);
  if (!status) throw new Error("The node sent an unreadable answer.");
  const headers: Record<string, string> = {};
  for (const line of lines) {
    const colon = line.indexOf(":");
    if (colon > 0) headers[line.slice(0, colon).trim().toLowerCase()] = line.slice(colon + 1).trim();
  }
  let body = bytes.subarray(end + 4);
  if (headers["transfer-encoding"]?.toLowerCase().includes("chunked")) {
    body = dechunk(body);
  } else if (headers["content-length"] !== undefined) {
    const length = Number(headers["content-length"]);
    if (!Number.isSafeInteger(length) || body.length < length) throw new Error("The node's answer was cut off.");
    body = body.subarray(0, length);
  }
  return { status: Number(status[1]), headers, body };
}
