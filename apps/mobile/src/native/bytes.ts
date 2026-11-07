import { utf8ToBytes } from "@noble/hashes/utils.js";

/// Bytes of a fetch body. A file or blob stays binary; it is not turned into text.
export async function bodyBytes(body: BodyInit | null | undefined): Promise<Uint8Array> {
  if (body == null) return new Uint8Array();
  if (typeof body === "string") return utf8ToBytes(body);
  if (body instanceof Uint8Array) return body;
  if (body instanceof ArrayBuffer) return new Uint8Array(body);
  if (ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
  if (typeof body === "object" && "arrayBuffer" in body && typeof body.arrayBuffer === "function") {
    return new Uint8Array(await body.arrayBuffer());
  }
  return new Uint8Array();
}

export function headerOf(headers: HeadersInit | undefined, name: string): string | undefined {
  if (!headers) return undefined;
  const wanted = name.toLowerCase();
  if (typeof Headers !== "undefined" && headers instanceof Headers) return headers.get(wanted) ?? undefined;
  if (Array.isArray(headers)) return headers.find(([key]) => key.toLowerCase() === wanted)?.[1];
  const record = headers as Record<string, string>;
  const key = Object.keys(record).find((item) => item.toLowerCase() === wanted);
  return key ? record[key] : undefined;
}

function isText(contentType: string): boolean {
  return contentType.startsWith("text/") || contentType.includes("json") || contentType.includes("xml") || contentType.includes("javascript");
}

/// A fetch Response whose body is the node's bytes. Images are not decoded as text.
export function responseFromAnswer(status: number, headers: Record<string, string>, body: Uint8Array): Response {
  const type = headers["content-type"] ?? "application/octet-stream";
  if (isText(type)) {
    return new Response(new TextDecoder().decode(body), { status, headers: { "Content-Type": type } });
  }
  return new Response(new Uint8Array(body), { status, headers: { "Content-Type": type } });
}
