// Small byte-array helpers for the response pane's image preview (M9
// gap-fill). `body_bytes` arrives from Rust as a plain JSON array of
// numbers (serde_json's default `Vec<u8>` encoding, not base64) — fine at
// the size a REST client's responses run, not worth a wire-format change
// for.

/** Base64-encode in chunks rather than `btoa(String.fromCharCode(...bytes))`
 * directly — spreading a large array into `String.fromCharCode` risks
 * blowing the call stack on a sizable image. */
export function bytesToBase64(bytes: number[]): string {
  const CHUNK_SIZE = 8192;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK_SIZE) {
    binary += String.fromCharCode(...bytes.slice(i, i + CHUNK_SIZE));
  }
  return btoa(binary);
}

export function isImageContentType(headers: [string, string][]): string | null {
  for (const [name, value] of headers) {
    if (name.toLowerCase() === "content-type" && value.toLowerCase().startsWith("image/")) {
      return value.split(";")[0].trim();
    }
  }
  return null;
}

const EXTENSION_BY_MIME: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/bmp": "bmp",
  "image/svg+xml": "svg",
};

export function imageFileExtension(contentType: string): string {
  return EXTENSION_BY_MIME[contentType.toLowerCase()] ?? "bin";
}
